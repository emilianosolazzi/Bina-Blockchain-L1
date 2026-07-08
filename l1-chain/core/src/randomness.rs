//! Randomness output + nullifier system for Bina Chain.
//!
//! Each mined block produces exactly ONE randomness output.  The output is
//! derived deterministically from the block hash and the Bitcoin seed, so it
//! is:
//!
//!  * **Unpredictable** — no one knows the valid nonce (and therefore the block
//!    hash) before the block is found.  The bitcoin_seed_hash further commits
//!    to live Bitcoin chain state that the miner does not control.
//!
//!  * **Unbiasable** — a miner who dislikes an output must discard the entire
//!    solved block and re-mine from scratch (full difficulty cost).
//!
//!  * **Unique** — `height` is monotonically increasing; two blocks at the
//!    same height cannot coexist in a valid chain.
//!
//!  * **Non-double-spendable** — a `NullifierSet` records each output's
//!    nullifier the first time it is consumed.  Any subsequent attempt to
//!    consume the same height returns `Err(AlreadySpent)`.
//!
//! Domain tags (never reused across other hashes in this codebase):
//!   Output   : "BINA-RAND-v1"
//!   Nullifier: "BINA-NULL-v1"

use std::collections::HashSet;
use anyhow::{bail, Result};
use blake3::Hasher;

// ──────────────────────────────────────────────────────────────────────────────
// Domain tags
// ──────────────────────────────────────────────────────────────────────────────
const TAG_OUTPUT:    &[u8] = b"BINA-RAND-v1";
const TAG_NULLIFIER: &[u8] = b"BINA-NULL-v1";

// ──────────────────────────────────────────────────────────────────────────────
// RandomnessOutput
// ──────────────────────────────────────────────────────────────────────────────

/// A verifiable, one-time-use randomness output produced by a mined L1 block.
#[derive(Debug, Clone)]
pub struct RandomnessOutput {
    /// Block height — acts as the unique sequence number.
    pub height: u64,
    /// Raw BLAKE3 hash of the mined block header.
    pub block_hash: [u8; 32],
    /// Bitcoin entropy seed baked into the block at mine time.
    pub bitcoin_seed_hash: [u8; 32],
    /// The actual random bytes exposed to consumers.
    ///   `blake3(TAG_OUTPUT || block_hash || bitcoin_seed_hash)`
    pub output: [u8; 32],
    /// One-time spend token.
    ///   `blake3(TAG_NULLIFIER || height_le8 || output)`
    pub nullifier: [u8; 32],
}

impl RandomnessOutput {
    /// Derive a `RandomnessOutput` from a mined block.
    pub fn from_block(
        height: u64,
        block_hash: [u8; 32],
        bitcoin_seed_hash: [u8; 32],
    ) -> Self {
        // output = blake3(TAG || height_le8 || block_hash || bitcoin_seed_hash)
        let output = {
            let mut h = Hasher::new();
            h.update(TAG_OUTPUT);
            h.update(&height.to_le_bytes());
            h.update(&block_hash);
            h.update(&bitcoin_seed_hash);
            *h.finalize().as_bytes()
        };

        // nullifier = blake3(TAG || height_le8 || output)
        let nullifier = {
            let mut h = Hasher::new();
            h.update(TAG_NULLIFIER);
            h.update(&height.to_le_bytes());
            h.update(&output);
            *h.finalize().as_bytes()
        };

        RandomnessOutput { height, block_hash, bitcoin_seed_hash, output, nullifier }
    }

    /// Return the randomness output as a hex string (64 chars).
    pub fn output_hex(&self) -> String { hex::encode(self.output) }

    /// Return the nullifier as a hex string (64 chars).
    pub fn nullifier_hex(&self) -> String { hex::encode(self.nullifier) }

    /// Count the number of leading zero bits in the randomness output.
    pub fn leading_zero_bits(&self) -> u32 {
        crate::block::leading_zero_bits(&self.output)
    }

    /// Verify that this output was derived correctly from its claimed block.
    pub fn verify(&self) -> bool {
        let expected = RandomnessOutput::from_block(
            self.height,
            self.block_hash,
            self.bitcoin_seed_hash,
        );
        expected.output    == self.output &&
        expected.nullifier == self.nullifier
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// NullifierSet — enforces non-double-spend
// ──────────────────────────────────────────────────────────────────────────────

/// In-memory nullifier registry.  In Phase 2 this will be backed by sled.
///
/// Thread safety: wrap in `Arc<Mutex<NullifierSet>>` for concurrent access.
#[derive(Default)]
pub struct NullifierSet {
    spent: HashSet<[u8; 32]>,
}

impl NullifierSet {
    pub fn new() -> Self { Self::default() }

    /// Mark `output` as consumed exactly once.
    ///
    /// Returns `Ok(output.output)` — the 32 random bytes the caller can use.
    /// Returns `Err(AlreadySpent)` if this nullifier was already recorded.
    pub fn consume(&mut self, output: &RandomnessOutput) -> Result<[u8; 32]> {
        if !self.spent.insert(output.nullifier) {
            bail!(
                "double-spend: height {} nullifier {} already spent",
                output.height,
                output.nullifier_hex()
            );
        }
        Ok(output.output)
    }

    /// Check without spending.
    pub fn is_spent(&self, output: &RandomnessOutput) -> bool {
        self.spent.contains(&output.nullifier)
    }

    /// Directly record a nullifier as spent — used to rebuild the set from
    /// persisted chain history at startup. Returns `false` if it was
    /// already present.
    pub fn mark_spent(&mut self, nullifier: [u8; 32]) -> bool {
        self.spent.insert(nullifier)
    }

    /// Number of outputs consumed so far.
    pub fn len(&self) -> usize { self.spent.len() }

    pub fn is_empty(&self) -> bool { self.spent.is_empty() }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_output(height: u64) -> RandomnessOutput {
        let mut bh  = [0u8; 32]; bh[0] = 0x01;
        let mut btc = [0u8; 32]; btc[0] = 0xde; btc[1] = 0xad;
        RandomnessOutput::from_block(height, bh, btc)
    }

    #[test]
    fn verify_roundtrip() {
        let o = dummy_output(42);
        assert!(o.verify(), "verify() must pass for freshly constructed output");
    }

    #[test]
    fn different_heights_different_outputs() {
        let a = dummy_output(1);
        let b = dummy_output(2);
        assert_ne!(a.output,    b.output);
        assert_ne!(a.nullifier, b.nullifier);
    }

    #[test]
    fn different_block_hashes_different_outputs() {
        let mut bh1 = [0u8; 32]; bh1[0] = 0xaa;
        let mut bh2 = [0u8; 32]; bh2[0] = 0xbb;
        let btc     = [0u8; 32];
        let a = RandomnessOutput::from_block(1, bh1, btc);
        let b = RandomnessOutput::from_block(1, bh2, btc);
        assert_ne!(a.output, b.output);
    }

    #[test]
    fn consume_once_ok() {
        let mut ns = NullifierSet::new();
        let o  = dummy_output(10);
        let r  = ns.consume(&o);
        assert!(r.is_ok());
        assert_eq!(r.unwrap(), o.output);
        assert_eq!(ns.len(), 1);
    }

    #[test]
    fn consume_twice_fails() {
        let mut ns = NullifierSet::new();
        let o = dummy_output(10);
        ns.consume(&o).unwrap();
        let r2 = ns.consume(&o);
        assert!(r2.is_err(), "second consume must fail with AlreadySpent");
        assert!(r2.unwrap_err().to_string().contains("double-spend"));
    }

    #[test]
    fn different_heights_both_spendable() {
        let mut ns = NullifierSet::new();
        let a = dummy_output(1);
        let b = dummy_output(2);
        ns.consume(&a).unwrap();
        // height 2 is a different nullifier — must succeed
        assert!(ns.consume(&b).is_ok());
        assert_eq!(ns.len(), 2);
    }
}
