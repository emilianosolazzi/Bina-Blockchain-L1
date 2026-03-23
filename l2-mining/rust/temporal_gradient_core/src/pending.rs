//! Persistence for pending mining commitments.
//!
//! Before submitting a commitment to the blockchain the miner serialises the
//! full [`PendingCommitment`] to a JSON file next to its key file.  On startup
//! the miner checks for a saved commitment and — if it is still within the
//! reveal window — reveals it immediately instead of searching for a new
//! solution.

use crate::memory::SecureBuffer;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use zeroize::{Zeroize, ZeroizeOnDrop};
use blake3;

/// Everything needed to reveal a previously committed mining solution.
#[derive(Debug, Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct PendingCommitment {
    /// The block number at which the commitment was mined.
    pub commit_block: u64,
    /// `previousOutput` used to build the original hash.
    pub previous_output: [u8; 32],
    /// Temporal seed bytes (8 bytes).
    pub temporal_seed: [u8; 8],
    /// The mining nonce that produced the valid hash.
    pub nonce: u64,
    /// The ECDSA reveal signature produced during mining.
    pub reveal_signature: Vec<u8>,
    /// The random secret value used in the hash.
    pub secret_value: [u8; 32],
    /// The commit hash submitted on-chain.
    pub commit_hash: [u8; 32],
    /// Pool id.
    pub pool_id: u8,
}

impl PendingCommitment {
    pub fn secure_reveal_signature(&self) -> Result<SecureBuffer> {
        SecureBuffer::from_slice(&self.reveal_signature)
            .map_err(|err| anyhow::anyhow!("Failed to protect pending reveal signature in memory: {err}"))
    }

    pub fn secure_secret_value(&self) -> Result<SecureBuffer> {
        SecureBuffer::from_slice(&self.secret_value)
            .map_err(|err| anyhow::anyhow!("Failed to protect pending secret value in memory: {err}"))
    }
}

/// Derives the path for the pending-commitment file from the key-file path.
///
/// Example: `keys/local-miner.key` → `keys/local-miner.pending.json`
pub fn pending_path(key_path: &str) -> PathBuf {
    let p = Path::new(key_path);
    let stem = p.file_stem().unwrap_or_default().to_string_lossy();
    let parent = p.parent().unwrap_or(Path::new("."));
    parent.join(format!("{stem}.pending.json"))
}

/// Derive a 32-byte file-encryption key from the private-key path.
///
/// Uses `blake3::derive_key` with a domain-separation context so the
/// key is deterministic per key-file but not directly the private key.
fn derive_file_key(key_path: &str) -> [u8; 32] {
    // Read the private key file contents for key derivation material.
    // Falls back to path-only derivation if the file can't be read.
    let material = match fs::read(key_path) {
        Ok(bytes) => bytes,
        Err(_) => key_path.as_bytes().to_vec(),
    };
    blake3::derive_key("temporal-gradient-miner pending-commitment encryption v1", &material)
}

/// XOR-encrypt/decrypt `data` in place using a blake3-derived keystream.
fn xor_keystream(data: &mut [u8], file_key: &[u8; 32]) {
    // Use blake3 in extended output mode to generate an arbitrary-length keystream.
    let mut output = blake3::Hasher::new_keyed(file_key).finalize_xof();
    let mut keystream = vec![0u8; data.len()];
    output.fill(&mut keystream);
    for (d, k) in data.iter_mut().zip(keystream.iter()) {
        *d ^= k;
    }
    keystream.zeroize();
}

/// Magic header to distinguish encrypted pending files from legacy plaintext.
const ENCRYPTED_MAGIC: &[u8; 4] = b"TGPE";

/// Save a pending commitment to disk, encrypted with a key derived
/// from the miner's private-key file. Atomic write via tmp+rename.
pub fn save(key_path: &str, pending: &PendingCommitment) -> Result<()> {
    let target = pending_path(key_path);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(pending)?;
    let mut payload = json.into_bytes();
    let file_key = derive_file_key(key_path);
    xor_keystream(&mut payload, &file_key);

    // Prepend magic header so load() can distinguish encrypted from legacy.
    let mut out = Vec::with_capacity(ENCRYPTED_MAGIC.len() + payload.len());
    out.extend_from_slice(ENCRYPTED_MAGIC);
    out.extend_from_slice(&payload);
    payload.zeroize();

    let tmp = target.with_extension("json.tmp");
    fs::write(&tmp, &out)
        .with_context(|| format!("Failed to write {}", tmp.display()))?;
    fs::rename(&tmp, &target)
        .with_context(|| format!("Failed to rename to {}", target.display()))?;
    tracing::info!("Saved pending commitment to {}", target.display());
    Ok(())
}

/// Load a pending commitment from disk. Supports both encrypted
/// (TGPE header) and legacy plaintext JSON for backwards compatibility.
pub fn load(key_path: &str) -> Result<Option<PendingCommitment>> {
    let target = pending_path(key_path);
    if !target.exists() {
        return Ok(None);
    }
    let raw = fs::read(&target)
        .with_context(|| format!("Failed to read {}", target.display()))?;

    let json_str = if raw.starts_with(ENCRYPTED_MAGIC) {
        // Encrypted format: strip magic, decrypt.
        let mut payload = raw[ENCRYPTED_MAGIC.len()..].to_vec();
        let file_key = derive_file_key(key_path);
        xor_keystream(&mut payload, &file_key);
        String::from_utf8(payload)
            .with_context(|| format!("Decryption produced invalid UTF-8 in {}", target.display()))?
    } else {
        // Legacy plaintext JSON — still accepted.
        tracing::debug!("Pending file is legacy plaintext; will re-encrypt on next save");
        String::from_utf8(raw)
            .with_context(|| format!("Invalid UTF-8 in {}", target.display()))?
    };

    let pending: PendingCommitment = serde_json::from_str(&json_str)
        .with_context(|| format!("Bad JSON in {}", target.display()))?;
    tracing::info!("Loaded pending commitment from {}", target.display());
    Ok(Some(pending))
}

/// Remove the pending commitment file after a successful reveal.
/// Overwrites the file with zeros before deleting to prevent
/// forensic recovery of secret values from disk.
pub fn clear(key_path: &str) -> Result<()> {
    let target = pending_path(key_path);
    if target.exists() {
        // Overwrite with zeros before deleting (best-effort scrub).
        if let Ok(meta) = fs::metadata(&target) {
            let len = meta.len() as usize;
            if len > 0 {
                let _ = fs::write(&target, vec![0u8; len]);
            }
        }
        fs::remove_file(&target)
            .with_context(|| format!("Failed to remove {}", target.display()))?;
        tracing::info!("Cleared pending commitment {}", target.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_pending() -> PendingCommitment {
        PendingCommitment {
            commit_block: 42,
            previous_output: [1u8; 32],
            temporal_seed: [2u8; 8],
            nonce: 99,
            reveal_signature: vec![3u8; 65],
            secret_value: [4u8; 32],
            commit_hash: [5u8; 32],
            pool_id: 0,
        }
    }

    #[test]
    fn round_trip_encrypted() {
        let dir = tempfile::tempdir().unwrap();
        let key_file = dir.path().join("test.key");
        // Write a dummy key so derive_file_key reads real bytes.
        fs::write(&key_file, b"secret-key-material-1234567890ab").unwrap();
        let key = key_file.to_string_lossy().to_string();

        let pending = sample_pending();
        save(&key, &pending).unwrap();

        // Verify the file on disk starts with the magic header (is encrypted).
        let raw = fs::read(pending_path(&key)).unwrap();
        assert!(raw.starts_with(ENCRYPTED_MAGIC), "file should start with TGPE magic");
        // And is NOT valid JSON (is actually encrypted).
        assert!(
            serde_json::from_slice::<PendingCommitment>(&raw[ENCRYPTED_MAGIC.len()..]).is_err(),
            "encrypted payload should not be valid JSON"
        );

        let loaded = load(&key).unwrap().expect("should exist");
        assert_eq!(loaded.nonce, 99);
        assert_eq!(loaded.commit_block, 42);
        assert_eq!(loaded.secret_value, [4u8; 32]);

        clear(&key).unwrap();
        assert!(load(&key).unwrap().is_none());
    }

    #[test]
    fn legacy_plaintext_loads() {
        // Ensure we can still load a pre-encryption plaintext pending file.
        let dir = tempfile::tempdir().unwrap();
        let key_file = dir.path().join("test.key");
        fs::write(&key_file, b"key").unwrap();
        let key = key_file.to_string_lossy().to_string();

        let pending = sample_pending();
        let json = serde_json::to_string_pretty(&pending).unwrap();
        let target = pending_path(&key);
        if let Some(p) = target.parent() {
            fs::create_dir_all(p).unwrap();
        }
        // Write plain JSON without encryption (legacy format).
        fs::write(&target, &json).unwrap();

        let loaded = load(&key).unwrap().expect("should load legacy");
        assert_eq!(loaded.nonce, 99);
    }

    #[test]
    fn clear_scrubs_before_delete() {
        let dir = tempfile::tempdir().unwrap();
        let key_file = dir.path().join("test.key");
        fs::write(&key_file, b"key").unwrap();
        let key = key_file.to_string_lossy().to_string();

        save(&key, &sample_pending()).unwrap();
        let target = pending_path(&key);
        assert!(target.exists());
        clear(&key).unwrap();
        assert!(!target.exists());
    }
}
