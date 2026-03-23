// examples/stale_block_demo.rs
// ─────────────────────────────────────────────────────────────────
// Live demonstration of the TGBT Stale Block Miner against the
// real Bitcoin network via mempool.space public API.
//
// What this does:
//   1. Fetches the latest Bitcoin block height
//   2. Fetches the raw 80-byte header of the most recent blocks
//   3. Simulates a realistic fork scenario using adjacent real blocks
//   4. Runs the full stale block mining pipeline:
//      – Header parsing & double-SHA256 verification
//      – PoW leading-zero-bit counting
//      – Domain-tagged entropy extraction
//      – Quality scoring (PoW + reorg depth + freshness + divergence)
//      – StaleWorkProof generation & self-verification
//      – LoserChainTracker accumulation
//      – Fork event detection
//   5. Prints all results in a human-readable format
//
// Usage:
//   cargo run --example stale_block_demo -p temporal_gradient_core
// ─────────────────────────────────────────────────────────────────

use temporal_gradient_core::{
    ChainForkEvent, ChainTip, LoserChainTracker, ScoreBreakdown, StaleBlockHeader,
    StaleBlockMiner, StaleBlockMinerConfig, StaleEntropyReport, StaleWorkProof, TipStatus,
};

use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

const API_BASE: &str = "https://mempool.space/api";

// ─── API response types ─────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct BlockSummary {
    id: String,            // block hash hex
    height: u64,
    version: u32,
    timestamp: u64,
    nonce: u64,
    bits: u64,
    merkle_root: String,
    previousblockhash: String,
    difficulty: f64,
    size: u64,
    weight: u64,
    tx_count: u64,
}

// ─── Helpers ────────────────────────────────────────────────────

fn decode_hex_32(hex_str: &str) -> [u8; 32] {
    let bytes = hex::decode(hex_str).unwrap_or_else(|_| vec![0u8; 32]);
    let mut arr = [0u8; 32];
    let len = bytes.len().min(32);
    arr[..len].copy_from_slice(&bytes[..len]);
    arr
}

/// Bitcoin displays hashes in reversed byte order. mempool.space returns
/// them in display order. We need to reverse for internal representation.
fn decode_bitcoin_hash(hex_str: &str) -> [u8; 32] {
    let mut arr = decode_hex_32(hex_str);
    arr.reverse();
    arr
}

fn hash_hex_display(hash: &[u8; 32]) -> String {
    let mut reversed = *hash;
    reversed.reverse();
    hex::encode(reversed)
}

fn double_sha256(data: &[u8]) -> [u8; 32] {
    let first = Sha256::digest(data);
    Sha256::digest(first).into()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn separator() {
    println!("{}", "═".repeat(78));
}

fn thin_sep() {
    println!("{}", "─".repeat(78));
}

// ─── Main ───────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::builder()
        .user_agent("TGBT-StaleBlockMiner/0.1")
        .build()?;

    println!();
    separator();
    println!("  TGBT STALE BLOCK MINER — LIVE BITCOIN DEMONSTRATION");
    println!("  Connecting to mempool.space API...");
    separator();
    println!();

    // ── Step 1: Fetch latest block tip ───────────────────────────
    let tip_height: u64 = client
        .get(format!("{API_BASE}/blocks/tip/height"))
        .send()
        .await?
        .text()
        .await?
        .trim()
        .parse()?;

    println!("  [1] Current Bitcoin tip height: {tip_height}");
    println!();

    // ── Step 2: Fetch the last 5 blocks ──────────────────────────
    let blocks: Vec<BlockSummary> = client
        .get(format!("{API_BASE}/blocks/{tip_height}"))
        .send()
        .await?
        .json()
        .await?;

    let blocks: Vec<&BlockSummary> = blocks.iter().take(5).collect();

    println!("  [2] Fetched {} recent block headers:", blocks.len());
    thin_sep();
    for b in &blocks {
        println!(
            "      Height {:>8} | {} | nonce: {:>10} | txs: {:>5} | diff: {:.2e}",
            b.height,
            &b.id[..16],
            b.nonce,
            b.tx_count,
            b.difficulty,
        );
    }
    println!();

    // ── Step 3: Fetch raw headers and parse them ─────────────────
    println!("  [3] Fetching raw 80-byte headers & verifying PoW...");
    thin_sep();

    let mut parsed_headers: Vec<(StaleBlockHeader, BlockSummary)> = Vec::new();

    for block in &blocks {
        let raw_hex: String = client
            .get(format!("{API_BASE}/block/{}/header", block.id))
            .send()
            .await?
            .text()
            .await?;
        let raw_hex = raw_hex.trim();
        let raw_bytes = hex::decode(raw_hex)?;

        let header = StaleBlockHeader::from_raw(&raw_bytes, block.height)?;

        // Verify our double-SHA256 matches the block hash
        let computed_hash = double_sha256(&raw_bytes);
        let hash_matches = computed_hash == header.block_hash;

        let lz = header.leading_zero_bits();

        println!(
            "      Height {} | PoW: {} leading zeros | Hash verified: {} | Block: {}...",
            block.height,
            lz,
            if hash_matches { "✓" } else { "✗" },
            &block.id[..24],
        );

        // Clone block data for later use
        parsed_headers.push((header, BlockSummary {
            id: block.id.clone(),
            height: block.height,
            version: block.version,
            timestamp: block.timestamp,
            nonce: block.nonce,
            bits: block.bits,
            merkle_root: block.merkle_root.clone(),
            previousblockhash: block.previousblockhash.clone(),
            difficulty: block.difficulty,
            size: block.size,
            weight: block.weight,
            tx_count: block.tx_count,
        }));
    }
    println!();

    // ── Step 4: Entropy extraction from real headers ─────────────
    println!("  [4] Extracting domain-tagged entropy from each block header...");
    thin_sep();

    for (header, block) in &parsed_headers {
        let entropy = header.extract_entropy();
        println!("      Height {} | Entropy: {}", block.height, hex::encode(entropy));
    }
    println!();

    // ── Step 5: Simulate a fork — treat block N-1 as "stale" ────
    // In reality, stale blocks happen when two miners find a block at
    // the same height. We simulate this by treating the second-newest
    // block as if it were an orphan that lost to the newest block,
    // then processing it through the full pipeline.

    separator();
    println!("  [5] SIMULATED FORK SCENARIO");
    println!("      (Treating a real block as if it lost the chain-tip race)");
    separator();
    println!();

    if parsed_headers.len() >= 2 {
        let (canonical_header, canonical_block) = &parsed_headers[0];
        let (stale_header, stale_block) = &parsed_headers[1];

        println!("      CANONICAL (winner) : Height {} — {}",
            canonical_block.height, &canonical_block.id[..32]);
        println!("      STALE     (loser)  : Height {} — {}",
            stale_block.height, &stale_block.id[..32]);
        println!();

        // Create a StaleBlockMiner with relaxed settings for demo
        let config = StaleBlockMinerConfig {
            bitcoin_api_url: API_BASE.to_string(),
            api_key: None,
            poll_interval_secs: 30,
            min_leading_zeros: 0,    // Accept any PoW for demo
            max_stale_age_secs: 365 * 24 * 3600, // 1 year for demo
            submitter_address: "TGBT-Demo-Miner".to_string(),
            auto_submit: false,
        };
        let mut miner = StaleBlockMiner::new(config);

        // Submit the "stale" block
        let raw = stale_header.to_raw();
        let canonical_hash = canonical_header.block_hash;

        match miner.submit_stale_header(&raw, stale_block.height, canonical_hash, 1) {
            Ok(proof) => {
                println!("  ┌─ STALE WORK PROOF ────────────────────────────────────────┐");
                println!("  │ Proof ID      : {}...│", &proof.proof_id[..48]);
                println!("  │ Block Hash    : {}...│", &hash_hex_display(&proof.block_hash)[..48]);
                println!("  │ Canonical Hash: {}...│", &hash_hex_display(&proof.canonical_hash)[..48]);
                println!("  │ Height        : {:>48}│", proof.height);
                println!("  │ Leading Zeros : {:>48}│", proof.leading_zeros);
                println!("  │ Reorg Depth   : {:>48}│", proof.reorg_depth);
                println!("  │ Quality Score : {:>45}/100│", proof.quality_score);
                println!("  │ Entropy       : {}...│", &hex::encode(proof.entropy)[..48]);
                println!("  │ Submitter     : {:>48}│", proof.submitter);
                println!("  └────────────────────────────────────────────────────────────┘");
                println!();

                // Self-verify the proof
                match proof.verify_self() {
                    Ok(()) => println!("      ✓ Proof self-verification: PASSED"),
                    Err(e) => println!("      ✗ Proof self-verification: FAILED — {e}"),
                }
                println!();

                // Generate entropy report
                let report = StaleEntropyReport::build(stale_header, canonical_hash, 1);
                println!("  ┌─ ENTROPY REPORT ──────────────────────────────────────────┐");
                println!("  │ Block         : {}...│", &report.block_hash_hex[..48]);
                println!("  │ Primary       : {}...│", &hex::encode(report.primary_entropy)[..48]);
                println!("  │ Secondary     : {}...│", &hex::encode(report.secondary_entropy)[..48]);
                println!("  │ Fork Diverge  : {}...│", &hex::encode(report.fork_divergence_entropy)[..48]);
                println!("  │ Quality       : {:>45}/100│", report.quality_score);
                println!("  │ ├─ PoW Diff   : {:>45}/30 │", report.score_breakdown.pow_difficulty_score);
                println!("  │ ├─ Reorg Dep  : {:>45}/25 │", report.score_breakdown.reorg_depth_score);
                println!("  │ ├─ Divergence : {:>45}/25 │", report.score_breakdown.divergence_score);
                println!("  │ └─ Freshness  : {:>45}/20 │", report.score_breakdown.freshness_score);
                println!("  └────────────────────────────────────────────────────────────┘");
                println!();
            }
            Err(e) => {
                println!("      ✗ Could not create proof: {e}");
                println!();
            }
        }

        // ── Step 6: Simulate tip updates → fork detection ────────
        separator();
        println!("  [6] FORK DETECTION — Simulating chain tip transitions");
        separator();
        println!();

        let mut tracker = LoserChainTracker::new();

        // First observation: two competing tips at adjacent heights
        let initial_tips = vec![
            ChainTip {
                block_hash: stale_header.block_hash,
                height: stale_block.height,
                status: TipStatus::Active,
                first_seen: now_secs() - 60,
            },
            ChainTip {
                block_hash: canonical_header.block_hash,
                height: canonical_block.height,
                status: TipStatus::Competing,
                first_seen: now_secs() - 30,
            },
        ];
        let events = tracker.update_tips(initial_tips);
        println!("      Initial tips set: 1 Active + 1 Competing (no events yet)");
        println!("      Fork events detected: {}", events.len());
        println!();

        // Second observation: canonical wins, stale tip disappears
        let resolved_tips = vec![
            ChainTip {
                block_hash: canonical_header.block_hash,
                height: canonical_block.height,
                status: TipStatus::Active,
                first_seen: now_secs() - 30,
            },
        ];
        let events = tracker.update_tips(resolved_tips);
        println!("      Tips resolved: canonical wins, stale tip gone.");
        println!("      Fork events detected: {}", events.len());
        for event in &events {
            println!();
            println!("  ┌─ FORK EVENT ──────────────────────────────────────────────┐");
            println!("  │ Fork Height   : {:>48}│", event.fork_height);
            println!("  │ Winner        : {}...│", &hash_hex_display(&event.winner_hash)[..48]);
            println!("  │ Loser Count   : {:>48}│", event.loser_hashes.len());
            for (i, loser) in event.loser_hashes.iter().enumerate() {
                println!("  │ Loser #{}      : {}...│", i, &hash_hex_display(loser)[..48]);
            }
            println!("  │ Reorg Depth   : {:>48}│", event.reorg_depth);
            println!("  │ Fork Entropy  : {}...│", &hex::encode(event.fork_entropy)[..48]);
            println!("  │ Entropy Qual  : {:>45}/100│", event.entropy_quality());
            println!("  └────────────────────────────────────────────────────────────┘");
        }
        println!();

        // Tracker stats
        let stats = tracker.stats();
        thin_sep();
        println!("  LOSER CHAIN TRACKER STATS:");
        println!("      Total stale blocks  : {}", stats.total_stale_blocks);
        println!("      Total reorg events  : {}", stats.total_reorg_events);
        println!("      Avg quality score   : {}", stats.average_quality_score);
        println!("      Max reorg depth     : {}", stats.max_reorg_depth);
        println!("      Active tips         : {}", stats.active_tips);
        println!("      Competing tips      : {}", stats.competing_tips);
        println!("      Cumulative entropy  : {}...",
            &stats.cumulative_entropy_hex[..48]);
        thin_sep();
        println!();

        // ── Step 7: Process all blocks through the miner ─────────
        separator();
        println!("  [7] BATCH: Processing all {} blocks for entropy comparison", parsed_headers.len());
        separator();
        println!();

        for (i, (header, block)) in parsed_headers.iter().enumerate() {
            let is_canonical = i == 0;
            let entropy = header.extract_entropy();
            let lz = header.leading_zero_bits();

            println!("      Block {} (height {}) {}",
                i + 1,
                block.height,
                if is_canonical { "← CANONICAL" } else { "← LOSER CHAIN" }
            );
            println!("        Hash       : {}...", &block.id[..48]);
            println!("        PoW        : {} leading zero bits", lz);
            println!("        Nonce      : {}", block.nonce);
            println!("        Timestamp  : {} ({}s ago)",
                block.timestamp,
                now_secs().saturating_sub(block.timestamp));
            println!("        Merkle Root: {}...", &block.merkle_root[..48]);
            println!("        Entropy    : {}...", &hex::encode(entropy)[..48]);

            // Show how different the entropy is from block to block
            if i > 0 {
                let prev_entropy = parsed_headers[i - 1].0.extract_entropy();
                let xor: Vec<u8> = entropy.iter().zip(prev_entropy.iter())
                    .map(|(a, b)| a ^ b)
                    .collect();
                let differing_bits: u32 = xor.iter().map(|b| b.count_ones()).sum();
                println!("        Divergence : {} / 256 bits differ from previous block",
                    differing_bits);
            }
            println!();
        }
    }

    // ── Summary ──────────────────────────────────────────────────
    separator();
    println!("  DEMO COMPLETE");
    println!();
    println!("  What you just saw:");
    println!("    • Real Bitcoin block headers fetched from mempool.space");
    println!("    • Double-SHA256 PoW verification on live mainnet blocks");
    println!("    • Domain-tagged entropy extraction from every header field");
    println!("    • Quality scoring with 4-axis breakdown");
    println!("    • StaleWorkProof generation with self-verification");
    println!("    • Fork detection via chain-tip state transitions");
    println!("    • Cumulative XOR entropy accumulation across stale blocks");
    println!();
    println!("  In production, the StaleBlockMiner polls Bitcoin nodes for");
    println!("  chain tips and automatically detects ~1-2 real stale blocks");
    println!("  per day, harvesting their wasted PoW as high-quality entropy");
    println!("  for the Temporal Gradient Beacon.");
    separator();
    println!();

    Ok(())
}
