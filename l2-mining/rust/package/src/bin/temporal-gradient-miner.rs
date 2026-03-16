use anyhow::{Context, Result};
use clap::Parser;
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use temporal_gradient_core::{
    ensure_app_layout, load_or_create_config, relay_connect, relay_connect_pinned,
    setup_logging, spawn_miner, wallet_address_from_config, MinerConfig, ReliableRelayChannel,
    SecureBuffer, SecureTransport, TelemetrySnapshot, TransportStatsSnapshot,
};
use tokio::signal;

#[derive(Debug, Parser)]
#[command(name = "temporal-gradient-miner")]
#[command(about = "Temporal Gradient L2 miner runtime")]
struct Cli {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    telemetry_file: Option<PathBuf>,
    #[arg(long)]
    exit_after_solutions: Option<u64>,
    #[arg(long)]
    relay_endpoint: Option<String>,
    #[arg(long)]
    relay_pin_sha256: Option<String>,
    #[arg(long)]
    relay_hmac_key: Option<String>,
    #[arg(long, default_value_t = false)]
    quiet: bool,
}

#[derive(Debug, Clone)]
struct RelayEgressConfig {
    endpoint: String,
    pinned_cert_sha256: Option<[u8; 32]>,
    hmac_key: Option<[u8; 32]>,
}

#[derive(Debug, Serialize)]
struct RelayTelemetryEnvelope<'a> {
    schema: &'static str,
    miner_name: &'a str,
    wallet_address: &'a str,
    telemetry: &'a TelemetrySnapshot,
}

#[derive(Debug, Clone)]
#[derive(Serialize)]
struct RelayStatus {
    enabled: bool,
    endpoint: Option<String>,
    state: RelayState,
    stats: TransportStatsSnapshot,
    last_error: Option<String>,
    updated_at: String,
}

impl Default for RelayStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: None,
            state: RelayState::Disabled,
            stats: TransportStatsSnapshot {
                bytes_sent: 0,
                bytes_received: 0,
                messages_sent: 0,
                messages_received: 0,
                noise_bytes_sent: 0,
                reconnect_count: 0,
                key_refreshes: 0,
                integrity_failures: 0,
            },
            last_error: None,
            updated_at: chrono_like_now(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum RelayState {
    Disabled,
    Connecting,
    Connected,
    Error,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = ensure_app_layout()?;
    let (mut config, config_path) = load_or_create_config(cli.config.as_deref())?;
    if let Some(path) = cli.telemetry_file {
        config.telemetry_file = Some(path.to_string_lossy().to_string());
    }
    if let Some(limit) = cli.exit_after_solutions {
        config.exit_after_solutions = Some(limit);
    }
    if let Some(endpoint) = cli.relay_endpoint {
        config.relay_endpoint = Some(endpoint);
    }
    if let Some(pin) = cli.relay_pin_sha256 {
        config.relay_pinned_cert_sha256 = Some(pin);
    }
    if let Some(hmac_key) = cli.relay_hmac_key {
        config.relay_hmac_key = Some(hmac_key);
    }

    setup_logging(&config.log_level)?;
    tracing::info!("Loaded config from {}", config_path.display());

    let telemetry_path = config.telemetry_path()?;
    let relay_status_path = relay_status_path_for_telemetry(&telemetry_path);
    if let Some(parent) = telemetry_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    tracing::info!("Telemetry stream: {}", telemetry_path.display());
    tracing::info!("Install root: {}", paths.install_root.display());

    let interactive = std::io::stdout().is_terminal() && !cli.quiet;
    let wallet_addr = wallet_address_from_config(&config).unwrap_or_else(|_| "unknown".into());
    let relay_config = RelayEgressConfig::from_miner_config(&config)?;
    let relay_status = Arc::new(Mutex::new(RelayStatus {
        enabled: relay_config.is_some(),
        endpoint: relay_config.as_ref().map(|cfg| cfg.endpoint.clone()),
        state: if relay_config.is_some() {
            RelayState::Connecting
        } else {
            RelayState::Disabled
        },
        ..RelayStatus::default()
    }));
    persist_relay_status(&relay_status, &relay_status_path)?;

    if interactive {
        print!("{}", display::banner());
        println!("{}", display::first_run_note());
    }

    let handle = spawn_miner(config.clone())?;
    let mut rx = handle.subscribe();
    let mut relay_rx = relay_config.as_ref().map(|_| handle.subscribe());
    let quiet = cli.quiet;
    let relay_wallet = wallet_addr.clone();
    let relay_miner_name = config.miner_name.clone();
    let writer_status = Arc::clone(&relay_status);
    let writer_task = tokio::spawn(async move {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&telemetry_path)
            .with_context(|| format!("Failed to open {}", telemetry_path.display()))?;

        while let Ok(snapshot) = rx.recv().await {
            // Always write JSON to the telemetry file
            let line = serde_json::to_string(&snapshot)?;
            writeln!(file, "{}", line)?;

            if interactive {
                let relay_status = writer_status
                    .lock()
                    .map(|guard| guard.clone())
                    .unwrap_or_default();
                print!("{}", display::format_dashboard(&snapshot, &wallet_addr, &relay_status));
            } else if !quiet {
                println!("{}", line);
            }
        }

        Ok::<(), anyhow::Error>(())
    });

    let relay_task = relay_config.map(|relay_config| {
        let relay_status = Arc::clone(&relay_status);
        let relay_status_path = relay_status_path.clone();
        tokio::spawn(async move {
            update_relay_status(&relay_status, |status| {
                status.state = RelayState::Connecting;
                status.last_error = None;
            });
            persist_relay_status(&relay_status, &relay_status_path)?;
            let mut channel = connect_relay_channel(&relay_config).await?;
            tracing::info!("Relay telemetry egress connected to {}", relay_config.endpoint);
            update_relay_status(&relay_status, |status| {
                status.state = RelayState::Connected;
                status.stats = channel.stats();
                status.last_error = None;
            });
            persist_relay_status(&relay_status, &relay_status_path)?;
            let rx = relay_rx
                .as_mut()
                .expect("relay receiver should exist when relay config is present");

            while let Ok(snapshot) = rx.recv().await {
                let envelope = RelayTelemetryEnvelope {
                    schema: "tg.telemetry.v1",
                    miner_name: &relay_miner_name,
                    wallet_address: &relay_wallet,
                    telemetry: &snapshot,
                };
                let payload = serde_json::to_vec(&envelope)?;
                match channel.send(&payload).await {
                    Ok(()) => {
                        let stats = channel.stats();
                        update_relay_status(&relay_status, |status| {
                            status.state = RelayState::Connected;
                            status.stats = stats;
                            status.last_error = None;
                        });
                        persist_relay_status(&relay_status, &relay_status_path)?;
                    }
                    Err(err) => {
                        let err = err.context(format!(
                            "Failed to relay telemetry to {}",
                            relay_config.endpoint
                        ));
                        update_relay_status(&relay_status, |status| {
                            status.state = RelayState::Error;
                            status.last_error = Some(format!("{err:#}"));
                            status.stats = channel.stats();
                        });
                        persist_relay_status(&relay_status, &relay_status_path)?;
                        return Err(err);
                    }
                }
            }

            update_relay_status(&relay_status, |status| {
                status.state = RelayState::Disabled;
            });
            persist_relay_status(&relay_status, &relay_status_path)?;
            channel.close().await.ok();
            Ok::<(), anyhow::Error>(())
        })
    });

    let shutdown = handle.shutdown_token();
    tokio::spawn(async move {
        if signal::ctrl_c().await.is_ok() {
            tracing::warn!("Ctrl+C received, shutting down miner");
            shutdown.cancel();
        }
    });

    handle.wait().await?;
    writer_task.await??;
    if let Some(task) = relay_task {
        task.await??;
    }
    Ok(())
}

impl RelayEgressConfig {
    fn from_miner_config(config: &MinerConfig) -> Result<Option<Self>> {
        let Some(endpoint) = config.relay_endpoint.clone().filter(|value| !value.trim().is_empty()) else {
            return Ok(None);
        };

        let pinned_cert_sha256 = match config.relay_pinned_cert_sha256.as_deref() {
            Some(value) if !value.trim().is_empty() => Some(parse_hex_32(value).with_context(|| {
                "Invalid relay_pinned_cert_sha256; expected 32-byte hex string"
            })?),
            _ => None,
        };

        let hmac_key = match config.relay_hmac_key.as_deref() {
            Some(value) if !value.trim().is_empty() => Some(parse_hex_32(value)
                .with_context(|| "Invalid relay_hmac_key; expected 32-byte hex string")?),
            _ => None,
        };

        Ok(Some(Self {
            endpoint,
            pinned_cert_sha256,
            hmac_key,
        }))
    }
}

async fn connect_relay_channel(config: &RelayEgressConfig) -> Result<ReliableRelayChannel> {
    match (config.pinned_cert_sha256, config.hmac_key) {
        (Some(pin), Some(hmac_key)) => relay_connect_pinned(&config.endpoint, pin, hmac_key).await,
        _ => relay_connect(&config.endpoint).await,
    }
}

fn parse_hex_32(value: &str) -> Result<[u8; 32]> {
    let trimmed = value.trim().strip_prefix("0x").unwrap_or(value.trim());
    let mut decoded = hex::decode(trimmed)
        .with_context(|| format!("Failed to decode hex value: {value}"))?;
    let decoded_len = decoded.len();
    let secure = SecureBuffer::from_slice(&decoded)
        .map_err(|err| anyhow::anyhow!("Failed to protect parsed secret in memory: {err}"))?;
    decoded.fill(0);
    secure
        .to_array::<32>()
        .map_err(|_| anyhow::anyhow!("Expected exactly 32 bytes, got {}", decoded_len))
}

fn update_relay_status(state: &Arc<Mutex<RelayStatus>>, update: impl FnOnce(&mut RelayStatus)) {
    if let Ok(mut guard) = state.lock() {
        update(&mut guard);
        guard.updated_at = chrono_like_now();
    }
}

fn persist_relay_status(state: &Arc<Mutex<RelayStatus>>, path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let snapshot = state
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    let payload = serde_json::to_string_pretty(&snapshot)?;
    std::fs::write(path, payload)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

fn relay_status_path_for_telemetry(telemetry_path: &std::path::Path) -> PathBuf {
    let parent = telemetry_path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let stem = telemetry_path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("miner-telemetry");
    parent.join(format!("{stem}.relay-status.json"))
}

fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{}", now)
}

// ─── Interactive terminal display ──────────────────────────────────────────

mod display {
    use super::{RelayState, RelayStatus};
    use temporal_gradient_core::{MiningPhase, TelemetrySnapshot};

    const DIVIDER: &str = "═══════════════════════════════════════════════════════════════";

    pub fn banner() -> String {
        format!(
            "\n{d}\n  \x1b[1;36mTemporal Gradient Miner v0.1.0\x1b[0m\n  CPU-only \u{00b7} Solo mining \u{00b7} Temperature-safe\n{d}\n",
            d = DIVIDER
        )
    }

    pub fn first_run_note() -> String {
        [
            "",
            "  \x1b[33mFIRST-RUN NOTE\x1b[0m",
            "  Your first reward takes ~2 hours (commit-reveal cycle).",
            "  After that, mining is continuous.",
            "",
            "  \x1b[2mWHY THE WAIT?  This delay is what makes the randomness trustless.",
            "  No one — not even you — can predict or manipulate the output.",
            "  It's not a limitation, it's the security model.\x1b[0m",
            "",
        ]
        .join("\n")
    }

    pub fn format_dashboard(snap: &TelemetrySnapshot, wallet: &str, relay: &RelayStatus) -> String {
        let mut out = String::with_capacity(2048);

        // Clear screen + move cursor home
        out.push_str("\x1b[2J\x1b[H");
        out.push_str(&banner());

        // ── Mining Status ──────────────────────────────────────────
        out.push_str("  \x1b[1;37mMINING STATUS\x1b[0m\n");
        let phase_str = snap
            .mining_phase
            .map(|p| phase_colored(&p))
            .unwrap_or_else(|| "Initializing...".into());
        out.push_str(&format!("  Phase:      {}\n", phase_str));
        out.push_str(&format!(
            "  Hashrate:   {} \x1b[2m|\x1b[0m Workers: {} \x1b[2m|\x1b[0m Uptime: {}\n",
            fmt_hashrate(snap.hashrate_hs),
            snap.worker_count,
            fmt_duration(snap.uptime_seconds)
        ));
        out.push_str(&format!(
            "  Hashes:     {} \x1b[2m|\x1b[0m Solutions: {}\n",
            fmt_number(snap.hashes),
            snap.solutions
        ));
        if let Some(ref hash) = snap.last_solution_hash_hex {
            out.push_str(&format!("  Last hash:  \x1b[2m{}\x1b[0m\n", shorten(hash, 18)));
        }

        out.push('\n');

        // ── Commit-Reveal Progress ─────────────────────────────────
        out.push_str("  \x1b[1;37mCOMMIT-REVEAL PROGRESS\x1b[0m\n");
        match snap.mining_phase {
            Some(MiningPhase::Searching) => {
                out.push_str("  Searching for valid hash below difficulty threshold...\n");
                out.push_str("  \x1b[2mSolutions are found quickly \u{2014} the wait is in the reveal.\x1b[0m\n");
            }
            Some(MiningPhase::SolutionFound) => {
                out.push_str("  \x1b[32;1m\u{2714} Solution found!\x1b[0m  Preparing commitment...\n");
            }
            Some(MiningPhase::WaitingForClearance) => {
                if let Some(remaining) = snap.phase_blocks_remaining {
                    let pct = 1.0 - (remaining as f32 / 500.0_f32).min(1.0);
                    out.push_str(&format!("  {}\n", progress_bar(pct, 50)));
                    out.push_str(&format!(
                        "  Waiting for prior commitment to expire \u{2014} {} blocks (~{})\n",
                        remaining,
                        fmt_duration(snap.phase_eta_seconds.unwrap_or(remaining * 12))
                    ));
                    out.push_str(
                        "  \x1b[2mOne-time wait from a prior interrupted session.\x1b[0m\n",
                    );
                }
            }
            Some(MiningPhase::Committing) => {
                out.push_str("  \x1b[33mSubmitting commitment transaction...\x1b[0m\n");
                out.push_str(
                    "  \x1b[2mYour solution hash is being locked on-chain.\x1b[0m\n",
                );
            }
            Some(MiningPhase::CommitmentLocked) => {
                if let Some(remaining) = snap.phase_blocks_remaining {
                    let eta = snap.phase_eta_seconds.unwrap_or(remaining * 12);
                    let total = 2u64.max(remaining); // minCommitmentAge = 2
                    let elapsed = total.saturating_sub(remaining);
                    let pct = (elapsed as f32 / total.max(1) as f32).clamp(0.0, 1.0);
                    out.push_str(&format!("  {}\n", progress_bar(pct, 50)));
                    out.push_str(&format!(
                        "  \x1b[36mCommitment locked\x1b[0m \u{2014} revealing in ~{}\n",
                        fmt_duration(eta)
                    ));
                    out.push_str(
                        "  \x1b[2mThis delay ensures trustless randomness.\x1b[0m\n",
                    );
                } else {
                    out.push_str("  Commitment locked \u{2014} preparing reveal...\n");
                }
            }
            Some(MiningPhase::Revealing) => {
                out.push_str("  \x1b[33;1mRevealing solution on-chain...\x1b[0m\n");
                out.push_str(
                    "  \x1b[2mYour reward is about to be minted!\x1b[0m\n",
                );
            }
            Some(MiningPhase::RewardReceived) => {
                out.push_str(
                    "  \x1b[32;1m\u{2714}\u{2714}\u{2714} REWARD RECEIVED \u{2714}\u{2714}\u{2714}\x1b[0m  TGBT minted to your wallet!\n",
                );
            }
            None => {
                out.push_str("  Initializing...\n");
            }
        }

        out.push('\n');

        // ── Rewards ────────────────────────────────────────────────
        out.push_str("  \x1b[1;37mREWARDS\x1b[0m\n");
        out.push_str("  Next reward:      ~1.0 TGBT\n");
        out.push_str(&format!(
            "  Session total:    \x1b[1m{:.1} TGBT\x1b[0m  ({} accepted, {} rejected)\n",
            snap.total_rewards_estimate, snap.accepted_submissions, snap.rejected_submissions
        ));
        out.push_str(&format!("  Wallet:           {}\n", shorten(wallet, 12)));
        out.push_str(
            "  \x1b[2mRewards are minted directly to your wallet as TGBT tokens.\x1b[0m\n",
        );

        out.push('\n');

        // ── Relay Egress ───────────────────────────────────────────
        out.push_str("  \x1b[1;37mRELAY EGRESS\x1b[0m\n");
        out.push_str(&format!(
            "  Status:           {}\n",
            relay_state_colored(relay.state)
        ));
        if let Some(endpoint) = &relay.endpoint {
            out.push_str(&format!("  Endpoint:         {}\n", shorten(endpoint, 28)));
        } else {
            out.push_str("  Endpoint:         not configured\n");
        }
        if relay.enabled {
            out.push_str(&format!(
                "  Mirrored:         {} msgs \x1b[2m|\x1b[0m {} sent \x1b[2m|\x1b[0m {} reconnects\n",
                fmt_number(relay.stats.messages_sent),
                fmt_bytes(relay.stats.bytes_sent),
                fmt_number(relay.stats.reconnect_count)
            ));
            out.push_str(&format!(
                "  Cover traffic:    {} \x1b[2m|\x1b[0m Key ratchets: {}\n",
                fmt_bytes(relay.stats.noise_bytes_sent),
                fmt_number(relay.stats.key_refreshes)
            ));
            if let Some(last_error) = &relay.last_error {
                out.push_str(&format!(
                    "  Last error:       \x1b[31m{}\x1b[0m\n",
                    shorten(last_error, 52)
                ));
            }
        } else {
            out.push_str("  \x1b[2mSecure relay egress is disabled for this miner session.\x1b[0m\n");
        }

        out.push_str(&format!("\n{}\n", DIVIDER));

        out
    }

    // ── Formatting helpers ─────────────────────────────────────────

    fn phase_colored(phase: &MiningPhase) -> String {
        match phase {
            MiningPhase::Searching => format!("\x1b[37m{}\x1b[0m", phase),
            MiningPhase::SolutionFound => format!("\x1b[32;1m{}\x1b[0m", phase),
            MiningPhase::WaitingForClearance => format!("\x1b[33m{}\x1b[0m", phase),
            MiningPhase::Committing => format!("\x1b[33m{}\x1b[0m", phase),
            MiningPhase::CommitmentLocked => format!("\x1b[36m{}\x1b[0m", phase),
            MiningPhase::Revealing => format!("\x1b[33;1m{}\x1b[0m", phase),
            MiningPhase::RewardReceived => format!("\x1b[32;1m{}\x1b[0m", phase),
        }
    }

    fn progress_bar(pct: f32, width: usize) -> String {
        let pct = pct.clamp(0.0, 1.0);
        let filled = (pct * width as f32).round() as usize;
        let empty = width.saturating_sub(filled);
        format!(
            "[\x1b[32m{}\x1b[0m{}] {:>3}%",
            "=".repeat(filled),
            "-".repeat(empty),
            (pct * 100.0).round() as u32
        )
    }

    fn fmt_duration(secs: u64) -> String {
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m {:02}s", secs / 60, secs % 60)
        } else {
            format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
        }
    }

    fn fmt_hashrate(hs: f64) -> String {
        if hs < 1000.0 {
            format!("{:.0} H/s", hs)
        } else if hs < 1_000_000.0 {
            format!("{:.1} kH/s", hs / 1000.0)
        } else {
            format!("{:.1} MH/s", hs / 1_000_000.0)
        }
    }

    fn fmt_number(n: u64) -> String {
        let s = n.to_string();
        let mut result = String::new();
        for (i, c) in s.chars().rev().enumerate() {
            if i > 0 && i % 3 == 0 {
                result.push(',');
            }
            result.push(c);
        }
        result.chars().rev().collect()
    }

    fn shorten(s: &str, visible: usize) -> String {
        if s.len() > visible + 4 {
            format!("{}...{}", &s[..visible.min(s.len())], &s[s.len().saturating_sub(4)..])
        } else {
            s.to_string()
        }
    }

    fn fmt_bytes(bytes: u64) -> String {
        const KB: f64 = 1024.0;
        const MB: f64 = KB * 1024.0;
        const GB: f64 = MB * 1024.0;
        let b = bytes as f64;
        if b < KB {
            format!("{} B", bytes)
        } else if b < MB {
            format!("{:.1} KB", b / KB)
        } else if b < GB {
            format!("{:.1} MB", b / MB)
        } else {
            format!("{:.2} GB", b / GB)
        }
    }

    fn relay_state_colored(state: RelayState) -> String {
        match state {
            RelayState::Disabled => "\x1b[2mdisabled\x1b[0m".to_string(),
            RelayState::Connecting => "\x1b[33mconnecting\x1b[0m".to_string(),
            RelayState::Connected => "\x1b[32;1mconnected\x1b[0m".to_string(),
            RelayState::Error => "\x1b[31;1merror\x1b[0m".to_string(),
        }
    }
}
