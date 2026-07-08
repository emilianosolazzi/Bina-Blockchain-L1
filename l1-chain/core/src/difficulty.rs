//! Bina Chain dynamic difficulty adjuster
//!
//! IMPORTANT — determinism: every timestamp fed into this adjuster (via
//! `new`/`restore` and `record_block`) MUST be a block's own consensus
//! timestamp (`L1BlockHeader.timestamp`, Unix ms), never a validator's local
//! wall clock. The adjuster is a pure fold over chain history: given the same
//! sequence of (height, timestamp) pairs from genesis, every node — whether
//! mining live, replaying a synced chain, or resuming after a restart —
//! reaches the exact same `current_bits` at every height. Feeding it wall
//! clock time makes the required difficulty node-local and non-replayable,
//! which breaks cross-node agreement on which claims are valid.
//!
//! Parameters
//! ─────────────────────────────────────────────────────────────────────────
//!   Target block time  : 40 ms
//!   Acceptable window  : derived by the epoch ratio before adjusting
//!   Epoch size         : 20 blocks         (adjust every 20 blocks)
//!   Difficulty range   : 25 – 45 leading zero bits
//!   Initial difficulty : 25 bits           (safe cold-start)
//!
//! Algorithm
//! ─────────────────────────────────────────────────────────────────────────
//!   At the end of each 20-block epoch:
//!     actual_ms  = wall-clock time for the last 20 blocks
//!     target_ms  = 20 × 40 = 800 ms
//!     ratio      = actual_ms / target_ms
//!     delta_bits = −round(log2(ratio))   [negative = too fast → harder]
//!     delta is capped to [−3, +3] per epoch to prevent oscillation
//!     new_bits   = clamp(current_bits + delta_bits, MIN_BITS, MAX_BITS)
//!
//!   Example:
//!     actual = 36,500 ms (half of target) → ratio = 0.5 → log2 = −1 → delta = +1 bit (harder)
//!     actual = 146,000 ms (2× target)    → ratio = 2.0 → log2 = +1 → delta = −1 bit (easier)

/// Minimum allowed difficulty (25 leading zero bits ≈ 33 M hashes expected).
pub const MIN_BITS: u32 = 25;
/// Maximum allowed difficulty (45 leading zero bits ≈ 35 T hashes expected).
pub const MAX_BITS: u32 = 45;
/// Target milliseconds per block.
pub const TARGET_BLOCK_MS: u64 = 40;
/// Number of blocks between difficulty adjustments.
pub const EPOCH_SIZE: u64 = 20;
/// Maximum bits to change in a single epoch (prevents wild swings).
pub const MAX_DELTA_BITS: i32 = 3;

/// Returned when the adjuster fires at the end of an epoch.
#[derive(Debug, Clone)]
pub struct AdjustmentInfo {
    /// The new difficulty in bits (after clamping).
    pub new_bits:      u32,
    /// Old difficulty before this adjustment.
    pub old_bits:      u32,
    /// Actual elapsed time for the epoch in milliseconds.
    pub actual_ms:     u64,
    /// Target elapsed time for the epoch in milliseconds.
    pub target_ms:     u64,
    /// Raw bit delta before clamping.
    pub raw_delta:     i32,
    /// Block height at which the adjustment occurred.
    pub at_height:     u64,
}

/// Tracks block timings and adjusts difficulty every `EPOCH_SIZE` blocks.
pub struct DifficultyAdjuster {
    current_bits:        u32,
    epoch_start_ms:      u64,   // wall-clock ms at start of current epoch
    epoch_start_height:  u64,
    last_adjustment:     Option<AdjustmentInfo>,
}

impl DifficultyAdjuster {
    /// Create a fresh adjuster for a brand-new chain, anchored at the
    /// genesis block's own consensus timestamp (Unix ms) — NOT wall clock.
    pub fn new(initial_bits: u32, genesis_timestamp_ms: u64) -> Self {
        let bits = initial_bits.max(MIN_BITS).min(MAX_BITS);
        Self {
            current_bits:       bits,
            epoch_start_ms:     genesis_timestamp_ms,
            epoch_start_height: 0,
            last_adjustment:    None,
        }
    }

    /// Restore an adjuster mid-chain from persisted or replayed state
    /// (e.g. after a node restart, or after syncing a range of blocks from a
    /// peer). `epoch_start_ms`/`epoch_start_height` must be the timestamp and
    /// height of the last epoch boundary this adjuster observed, so the next
    /// `record_block` call measures the correct epoch duration.
    pub fn restore(current_bits: u32, epoch_start_ms: u64, epoch_start_height: u64) -> Self {
        Self {
            current_bits: current_bits.max(MIN_BITS).min(MAX_BITS),
            epoch_start_ms,
            epoch_start_height,
            last_adjustment: None,
        }
    }

    /// Current difficulty in bits.
    pub fn current_bits(&self) -> u32 { self.current_bits }

    /// Consensus timestamp (ms) at the start of the current epoch — persist
    /// this alongside `current_bits`/`epoch_start_height` to resume exactly.
    pub fn epoch_start_ms(&self) -> u64 { self.epoch_start_ms }

    /// Height at the start of the current epoch.
    pub fn epoch_start_height(&self) -> u64 { self.epoch_start_height }

    /// Information about the most recent adjustment, if any.
    pub fn last_adjustment(&self) -> Option<&AdjustmentInfo> {
        self.last_adjustment.as_ref()
    }

    /// Call after each block is accepted onto the chain.
    ///
    /// `height`      — the height of the block just accepted (1-based)
    /// `block_ts_ms` — that block's own consensus timestamp (`header.timestamp`,
    ///                 Unix ms) — never a validator's wall clock.
    ///
    /// Returns `Some(AdjustmentInfo)` if this block completed an epoch and
    /// the difficulty was (potentially) changed.  Returns `None` otherwise.
    pub fn record_block(&mut self, height: u64, block_ts_ms: u64) -> Option<AdjustmentInfo> {
        if height == 0 || height % EPOCH_SIZE != 0 {
            return None;
        }

        let actual_ms  = block_ts_ms.saturating_sub(self.epoch_start_ms);
        let target_ms  = EPOCH_SIZE * TARGET_BLOCK_MS;
        let old_bits   = self.current_bits;

        let raw_delta = if actual_ms == 0 {
            MAX_DELTA_BITS   // pathological case: instant blocks → max hardening
        } else {
            let ratio     = actual_ms as f64 / target_ms as f64;
            let log_ratio = ratio.log2();
            // negative log_ratio → too fast → increase bits → delta > 0
            let d = (-log_ratio).round() as i32;
            d.max(-MAX_DELTA_BITS).min(MAX_DELTA_BITS)
        };

        let new_bits = ((old_bits as i32 + raw_delta)
            .max(MIN_BITS as i32)
            .min(MAX_BITS as i32)) as u32;

        self.current_bits       = new_bits;
        self.epoch_start_ms     = block_ts_ms;
        self.epoch_start_height = height;

        let info = AdjustmentInfo {
            new_bits,
            old_bits,
            actual_ms,
            target_ms,
            raw_delta,
            at_height: height,
        };
        self.last_adjustment = Some(info.clone());
        Some(info)
    }

    /// Human-readable summary of the last adjustment (for logging).
    pub fn adjustment_log(info: &AdjustmentInfo) -> String {
        let avg_ms = info.actual_ms / EPOCH_SIZE;
        let arrow  = if info.new_bits > info.old_bits { "▲ harder" }
                     else if info.new_bits < info.old_bits { "▼ easier" }
                     else { "= unchanged" };
        // Epochs spanning node downtime can average minutes+; show ms for
        // normal operation (target is 40ms) and seconds only past 10s.
        let avg = if avg_ms < 10_000 {
            format!("{avg_ms}ms")
        } else {
            format!("{:.2}s", avg_ms as f64 / 1000.0)
        };
        format!(
            "[difficulty] epoch at h={} | avg {}/block | {} bits → {} bits {} (Δ{})",
            info.at_height,
            avg,
            info.old_bits,
            info.new_bits,
            arrow,
            info.raw_delta,
        )
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_adjustment_mid_epoch() {
        let mut adj = DifficultyAdjuster::new(25, 0);
        for h in 1..20u64 {
            assert!(adj.record_block(h, h * TARGET_BLOCK_MS).is_none());
        }
    }

    #[test]
    fn perfect_timing_no_change() {
        let mut adj = DifficultyAdjuster::new(30, 0);
        // Exactly TARGET_BLOCK_MS per block → ratio = 1.0 → delta = 0
        let info = adj.record_block(20, 20 * TARGET_BLOCK_MS).unwrap();
        assert_eq!(info.new_bits, 30, "perfect timing should leave bits unchanged");
        assert_eq!(info.raw_delta, 0);
    }

    #[test]
    fn too_fast_increases_difficulty() {
        let mut adj = DifficultyAdjuster::new(30, 0);
        // Half the target time → ratio = 0.5 → log2 = -1 → delta = +1
        let info = adj.record_block(20, 20 * TARGET_BLOCK_MS / 2).unwrap();
        assert!(info.new_bits > 30, "too-fast epoch must increase difficulty");
    }

    #[test]
    fn too_slow_decreases_difficulty() {
        let mut adj = DifficultyAdjuster::new(30, 0);
        // Double the target time → ratio = 2.0 → log2 = +1 → delta = -1
        let info = adj.record_block(20, 20 * TARGET_BLOCK_MS * 2).unwrap();
        assert!(info.new_bits < 30, "too-slow epoch must decrease difficulty");
    }

    #[test]
    fn clamped_to_min() {
        let mut adj = DifficultyAdjuster::new(MIN_BITS, 0);
        // Very slow: ratio = 100 → log2 ≈ 6.6 → capped to -3, but MIN_BITS floors it
        let info = adj.record_block(20, 20 * TARGET_BLOCK_MS * 100).unwrap();
        assert_eq!(info.new_bits, MIN_BITS);
    }

    #[test]
    fn clamped_to_max() {
        let mut adj = DifficultyAdjuster::new(MAX_BITS, 0);
        // Instant blocks
        let info = adj.record_block(20, 0).unwrap();
        assert_eq!(info.new_bits, MAX_BITS);
    }

    #[test]
    fn restore_reproduces_identical_result_to_continuous_run() {
        // A continuously-running adjuster and one that "restarts" mid-chain
        // (restored from persisted state) must reach the same difficulty at
        // the same height when fed the same block timestamps.
        let mut continuous = DifficultyAdjuster::new(30, 0);
        continuous.record_block(20, 20 * TARGET_BLOCK_MS / 2).unwrap(); // fast epoch → harder

        let mut restored = DifficultyAdjuster::restore(30, 0, 0);
        restored.record_block(20, 20 * TARGET_BLOCK_MS / 2).unwrap();

        assert_eq!(continuous.current_bits(), restored.current_bits());
        assert_eq!(continuous.epoch_start_ms(), restored.epoch_start_ms());
        assert_eq!(continuous.epoch_start_height(), restored.epoch_start_height());
    }

    #[test]
    fn delta_capped_at_max_delta() {
        let mut adj = DifficultyAdjuster::new(30, 0);
        // Extremely fast → raw delta would be huge, must be capped
        let info = adj.record_block(20, 1).unwrap();
        assert!(info.raw_delta <= MAX_DELTA_BITS, "delta must not exceed MAX_DELTA_BITS");
    }
}
