use anyhow::{Context, Result};
use clap::Parser;
use std::fs::OpenOptions;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use temporal_gradient_core::{
    ensure_app_layout, load_or_create_config, setup_logging, spawn_miner,
    wallet_address_from_config,
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
    #[arg(long, default_value_t = false)]
    quiet: bool,
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

    setup_logging(&config.log_level)?;
    tracing::info!("Loaded config from {}", config_path.display());

    let telemetry_path = config.telemetry_path()?;
    if let Some(parent) = telemetry_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    tracing::info!("Telemetry stream: {}", telemetry_path.display());
    tracing::info!("Install root: {}", paths.install_root.display());

    let interactive = std::io::stdout().is_terminal() && !cli.quiet;
    let wallet_addr = wallet_address_from_config(&config).unwrap_or_else(|_| "unknown".into());

    if interactive {
        print!("{}", display::banner());
        println!("{}", display::first_run_note());
    }

    let handle = spawn_miner(config.clone())?;
    let mut rx = handle.subscribe();
    let quiet = cli.quiet;
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
                print!("{}", display::format_dashboard(&snapshot, &wallet_addr));
            } else if !quiet {
                println!("{}", line);
            }
        }

        Ok::<(), anyhow::Error>(())
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
    Ok(())
}

// ─── Interactive terminal display ──────────────────────────────────────────

mod display {
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

    pub fn format_dashboard(snap: &TelemetrySnapshot, wallet: &str) -> String {
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
}
