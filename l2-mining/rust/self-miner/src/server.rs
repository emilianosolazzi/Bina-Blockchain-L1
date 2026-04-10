//! Embedded axum HTTP server for the self-miner dashboard.
//!
//! Serves the dashboard HTML, telemetry API, mining control, and SSE stream.
//! All handlers read from the same `MinerHandle` broadcast channel and local
//! files — no external services required.

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
    routing::get,
    Json, Router,
};
use crate::heartbeat::HeartbeatStatus;
use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::Arc;
use temporal_gradient_core::{MinerHandle, MiningControl, TelemetrySnapshot};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// Dashboard HTML embedded at compile time.
const DASHBOARD_HTML: &str = include_str!("../assets/dashboard.html");

/// Shared application state accessible from all handlers.
#[derive(Clone)]
pub struct AppState {
    pub handle: Arc<MinerHandle>,
    pub telemetry_path: PathBuf,
    pub control_path: PathBuf,
    pub latest: Arc<RwLock<Option<TelemetrySnapshot>>>,
    pub heartbeat_status: Arc<RwLock<HeartbeatStatus>>,
    pub shutdown: CancellationToken,
    pub wallet_address: String,
    pub rpc_url: String,
    pub contract_address: String,
    pub pool_id: u8,
    pub config_path: PathBuf,
}

/// Start the HTTP server on `addr`. Runs until `shutdown` is cancelled.
pub async fn run_server(state: AppState, port: u16) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/", get(serve_dashboard))
        .route("/api/latest", get(api_latest))
        .route("/api/history", get(api_history))
        .route("/api/miner/control", get(api_get_control).post(api_post_control))
        .route("/api/health", get(api_health))
        .route("/api/system/status", get(api_system_status))
        .route("/api/heartbeat/status", get(api_heartbeat_status))
        .route("/api/heartbeat/alerts", get(api_heartbeat_alerts))
        .route("/api/cpu", get(api_cpu))
        .route("/api/entropy-quality", get(api_entropy_quality))
        .route("/events", get(api_sse))
        .with_state(state.clone());

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    tracing::info!("Dashboard server listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            state.shutdown.cancelled().await;
        })
        .await?;

    Ok(())
}

// ── Handlers ──────────────────────────────────────────────────────────────

/// Serve the embedded dashboard HTML.
async fn serve_dashboard() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        DASHBOARD_HTML,
    )
        .into_response()
}

/// GET /api/latest — latest telemetry snapshot.
async fn api_latest(State(state): State<AppState>) -> Json<serde_json::Value> {
    let snap = state.latest.read().await;
    match snap.as_ref() {
        Some(s) => Json(serde_json::json!({
            "telemetryPath": state.telemetry_path.to_string_lossy(),
            "latest": s,
        })),
        None => Json(serde_json::json!({
            "telemetryPath": state.telemetry_path.to_string_lossy(),
            "latest": null,
        })),
    }
}

/// Query parameters for the history endpoint.
#[derive(Debug, Deserialize)]
struct HistoryQuery {
    limit: Option<usize>,
}

/// GET /api/history?limit=N — tail the telemetry JSONL file.
async fn api_history(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<HistoryQuery>,
) -> Json<serde_json::Value> {
    let limit = query.limit.unwrap_or(120).min(500);
    let path = state.telemetry_path.clone();
    let snapshots = tokio::task::spawn_blocking(move || read_tail_snapshots(&path, limit))
        .await
        .unwrap_or_default();
    let latest = snapshots.last().cloned();
    Json(serde_json::json!({
        "telemetryPath": state.telemetry_path.to_string_lossy(),
        "latest": latest,
        "history": snapshots,
    }))
}

/// GET /api/miner/control — read current mining control state.
async fn api_get_control(State(state): State<AppState>) -> Json<MiningControl> {
    let path = state.control_path.clone();
    let ctrl = tokio::task::spawn_blocking(move || read_control_file(&path))
        .await
        .unwrap_or_default();
    Json(ctrl)
}

/// POST /api/miner/control — write mining control state.
async fn api_post_control(
    State(state): State<AppState>,
    Json(body): Json<ControlInput>,
) -> Result<Json<MiningControl>, StatusCode> {
    let ctrl = MiningControl {
        paused: body.paused.unwrap_or(false),
        power_pct: clamp_power(body.power_pct.unwrap_or(100)),
        ..MiningControl::default()
    };
    let json =
        serde_json::to_string_pretty(&ctrl).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let path = state.control_path.clone();
    tokio::task::spawn_blocking(move || std::fs::write(&path, json))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(ctrl))
}

#[derive(Debug, Deserialize)]
struct ControlInput {
    paused: Option<bool>,
    power_pct: Option<u8>,
}

/// GET /api/health — basic health check.
async fn api_health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let snap = state.latest.read().await;
    let running = snap
        .as_ref()
        .map(|s| format!("{:?}", s.state))
        .unwrap_or_else(|| "unknown".into());
    Json(serde_json::json!({
        "status": "ok",
        "miner_state": running,
        "mode": "self-miner",
    }))
}

/// GET /api/system/status — full system status including wallet address.
async fn api_system_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let snap = state.latest.read().await;
    let miner_state = snap
        .as_ref()
        .map(|s| format!("{:?}", s.state))
        .unwrap_or_else(|| "unknown".into());
    let solutions = snap.as_ref().map(|s| s.solutions).unwrap_or(0);
    let accepted = snap.as_ref().map(|s| s.accepted_submissions).unwrap_or(0);
    let hashrate = snap.as_ref().map(|s| s.hashrate_hs).unwrap_or(0.0);
    let rewards = snap.as_ref().map(|s| s.total_rewards_estimate).unwrap_or(0.0);
    drop(snap);

    Json(serde_json::json!({
        "status": "ok",
        "mode": "self-miner",
        "miner": {
            "state": miner_state,
            "solutions": solutions,
            "accepted": accepted,
            "hashrate": hashrate,
            "rewards": rewards,
        },
        "chain": {
            "walletAddress": state.wallet_address,
            "rpcUrl": state.rpc_url,
            "contractAddress": state.contract_address,
            "chainId": 42161,
            "poolId": state.pool_id,
        },
        "dashboard": {
            "telemetryFile": state.telemetry_path.to_string_lossy(),
            "configFile": state.config_path.to_string_lossy(),
        },
    }))
}

/// GET /api/heartbeat/status — embedded heartbeat status.
async fn api_heartbeat_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let st = state.heartbeat_status.read().await;
    Json(serde_json::to_value(&*st).unwrap_or_else(|_| serde_json::json!({})))
}

/// GET /api/heartbeat/alerts — active alerts + ransomware summary.
async fn api_heartbeat_alerts(State(state): State<AppState>) -> Json<serde_json::Value> {
    let st = state.heartbeat_status.read().await;
    Json(serde_json::json!({
        "active": st.security.active_alerts,
        "ransomware": st.security.ransomware,
        "intrusion_score": st.security.intrusion_score,
    }))
}

/// GET /api/cpu — CPU identity, features, and temperature.
async fn api_cpu(State(state): State<AppState>) -> Json<serde_json::Value> {
    let st = state.heartbeat_status.read().await;
    match &st.cpu {
        Some(cpu) => Json(serde_json::to_value(cpu).unwrap_or_else(|_| serde_json::json!({}))),
        None => Json(serde_json::json!({"status": "detecting"})),
    }
}

/// GET /api/entropy-quality — latest entropy quality scoring.
async fn api_entropy_quality(State(state): State<AppState>) -> Json<serde_json::Value> {
    let st = state.heartbeat_status.read().await;
    match &st.entropy_quality {
        Some(eq) => Json(serde_json::to_value(eq).unwrap_or_else(|_| serde_json::json!({}))),
        None => Json(serde_json::json!({"status": "waiting_for_solutions"})),
    }
}

/// GET /events — Server-Sent Events stream of telemetry snapshots + heartbeat.
async fn api_sse(
    State(state): State<AppState>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let mut rx = state.handle.subscribe();
    let shutdown = state.shutdown.clone();
    let heartbeat_status = state.heartbeat_status.clone();
    let stream = async_stream::stream! {
        let mut hb_interval = tokio::time::interval(std::time::Duration::from_secs(3));
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                result = rx.recv() => {
                    match result {
                        Ok(snapshot) => {
                            if let Ok(json) = serde_json::to_string(&snapshot) {
                                yield Ok(Event::default().event("snapshot").data(json));
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
                _ = hb_interval.tick() => {
                    let st = heartbeat_status.read().await;
                    if let Ok(json) = serde_json::to_value(&*st) {
                        if let Ok(s) = serde_json::to_string(&json) {
                            yield Ok(Event::default().event("heartbeat").data(s));
                        }
                    }
                }
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// Read the last `limit` JSON lines from the telemetry file.
fn read_tail_snapshots(path: &PathBuf, limit: usize) -> Vec<TelemetrySnapshot> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return vec![],
    };
    let reader = BufReader::new(file);
    let mut lines: Vec<String> = reader
        .lines()
        .filter_map(|l| l.ok())
        .filter(|l| !l.trim().is_empty())
        .collect();
    // Keep only the last `limit` lines
    if lines.len() > limit {
        lines = lines.split_off(lines.len() - limit);
    }
    lines
        .into_iter()
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect()
}

/// Read the mining control file, defaulting to unpaused / 100% power.
fn read_control_file(path: &PathBuf) -> MiningControl {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Clamp power percentage to valid steps.
fn clamp_power(pct: u8) -> u8 {
    match pct {
        0..=37 => 25,
        38..=62 => 50,
        63..=87 => 75,
        _ => 100,
    }
}
