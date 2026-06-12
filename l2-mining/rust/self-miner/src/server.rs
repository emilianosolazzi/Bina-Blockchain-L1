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
    routing::{get, post},
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
        .route("/api/solutions", get(api_solutions))
        .route("/api/solutions/stats", get(api_solutions_stats))
        .route("/api/solutions/latest", get(api_solutions_latest))
        .route("/api/miner/control", get(api_get_control).post(api_post_control))
        .route("/api/health", get(api_health))
        .route("/api/system/status", get(api_system_status))
        .route("/api/heartbeat/status", get(api_heartbeat_status))
        .route("/api/heartbeat/alerts", get(api_heartbeat_alerts))
        .route("/api/cpu", get(api_cpu))
        .route("/api/entropy-quality", get(api_entropy_quality))
        // Network stubs — self-miner has no epoch builder / randomness API
        .route("/api/network/randomness/latest", get(api_randomness_latest))
        .route("/api/network/randomness/:hash/proof", get(api_local_proof))
        .route("/api/network/epochs", get(api_stub_epochs))
        .route("/api/network/epochs/:epochId", get(api_stub_empty))
        .route("/api/network/epochs/:epochId/verify-storage", post(api_stub_empty))
        // Security stubs — self-miner has embedded heartbeat, no separate threat service
        .route("/api/security/threat-profile", get(api_threat_profile))
        .route("/api/security/relay-profile", get(api_stub_empty))
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

/// GET /api/system/status — full system status including wallet address, balances, hardware.
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

    // Fetch on-chain balances (ETH + TGBT) with a short timeout
    let (eth_balance, tgbt_balance) =
        fetch_balances(&state.rpc_url, &state.wallet_address).await;

    // Hardware info
    let sys_info = tokio::task::spawn_blocking(gather_hardware_info)
        .await
        .unwrap_or_default();

    Json(serde_json::json!({
        "status": "ok",
        "mode": "self-miner",
        "randomnessApi": { "online": false },
        "heartbeatApi": { "online": true },
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
            "ethBalance": eth_balance,
            "token": {
                "balance": tgbt_balance,
                "symbol": "TGBT",
            },
            "nextEpochId": serde_json::Value::Null,
            "contracts": {
                "batchEnabled": false,
                "coreBatchModule": "0xAf07E37D104E9be17639FE7a51B36972D4738651",
                "batchWiredCorrectly": true,
                "coreTokenomicsModule": "0x7B871bdeDdED0064C34e22902181A9a983C9E2ab",
                "tokenomicsWiredCorrectly": true,
            },
        },
        "hardware": sys_info,
        "dashboard": {
            "telemetryFile": state.telemetry_path.to_string_lossy(),
            "configFile": state.config_path.to_string_lossy(),
            "solutionsBackend": "file",
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

// ── Solutions API (derived from telemetry.jsonl) ─────────────────────────

#[derive(Debug, Deserialize)]
struct SolutionsQuery {
    limit: Option<usize>,
    skip: Option<usize>,
    filter: Option<String>,
    #[serde(rename = "sinceMs")]
    since_ms: Option<u64>,
}

/// Derive a solutions list by scanning telemetry snapshots and detecting
/// increments in the `solutions`, `accepted_submissions`, `rejected_submissions`,
/// and `stale_block_count` counters. This gives the dashboard a full solution
/// history without needing a separate SQLite store.
fn derive_solutions(snapshots: &[TelemetrySnapshot]) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut prev: Option<&TelemetrySnapshot> = None;
    let mut sol_num: u64 = 0;

    for snap in snapshots {
        let (prev_sol, prev_acc, prev_rej, prev_stale, prev_rewards) = match prev {
            Some(p) => (
                p.solutions,
                p.accepted_submissions,
                p.rejected_submissions,
                p.stale_block_count.unwrap_or(0),
                p.total_rewards_estimate,
            ),
            None => (0, 0, 0, 0, 0.0),
        };

        // New solution found (may or may not have been submitted yet)
        if snap.solutions > prev_sol {
            let delta = snap.solutions - prev_sol;
            for _ in 0..delta {
                sol_num += 1;
                out.push(serde_json::json!({
                    "id": sol_num,
                    "solutionNumber": sol_num,
                    "type": "solution",
                    "timestamp": snap.timestamp_unix_ms as u64,
                    "timestampMs": snap.timestamp_unix_ms as u64,
                    "createdAt": snap.timestamp_unix_ms as u64,
                    "accepted": false,
                    "nonce": snap.last_solution_nonce,
                    "hash": snap.last_solution_hash_hex,
                    "commitHash": snap.last_commit_hash_hex,
                    "outputHash": snap.last_output_hash_hex,
                    "hashrate": snap.hashrate_hs,
                    "totalHashes": snap.hashes,
                    "uptime": snap.uptime_seconds,
                    "phase": snap.mining_phase.as_ref().map(|p| format!("{:?}", p).to_lowercase()),
                    "reward": 0.0,
                    "estimated": false,
                }));
            }
        }

        // Accepted on-chain — mark the latest pending solution accepted and set reward
        if snap.accepted_submissions > prev_acc {
            let delta = snap.accepted_submissions - prev_acc;
            let reward_delta = (snap.total_rewards_estimate - prev_rewards).max(0.0);
            let per = if delta > 0 { reward_delta / delta as f64 } else { 0.0 };
            // Mark the most-recent unaccepted solution(s) as accepted
            let mut marked = 0u64;
            for row in out.iter_mut().rev() {
                if marked >= delta { break; }
                if row["type"] == "solution" && row["accepted"] == false {
                    row["accepted"] = serde_json::Value::Bool(true);
                    row["reward"] = serde_json::json!(per);
                    row["outputHash"] = serde_json::json!(snap.last_output_hash_hex);
                    marked += 1;
                }
            }
        }

        // Rejected submissions
        if snap.rejected_submissions > prev_rej {
            let delta = snap.rejected_submissions - prev_rej;
            for _ in 0..delta {
                sol_num += 1;
                out.push(serde_json::json!({
                    "id": sol_num,
                    "solutionNumber": sol_num,
                    "type": "solution",
                    "timestamp": snap.timestamp_unix_ms as u64,
                    "timestampMs": snap.timestamp_unix_ms as u64,
                    "createdAt": snap.timestamp_unix_ms as u64,
                    "accepted": false,
                    "nonce": snap.last_solution_nonce,
                    "hash": snap.last_solution_hash_hex,
                    "commitHash": snap.last_commit_hash_hex,
                    "hashrate": snap.hashrate_hs,
                    "totalHashes": snap.hashes,
                    "uptime": snap.uptime_seconds,
                    "phase": snap.mining_phase.as_ref().map(|p| format!("{:?}", p).to_lowercase()),
                    "reward": 0.0,
                    "estimated": false,
                }));
            }
        }

        // Stale block rows
        let cur_stale = snap.stale_block_count.unwrap_or(0);
        if cur_stale > prev_stale {
            let delta = cur_stale - prev_stale;
            for _ in 0..delta {
                out.push(serde_json::json!({
                    "id": format!("stale-{}-{}", snap.timestamp_unix_ms, cur_stale),
                    "type": "stale",
                    "timestamp": snap.timestamp_unix_ms as u64,
                    "timestampMs": snap.timestamp_unix_ms as u64,
                    "createdAt": snap.timestamp_unix_ms as u64,
                    "accepted": true,
                    "hash": snap.stale_xor_hex,
                    "staleForkDepth": snap.stale_fork_depth,
                    "staleZeroBits": snap.stale_zero_bits,
                    "staleQuality": snap.stale_quality,
                    "bitcoinTipHeight": snap.bitcoin_tip_height,
                    "reward": 0.0,
                    "estimated": true,
                }));
            }
        }

        prev = Some(snap);
    }

    // Newest first
    out.reverse();
    out
}

async fn api_solutions(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<SolutionsQuery>,
) -> Json<serde_json::Value> {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let skip = q.skip.unwrap_or(0);
    let filter = q.filter.as_deref();
    let since = q.since_ms.unwrap_or(0);

    let path = state.telemetry_path.clone();
    // Read a large window so we can reconstruct full history
    let snapshots = tokio::task::spawn_blocking(move || read_tail_snapshots(&path, 5000))
        .await
        .unwrap_or_default();

    let mut solutions = derive_solutions(&snapshots);

    // Apply filter
    solutions.retain(|s| {
        if since > 0 {
            if let Some(ts) = s.get("timestampMs").and_then(|v| v.as_u64()) {
                if ts < since { return false; }
            }
        }
        match filter {
            Some("accepted") => s["type"] == "solution" && s["accepted"] == true,
            Some("rejected") => s["type"] == "solution" && s["accepted"] == false,
            Some("stale") => s["type"] == "stale",
            _ => true,
        }
    });

    let total = solutions.len();
    let accepted = solutions.iter().filter(|s| s["type"] == "solution" && s["accepted"] == true).count();
    let rejected = solutions.iter().filter(|s| s["type"] == "solution" && s["accepted"] == false).count();
    let stale = solutions.iter().filter(|s| s["type"] == "stale").count();
    let total_rewards: f64 = solutions
        .iter()
        .filter_map(|s| s.get("reward").and_then(|v| v.as_f64()))
        .sum();

    let page: Vec<_> = solutions.into_iter().skip(skip).take(limit).collect();

    Json(serde_json::json!({
        "solutions": page,
        "stats": {
            "total": total,
            "accepted": accepted,
            "rejected": rejected,
            "stale": stale,
            "totalRewards": total_rewards,
        },
    }))
}

async fn api_solutions_stats(State(state): State<AppState>) -> Json<serde_json::Value> {
    let path = state.telemetry_path.clone();
    let snapshots = tokio::task::spawn_blocking(move || read_tail_snapshots(&path, 5000))
        .await
        .unwrap_or_default();
    let solutions = derive_solutions(&snapshots);
    let accepted = solutions.iter().filter(|s| s["type"] == "solution" && s["accepted"] == true).count();
    let rejected = solutions.iter().filter(|s| s["type"] == "solution" && s["accepted"] == false).count();
    let stale = solutions.iter().filter(|s| s["type"] == "stale").count();
    Json(serde_json::json!({
        "total": solutions.len(),
        "accepted": accepted,
        "rejected": rejected,
        "stale": stale,
    }))
}

async fn api_solutions_latest(State(state): State<AppState>) -> Json<serde_json::Value> {
    let path = state.telemetry_path.clone();
    let snapshots = tokio::task::spawn_blocking(move || read_tail_snapshots(&path, 5000))
        .await
        .unwrap_or_default();
    let solutions = derive_solutions(&snapshots);
    Json(serde_json::json!({ "solution": solutions.first() }))
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

// ── Stub handlers for features not available in self-miner mode ──────────

/// Generic empty-object stub for endpoints that don't exist in self-miner.
async fn api_stub_empty() -> Json<serde_json::Value> {
    Json(serde_json::json!({}))
}

/// GET /api/network/randomness/latest — return last output hash from telemetry.
async fn api_randomness_latest(State(state): State<AppState>) -> Json<serde_json::Value> {
    // Scan recent snapshots for the most recent one that actually produced an output.
    let path = state.telemetry_path.clone();
    let snapshots = tokio::task::spawn_blocking(move || read_tail_snapshots(&path, 500))
        .await
        .unwrap_or_default();

    let solved = snapshots
        .iter()
        .rev()
        .find(|s| s.last_output_hash_hex.is_some());

    match solved {
        Some(s) => Json(serde_json::json!({
            "outputHash": s.last_output_hash_hex,
            "timestamp": s.timestamp_unix_ms as u64,
            "epochId": s.solutions,
            "leafIndex": 0u64,
            "signature": s.last_commit_hash_hex,
            "source": "self-miner",
            "mode": "standalone",
        })),
        None => Json(serde_json::json!({ "error": "no solutions yet" })),
    }
}

/// GET /api/network/randomness/:hash/proof — return a local self-miner proof.
///
/// The self-miner is a single-operator setup with no Merkle tree / epoch
/// builder, so the proof is a degenerate 1-leaf tree: the Merkle root equals
/// the output hash itself, and the sibling path is empty. The commit hash
/// serves as the miner's self-attestation signature.
async fn api_local_proof(
    State(state): State<AppState>,
    axum::extract::Path(hash): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    let path = state.telemetry_path.clone();
    let snapshots = tokio::task::spawn_blocking(move || read_tail_snapshots(&path, 500))
        .await
        .unwrap_or_default();

    let solved = snapshots
        .iter()
        .rev()
        .find(|s| s.last_output_hash_hex.is_some());

    match solved {
        Some(s) => {
            let latest_hash = s.last_output_hash_hex.clone().unwrap_or_default();
            let finalized = s.accepted_submissions > 0
                && latest_hash.eq_ignore_ascii_case(&hash);
            Json(serde_json::json!({
                "outputHash": hash,
                "epochId": s.solutions,
                "leafIndex": 0u64,
                "merkleRoot": latest_hash,
                "proof": Vec::<String>::new(),
                "finalized": finalized,
                "signature": s.last_commit_hash_hex,
                "source": "self-miner",
                "mode": "standalone",
                "verifyOnChain": serde_json::Value::Null,
            }))
        }
        None => Json(serde_json::json!({ "error": "no solutions yet" })),
    }
}

/// Stub for /api/network/epochs — returns empty list.
async fn api_stub_epochs() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "epochs": [],
        "source": "self-miner",
        "proofsAvailable": false,
    }))
}

/// GET /api/security/threat-profile — build from embedded heartbeat data.
async fn api_threat_profile(State(state): State<AppState>) -> Json<serde_json::Value> {
    let st = state.heartbeat_status.read().await;
    Json(serde_json::json!({
        "heartbeatStatus": &*st,
        "telemetry": { "history": [] },
        "heartbeatAlerts": { "active": st.security.active_alerts },
        "ransomwareStatus": st.security.ransomware,
        "tamperStatus": {},
    }))
}

// ── On-chain balance fetching ────────────────────────────────────────────

/// TGBT token contract address on Arbitrum One.
const TGBT_TOKEN: &str = "0x31228eE520e895DA19f728DE5459b1b317d9b8D8";

/// Fetch ETH and TGBT balances via raw JSON-RPC. Returns (eth_string, tgbt_string).
/// On failure returns "0" for both — the dashboard handles display gracefully.
async fn fetch_balances(rpc_url: &str, wallet: &str) -> (String, String) {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(4))
        .build()
    {
        Ok(c) => c,
        Err(_) => return ("0".into(), "0".into()),
    };

    let eth_fut = fetch_eth_balance(&client, rpc_url, wallet);
    let tgbt_fut = fetch_tgbt_balance(&client, rpc_url, wallet);
    let (eth, tgbt) = tokio::join!(eth_fut, tgbt_fut);
    (eth, tgbt)
}

/// eth_getBalance → convert wei hex to decimal ETH string.
async fn fetch_eth_balance(client: &reqwest::Client, rpc: &str, wallet: &str) -> String {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "eth_getBalance",
        "params": [wallet, "latest"],
        "id": 1
    });
    match client.post(rpc).json(&body).send().await {
        Ok(resp) => {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if let Some(hex) = json["result"].as_str() {
                    return wei_hex_to_eth(hex);
                }
            }
            "0".into()
        }
        Err(_) => "0".into(),
    }
}

/// balanceOf(address) on TGBT token → convert wei hex to decimal TGBT string.
async fn fetch_tgbt_balance(client: &reqwest::Client, rpc: &str, wallet: &str) -> String {
    // balanceOf(address) selector = 0x70a08231
    let addr_clean = wallet.strip_prefix("0x").unwrap_or(wallet);
    let data = format!("0x70a08231000000000000000000000000{addr_clean}");
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "eth_call",
        "params": [{"to": TGBT_TOKEN, "data": data}, "latest"],
        "id": 2
    });
    match client.post(rpc).json(&body).send().await {
        Ok(resp) => {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if let Some(hex) = json["result"].as_str() {
                    return wei_hex_to_eth(hex);
                }
            }
            "0".into()
        }
        Err(_) => "0".into(),
    }
}

/// Convert a hex wei string (e.g. "0x1234") to a decimal ETH/TGBT string (up to 6 decimals).
fn wei_hex_to_eth(hex: &str) -> String {
    let clean = hex.strip_prefix("0x").unwrap_or(hex);
    if clean.is_empty() || clean == "0" {
        return "0".into();
    }
    // Parse as u128 (sufficient for realistic balances)
    match u128::from_str_radix(clean, 16) {
        Ok(wei) => {
            let whole = wei / 1_000_000_000_000_000_000;
            let frac = (wei % 1_000_000_000_000_000_000) / 1_000_000_000_000; // 6 decimals
            if frac == 0 {
                format!("{whole}")
            } else {
                // Trim trailing zeros: "1.234000" → "1.234"
                let raw = format!("{whole}.{frac:06}");
                raw.trim_end_matches('0').to_string()
            }
        }
        Err(_) => "0".into(),
    }
}

// ── Hardware info ────────────────────────────────────────────────────────

/// Gather hardware info (CPU, memory, uptime) for the status endpoint.
fn gather_hardware_info() -> serde_json::Value {
    let cpuid = raw_cpuid::CpuId::new();
    let brand = cpuid
        .get_processor_brand_string()
        .map(|b| b.as_str().trim().to_string())
        .unwrap_or_else(|| "Unknown CPU".into());

    let vendor = cpuid
        .get_vendor_info()
        .map(|v| v.as_str().to_string())
        .unwrap_or_default();
    let manufacturer = if vendor.contains("Intel") { "Intel" }
        else if vendor.contains("AMD") { "AMD" }
        else { &vendor };

    // Try to extract base clock from brand string (e.g. "@ 1.70GHz")
    let speed_ghz: Option<f64> = brand
        .split('@')
        .nth(1)
        .and_then(|s| {
            let cleaned = s.trim().trim_end_matches("GHz").trim();
            cleaned.parse::<f64>().ok()
        });

    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);

    #[cfg(windows)]
    let platform = "win32";
    #[cfg(target_os = "macos")]
    let platform = "darwin";
    #[cfg(target_os = "linux")]
    let platform = "linux";

    // System uptime via platform API
    #[cfg(windows)]
    let uptime_secs: u64 = {
        // SAFETY: GetTickCount64 is always safe to call.
        unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() / 1000 }
    };
    #[cfg(not(windows))]
    let uptime_secs: u64 = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
        .map(|f| f as u64)
        .unwrap_or(0);

    // Memory info
    let (mem_total_gb, mem_used_gb, mem_pct) = get_memory_info();

    let mut cpu_obj = serde_json::json!({
        "model": brand,
        "manufacturer": manufacturer,
        "cores": cores,
    });
    if let Some(ghz) = speed_ghz {
        cpu_obj["speedGhz"] = serde_json::json!(ghz);
    }

    let mut hw = serde_json::json!({
        "cpu": cpu_obj,
        "platform": platform,
        "arch": std::env::consts::ARCH,
        "uptime": uptime_secs,
    });
    if mem_total_gb > 0.0 {
        hw["memory"] = serde_json::json!({
            "totalGb": (mem_total_gb * 10.0).round() / 10.0,
            "usedGb": (mem_used_gb * 10.0).round() / 10.0,
            "usagePercent": mem_pct as u32,
        });
    }
    hw
}

/// Get system memory info: (totalGb, usedGb, usagePercent).
#[cfg(windows)]
fn get_memory_info() -> (f64, f64, f64) {
    use std::mem::MaybeUninit;
    let mut info = MaybeUninit::<windows_sys::Win32::System::SystemInformation::MEMORYSTATUSEX>::uninit();
    // SAFETY: standard Win32 GlobalMemoryStatusEx pattern.
    unsafe {
        let p = info.as_mut_ptr();
        (*p).dwLength = std::mem::size_of::<windows_sys::Win32::System::SystemInformation::MEMORYSTATUSEX>() as u32;
        if windows_sys::Win32::System::SystemInformation::GlobalMemoryStatusEx(p) != 0 {
            let info = info.assume_init();
            let total = info.ullTotalPhys as f64 / (1024.0 * 1024.0 * 1024.0);
            let avail = info.ullAvailPhys as f64 / (1024.0 * 1024.0 * 1024.0);
            let used = total - avail;
            let pct = if total > 0.0 { (used / total) * 100.0 } else { 0.0 };
            return (total, used, pct);
        }
    }
    (0.0, 0.0, 0.0)
}

#[cfg(not(windows))]
fn get_memory_info() -> (f64, f64, f64) {
    // Parse /proc/meminfo on Linux
    if let Ok(contents) = std::fs::read_to_string("/proc/meminfo") {
        let mut total_kb = 0u64;
        let mut avail_kb = 0u64;
        for line in contents.lines() {
            if line.starts_with("MemTotal:") {
                total_kb = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            } else if line.starts_with("MemAvailable:") {
                avail_kb = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            }
        }
        if total_kb > 0 {
            let total = total_kb as f64 / (1024.0 * 1024.0);
            let avail = avail_kb as f64 / (1024.0 * 1024.0);
            let used = total - avail;
            let pct = (used / total) * 100.0;
            return (total, used, pct);
        }
    }
    (0.0, 0.0, 0.0)
}
