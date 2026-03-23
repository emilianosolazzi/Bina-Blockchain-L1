//! # Temporal Seed
//!
//! Encodes a UNIX timestamp into an 8-byte seed whose layout matches
//! the deployed `MiningLib.sol._validateTemporalSeed` contract on
//! Arbitrum.
//!
//! ## On-chain constraint (immutable)
//!
//! ```solidity
//! if (temporalSeed[0] != 0x00) revert InvalidBOMMarker();
//! uint64 seedTimestamp = uint64(bytes8(temporalSeed));
//! if (seedTimestamp < 1704067200) revert TimestampTooOld();
//! ```
//!
//! ## Layout
//!
//! `[0x00] [timestamp big-endian, 7 bytes]`
//!
//! Byte 0 is always `0x00` (contract-required BOM).  Bytes 1-7 hold
//! the lower 56 bits of the UNIX timestamp in big-endian.  The full
//! 8-byte buffer is read on-chain as `uint64(bytes8(temporalSeed))`,
//! which gives the same numeric value because current timestamps are
//! well below 2^56.
//!
//! Zero-buffer protection: an all-zero seed decodes to timestamp 0,
//! which fails the `MIN_TEMPORAL_SEED_TIMESTAMP` check in `decode`.

use anyhow::{anyhow, Result};
use std::time::{SystemTime, UNIX_EPOCH};

/// Seed length in bytes.
pub const TEMPORAL_SEED_LENGTH: usize = 8;

/// Byte-order mark: must be `0x00` to satisfy the deployed
/// `MiningLib._validateTemporalSeed` check on Arbitrum.
///
/// **Do not change this value** — the on-chain contract is immutable
/// and will revert any reveal whose first byte is non-zero.
pub const TEMPORAL_SEED_BOM: u8 = 0x00;

/// Maximum encodable timestamp (2^56 − 1 ≈ year 4,000,000+).
const MAX_TEMPORAL_SEED_TIMESTAMP: u64 = (1u64 << 56) - 1;

/// Minimum valid timestamp: 2020-01-01T00:00:00 UTC (epoch 1_577_836_800).
/// Anything earlier is almost certainly uninitialised or test data.
///
/// Note: the on-chain contract enforces a *stricter* minimum of
/// 1_704_067_200 (2024-01-01), so seeds that pass this Rust check may
/// still be rejected on-chain if they are between 2020 and 2024.
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
/// Validates:
/// 1. Length == 8.
/// 2. BOM byte == `0x00` (matches on-chain contract).
/// 3. Decoded timestamp >= `MIN_TEMPORAL_SEED_TIMESTAMP` (rejects
///    all-zero buffers and obviously bogus values).
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
    let ts = u64::from_be_bytes(timestamp_bytes);

    if ts < MIN_TEMPORAL_SEED_TIMESTAMP {
        return Err(anyhow!(
            "decoded timestamp {} is below minimum ({})",
            ts,
            MIN_TEMPORAL_SEED_TIMESTAMP
        ));
    }

    Ok(ts)
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
        seed[0] = 0xAB; // tamper BOM → non-zero, rejected by contract
        assert!(decode_temporal_seed_timestamp(&seed).is_err());
    }

    #[test]
    fn rejects_all_zeros() {
        // An uninitialised buffer has BOM=0x00 (valid) but timestamp=0
        // which is far below MIN_TEMPORAL_SEED_TIMESTAMP → rejected.
        let err = decode_temporal_seed_timestamp(&[0u8; 8]).unwrap_err();
        assert!(
            err.to_string().contains("below minimum"),
            "expected MIN_TEMPORAL_SEED_TIMESTAMP rejection, got: {err}"
        );
    }

    // ── BOM ─────────────────────────────────────────────────────

    #[test]
    fn bom_matches_contract() {
        // MiningLib._validateTemporalSeed requires temporalSeed[0] == 0x00
        assert_eq!(TEMPORAL_SEED_BOM, 0x00, "BOM must be 0x00 for contract compat");
        let seed = encode_temporal_seed(1_746_385_920).unwrap();
        assert_eq!(seed[0], 0x00);
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

    // ── Contract compatibility ──────────────────────────────────

    #[test]
    fn solidity_uint64_interpretation_matches() {
        // Solidity reads: uint64(bytes8(temporalSeed))
        // With BOM=0x00 the top byte is 0, so the Solidity uint64 is
        // identical to our decoded timestamp (for timestamps < 2^56).
        let ts = 1_746_385_920u64; // 2025-05-04
        let seed = encode_temporal_seed(ts).unwrap();

        // Simulate Solidity: interpret full 8 bytes as big-endian uint64
        let solidity_uint64 = u64::from_be_bytes(seed);
        assert_eq!(solidity_uint64, ts, "Rust decode must match Solidity uint64(bytes8(...))");
    }

    #[test]
    fn on_chain_min_timestamp_compatible() {
        // On-chain minimum is 1_704_067_200 (Jan 1 2024).
        // Our Rust minimum is 1_577_836_800 (Jan 1 2020).
        // Seeds in [2020, 2024) will pass Rust but fail on-chain — that's
        // fine, the contract is the final enforcer.
        let on_chain_min = 1_704_067_200u64;
        assert!(MIN_TEMPORAL_SEED_TIMESTAMP < on_chain_min);
        // But a current-era timestamp passes both Rust and contract:
        let ts = 1_746_385_920u64;
        assert!(ts >= on_chain_min);
        let seed = encode_temporal_seed(ts).unwrap();
        assert_eq!(seed[0], 0x00); // passes contract BOM check
    }
}
