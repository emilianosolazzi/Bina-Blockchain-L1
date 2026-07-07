/// Live Bitcoin entropy fetcher.
///
/// Sources:
///   1. mempool.space  — current tip hash + block data (merkle_root, coinbase script)
///   2. blockstream.info — independent tip; XOR with mempool tip to build stale_xor_pool
///   3. Dead UTXO: coinbase script of the tip block (all OP_RETURN-like committed data)
///
/// The three values feed into BtcEntropyState::bitcoin_seed_hash().

use anyhow::{Context, Result};
use l1_core::bitcoin_entropy::{blake3_keyed, hex_to_32, xor32, BtcEntropyState};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

const MEMPOOL:      &str = "https://mempool.space/api";
const BLOCKSTREAM:  &str = "https://blockstream.info/api";
/// Timeout for every individual HTTP call.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

// ── JSON response shapes ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct BlockInfo {
    height:     u64,
    merkle_root: String,
    nonce:      u64,
    timestamp:  u64,
}

#[derive(Deserialize)]
struct TxInfo {
    vin:  Vec<TxInput>,
}

#[derive(Deserialize)]
struct TxInput {
    /// Present for coinbase transactions (mempool.space: scriptsig hex, bitcoind: coinbase hex)
    coinbase:   Option<String>,
    /// mempool.space returns scriptsig as a plain hex string
    scriptsig:  Option<String>,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Fetch live Bitcoin entropy.  Falls back gracefully on any API error.
pub async fn fetch_live_entropy() -> Result<BtcEntropyState> {
    let client = Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()?;

    // ── Step 1: mempool.space tip hash ────────────────────────────────────────
    let tip_hex: String = client
        .get(format!("{}/blocks/tip/hash", MEMPOOL))
        .send().await
        .context("mempool.space: tip/hash request failed")?
        .text().await?;
    let tip_hex = tip_hex.trim().to_string();
    let tip_hash = hex_to_32(&tip_hex)
        .context("mempool.space: tip hash not 32 bytes")?;

    // ── Step 2: block metadata (merkle_root, nonce, height) ──────────────────
    let block: BlockInfo = client
        .get(format!("{}/block/{}", MEMPOOL, tip_hex))
        .send().await
        .context("mempool.space: block info request failed")?
        .json().await
        .context("mempool.space: block info JSON parse failed")?;

    let merkle_bytes = hex_to_32(&block.merkle_root)
        .unwrap_or([0u8; 32]);

    // ── Step 3: Dead UTXO entropy — coinbase script of the tip block ──────────
    //
    // The coinbase transaction commits to:
    //   • the block reward (unspendable once spent or buried)
    //   • the extra nonce chosen by the miner
    //   • often an OP_RETURN with pool tag / arbitrary data
    //
    // We fetch the first txid in the block (always the coinbase) via
    //   GET /block/{hash}/txids  → ["coinbase_txid", ...]
    // then
    //   GET /tx/{coinbase_txid}  → { vin: [{ coinbase: "hex…" }] }
    let utxo_entropy = fetch_coinbase_entropy(&client, &tip_hex, merkle_bytes).await
        .unwrap_or_else(|e| {
            eprintln!("[btc-entropy] coinbase fetch failed: {e} — using merkle_root fallback");
            merkle_bytes
        });

    // ── Step 4: Stale-block entropy — compare tips from two providers ─────────
    //
    // If mempool.space and blockstream.info return different tip hashes, the
    // providers disagree on the current Bitcoin tip. XOR the two hashes to make the stale_xor_pool non-zero.
    // If they agree, mix block nonce + timestamp so the pool is never all-zero.
    let (stale_xor_pool, fork_detected) =
        build_stale_pool(&client, &tip_hex, tip_hash, block.nonce, block.timestamp).await;

    let seed_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    Ok(BtcEntropyState {
        tip_hash,
        utxo_entropy,
        stale_xor_pool,
        seed_timestamp,
        tip_height: block.height,
        fork_detected,
    })
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Fetch and hash the coinbase script of the block's first transaction.
async fn fetch_coinbase_entropy(
    client:       &Client,
    tip_hex:      &str,
    merkle_bytes: [u8; 32],
) -> Result<[u8; 32]> {
    // GET /block/{hash}/txids returns a JSON array of txid strings
    let txids_raw = client
        .get(format!("{}/block/{}/txids", MEMPOOL, tip_hex))
        .send().await?
        .text().await
        .context("txids fetch failed")?;

    // The endpoint returns a JSON array; parse it
    let txids: Vec<String> = serde_json::from_str(&txids_raw)
        .context("txids JSON parse failed")?;

    let coinbase_txid = txids.first()
        .context("block has no transactions")?;

    // GET /tx/{txid}
    let tx: TxInfo = client
        .get(format!("{}/tx/{}", MEMPOOL, coinbase_txid))
        .send().await?
        .json().await
        .context("coinbase tx JSON parse failed")?;

    let script_hex: String = tx.vin.first()
        .and_then(|i| {
            // mempool.space coinbase tx: scriptsig is the coinbase script hex
            // bitcoind-style: coinbase field
            i.scriptsig.clone().or_else(|| i.coinbase.clone())
        })
        .unwrap_or_default();

    // blake3("BINA-UTXO-v1" || script_bytes) XOR merkle_root
    // The XOR binds the coinbase (historical) to the current block root (present).
    let script_bytes = hex::decode(&script_hex).unwrap_or_default();
    let script_hash  = blake3_keyed(b"BINA-UTXO-v1", &script_bytes);
    Ok(xor32(script_hash, merkle_bytes))
}

/// Return (stale_xor_pool, fork_detected).
async fn build_stale_pool(
    client:      &Client,
    mempool_tip: &str,
    tip_hash:    [u8; 32],
    block_nonce: u64,
    block_ts:    u64,
) -> ([u8; 32], bool) {
    let bs_result = async {
        client
            .get(format!("{}/blocks/tip/hash", BLOCKSTREAM))
            .send().await?
            .text().await
    }.await;

    match bs_result {
        Ok(bs_hex) => {
            let bs_hex = bs_hex.trim();
            if bs_hex != mempool_tip {
                // Real competing tip — XOR the two hashes
                match hex_to_32(bs_hex) {
                    Ok(bs_hash) => {
                        eprintln!("[btc-entropy] BTC tip divergence: mempool={} blockstream={}",
                            &mempool_tip[..12], &bs_hex[..12]);
                        (xor32(tip_hash, bs_hash), true)
                    }
                    Err(_) => (fallback_pool(block_nonce, block_ts, tip_hash), false),
                }
            } else {
                // Providers agree; mix nonce+timestamp so pool is never all-zero
                (fallback_pool(block_nonce, block_ts, tip_hash), false)
            }
        }
        Err(e) => {
            eprintln!("[btc-entropy] blockstream.info unreachable: {e}");
            (fallback_pool(block_nonce, block_ts, tip_hash), false)
        }
    }
}

/// When no fork is detected, build a non-zero pool from committed block fields.
fn fallback_pool(nonce: u64, timestamp: u64, tip_hash: [u8; 32]) -> [u8; 32] {
    let mut seed = [0u8; 32];
    seed[..8].copy_from_slice(&nonce.to_le_bytes());
    seed[8..16].copy_from_slice(&timestamp.to_le_bytes());
    xor32(seed, tip_hash)
}
