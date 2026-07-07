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
    pub timestamp:        u64,        // Unix seconds
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
        timestamp:         1751241600, // 2025-06-30 00:00:00 UTC
        nonce:             0,
        miner_address:     GENESIS_MINER_ADDRESS,
        difficulty_bits:   0,
        bitcoin_seed_hash: [0u8; 32],
    };
    L1Block { header }
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
