use anyhow::{Context, Result};
use clap::Parser;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use temporal_gradient_core::{
    ensure_app_layout, load_or_create_config, setup_logging, spawn_miner, TelemetrySnapshot,
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
            write_snapshot(&mut file, &snapshot, quiet)?;
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

fn write_snapshot(file: &mut std::fs::File, snapshot: &TelemetrySnapshot, quiet: bool) -> Result<()> {
    let line = serde_json::to_string(snapshot)?;
    writeln!(file, "{}", line)?;
    if !quiet {
        println!("{}", line);
    }
    Ok(())
}
