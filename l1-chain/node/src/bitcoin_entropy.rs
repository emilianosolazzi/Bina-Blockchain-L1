/// Live Bitcoin entropy fetcher.
///
/// Sources:
///   1. mempool.space      — current tip hash + block data
///   2. blockstream.info   — independent tip comparison
///   3. Coinbase script    — miner-committed coinbase script from the tip block
///
/// Important consensus note:
///   This module is a live HTTP fetcher. It is suitable for building a miner
///   block template and producing audit/provenance artifacts. It should not be
///   used by consensus validators as a live HTTP dependency.
///
/// Consensus validation should verify committed data/proofs, not re-fetch from
/// public APIs during block validation.

use anyhow::{anyhow, bail, Context, Result};
use l1_core::bitcoin_entropy::{blake3_keyed, hex_to_32, xor32, BtcEntropyState};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_MEMPOOL: &str = "https://mempool.space/api";
const DEFAULT_BLOCKSTREAM: &str = "https://blockstream.info/api";
const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// Runtime configuration for Bitcoin entropy fetching.
#[derive(Debug, Clone)]
pub struct BtcEntropyConfig {
    pub mempool_base_url: String,
    pub blockstream_base_url: String,
    pub timeout: Duration,
    /// If true, coinbase fetch failures fall back to the block merkle root.
    /// If false, coinbase fetch failures return an error.
    pub allow_coinbase_fallback: bool,
    /// If true, Blockstream tip fetch failures use deterministic fallback pool.
    /// If false, Blockstream failure returns an error.
    pub allow_provider_fallback: bool,
}

impl Default for BtcEntropyConfig {
    fn default() -> Self {
        Self {
            mempool_base_url: DEFAULT_MEMPOOL.to_string(),
            blockstream_base_url: DEFAULT_BLOCKSTREAM.to_string(),
            timeout: DEFAULT_HTTP_TIMEOUT,
            allow_coinbase_fallback: true,
            allow_provider_fallback: true,
        }
    }
}

/// Extra provenance useful for audits/logs/proof JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BtcEntropyFetchProof {
    pub mempool_tip_hash: String,
    pub mempool_tip_height: u64,
    pub mempool_merkle_root: String,
    pub mempool_block_nonce: u64,
    pub mempool_block_timestamp: u64,

    pub coinbase_txid: Option<String>,
    pub coinbase_script_present: bool,
    pub coinbase_fallback_used: bool,

    pub blockstream_tip_hash: Option<String>,
    pub provider_tip_divergence: bool,
    pub provider_fallback_used: bool,

    /// Local wall-clock fetch timestamp. Audit only; do not use in consensus seed.
    pub fetched_at_unix: u64,
}

/// Combined fetch result.
#[derive(Debug, Clone)]
pub struct LiveEntropyResult {
    pub state: BtcEntropyState,
    pub proof: BtcEntropyFetchProof,
}

// ── JSON response shapes ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct BlockInfo {
    height: u64,
    merkle_root: String,
    nonce: u64,
    timestamp: u64,
}

#[derive(Debug, Deserialize)]
struct TxInfo {
    vin: Vec<TxInput>,
}

#[derive(Debug, Deserialize)]
struct TxInput {
    /// Present for coinbase transactions in bitcoind-style APIs.
    coinbase: Option<String>,
    /// mempool.space returns coinbase script as scriptsig hex.
    scriptsig: Option<String>,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Backwards-compatible API: fetch live entropy using default settings.
///
/// Returns only the `BtcEntropyState`.
pub async fn fetch_live_entropy() -> Result<BtcEntropyState> {
    Ok(fetch_live_entropy_with_config(&BtcEntropyConfig::default())
        .await?
        .state)
}

/// Fetch live entropy plus provenance/audit proof.
pub async fn fetch_live_entropy_with_proof() -> Result<LiveEntropyResult> {
    fetch_live_entropy_with_config(&BtcEntropyConfig::default()).await
}

/// Fetch live Bitcoin entropy using explicit config.
pub async fn fetch_live_entropy_with_config(cfg: &BtcEntropyConfig) -> Result<LiveEntropyResult> {
    let client = Client::builder()
        .timeout(cfg.timeout)
        .build()
        .context("failed to build HTTP client")?;

    let fetched_at_unix = unix_now_secs();

    // ── Step 1: mempool.space tip hash ────────────────────────────────────────
    let tip_hex = get_text(
        &client,
        format!("{}/blocks/tip/hash", cfg.mempool_base_url),
        "mempool.space: tip/hash",
    )
    .await?;

    let tip_hex = tip_hex.trim().to_string();

    let tip_hash = hex_to_32(&tip_hex).context("mempool.space: tip hash not 32 bytes")?;

    // ── Step 2: block metadata ────────────────────────────────────────────────
    let block: BlockInfo = get_json(
        &client,
        format!("{}/block/{}", cfg.mempool_base_url, tip_hex),
        "mempool.space: block info",
    )
    .await?;

    let merkle_bytes = hex_to_32(&block.merkle_root)
        .context("mempool.space: merkle_root not 32 bytes")?;

    // ── Step 3: coinbase script entropy ───────────────────────────────────────
    let coinbase_result =
        fetch_coinbase_entropy(&client, &cfg.mempool_base_url, &tip_hex, merkle_bytes).await;

    let (utxo_entropy, coinbase_txid, coinbase_script_present, coinbase_fallback_used) =
        match coinbase_result {
            Ok(info) => (
                info.entropy,
                Some(info.coinbase_txid),
                info.script_present,
                false,
            ),
            Err(e) if cfg.allow_coinbase_fallback => {
                eprintln!(
                    "[btc-entropy] coinbase fetch failed: {e} — using merkle_root fallback"
                );
                (merkle_bytes, None, false, true)
            }
            Err(e) => return Err(e).context("coinbase entropy fetch failed"),
        };

    // ── Step 4: independent provider comparison ───────────────────────────────
    let stale_result = build_stale_pool(
        &client,
        cfg,
        &tip_hex,
        tip_hash,
        block.nonce,
        block.timestamp,
    )
    .await?;

    // Important:
    // Use Bitcoin block timestamp as the seed timestamp if BtcEntropyState hashes
    // this field. Local fetch timestamp belongs in proof only.
    let state = BtcEntropyState {
        tip_hash,
        utxo_entropy,
        stale_xor_pool: stale_result.stale_xor_pool,
        seed_timestamp: block.timestamp,
        tip_height: block.height,
        // Existing field name preserved for compatibility.
        // Semantically this means provider tip divergence, not proven Bitcoin fork.
        fork_detected: stale_result.provider_tip_divergence,
    };

    let proof = BtcEntropyFetchProof {
        mempool_tip_hash: tip_hex,
        mempool_tip_height: block.height,
        mempool_merkle_root: block.merkle_root,
        mempool_block_nonce: block.nonce,
        mempool_block_timestamp: block.timestamp,

        coinbase_txid,
        coinbase_script_present,
        coinbase_fallback_used,

        blockstream_tip_hash: stale_result.blockstream_tip_hash,
        provider_tip_divergence: stale_result.provider_tip_divergence,
        provider_fallback_used: stale_result.provider_fallback_used,

        fetched_at_unix,
    };

    Ok(LiveEntropyResult { state, proof })
}

// ── Internal fetch helpers ────────────────────────────────────────────────────

async fn get_text(client: &Client, url: String, label: &str) -> Result<String> {
    client
        .get(url)
        .send()
        .await
        .with_context(|| format!("{label}: request failed"))?
        .error_for_status()
        .with_context(|| format!("{label}: non-success HTTP status"))?
        .text()
        .await
        .with_context(|| format!("{label}: body read failed"))
}

async fn get_json<T>(client: &Client, url: String, label: &str) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    client
        .get(url)
        .send()
        .await
        .with_context(|| format!("{label}: request failed"))?
        .error_for_status()
        .with_context(|| format!("{label}: non-success HTTP status"))?
        .json::<T>()
        .await
        .with_context(|| format!("{label}: JSON parse failed"))
}

// ── Coinbase entropy ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct CoinbaseEntropyInfo {
    entropy: [u8; 32],
    coinbase_txid: String,
    script_present: bool,
}

/// Fetch and hash the coinbase script of the block's first transaction.
///
/// Output:
///   blake3("BINA-UTXO-v1" || coinbase_script_bytes) XOR merkle_root
///
/// If the coinbase script is missing, this returns the merkle root as a deliberate
/// fallback. Malformed hex returns an error.
async fn fetch_coinbase_entropy(
    client: &Client,
    mempool_base_url: &str,
    tip_hex: &str,
    merkle_bytes: [u8; 32],
) -> Result<CoinbaseEntropyInfo> {
    let txids_raw = get_text(
        client,
        format!("{}/block/{}/txids", mempool_base_url, tip_hex),
        "mempool.space: block txids",
    )
    .await?;

    let txids: Vec<String> =
        serde_json::from_str(&txids_raw).context("mempool.space: txids JSON parse failed")?;

    let coinbase_txid = txids
        .first()
        .cloned()
        .context("mempool.space: block has no transactions")?;

    let tx: TxInfo = get_json(
        client,
        format!("{}/tx/{}", mempool_base_url, coinbase_txid),
        "mempool.space: coinbase tx",
    )
    .await?;

    let script_hex = tx
        .vin
        .first()
        .and_then(|i| i.scriptsig.clone().or_else(|| i.coinbase.clone()))
        .unwrap_or_default();

    if script_hex.is_empty() {
        return Ok(CoinbaseEntropyInfo {
            entropy: merkle_bytes,
            coinbase_txid,
            script_present: false,
        });
    }

    let script_bytes =
        hex::decode(&script_hex).context("mempool.space: coinbase script hex decode failed")?;

    let script_hash = blake3_keyed(b"BINA-UTXO-v1", &script_bytes);

    Ok(CoinbaseEntropyInfo {
        entropy: xor32(script_hash, merkle_bytes),
        coinbase_txid,
        script_present: true,
    })
}

// ── Provider comparison / stale pool ──────────────────────────────────────────

#[derive(Debug, Clone)]
struct StalePoolResult {
    stale_xor_pool: [u8; 32],
    blockstream_tip_hash: Option<String>,
    provider_tip_divergence: bool,
    provider_fallback_used: bool,
}

/// Build the stale/provider-divergence pool.
///
/// If mempool.space and blockstream.info disagree, we XOR both hashes.
/// This indicates provider tip divergence, not necessarily a proven Bitcoin fork.
///
/// If they agree, or if Blockstream is unreachable and fallback is allowed,
/// we use committed Bitcoin block fields to avoid an all-zero pool.
async fn build_stale_pool(
    client: &Client,
    cfg: &BtcEntropyConfig,
    mempool_tip: &str,
    tip_hash: [u8; 32],
    block_nonce: u64,
    block_ts: u64,
) -> Result<StalePoolResult> {
    let bs_result = get_text(
        client,
        format!("{}/blocks/tip/hash", cfg.blockstream_base_url),
        "blockstream.info: tip/hash",
    )
    .await;

    match bs_result {
        Ok(bs_hex) => {
            let bs_hex = bs_hex.trim().to_string();

            if bs_hex != mempool_tip {
                let bs_hash = hex_to_32(&bs_hex)
                    .context("blockstream.info: tip hash not 32 bytes")?;

                eprintln!(
                    "[btc-entropy] BTC provider tip divergence: mempool={} blockstream={}",
                    short_hex(mempool_tip),
                    short_hex(&bs_hex)
                );

                Ok(StalePoolResult {
                    stale_xor_pool: xor32(tip_hash, bs_hash),
                    blockstream_tip_hash: Some(bs_hex),
                    provider_tip_divergence: true,
                    provider_fallback_used: false,
                })
            } else {
                Ok(StalePoolResult {
                    stale_xor_pool: fallback_pool(block_nonce, block_ts, tip_hash),
                    blockstream_tip_hash: Some(bs_hex),
                    provider_tip_divergence: false,
                    provider_fallback_used: false,
                })
            }
        }
        Err(e) if cfg.allow_provider_fallback => {
            eprintln!("[btc-entropy] blockstream.info unreachable: {e}");

            Ok(StalePoolResult {
                stale_xor_pool: fallback_pool(block_nonce, block_ts, tip_hash),
                blockstream_tip_hash: None,
                provider_tip_divergence: false,
                provider_fallback_used: true,
            })
        }
        Err(e) => Err(e).context("blockstream.info tip fetch failed"),
    }
}

/// Fallback pool from committed Bitcoin block fields.
///
/// This prevents an all-zero stale pool and binds the fallback to committed
/// Bitcoin data. It is not extra secret entropy.
fn fallback_pool(nonce: u64, timestamp: u64, tip_hash: [u8; 32]) -> [u8; 32] {
    let mut seed = [0u8; 32];
    seed[..8].copy_from_slice(&nonce.to_le_bytes());
    seed[8..16].copy_from_slice(&timestamp.to_le_bytes());
    xor32(seed, tip_hash)
}

// ── Small utilities ───────────────────────────────────────────────────────────

fn short_hex(s: &str) -> &str {
    let n = s.len().min(12);
    &s[..n]
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_hex_handles_short_input() {
        assert_eq!(short_hex("abc"), "abc");
        assert_eq!(short_hex("abcdefghijklmnop"), "abcdefghijkl");
    }

    #[test]
    fn fallback_pool_changes_with_nonce() {
        let tip_hash = [7u8; 32];

        let a = fallback_pool(1, 100, tip_hash);
        let b = fallback_pool(2, 100, tip_hash);

        assert_ne!(a, b);
    }

    #[test]
    fn fallback_pool_changes_with_timestamp() {
        let tip_hash = [7u8; 32];

        let a = fallback_pool(1, 100, tip_hash);
        let b = fallback_pool(1, 101, tip_hash);

        assert_ne!(a, b);
    }

    #[test]
    fn fallback_pool_is_nonzero_for_normal_tip_hash() {
        let tip_hash = [7u8; 32];

        let pool = fallback_pool(1, 100, tip_hash);

        assert_ne!(pool, [0u8; 32]);
    }

    #[test]
    fn default_config_is_valid() {
        let cfg = BtcEntropyConfig::default();

        assert!(cfg.mempool_base_url.starts_with("https://"));
        assert!(cfg.blockstream_base_url.starts_with("https://"));
        assert!(cfg.timeout.as_secs() > 0);
    }
}