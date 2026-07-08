use serde::{Deserialize, Serialize};

/// Domain separation tag — every block hash includes this prefix.
pub const DOMAIN_TAG: &[u8] = b"BINA-L1-v1";

/// Genesis block's previous-hash sentinel.
pub const GENESIS_PREV_HASH: [u8; 32] = [0u8; 32];

/// Fixed genesis miner sentinel.
///
/// The genesis block is a consensus constant. It must not depend on whichever
/// wallet happens to start a local node first, otherwise every miner would be
/// on a different chain.
pub const GENESIS_MINER_ADDRESS: [u8; 20] = [0u8; 20];

/// The 80-ish byte block header whose BLAKE3 hash is the PoW target.
///
/// `bitcoin_seed_hash` = blake3("BINA-BTC-v1" || btc_tip_hash || stale_xor_pool).
/// For the current test it is set to a mock value; the live feed wires it in later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L1BlockHeader {
    pub version:          u32,
    pub height:           u64,
    pub prev_hash:        [u8; 32],
    pub merkle_root:      [u8; 32],   // hash of tx list; [0;32] for empty block
    /// Commitment to the full ledger state after executing this block's
    /// transactions and crediting its reward (`RewardLedger::state_root`).
    /// Computed by the miner *before* mining (execution is deterministic and
    /// doesn't depend on the nonce), so any validator can independently
    /// recompute it against its own copy of the parent state and reject a
    /// header that claims an execution result it cannot reproduce.
    pub state_root:       [u8; 32],
    /// Unix milliseconds. This is the *consensus* clock: difficulty retargeting
    /// and Bitcoin-seed checkpoint validation are pure functions of this field,
    /// replayed identically by every node from the chain history. It must never
    /// be derived from a node's own wall clock at accept-time — only from the
    /// value the block's own miner embedded and signed over. Second-granularity
    /// would be too coarse to measure sub-second epochs at this chain's block
    /// target, hence milliseconds rather than the more common Unix-seconds.
    pub timestamp:        u64,
    pub nonce:            u64,
    pub miner_address:    [u8; 20],   // secp256k1 pubkey hash (filled in by miner key later)
    pub difficulty_bits:  u32,        // leading zero bits required
    pub bitcoin_seed_hash: [u8; 32],  // Bitcoin entropy anchor
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L1Block {
    pub header: L1BlockHeader,
}

impl L1BlockHeader {
    /// BLAKE3 hash of the full header.
    /// hash = blake3(DOMAIN_TAG || all fields serialised LE)
    pub fn hash(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        h.update(DOMAIN_TAG);
        h.update(&self.version.to_le_bytes());
        h.update(&self.height.to_le_bytes());
        h.update(&self.prev_hash);
        h.update(&self.merkle_root);
        h.update(&self.state_root);
        h.update(&self.timestamp.to_le_bytes());
        h.update(&self.nonce.to_le_bytes());
        h.update(&self.miner_address);
        h.update(&self.difficulty_bits.to_le_bytes());
        h.update(&self.bitcoin_seed_hash);
        *h.finalize().as_bytes()
    }
}

/// Returns true if the first `bits` bits of `hash` are all zero.
///
/// This is the difficulty check: more leading zero bits = harder.
pub fn meets_difficulty(hash: &[u8; 32], bits: u32) -> bool {
    if bits == 0 {
        return true;
    }
    let full_bytes = (bits / 8) as usize;
    let remainder  = bits % 8;

    for &byte in hash.iter().take(full_bytes) {
        if byte != 0 {
            return false;
        }
    }
    if remainder > 0 && full_bytes < 32 {
        // top `remainder` bits of hash[full_bytes] must be zero
        let mask: u8 = 0xffu8 << (8 - remainder);
        if hash[full_bytes] & mask != 0 {
            return false;
        }
    }
    true
}

/// Count the number of leading zero bits in a hash.
pub fn leading_zero_bits(hash: &[u8; 32]) -> u32 {
    let mut count = 0u32;
    for &byte in hash.iter() {
        if byte == 0 {
            count += 8;
        } else {
            count += byte.leading_zeros();
            break;
        }
    }
    count
}

/// Returns the hardcoded genesis block (height 0, no real PoW).
pub fn genesis_block() -> L1Block {
    let header = L1BlockHeader {
        version:           1,
        height:            0,
        prev_hash:         GENESIS_PREV_HASH,
        merkle_root:       [0u8; 32],
        state_root:        crate::rewards::empty_state_root(),
        timestamp:         1751241600000, // 2025-06-30 00:00:00 UTC, in Unix ms
        nonce:             0,
        miner_address:     GENESIS_MINER_ADDRESS,
        difficulty_bits:   0,
        bitcoin_seed_hash: [0u8; 32],
    };
    L1Block { header }
}

/// Consensus rule for a block's embedded (miner-signed) timestamp: it must be
/// strictly greater than the previous block's timestamp (monotonic — closes
/// off "rewind the clock to bias difficulty retargeting") and must not be
/// further than `max_future_ms` ahead of the validator's own clock (bounds
/// how far a miner can push blocks into the future).
///
/// This is a pure function so any node — live-validating a fresh claim or
/// replaying history during sync — reaches the identical verdict.
pub fn timestamp_is_valid(candidate_ms: u64, prev_ms: u64, validator_now_ms: u64, max_future_ms: u64) -> bool {
    candidate_ms > prev_ms && candidate_ms <= validator_now_ms.saturating_add(max_future_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genesis_hash_is_deterministic() {
        let a = genesis_block().header.hash();
        let b = genesis_block().header.hash();
        assert_eq!(a, b);
    }

    #[test]
    fn timestamp_must_be_strictly_after_previous() {
        assert!(!timestamp_is_valid(1000, 1000, 1000, 30_000), "equal timestamp must be rejected");
        assert!(!timestamp_is_valid(999, 1000, 1000, 30_000), "earlier timestamp must be rejected");
        assert!(timestamp_is_valid(1001, 1000, 1001, 30_000));
    }

    #[test]
    fn timestamp_future_bound_enforced() {
        let now = 1_000_000u64;
        assert!(timestamp_is_valid(now + 30_000, 0, now, 30_000), "at the boundary must pass");
        assert!(!timestamp_is_valid(now + 30_001, 0, now, 30_000), "past the boundary must fail");
    }

    #[test]
    fn meets_difficulty_exact() {
        // A hash with 16 leading zero bits (first 2 bytes = 0)
        let mut hash = [0xffu8; 32];
        hash[0] = 0x00;
        hash[1] = 0x00;
        assert!(meets_difficulty(&hash, 16));
        assert!(!meets_difficulty(&hash, 17)); // 17th bit = 1 (0xff >> ... = fail)

        // All zeros passes any bit count
        let zeros = [0u8; 32];
        assert!(meets_difficulty(&zeros, 256));
    }

    #[test]
    fn leading_zero_bits_counts_correctly() {
        let mut hash = [0u8; 32];
        hash[2] = 0b0001_1111; // bit 3 from the top of byte 2 = first non-zero
        // 2 full zero bytes = 16 bits, then 3 more zero bits = 19 total
        assert_eq!(leading_zero_bits(&hash), 19);
    }
}
