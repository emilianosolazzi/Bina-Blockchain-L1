use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// Number of L1 blocks between mandatory Bitcoin-seed checkpoints.
///
/// `bitcoin_seed_hash` is gated for exact cross-node agreement on every
/// block, but each node fetches Bitcoin chain state independently from live
/// web APIs — polling a moving target every block guarantees disagreement
/// between nodes. Instead, the seed is only refreshed at checkpoint heights;
/// every block in between must reuse the seed pinned at the last checkpoint,
/// which is chain data (not a live fetch) and therefore trivially agreed on.
pub const BTC_CHECKPOINT_INTERVAL: u64 = 20;

/// True if `height` may pin a new Bitcoin-seed checkpoint.
/// Height 1 (the first mined block) always pins the initial checkpoint.
pub fn is_checkpoint_height(height: u64) -> bool {
    height == 1 || (height.saturating_sub(1)) % BTC_CHECKPOINT_INTERVAL == 0
}

/// The height of the checkpoint that governs `height` (i.e. the height whose
/// accepted block pinned the seed that `height` must reuse).
pub fn governing_checkpoint_height(height: u64) -> u64 {
    if height == 0 {
        return 0;
    }
    ((height - 1) / BTC_CHECKPOINT_INTERVAL) * BTC_CHECKPOINT_INTERVAL + 1
}

/// Raw Bitcoin-entropy components a miner attaches to a checkpoint-height
/// claim so any validating node can (a) recompute the commitment hash and
/// confirm it matches `header.bitcoin_seed_hash`, and (b) sanity-check the
/// claimed Bitcoin tip against its own independently-observed live state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BtcCheckpointProof {
    pub tip_hash:       [u8; 32],
    pub tip_height:     u64,
    pub utxo_entropy:   [u8; 32],
    pub stale_xor_pool: [u8; 32],
}

impl BtcCheckpointProof {
    pub fn from_state(state: &BtcEntropyState) -> Self {
        Self {
            tip_hash:       state.tip_hash,
            tip_height:     state.tip_height,
            utxo_entropy:   state.utxo_entropy,
            stale_xor_pool: state.stale_xor_pool,
        }
    }

    /// Recompute the seed hash this proof commits to. Must equal the
    /// claim's `header.bitcoin_seed_hash` for the checkpoint to be valid.
    pub fn seed_hash(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        h.update(b"BINA-BTC-v1");
        h.update(&self.tip_hash);
        h.update(&self.utxo_entropy);
        h.update(&self.stale_xor_pool);
        *h.finalize().as_bytes()
    }

    /// A checkpoint is plausible only if its claimed Bitcoin tip height is
    /// not behind the last accepted checkpoint (Bitcoin height is
    /// monotonic under normal operation) and sits within `tolerance` blocks
    /// of what the validator independently observes live. This stops a
    /// miner from pinning a fabricated Bitcoin state for a whole epoch while
    /// still tolerating ordinary provider/propagation lag between
    /// validators, who each poll Bitcoin APIs independently.
    ///
    /// This check is only meaningful for a *live* accept decision — it is
    /// intentionally NOT re-applied when replaying old, already-buried
    /// history during sync (see module docs), where trust instead comes
    /// from the accumulated PoW built on top, exactly as with any other
    /// historical chain data.
    pub fn plausible(&self, previous_checkpoint_tip_height: u64, observed_tip_height: u64, tolerance: u64) -> bool {
        if self.tip_height < previous_checkpoint_tip_height {
            return false;
        }
        self.tip_height.abs_diff(observed_tip_height) <= tolerance
    }
}

/// The three Bitcoin entropy sources combined into a single seed hash.
///
/// bitcoin_seed_hash = blake3("BINA-BTC-v1" || tip_hash || utxo_entropy || stale_xor_pool)
///
/// Sources:
///   tip_hash       — current canonical chain tip (changes ~every 10 min)
///   utxo_entropy   — coinbase script of the tip block (commits to all txs in that block)
///   stale_xor_pool — XOR/mix of independent API tips; non-zero when providers diverge
#[derive(Debug, Clone)]
pub struct BtcEntropyState {
    pub tip_hash:       [u8; 32],
    pub utxo_entropy:   [u8; 32],
    pub stale_xor_pool: [u8; 32],
    pub seed_timestamp: u64,
    pub tip_height:     u64,
    /// True if mempool.space and blockstream.info disagreed on the tip.
    /// This can be a real Bitcoin fork or provider lag; label it as tip divergence.
    pub fork_detected:  bool,
}

impl BtcEntropyState {
    /// Combined Bitcoin seed hash used as the `bitcoin_seed_hash` field in every L1 block header.
    pub fn bitcoin_seed_hash(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        h.update(b"BINA-BTC-v1");
        h.update(&self.tip_hash);
        h.update(&self.utxo_entropy);
        h.update(&self.stale_xor_pool);
        *h.finalize().as_bytes()
    }

    /// Returns false if the seed is older than `max_age_secs` (default: 1200 = 20 min).
    pub fn is_fresh(&self, max_age_secs: u64) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(self.seed_timestamp) < max_age_secs
    }

    pub fn age_secs(&self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(self.seed_timestamp)
    }

    /// Deterministic mock for offline/test use — does NOT require any network call.
    pub fn mock() -> Self {
        let mut tip = [0u8; 32];
        tip[..4].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        let mut utxo = [0u8; 32];
        utxo[..4].copy_from_slice(&[0xca, 0xfe, 0xba, 0xbe]);
        BtcEntropyState {
            tip_hash:       tip,
            utxo_entropy:   utxo,
            stale_xor_pool: [0u8; 32],
            seed_timestamp: 0,
            tip_height:     0,
            fork_detected:  false,
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// XOR two 32-byte arrays.
pub fn xor32(a: [u8; 32], b: [u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = a[i] ^ b[i];
    }
    out
}

/// blake3(domain || data) → [u8;32]
pub fn blake3_keyed(domain: &[u8], data: &[u8]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(domain);
    h.update(data);
    *h.finalize().as_bytes()
}

/// Decode a lowercase hex string into a [u8;32].  Returns an error if the
/// string is not exactly 64 hex characters.
pub fn hex_to_32(s: &str) -> Result<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 {
        bail!("expected 64 hex chars, got {}: {:?}", s.len(), &s[..s.len().min(16)]);
    }
    let bytes = hex::decode(s)?;
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

#[cfg(test)]
mod checkpoint_tests {
    use super::*;

    #[test]
    fn checkpoint_heights_are_periodic_from_one() {
        assert!(is_checkpoint_height(1));
        assert!(!is_checkpoint_height(2));
        assert!(!is_checkpoint_height(20));
        assert!(is_checkpoint_height(21));
        assert!(is_checkpoint_height(41));
        assert!(!is_checkpoint_height(40));
    }

    #[test]
    fn governing_checkpoint_maps_every_height_in_epoch_to_its_pin() {
        for h in 1..=20u64 {
            assert_eq!(governing_checkpoint_height(h), 1);
        }
        for h in 21..=40u64 {
            assert_eq!(governing_checkpoint_height(h), 21);
        }
    }

    #[test]
    fn checkpoint_proof_hash_matches_state_seed_hash() {
        let state = BtcEntropyState::mock();
        let proof = BtcCheckpointProof::from_state(&state);
        assert_eq!(proof.seed_hash(), state.bitcoin_seed_hash());
    }

    #[test]
    fn checkpoint_proof_rejects_backward_tip_height() {
        let mut proof = BtcCheckpointProof {
            tip_hash: [1u8; 32],
            tip_height: 100,
            utxo_entropy: [2u8; 32],
            stale_xor_pool: [3u8; 32],
        };
        assert!(!proof.plausible(150, 100, 5), "tip height behind the previous checkpoint must be rejected");
        proof.tip_height = 150;
        assert!(proof.plausible(150, 150, 5));
    }

    #[test]
    fn checkpoint_proof_rejects_implausible_drift_from_observed_tip() {
        let proof = BtcCheckpointProof {
            tip_hash: [1u8; 32],
            tip_height: 900_000,
            utxo_entropy: [2u8; 32],
            stale_xor_pool: [3u8; 32],
        };
        // A fabricated tip far from what this validator observes must fail.
        assert!(!proof.plausible(0, 100, 5));
        // Within tolerance of the observed tip must pass.
        assert!(proof.plausible(0, 900_002, 5));
    }
}
