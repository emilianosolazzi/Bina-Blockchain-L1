use anyhow::{bail, Result};

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
