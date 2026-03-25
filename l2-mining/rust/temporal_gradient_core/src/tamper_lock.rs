use crate::config::MinerConfig;
use crate::memory;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RansomwareStatusFile {
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub detected_at_unix_ms: Option<u128>,
    #[serde(default)]
    pub detected_at: Option<String>,
    #[serde(default)]
    pub evidence_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TamperSeal {
    pub version: u32,
    pub created_at_unix_ms: u128,
    pub config_hash: String,
    pub binary_hash: String,
    pub key_hash: String,
    pub seal_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TamperLockStatus {
    pub locked: bool,
    pub status: String,
    pub reason: Option<String>,
    pub triggered_at_unix_ms: Option<u128>,
    pub last_checked_unix_ms: u128,
    pub seal_hash: Option<String>,
    pub seal_path: String,
    pub ransomware_status: Option<String>,
    pub ransomware_reason: Option<String>,
    pub ransomware_detected_at_unix_ms: Option<u128>,
    pub ransomware_evidence_path: Option<String>,
}

impl Default for TamperLockStatus {
    fn default() -> Self {
        Self {
            locked: false,
            status: "uninitialized".to_string(),
            reason: None,
            triggered_at_unix_ms: None,
            last_checked_unix_ms: now_unix_ms(),
            seal_hash: None,
            seal_path: String::new(),
            ransomware_status: None,
            ransomware_reason: None,
            ransomware_detected_at_unix_ms: None,
            ransomware_evidence_path: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TamperLockHandle {
    inner: Arc<Mutex<TamperLockStatus>>,
}

impl TamperLockHandle {
    pub fn new(status: TamperLockStatus) -> Self {
        Self {
            inner: Arc::new(Mutex::new(status)),
        }
    }

    pub fn get(&self) -> TamperLockStatus {
        match self.inner.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn set(&self, status: TamperLockStatus) {
        match self.inner.lock() {
            Ok(mut guard) => *guard = status,
            Err(poisoned) => *poisoned.into_inner() = status,
        }
    }
}

pub struct TamperLockMonitor {
    config_hash: String,
    binary_hash: String,
    key_path: PathBuf,
    seal_path: PathBuf,
    ransomware_status_path: PathBuf,
    baseline: Option<TamperSeal>,
    handle: TamperLockHandle,
}

impl TamperLockMonitor {
    pub fn new(config: &MinerConfig) -> Result<Self> {
        let telemetry_path = config.telemetry_path()?;
        let seal_path = seal_file_path(&telemetry_path);
        let ransomware_status_path = ransomware_status_file_path(&telemetry_path);
        let key_path = PathBuf::from(&config.private_key_path);
        let config_hash = hash_bytes(&serde_json::to_vec(config)?);
        let binary_hash = hash_bytes(&read_current_exe_bytes()?);

        let mut status = TamperLockStatus {
            seal_path: seal_path.display().to_string(),
            ..TamperLockStatus::default()
        };

        let baseline = if seal_path.exists() {
            match fs::read_to_string(&seal_path)
                .with_context(|| format!("Failed to read trust seal {}", seal_path.display()))
                .and_then(|raw| serde_json::from_str::<TamperSeal>(&raw).context("Invalid trust seal JSON"))
            {
                Ok(seal) => {
                    status.status = "sealed".to_string();
                    status.seal_hash = Some(seal.seal_hash.clone());
                    Some(seal)
                }
                Err(err) => {
                    warn!("Tamper lock: trust seal unreadable: {err:#}");
                    status.locked = true;
                    status.status = "tamper_locked".to_string();
                    status.reason = Some(format!("trust seal unreadable: {err}"));
                    status.triggered_at_unix_ms = Some(now_unix_ms());
                    None
                }
            }
        } else {
            let seal = build_seal(&config_hash, &binary_hash, &read_key_hash(&key_path)?)?;
            write_seal_file(&seal_path, &seal)?;
            info!("Tamper lock: created initial trust seal at {}", seal_path.display());
            status.status = "sealed".to_string();
            status.seal_hash = Some(seal.seal_hash.clone());
            Some(seal)
        };

        let handle = TamperLockHandle::new(status);
        Ok(Self {
            config_hash,
            binary_hash,
            key_path,
            seal_path,
            ransomware_status_path,
            baseline,
            handle,
        })
    }

    pub fn handle(&self) -> TamperLockHandle {
        self.handle.clone()
    }

    pub fn reseal(&mut self) -> Result<TamperLockStatus> {
        if memory::debugger_present() {
            let status = self.locked_status("tamper_locked", "debugger detected — reseal refused".to_string(), None);
            self.handle.set(status.clone());
            return Ok(status);
        }

        if let Some(ransomware) = read_ransomware_status(&self.ransomware_status_path)? {
            if ransomware.active {
                let status = self.locked_status(
                    "ransomware_locked",
                    format!("active ransomware signal — reseal refused: {}", ransomware.reason.clone().unwrap_or_else(|| "protected miner paths flagged".to_string())),
                    Some(&ransomware),
                );
                self.handle.set(status.clone());
                return Ok(status);
            }
        }

        let key_hash = read_key_hash(&self.key_path)?;
        let seal = build_seal(&self.config_hash, &self.binary_hash, &key_hash)?;
        write_seal_file(&self.seal_path, &seal)?;
        self.baseline = Some(seal.clone());

        let status = TamperLockStatus {
            locked: false,
            status: "sealed".to_string(),
            reason: None,
            triggered_at_unix_ms: None,
            last_checked_unix_ms: now_unix_ms(),
            seal_hash: Some(seal.seal_hash),
            seal_path: self.seal_path.display().to_string(),
            ransomware_status: Some("clear".to_string()),
            ransomware_reason: None,
            ransomware_detected_at_unix_ms: None,
            ransomware_evidence_path: None,
        };
        self.handle.set(status.clone());
        info!("Tamper lock: resealed trust profile at {}", self.seal_path.display());
        Ok(status)
    }

    pub fn evaluate(&mut self) -> TamperLockStatus {
        let status = match self.evaluate_inner() {
            Ok(status) => status,
            Err(err) => {
                warn!("Tamper lock evaluation failed: {err:#}");
                self.locked_status("tamper_locked", format!("tamper evaluation failed: {err}"), None)
            }
        };
        self.handle.set(status.clone());
        status
    }

    fn evaluate_inner(&mut self) -> Result<TamperLockStatus> {
        if memory::debugger_present() {
            return Ok(self.locked_status("tamper_locked", "debugger detected on miner process".to_string(), None));
        }

        let ransomware = read_ransomware_status(&self.ransomware_status_path)?;
        if let Some(ref signal) = ransomware {
            if signal.active {
                return Ok(self.locked_status(
                    "ransomware_locked",
                    format!("ransomware signal detected: {}", signal.reason.clone().unwrap_or_else(|| "protected miner paths flagged".to_string())),
                    Some(signal),
                ));
            }
        }

        let Some(baseline) = self.baseline.clone() else {
            return Ok(self.locked_status("tamper_locked", "trust seal unavailable".to_string(), ransomware.as_ref()));
        };

        if baseline.config_hash != self.config_hash {
            return Ok(self.locked_status("tamper_locked", "config fingerprint differs from local trust seal".to_string(), ransomware.as_ref()));
        }

        let current_binary_hash = hash_bytes(&read_current_exe_bytes()?);
        if baseline.binary_hash != current_binary_hash {
            return Ok(self.locked_status("tamper_locked", "binary fingerprint differs from local trust seal".to_string(), ransomware.as_ref()));
        }

        let current_seal = read_trust_seal(&self.seal_path)?;
        if current_seal.seal_hash != baseline.seal_hash {
            return Ok(self.locked_status("tamper_locked", "trust seal file differs from in-memory baseline".to_string(), ransomware.as_ref()));
        }

        let key_hash = read_key_hash(&self.key_path)?;
        if baseline.key_hash != key_hash {
            return Ok(self.locked_status("tamper_locked", "private key fingerprint differs from local trust seal".to_string(), ransomware.as_ref()));
        }

        Ok(TamperLockStatus {
            locked: false,
            status: "sealed".to_string(),
            reason: None,
            triggered_at_unix_ms: None,
            last_checked_unix_ms: now_unix_ms(),
            seal_hash: Some(baseline.seal_hash),
            seal_path: self.seal_path.display().to_string(),
            ransomware_status: Some(ransomware.as_ref().map(|s| s.status.clone()).unwrap_or_else(|| "clear".to_string())),
            ransomware_reason: ransomware.as_ref().and_then(|s| s.reason.clone()),
            ransomware_detected_at_unix_ms: ransomware.as_ref().and_then(|s| s.detected_at_unix_ms),
            ransomware_evidence_path: ransomware.and_then(|s| s.evidence_path),
        })
    }

    fn locked_status(&self, status_label: &str, reason: String, ransomware: Option<&RansomwareStatusFile>) -> TamperLockStatus {
        let previous = self.handle.get();
        TamperLockStatus {
            locked: true,
            status: status_label.to_string(),
            reason: Some(reason),
            triggered_at_unix_ms: previous.triggered_at_unix_ms.or_else(|| Some(now_unix_ms())),
            last_checked_unix_ms: now_unix_ms(),
            seal_hash: previous.seal_hash,
            seal_path: self.seal_path.display().to_string(),
            ransomware_status: ransomware.map(|s| s.status.clone()).or(previous.ransomware_status),
            ransomware_reason: ransomware.and_then(|s| s.reason.clone()).or(previous.ransomware_reason),
            ransomware_detected_at_unix_ms: ransomware.and_then(|s| s.detected_at_unix_ms).or(previous.ransomware_detected_at_unix_ms),
            ransomware_evidence_path: ransomware.and_then(|s| s.evidence_path.clone()).or(previous.ransomware_evidence_path),
        }
    }
}

pub fn seal_file_path(telemetry_path: &Path) -> PathBuf {
    telemetry_path.with_file_name("miner-trust-seal.json")
}

pub fn ransomware_status_file_path(telemetry_path: &Path) -> PathBuf {
    telemetry_path.with_file_name("ransomware-status.json")
}

fn build_seal(config_hash: &str, binary_hash: &str, key_hash: &str) -> Result<TamperSeal> {
    let created_at_unix_ms = now_unix_ms();
    let seal_hash = hash_bytes(format!("{config_hash}|{binary_hash}|{key_hash}").as_bytes());
    Ok(TamperSeal {
        version: 1,
        created_at_unix_ms,
        config_hash: config_hash.to_string(),
        binary_hash: binary_hash.to_string(),
        key_hash: key_hash.to_string(),
        seal_hash,
    })
}

fn write_seal_file(path: &Path, seal: &TamperSeal) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(seal)?;
    fs::write(path, raw).with_context(|| format!("Failed to write trust seal {}", path.display()))
}

fn read_trust_seal(path: &Path) -> Result<TamperSeal> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("Failed to read trust seal {}", path.display()))?;
    serde_json::from_str::<TamperSeal>(&raw).context("Invalid trust seal JSON")
}

fn read_ransomware_status(path: &Path) -> Result<Option<RansomwareStatusFile>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("Failed to read ransomware status {}", path.display()))?;
    let status = serde_json::from_str::<RansomwareStatusFile>(&raw)
        .with_context(|| format!("Invalid ransomware status JSON in {}", path.display()))?;
    Ok(Some(status))
}

fn read_key_hash(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("Failed to read key file {}", path.display()))?;
    Ok(hash_bytes(&bytes))
}

fn read_current_exe_bytes() -> Result<Vec<u8>> {
    let exe = std::env::current_exe().context("Failed to resolve current executable path")?;
    fs::read(&exe).with_context(|| format!("Failed to read current executable {}", exe.display()))
}

fn hash_bytes(bytes: &[u8]) -> String {
    hex::encode(blake3::hash(bytes).as_bytes())
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
