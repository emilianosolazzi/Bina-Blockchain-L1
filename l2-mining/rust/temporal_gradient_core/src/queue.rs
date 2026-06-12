use crate::chain::LiveSubmission;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedSolution {
    pub submission: LiveSubmission,
    pub created_at: u64,
}

pub fn queue_dir(key_path: &str) -> PathBuf {
    let p = Path::new(key_path);
    let parent = p.parent().unwrap_or(Path::new("."));
    parent.join("queue")
}

pub fn pending_dir(key_path: &str) -> PathBuf {
    queue_dir(key_path).join("pending")
}

pub fn approved_dir(key_path: &str) -> PathBuf {
    queue_dir(key_path).join("approved")
}

pub fn rejected_dir(key_path: &str) -> PathBuf {
    queue_dir(key_path).join("rejected")
}

pub fn ensure_queue_dirs(key_path: &str) -> Result<()> {
    fs::create_dir_all(pending_dir(key_path))?;
    fs::create_dir_all(approved_dir(key_path))?;
    fs::create_dir_all(rejected_dir(key_path))?;
    Ok(())
}

pub fn file_path(dir: &Path, hash: &[u8; 32]) -> PathBuf {
    dir.join(format!("{}.json", hex::encode(hash)))
}

pub fn push_solution(key_path: &str, submission: LiveSubmission, approved: bool) -> Result<()> {
    ensure_queue_dirs(key_path)?;
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let queued = QueuedSolution {
        submission: submission.clone(),
        created_at,
    };
    let json = serde_json::to_string_pretty(&queued)?;
    let hash = submission.commitment.commit_hash;
    
    let target_dir = if approved {
        approved_dir(key_path)
    } else {
        pending_dir(key_path)
    };
    let path = file_path(&target_dir, &hash);
    
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, json)?;
    fs::rename(tmp, path)?;
    
    Ok(())
}

pub fn get_queue_counts(key_path: &str) -> (u64, u64, u64) {
    let mut pending = 0;
    if let Ok(entries) = fs::read_dir(pending_dir(key_path)) {
        pending = entries.filter_map(|e| e.ok()).filter(|e| e.path().extension().map_or(false, |ext| ext == "json")).count() as u64;
    }
    let mut approved = 0;
    if let Ok(entries) = fs::read_dir(approved_dir(key_path)) {
        approved = entries.filter_map(|e| e.ok()).filter(|e| e.path().extension().map_or(false, |ext| ext == "json")).count() as u64;
    }
    let mut rejected = 0;
    if let Ok(entries) = fs::read_dir(rejected_dir(key_path)) {
        rejected = entries.filter_map(|e| e.ok()).filter(|e| e.path().extension().map_or(false, |ext| ext == "json")).count() as u64;
    }
    (pending, approved, rejected)
}

/// Moves a pending solution to the approved directory
pub fn approve_solution(key_path: &str, hash_hex: &str) -> Result<bool> {
    ensure_queue_dirs(key_path)?;
    
    // Normalize hash
    let hash_hex = hash_hex.trim_start_matches("0x");
    
    let pending = pending_dir(key_path).join(format!("{}.json", hash_hex));
    if !pending.exists() {
        return Ok(false);
    }
    
    let approved = approved_dir(key_path).join(format!("{}.json", hash_hex));
    fs::rename(pending, approved)?;
    Ok(true)
}

pub fn reject_solution(key_path: &str, path: &PathBuf) -> Result<()> {
    ensure_queue_dirs(key_path)?;
    if let Some(filename) = path.file_name() {
        let rejected = rejected_dir(key_path).join(filename);
        fs::rename(path, rejected)?;
    }
    Ok(())
}

/// Pops the oldest approved solution and its path. The caller is responsible for deleting the file after processing.
pub fn pop_approved(key_path: &str, max_age_secs: u64, ignore_hashes: &std::collections::HashSet<String>) -> Result<Option<(QueuedSolution, PathBuf)>> {
    let dir = approved_dir(key_path);
    if !dir.exists() {
        return Ok(None);
    }
    
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let mut entries: Vec<_> = fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "json"))
        .collect();
    
    if entries.is_empty() {
        return Ok(None);
    }
    
    entries.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH));

    for entry in entries {
        let path = entry.path();
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let queued: QueuedSolution = match serde_json::from_str(&content) {
            Ok(q) => q,
            Err(_) => continue,
        };
        
        let hash_hex = hex::encode(queued.submission.commitment.commit_hash);
        if ignore_hashes.contains(&hash_hex) {
            continue;
        }
        
        let age = now.saturating_sub(queued.created_at);
        if max_age_secs > 0 && age > max_age_secs {
            tracing::warn!("Skipping stale solution ({} secs old): {:?}", age, path);
            let _ = reject_solution(key_path, &path);
            continue;
        }

        return Ok(Some((queued, path)));
    }
    
    Ok(None)
}

pub enum QueueCommand {
    List,
    Approve { hash: String },
    ApproveAll,
    Reject { hash: String },
    Flush,
    Stats,
}

pub fn list_pending(key_path: &str) -> Result<Vec<QueuedSolution>> {
    let dir = pending_dir(key_path);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut solutions = Vec::new();
    for entry in fs::read_dir(&dir)?.filter_map(|e| e.ok()) {
        if entry.path().extension().map_or(false, |ext| ext == "json") {
            if let Ok(content) = fs::read_to_string(entry.path()) {
                if let Ok(queued) = serde_json::from_str::<QueuedSolution>(&content) {
                    solutions.push(queued);
                }
            }
        }
    }
    Ok(solutions)
}

pub fn approve_all(key_path: &str) -> Result<usize> {
    let pending = list_pending(key_path)?;
    let mut count = 0;
    for q in pending {
        let hash_hex = hex::encode(q.submission.commitment.commit_hash);
        if approve_solution(key_path, &hash_hex).unwrap_or(false) {
            count += 1;
        }
    }
    Ok(count)
}

pub fn flush_pending(key_path: &str) -> Result<usize> {
    let dir = pending_dir(key_path);
    if !dir.exists() {
        return Ok(0);
    }
    let mut count = 0;
    for entry in fs::read_dir(&dir)?.filter_map(|e| e.ok()) {
        if entry.path().extension().map_or(false, |ext| ext == "json") {
            let _ = reject_solution(key_path, &entry.path());
            count += 1;
        }
    }
    Ok(count)
}

pub fn age_secs(queued: &QueuedSolution) -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    now.saturating_sub(queued.created_at)
}

pub fn handle_command(cmd: QueueCommand, key_path: &str) -> Result<()> {
    match cmd {
        QueueCommand::List => {
            let entries = list_pending(key_path)?;
            for e in entries {
                println!(
                    "Hash: {} | Age: {}s | Pool: {} | Nonce: {}",
                    hex::encode(e.submission.commitment.commit_hash),
                    age_secs(&e),
                    e.submission.commitment.pool_id,
                    e.submission.nonce
                );
            }
        }
        QueueCommand::Approve { hash } => {
            let moved = approve_solution(key_path, &hash)?;
            if moved {
                println!("✅ Approved: {}", hash);
            } else {
                println!("❌ Not found: {}", hash);
            }
        }
        QueueCommand::ApproveAll => {
            let count = approve_all(key_path)?;
            println!("✅ Approved {} solutions", count);
        }
        QueueCommand::Reject { hash } => {
            let path = pending_dir(key_path)
                .join(format!("{}.json", hash.trim_start_matches("0x")));
            reject_solution(key_path, &path)?;
            println!("🚫 Rejected: {}", hash);
        }
        QueueCommand::Flush => {
            let count = flush_pending(key_path)?;
            println!("🗑️ Flushed {} pending solutions", count);
        }
        QueueCommand::Stats => {
            let (pending, approved, rejected) = get_queue_counts(key_path);
            println!("📊 Pending: {} | Approved: {} | Rejected: {}", 
                pending, approved, rejected);
        }
    }
    Ok(())
}
