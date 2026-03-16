use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use temporal_gradient_core::{
    ChallengeType, EntropyStorageVerifier, ProviderReputation, SettlementGateDecision,
    StorageProof, StorageProtocol,
};

#[derive(Debug, Parser)]
#[command(name = "tg-storage-attestation")]
#[command(about = "Verify epoch archive storage and emit a settlement attestation")]
struct Cli {
    #[arg(long)]
    epoch_file: PathBuf,
    #[arg(long, default_value = "epoch-store-local")]
    provider: String,
    #[arg(long, default_value = "http://127.0.0.1:4271")]
    entropy_endpoint: String,
    #[arg(long, default_value = "local-archive")]
    entropy_contract: String,
}

#[derive(Debug, Deserialize)]
struct EpochFile {
    #[serde(rename = "epochId")]
    epoch_id: u64,
    #[serde(rename = "merkleRoot")]
    merkle_root: Option<String>,
}

#[derive(Debug, Serialize)]
struct ArchiveVerificationReport {
    epoch_id: u64,
    epoch_file: String,
    file_size: u64,
    provider: String,
    verification_result: temporal_gradient_core::VerificationResult,
    settlement_gate: SettlementGateDecision,
    provider_reputation: ProviderReputation,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let file_bytes = fs::read(&cli.epoch_file)
        .with_context(|| format!("Failed to read {}", cli.epoch_file.display()))?;
    let epoch: EpochFile = serde_json::from_slice(&file_bytes)
        .with_context(|| format!("Failed to parse {}", cli.epoch_file.display()))?;

    let mut verifier =
        EntropyStorageVerifier::new(&cli.entropy_endpoint, &cli.entropy_contract);
    verifier.add_authorized_provider(StorageProtocol::Entropy, &cli.provider);

    let challenge = verifier
        .generate_challenge(
            &format!("epoch-{}", epoch.epoch_id),
            file_bytes.len() as u64,
            StorageProtocol::Entropy,
            &cli.provider,
            ChallengeType::RandomSampling,
        )
        .await
        .map_err(anyhow::Error::msg)?;

    let proof = StorageProof {
        challenge_id: challenge.challenge_id.clone(),
        file_id: challenge.file_id.clone(),
        provider_id: cli.provider.clone(),
        timestamp: now_unix(),
        data_samples: collect_samples(&file_bytes, &challenge.random_indices),
        merkle_proofs: None,
        provider_signature: format!(
            "local-archive:{}:{}",
            cli.provider,
            cli.epoch_file.display()
        ),
    };

    let verification_result = verifier
        .verify_proof(proof)
        .await
        .map_err(anyhow::Error::msg)?;
    let settlement_gate = verifier
        .evaluate_settlement_gate(
            &challenge.challenge_id,
            Some(epoch.epoch_id),
            epoch.merkle_root.clone(),
        )
        .await
        .map_err(anyhow::Error::msg)?;
    let provider_reputation = verifier.get_provider_reputation(&cli.provider).await;

    let report = ArchiveVerificationReport {
        epoch_id: epoch.epoch_id,
        epoch_file: cli.epoch_file.display().to_string(),
        file_size: file_bytes.len() as u64,
        provider: cli.provider,
        verification_result,
        settlement_gate,
        provider_reputation,
    };

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn collect_samples(file_bytes: &[u8], indices: &[u64]) -> Vec<Vec<u8>> {
    indices
        .iter()
        .map(|index| {
            let start = (*index as usize).min(file_bytes.len().saturating_sub(1));
            let end = (start + 64).min(file_bytes.len());
            file_bytes[start..end].to_vec()
        })
        .collect()
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}