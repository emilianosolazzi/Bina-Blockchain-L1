//! Persistence for pending mining commitments.
//!
//! Before submitting a commitment to the blockchain the miner serialises the
//! full [`PendingCommitment`] to a JSON file next to its key file.  On startup
//! the miner checks for a saved commitment and — if it is still within the
//! reveal window — reveals it immediately instead of searching for a new
//! solution.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Everything needed to reveal a previously committed mining solution.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Derives the path for the pending-commitment file from the key-file path.
///
/// Example: `keys/local-miner.key` → `keys/local-miner.pending.json`
pub fn pending_path(key_path: &str) -> PathBuf {
    let p = Path::new(key_path);
    let stem = p.file_stem().unwrap_or_default().to_string_lossy();
    let parent = p.parent().unwrap_or(Path::new("."));
    parent.join(format!("{stem}.pending.json"))
}

/// Save a pending commitment to disk (atomic-ish: write tmp then rename).
pub fn save(key_path: &str, pending: &PendingCommitment) -> Result<()> {
    let target = pending_path(key_path);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(pending)?;
    let tmp = target.with_extension("json.tmp");
    fs::write(&tmp, &json)
        .with_context(|| format!("Failed to write {}", tmp.display()))?;
    fs::rename(&tmp, &target)
        .with_context(|| format!("Failed to rename to {}", target.display()))?;
    tracing::info!("Saved pending commitment to {}", target.display());
    Ok(())
}

/// Load a pending commitment from disk, if one exists.
pub fn load(key_path: &str) -> Result<Option<PendingCommitment>> {
    let target = pending_path(key_path);
    if !target.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&target)
        .with_context(|| format!("Failed to read {}", target.display()))?;
    let pending: PendingCommitment = serde_json::from_str(&raw)
        .with_context(|| format!("Bad JSON in {}", target.display()))?;
    tracing::info!("Loaded pending commitment from {}", target.display());
    Ok(Some(pending))
}

/// Remove the pending commitment file after a successful reveal.
pub fn clear(key_path: &str) -> Result<()> {
    let target = pending_path(key_path);
    if target.exists() {
        fs::remove_file(&target)
            .with_context(|| format!("Failed to remove {}", target.display()))?;
        tracing::info!("Cleared pending commitment {}", target.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("test.key").to_string_lossy().to_string();
        let pending = PendingCommitment {
            commit_block: 42,
            previous_output: [1u8; 32],
            temporal_seed: [2u8; 8],
            nonce: 99,
            reveal_signature: vec![3u8; 65],
            secret_value: [4u8; 32],
            commit_hash: [5u8; 32],
            pool_id: 0,
        };
        save(&key, &pending).unwrap();
        let loaded = load(&key).unwrap().expect("should exist");
        assert_eq!(loaded.nonce, 99);
        assert_eq!(loaded.commit_block, 42);
        clear(&key).unwrap();
        assert!(load(&key).unwrap().is_none());
    }
}
