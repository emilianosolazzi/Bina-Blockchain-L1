use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Instant;

use crate::block::{meets_difficulty, L1Block, L1BlockHeader};

/// Result of a successful mining attempt.
pub struct MineResult {
    pub block:       L1Block,
    pub hashes_tried: u64,
    pub elapsed_ms:  u64,
    /// Hashes per second across all threads.
    pub hashrate_hs: f64,
}

/// Mine a new block by searching nonces until `block_hash` satisfies `difficulty_bits`
/// leading zero bits.
///
/// Uses `threads` OS threads (std::thread) that stripe the nonce space.
/// An AtomicBool abort flag lets all threads stop as soon as one wins.
pub fn mine_block(
    height:            u64,
    prev_hash:         [u8; 32],
    miner_address:     [u8; 20],
    bitcoin_seed_hash: [u8; 32],
    difficulty_bits:   u32,
    threads:           usize,
) -> MineResult {
    let start     = Instant::now();
    let found     = Arc::new(AtomicBool::new(false));
    let winner    = Arc::new(Mutex::new(None::<u64>)); // winning nonce
    let total_hashes = Arc::new(AtomicU64::new(0));

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Template: every thread clones this and only varies `nonce`.
    let template = L1BlockHeader {
        version:          1,
        height,
        prev_hash,
        merkle_root:      [0u8; 32],
        timestamp,
        nonce:            0,
        miner_address,
        difficulty_bits,
        bitcoin_seed_hash,
    };

    let handles: Vec<_> = (0..threads)
        .map(|tid| {
            let found        = Arc::clone(&found);
            let winner       = Arc::clone(&winner);
            let total_hashes = Arc::clone(&total_hashes);
            let mut header   = template.clone();
            let step         = threads as u64;
            let mut nonce    = tid as u64;

            std::thread::spawn(move || {
                let mut local = 0u64;

                loop {
                    // Check abort flag every iteration (cheap Relaxed load)
                    if found.load(Ordering::Relaxed) {
                        total_hashes.fetch_add(local, Ordering::Relaxed);
                        return;
                    }

                    header.nonce = nonce;
                    let hash = header.hash();
                    local += 1;

                    if meets_difficulty(&hash, difficulty_bits) {
                        // Signal all threads to stop
                        found.store(true, Ordering::Relaxed);
                        total_hashes.fetch_add(local, Ordering::Relaxed);
                        *winner.lock().unwrap() = Some(nonce);
                        return;
                    }

                    // Wrap nonce safely; stride by thread count
                    nonce = nonce.wrapping_add(step);

                    // Flush local counter periodically to keep total_hashes live
                    if local % 500_000 == 0 {
                        total_hashes.fetch_add(local, Ordering::Relaxed);
                        local = 0;
                    }
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("mining thread panicked");
    }

    let elapsed     = start.elapsed();
    let elapsed_ms  = elapsed.as_millis() as u64;
    let hashes      = total_hashes.load(Ordering::Relaxed);
    let hashrate_hs = if elapsed.as_secs_f64() > 0.0 {
        hashes as f64 / elapsed.as_secs_f64()
    } else {
        f64::INFINITY
    };

    let winning_nonce = winner.lock().unwrap().unwrap_or(0);
    let mut winning_header = template;
    winning_header.nonce = winning_nonce;

    MineResult {
        block: L1Block { header: winning_header },
        hashes_tried: hashes,
        elapsed_ms,
        hashrate_hs,
    }
}
