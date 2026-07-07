//! Bina Chain dynamic difficulty adjuster
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
    /// Create a new adjuster starting at `initial_bits` and at `now_ms`.
    pub fn new(initial_bits: u32, now_ms: u64) -> Self {
        let bits = initial_bits.max(MIN_BITS).min(MAX_BITS);
        Self {
            current_bits:       bits,
            epoch_start_ms:     now_ms,
            epoch_start_height: 0,
            last_adjustment:    None,
        }
    }

    /// Current difficulty in bits.
    pub fn current_bits(&self) -> u32 { self.current_bits }

    /// Information about the most recent adjustment, if any.
    pub fn last_adjustment(&self) -> Option<&AdjustmentInfo> {
        self.last_adjustment.as_ref()
    }

    /// Call after each block is found.
    ///
    /// `height`  — the height of the block just mined (1-based)
    /// `now_ms`  — current wall-clock time in Unix milliseconds
    ///
    /// Returns `Some(AdjustmentInfo)` if this block completed an epoch and
    /// the difficulty was (potentially) changed.  Returns `None` otherwise.
    pub fn record_block(&mut self, height: u64, now_ms: u64) -> Option<AdjustmentInfo> {
        if height == 0 || height % EPOCH_SIZE != 0 {
            return None;
        }

        let actual_ms  = now_ms.saturating_sub(self.epoch_start_ms);
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
        self.epoch_start_ms     = now_ms;
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
        format!(
            "[difficulty] epoch at h={} | avg {:.2}s/block | {} bits → {} bits {} (Δ{})",
            info.at_height,
            avg_ms as f64 / 1000.0,
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
    fn delta_capped_at_max_delta() {
        let mut adj = DifficultyAdjuster::new(30, 0);
        // Extremely fast → raw delta would be huge, must be capped
        let info = adj.record_block(20, 1).unwrap();
        assert!(info.raw_delta <= MAX_DELTA_BITS, "delta must not exceed MAX_DELTA_BITS");
    }
}
