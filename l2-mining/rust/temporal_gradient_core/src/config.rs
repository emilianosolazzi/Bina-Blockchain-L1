use crate::paths::{app_paths, ensure_app_layout};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinerConfig {
    pub config_version: u32,
    pub miner_name: String,
    pub contract_address: String,
    pub rpc_url: String,
    pub private_key_path: String,
    pub pool_id: u8,
    pub threads: usize,
    pub batch_size: usize,
    pub gas_price_multiplier: f64,
    pub log_level: String,
    pub stats_interval_seconds: u64,
    pub block_time_millis: u64,
    pub max_retries: usize,
    pub exit_after_solutions: Option<u64>,
    pub telemetry_file: Option<String>,
    #[serde(default)]
    pub relay_endpoint: Option<String>,
    #[serde(default)]
    pub relay_pinned_cert_sha256: Option<String>,
    #[serde(default)]
    pub relay_hmac_key: Option<String>,
    pub difficulty_zero_bits: u8,
    pub pqc_mode: String,
}

impl Default for MinerConfig {
    fn default() -> Self {
        Self {
            config_version: 1,
            miner_name: "default-miner".to_string(),
            contract_address: "0xYourContractAddress".to_string(),
            rpc_url: "http://localhost:8545".to_string(),
            private_key_path: "keys/miner.key".to_string(),
            pool_id: 0,
            threads: 4,
            batch_size: 32,
            gas_price_multiplier: 1.10,
            log_level: "INFO".to_string(),
            stats_interval_seconds: 5,
            block_time_millis: 12_000,
            max_retries: 5,
            exit_after_solutions: None,
            telemetry_file: None,
            relay_endpoint: None,
            relay_pinned_cert_sha256: None,
            relay_hmac_key: None,
            difficulty_zero_bits: 11,
            pqc_mode: "enhanced".to_string(),
        }
    }
}

impl MinerConfig {
    pub fn normalize(&mut self) {
        if self.threads == 0 {
            self.threads = 1;
        }
        if self.batch_size == 0 {
            self.batch_size = 1;
        }
        if self.stats_interval_seconds == 0 {
            self.stats_interval_seconds = 5;
        }
        if self.block_time_millis == 0 {
            self.block_time_millis = 12_000;
        }
        if self.gas_price_multiplier <= 0.0 {
            self.gas_price_multiplier = 1.10;
        }
        if self.log_level.trim().is_empty() {
            self.log_level = "INFO".to_string();
        }
        if self.pqc_mode.trim().is_empty() {
            self.pqc_mode = "enhanced".to_string();
        }
    }

    pub fn has_live_target(&self) -> bool {
        self.contract_address != crate::chain::DEFAULT_CONTRACT_PLACEHOLDER
    }

    pub fn stats_interval(&self) -> Duration {
        Duration::from_secs(self.stats_interval_seconds)
    }

    pub fn telemetry_path(&self) -> Result<PathBuf> {
        if let Some(path) = &self.telemetry_file {
            return Ok(PathBuf::from(path));
        }

        let paths = app_paths()?;
        Ok(paths.telemetry_file)
    }

    pub fn save_to_path(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }

        let content = serde_json::to_string_pretty(self)?;
        fs::write(path, content).with_context(|| format!("Failed to write {}", path.display()))
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let mut config: MinerConfig = serde_json::from_str(&raw)
            .with_context(|| format!("Invalid JSON in {}", path.display()))?;
        config.normalize();
        Ok(config)
    }
}

pub fn default_config_json() -> Result<String> {
    Ok(serde_json::to_string_pretty(&MinerConfig::default())?)
}

pub fn load_or_create_config(config_path: Option<&Path>) -> Result<(MinerConfig, PathBuf)> {
    let paths = ensure_app_layout()?;
    let target = config_path
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.config_file.clone());

    if target.exists() {
        return Ok((MinerConfig::load_from_path(&target)?, target));
    }

    let mut config = MinerConfig::default();
    config.telemetry_file = Some(paths.telemetry_file.to_string_lossy().to_string());
    config.private_key_path = paths.key_dir.join("miner.key").to_string_lossy().to_string();
    config.save_to_path(&target)?;
    Ok((config, target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn creates_default_config_when_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("miner-config.json");
        let (config, written) = load_or_create_config(Some(&path)).unwrap();
        assert_eq!(written, path);
        assert_eq!(config.threads, 4);
        assert!(written.exists());
    }
}
