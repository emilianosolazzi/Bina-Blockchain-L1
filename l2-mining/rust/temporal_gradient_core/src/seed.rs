//! # Temporal Seed
//!
//! Encodes a UNIX timestamp into an 8-byte seed with a distinctive BOM
//! marker. The first byte is always `0x54` ('T') so that a zero-filled
//! buffer can never be mistaken for a valid seed.
//!
//! Layout: `[BOM(0x54)] [timestamp big-endian, 7 bytes]`
//!
//! The 56-bit timestamp field covers dates through the year ~4 million,
//! but a minimum bound (`MIN_TEMPORAL_SEED_TIMESTAMP`) rejects anything
//! before 2020-01-01 to catch uninitialised or bogus values.

use anyhow::{anyhow, Result};
use std::time::{SystemTime, UNIX_EPOCH};

/// Seed length in bytes.
pub const TEMPORAL_SEED_LENGTH: usize = 8;

/// Byte-order mark: ASCII 'T' (0x54). Chosen to be non-zero so that an
/// all-zeros buffer is never accepted as a valid seed.
pub const TEMPORAL_SEED_BOM: u8 = 0x54;

/// Maximum encodable timestamp (2^56 − 1 ≈ year 4,000,000+).
const MAX_TEMPORAL_SEED_TIMESTAMP: u64 = (1u64 << 56) - 1;

/// Minimum valid timestamp: 2020-01-01T00:00:00 UTC (epoch 1_577_836_800).
/// Anything earlier is almost certainly uninitialised or test data.
const MIN_TEMPORAL_SEED_TIMESTAMP: u64 = 1_577_836_800;

/// Encode a UNIX timestamp (seconds) into an 8-byte temporal seed.
///
/// Returns `Err` if the timestamp is outside the valid range
/// `[MIN_TEMPORAL_SEED_TIMESTAMP, MAX_TEMPORAL_SEED_TIMESTAMP]`.
pub fn encode_temporal_seed(timestamp_secs: u64) -> Result<[u8; TEMPORAL_SEED_LENGTH]> {
    if timestamp_secs < MIN_TEMPORAL_SEED_TIMESTAMP {
        return Err(anyhow!(
            "timestamp {} is before the minimum ({})",
            timestamp_secs,
            MIN_TEMPORAL_SEED_TIMESTAMP
        ));
    }
    if timestamp_secs > MAX_TEMPORAL_SEED_TIMESTAMP {
        return Err(anyhow!("timestamp exceeds 56-bit temporal seed capacity"));
    }

    let mut seed = [0u8; TEMPORAL_SEED_LENGTH];
    seed[0] = TEMPORAL_SEED_BOM;
    seed[1..].copy_from_slice(&timestamp_secs.to_be_bytes()[1..]);
    Ok(seed)
}

/// Decode the UNIX timestamp from an 8-byte temporal seed.
///
/// Validates the BOM marker and length before returning the timestamp.
pub fn decode_temporal_seed_timestamp(temporal_seed: &[u8]) -> Result<u64> {
    if temporal_seed.len() != TEMPORAL_SEED_LENGTH {
        return Err(anyhow!(
            "invalid temporal seed length: expected {}, got {}",
            TEMPORAL_SEED_LENGTH,
            temporal_seed.len()
        ));
    }
    if temporal_seed[0] != TEMPORAL_SEED_BOM {
        return Err(anyhow!(
            "invalid temporal seed BOM: expected 0x{:02x}, got 0x{:02x}",
            TEMPORAL_SEED_BOM,
            temporal_seed[0]
        ));
    }

    let mut timestamp_bytes = [0u8; 8];
    timestamp_bytes[1..].copy_from_slice(&temporal_seed[1..]);
    Ok(u64::from_be_bytes(timestamp_bytes))
}

/// Generate a temporal seed from the current system time.
///
/// Returns `Err` if the system clock is before the UNIX epoch or before
/// the minimum accepted timestamp (2020-01-01).
pub fn generate_temporal_seed() -> Result<[u8; TEMPORAL_SEED_LENGTH]> {
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| anyhow!("system clock is before UNIX epoch: {}", e))?
        .as_secs();
    encode_temporal_seed(now_secs)
}

/// Check whether a decoded seed timestamp is within `max_age_secs` of `now`.
///
/// Useful for commit-reveal freshness checks — reject seeds that are too
/// old or suspiciously in the future.
pub fn is_seed_fresh(temporal_seed: &[u8], max_age_secs: u64) -> Result<bool> {
    let seed_ts = decode_temporal_seed_timestamp(temporal_seed)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| anyhow!("system clock error: {}", e))?
        .as_secs();

    // Allow a small clock-skew grace (5 s) for seeds slightly in the future.
    const FUTURE_GRACE_SECS: u64 = 5;
    if seed_ts > now + FUTURE_GRACE_SECS {
        return Ok(false); // seed is in the future
    }

    Ok(now.saturating_sub(seed_ts) <= max_age_secs)
}

// ─────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Round-trip ───────────────────────────────────────────────

    #[test]
    fn round_trips_timestamp() {
        let ts = 1_746_385_920u64; // 2025-05-04
        let seed = encode_temporal_seed(ts).unwrap();
        assert_eq!(seed[0], TEMPORAL_SEED_BOM);
        assert_eq!(decode_temporal_seed_timestamp(&seed).unwrap(), ts);
    }

    #[test]
    fn round_trips_min_valid_timestamp() {
        let ts = MIN_TEMPORAL_SEED_TIMESTAMP;
        let seed = encode_temporal_seed(ts).unwrap();
        assert_eq!(decode_temporal_seed_timestamp(&seed).unwrap(), ts);
    }

    #[test]
    fn round_trips_large_timestamp() {
        let ts = (1u64 << 56) - 2; // near-max
        let seed = encode_temporal_seed(ts).unwrap();
        assert_eq!(decode_temporal_seed_timestamp(&seed).unwrap(), ts);
    }

    // ── Rejection ───────────────────────────────────────────────

    #[test]
    fn rejects_zero_timestamp() {
        assert!(encode_temporal_seed(0).is_err());
    }

    #[test]
    fn rejects_old_timestamp() {
        // 2019-12-31 — below the minimum
        assert!(encode_temporal_seed(1_577_750_400).is_err());
    }

    #[test]
    fn rejects_overflow_timestamp() {
        assert!(encode_temporal_seed(MAX_TEMPORAL_SEED_TIMESTAMP + 1).is_err());
    }

    #[test]
    fn rejects_wrong_length() {
        assert!(decode_temporal_seed_timestamp(&[0x54, 0, 0]).is_err());
    }

    #[test]
    fn rejects_wrong_bom() {
        let mut seed = encode_temporal_seed(1_746_385_920).unwrap();
        seed[0] = 0x00; // tamper BOM
        assert!(decode_temporal_seed_timestamp(&seed).is_err());
    }

    #[test]
    fn rejects_all_zeros() {
        // An uninitialised buffer must not pass.
        assert!(decode_temporal_seed_timestamp(&[0u8; 8]).is_err());
    }

    // ── BOM ─────────────────────────────────────────────────────

    #[test]
    fn bom_is_nonzero() {
        assert_ne!(TEMPORAL_SEED_BOM, 0, "BOM must not be zero");
    }

    // ── Generation ──────────────────────────────────────────────

    #[test]
    fn generate_produces_valid_seed() {
        let seed = generate_temporal_seed().unwrap();
        assert_eq!(seed[0], TEMPORAL_SEED_BOM);
        let ts = decode_temporal_seed_timestamp(&seed).unwrap();
        assert!(ts >= MIN_TEMPORAL_SEED_TIMESTAMP);
    }

    // ── Freshness ───────────────────────────────────────────────

    #[test]
    fn fresh_seed_is_fresh() {
        let seed = generate_temporal_seed().unwrap();
        assert!(is_seed_fresh(&seed, 60).unwrap());
    }

    #[test]
    fn old_seed_is_stale() {
        // 2020-01-02 — well over any reasonable max_age
        let seed = encode_temporal_seed(MIN_TEMPORAL_SEED_TIMESTAMP + 1).unwrap();
        assert!(!is_seed_fresh(&seed, 3600).unwrap());
    }
}
