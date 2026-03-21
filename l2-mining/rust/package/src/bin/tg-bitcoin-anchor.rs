use anyhow::{Context, Result};
use clap::Parser;
use serde::Serialize;
use temporal_gradient_core::{DeadUTXOAnchor, UTXOFetcher};

#[derive(Debug, Parser)]
#[command(name = "tg-bitcoin-anchor")]
#[command(about = "Anchor an epoch Merkle root to a dead Bitcoin UTXO")]
struct Cli {
    #[arg(long)]
    epoch_id: u64,
    #[arg(long)]
    merkle_root: String,
    #[arg(long, default_value = "op_return")]
    preference: String,
    #[arg(long)]
    storage_ref: Option<String>,
}

#[derive(Debug, Serialize)]
struct AnchorReport {
    epoch_id: u64,
    merkle_root: String,
    preference: String,
    storage_reference: Option<String>,
    anchor_id: String,
    anchor: Option<DeadUTXOAnchor>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let fetcher = UTXOFetcher::new();
    fetcher
        .ensure_csv_loaded_from_env()
        .await
        .map_err(anyhow::Error::msg)
        .context("dead UTXO inventory is required for Bitcoin anchoring")?;

    let anchor_bytes = parse_anchor_bytes(&cli.merkle_root);
    let anchor_id = fetcher
        .create_entropy_anchor_with_reference(
            &anchor_bytes,
            &cli.preference,
            cli.storage_ref.clone(),
        )
        .await
        .map_err(anyhow::Error::msg)
        .context("failed to create Bitcoin anchor")?;

    let report = AnchorReport {
        epoch_id: cli.epoch_id,
        merkle_root: cli.merkle_root,
        preference: cli.preference,
        storage_reference: cli.storage_ref,
        anchor: fetcher.get_anchor_by_id(&anchor_id).await,
        anchor_id,
    };

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn parse_anchor_bytes(value: &str) -> Vec<u8> {
    let normalized = value.trim();
    if let Some(hex_text) = normalized.strip_prefix("0x") {
        if hex_text.len() % 2 == 0 {
            if let Ok(bytes) = hex::decode(hex_text) {
                return bytes;
            }
        }
    }
    normalized.as_bytes().to_vec()
}