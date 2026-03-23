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

/// Derive a 32-byte file-encryption key from the private-key file contents.
///
/// Uses `blake3::derive_key` with a domain-separation context so the
/// derived key is deterministic per key-file but not directly the
/// private key.  Returns `Err` if the key file cannot be read — never
/// falls back to using the *path* string, which would be predictable.
fn derive_file_key(key_path: &str) -> Result<[u8; 32]> {
    let material = fs::read(key_path)
        .with_context(|| format!("Cannot read key file for pending-file encryption: {key_path}"))?;
    Ok(blake3::derive_key(
        "temporal-gradient-miner pending-commitment encryption v1",
        &material,
    ))
}

/// XOR-encrypt/decrypt `data` in place using a blake3-derived keystream.
///
/// When `nonce` is `Some`, the nonce is mixed into the hash before
/// producing the keystream, ensuring a unique stream per save (v2 format).
/// When `nonce` is `None`, the legacy v1 keystream is used for backward
/// compatibility with existing `TGPE` files on disk.
fn xor_keystream(data: &mut [u8], file_key: &[u8; 32], nonce: Option<&[u8; 16]>) {
    let mut hasher = blake3::Hasher::new_keyed(file_key);
    if let Some(n) = nonce {
        hasher.update(n);
    }
    let mut output = hasher.finalize_xof();
    let mut keystream = vec![0u8; data.len()];
    output.fill(&mut keystream);
    for (d, k) in data.iter_mut().zip(keystream.iter()) {
        *d ^= k;
    }
    keystream.zeroize();
}

/// Nonce length for v2 encrypted files.
const NONCE_LEN: usize = 16;

/// V2 magic header: includes a per-save random nonce to prevent two-time-pad.
/// File layout: `TGP2`(4) + nonce(16) + encrypted_payload.
const ENCRYPTED_MAGIC_V2: &[u8; 4] = b"TGP2";

/// V1 (legacy) magic header: deterministic keystream, no nonce.
const ENCRYPTED_MAGIC_V1: &[u8; 4] = b"TGPE";

/// Save a pending commitment to disk, encrypted with a key derived
/// from the miner's private-key file.
///
/// Uses v2 format: `TGP2` + 16-byte random nonce + encrypted payload.
/// Atomic write via tmp+rename.
pub fn save(key_path: &str, pending: &PendingCommitment) -> Result<()> {
    let target = pending_path(key_path);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create {}", parent.display()))?;
    }

    let json = serde_json::to_string_pretty(pending)?;
    let mut payload = json.into_bytes();

    let mut file_key = derive_file_key(key_path)?;

    // Generate a random nonce so each save produces a unique keystream.
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce)
        .map_err(|e| anyhow::anyhow!("RNG failure generating nonce: {e}"))?;

    xor_keystream(&mut payload, &file_key, Some(&nonce));
    file_key.zeroize();

    // File layout: magic(4) + nonce(16) + encrypted_payload.
    let mut out = Vec::with_capacity(
        ENCRYPTED_MAGIC_V2.len() + NONCE_LEN + payload.len(),
    );
    out.extend_from_slice(ENCRYPTED_MAGIC_V2);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&payload);
    payload.zeroize();
    nonce.zeroize();

    let tmp = target.with_extension("json.tmp");
    fs::write(&tmp, &out)
        .with_context(|| format!("Failed to write {}", tmp.display()))?;
    fs::rename(&tmp, &target)
        .with_context(|| format!("Failed to rename to {}", target.display()))?;
    tracing::info!("Saved pending commitment to {}", target.display());
    Ok(())
}

/// Load a pending commitment from disk.
///
/// Supports three on-disk formats for backwards compatibility:
/// 1. `TGP2` — v2 encrypted (nonce + keystream)
/// 2. `TGPE` — v1 encrypted (deterministic keystream, no nonce)
/// 3. Plain JSON — pre-encryption legacy
pub fn load(key_path: &str) -> Result<Option<PendingCommitment>> {
    let target = pending_path(key_path);
    if !target.exists() {
        return Ok(None);
    }
    let raw = fs::read(&target)
        .with_context(|| format!("Failed to read {}", target.display()))?;

    let json_str = if raw.starts_with(ENCRYPTED_MAGIC_V2) {
        // V2 format: magic(4) + nonce(16) + encrypted payload.
        let header_len = ENCRYPTED_MAGIC_V2.len() + NONCE_LEN;
        if raw.len() < header_len {
            return Err(anyhow::anyhow!(
                "Truncated v2 pending file: {} bytes", raw.len()
            ));
        }
        let nonce: [u8; NONCE_LEN] = raw[ENCRYPTED_MAGIC_V2.len()..header_len]
            .try_into()
            .expect("nonce length mismatch");
        let mut payload = raw[header_len..].to_vec();
        let mut file_key = derive_file_key(key_path)?;
        xor_keystream(&mut payload, &file_key, Some(&nonce));
        file_key.zeroize();
        String::from_utf8(payload)
            .with_context(|| format!("V2 decryption produced invalid UTF-8 in {}", target.display()))?
    } else if raw.starts_with(ENCRYPTED_MAGIC_V1) {
        // V1 legacy encrypted format: no nonce.
        tracing::debug!("Pending file uses v1 encryption; will upgrade to v2 on next save");
        let mut payload = raw[ENCRYPTED_MAGIC_V1.len()..].to_vec();
        let mut file_key = derive_file_key(key_path)?;
        xor_keystream(&mut payload, &file_key, None);
        file_key.zeroize();
        String::from_utf8(payload)
            .with_context(|| format!("V1 decryption produced invalid UTF-8 in {}", target.display()))?
    } else {
        // Legacy plaintext JSON — still accepted.
        tracing::debug!("Pending file is legacy plaintext; will encrypt on next save");
        String::from_utf8(raw)
            .with_context(|| format!("Invalid UTF-8 in {}", target.display()))?
    };

    let pending: PendingCommitment = serde_json::from_str(&json_str)
        .with_context(|| format!("Bad JSON in {}", target.display()))?;
    tracing::info!("Loaded pending commitment from {}", target.display());
    Ok(Some(pending))
}

/// Scrub a single file: overwrite with zeros then delete.
fn scrub_and_remove(path: &Path) -> Result<()> {
    if path.exists() {
        if let Ok(meta) = fs::metadata(path) {
            let len = meta.len() as usize;
            if len > 0 {
                let _ = fs::write(path, vec![0u8; len]);
            }
        }
        fs::remove_file(path)
            .with_context(|| format!("Failed to remove {}", path.display()))?;
    }
    Ok(())
}

/// Remove the pending commitment file after a successful reveal.
/// Overwrites the file with zeros before deleting to prevent
/// forensic recovery of secret values from disk.
///
/// Also cleans up any leftover `.json.tmp` from a crashed save.
pub fn clear(key_path: &str) -> Result<()> {
    let target = pending_path(key_path);

    // Scrub the main pending file.
    if target.exists() {
        scrub_and_remove(&target)?;
        tracing::info!("Cleared pending commitment {}", target.display());
    }

    // Scrub any leftover tmp file from a crashed atomic write.
    let tmp = target.with_extension("json.tmp");
    if tmp.exists() {
        scrub_and_remove(&tmp)?;
        tracing::debug!("Cleaned up leftover tmp file {}", tmp.display());
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

    /// Helper: create a temp dir with a dummy key file, return key path string.
    fn setup_key_file() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let key_file = dir.path().join("test.key");
        fs::write(&key_file, b"secret-key-material-1234567890ab").unwrap();
        let key = key_file.to_string_lossy().to_string();
        (dir, key)
    }

    // ── Round-trip (v2 format) ──────────────────────────────────

    #[test]
    fn round_trip_v2_encrypted() {
        let (_dir, key) = setup_key_file();
        let pending = sample_pending();
        save(&key, &pending).unwrap();

        // On-disk file must start with TGP2 magic.
        let raw = fs::read(pending_path(&key)).unwrap();
        assert!(raw.starts_with(ENCRYPTED_MAGIC_V2), "should start with TGP2");

        // Payload after magic+nonce must NOT be valid JSON (encrypted).
        let header_len = ENCRYPTED_MAGIC_V2.len() + NONCE_LEN;
        assert!(
            serde_json::from_slice::<PendingCommitment>(&raw[header_len..]).is_err(),
            "encrypted payload must not be valid JSON"
        );

        let loaded = load(&key).unwrap().expect("should exist");
        assert_eq!(loaded.nonce, 99);
        assert_eq!(loaded.commit_block, 42);
        assert_eq!(loaded.secret_value, [4u8; 32]);

        clear(&key).unwrap();
        assert!(load(&key).unwrap().is_none());
    }

    // ── Two saves produce different ciphertext (nonce works) ────

    #[test]
    fn two_saves_produce_different_ciphertext() {
        let (_dir, key) = setup_key_file();
        let pending = sample_pending();

        save(&key, &pending).unwrap();
        let raw1 = fs::read(pending_path(&key)).unwrap();

        save(&key, &pending).unwrap();
        let raw2 = fs::read(pending_path(&key)).unwrap();

        assert_ne!(raw1, raw2, "two saves must produce different ciphertext");
    }

    // ── Legacy v1 (TGPE) backward compat ────────────────────────

    #[test]
    fn legacy_v1_encrypted_loads() {
        let (_dir, key) = setup_key_file();
        let pending = sample_pending();

        // Simulate a v1-encrypted file: TGPE + deterministic keystream (no nonce).
        let json = serde_json::to_string_pretty(&pending).unwrap();
        let mut payload = json.into_bytes();
        let file_key = derive_file_key(&key).unwrap();
        xor_keystream(&mut payload, &file_key, None);

        let target = pending_path(&key);
        let mut out = Vec::new();
        out.extend_from_slice(ENCRYPTED_MAGIC_V1);
        out.extend_from_slice(&payload);
        fs::write(&target, &out).unwrap();

        let loaded = load(&key).unwrap().expect("should load v1 legacy");
        assert_eq!(loaded.nonce, 99);
    }

    // ── Legacy plaintext backward compat ────────────────────────

    #[test]
    fn legacy_plaintext_loads() {
        let (_dir, key) = setup_key_file();
        let pending = sample_pending();
        let json = serde_json::to_string_pretty(&pending).unwrap();
        let target = pending_path(&key);
        if let Some(p) = target.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(&target, &json).unwrap();

        let loaded = load(&key).unwrap().expect("should load plaintext legacy");
        assert_eq!(loaded.nonce, 99);
    }

    // ── Clear scrubs main + tmp ─────────────────────────────────

    #[test]
    fn clear_scrubs_main_and_tmp() {
        let (_dir, key) = setup_key_file();
        save(&key, &sample_pending()).unwrap();

        let target = pending_path(&key);
        let tmp = target.with_extension("json.tmp");

        // Simulate a leftover tmp file from a crashed save.
        fs::write(&tmp, b"leftover-crash-data").unwrap();
        assert!(target.exists());
        assert!(tmp.exists());

        clear(&key).unwrap();
        assert!(!target.exists(), "main file should be deleted");
        assert!(!tmp.exists(), "tmp file should be cleaned up");
    }

    // ── Missing key file produces clear error ───────────────────

    #[test]
    fn save_fails_with_missing_key_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no-such-file.key");
        let key = missing.to_string_lossy().to_string();
        let err = save(&key, &sample_pending());
        assert!(err.is_err(), "should fail when key file is missing");
        let msg = format!("{:#}", err.unwrap_err());
        assert!(msg.contains("Cannot read key file"), "error: {msg}");
    }

    // ── Truncated v2 file is rejected ───────────────────────────

    #[test]
    fn truncated_v2_file_rejected() {
        let (_dir, key) = setup_key_file();
        let target = pending_path(&key);
        if let Some(p) = target.parent() {
            fs::create_dir_all(p).unwrap();
        }
        // Write TGP2 magic but not enough bytes for the nonce.
        fs::write(&target, b"TGP2short").unwrap();
        let result = load(&key);
        assert!(result.is_err(), "truncated v2 should fail");
    }
}
