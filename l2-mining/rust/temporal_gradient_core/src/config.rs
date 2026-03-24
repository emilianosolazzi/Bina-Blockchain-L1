use crate::paths::{app_paths, ensure_app_layout};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Optional configuration for stale-block (Bitcoin orphan) entropy mining.
///
/// Node operators who have access to a Bitcoin full node or mempool API
/// can enable stale-block mining by adding this section to their config.
/// When absent or `enabled: false`, stale-block mining is completely inactive.
#[cfg(feature = "stale-mining")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaleBlockConfig {
    /// Master switch — must be `true` to activate stale-block mining.
    #[serde(default)]
    pub enabled: bool,
    /// Bitcoin RPC or API endpoint (e.g. "https://api.nativebtc.org").
    pub bitcoin_api_url: String,
    /// API key for authenticated endpoints. Appended as `?key=...` for
    /// REST calls and used in the WebSocket URL.
    #[serde(default)]
    pub api_key: Option<String>,
    /// How often to poll for new chain tips (seconds). Minimum 10.
    /// Used as the fallback interval when the WebSocket stream is unavailable.
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    /// Minimum PoW leading zeros required for a stale header. Default 11.
    #[serde(default = "default_min_leading_zeros")]
    pub min_leading_zeros: u32,
    /// Maximum stale block age to accept (seconds). Default 7200.
    #[serde(default = "default_max_stale_age")]
    pub max_stale_age_secs: u64,
    /// Submitter address for proof attribution on the L2 contract.
    #[serde(default)]
    pub submitter_address: String,
    /// Whether to automatically submit proofs to the L2 contract.
    #[serde(default)]
    pub auto_submit: bool,
}

#[cfg(feature = "stale-mining")]
fn default_poll_interval() -> u64 { 30 }
#[cfg(feature = "stale-mining")]
fn default_min_leading_zeros() -> u32 { 11 }
#[cfg(feature = "stale-mining")]
fn default_max_stale_age() -> u64 { 7200 }

#[cfg(feature = "stale-mining")]
impl StaleBlockConfig {
    /// Clamp values into safe ranges.
    pub fn normalize(&mut self) {
        if self.poll_interval_secs < 10 {
            self.poll_interval_secs = 10;
        }
        if self.min_leading_zeros == 0 {
            self.min_leading_zeros = 11;
        }
        if self.max_stale_age_secs == 0 {
            self.max_stale_age_secs = 7200;
        }
        // Strip whitespace from URL
        self.bitcoin_api_url = self.bitcoin_api_url.trim().to_string();
    }

    /// Convert to the internal `StaleBlockMinerConfig` used by the miner.
    pub fn to_miner_config(&self) -> crate::stale_block_miner::StaleBlockMinerConfig {
        crate::stale_block_miner::StaleBlockMinerConfig {
            bitcoin_api_url: self.bitcoin_api_url.clone(),
            api_key: self.api_key.clone(),
            poll_interval_secs: self.poll_interval_secs,
            min_leading_zeros: self.min_leading_zeros,
            max_stale_age_secs: self.max_stale_age_secs,
            submitter_address: self.submitter_address.clone(),
            auto_submit: self.auto_submit,
        }
    }
}

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
    /// Optional API key for authenticated RPC endpoints. Sent as an
    /// `x-api-key` HTTP header on every JSON-RPC request.
    #[serde(default)]
    pub rpc_api_key: Option<String>,
    /// Delay in seconds between mining cycles. Reduces RPC call frequency for
    /// free/public endpoints. Defaults to 10 seconds when omitted.
    #[serde(default = "default_cycle_delay")]
    pub cycle_delay_secs: u64,
    /// Optional stale-block mining config. Only compiled with the `stale-mining` feature.
    /// Operators add this section and set `enabled: true` to opt-in.
    #[cfg(feature = "stale-mining")]
    #[serde(default)]
    pub stale_block: Option<StaleBlockConfig>,
}

fn default_cycle_delay() -> u64 {
    10
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
            batch_size: 1024,
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
            rpc_api_key: None,
            cycle_delay_secs: 10,
            #[cfg(feature = "stale-mining")]
            stale_block: None,
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
        #[cfg(feature = "stale-mining")]
        if let Some(ref mut sb) = self.stale_block {
            sb.normalize();
            // Inherit top-level rpc_api_key when stale_block.api_key is missing.
            if sb.api_key.is_none() {
                sb.api_key = self.rpc_api_key.clone();
            }
        }
    }

    pub fn has_live_target(&self) -> bool {
        self.contract_address != crate::chain::DEFAULT_CONTRACT_PLACEHOLDER
    }

    /// Returns `true` when stale-block mining is compiled in AND enabled in the config.
    #[cfg(feature = "stale-mining")]
    pub fn stale_mining_enabled(&self) -> bool {
        self.stale_block.as_ref().map_or(false, |sb| sb.enabled)
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

    /// Path to the mining control file (`miner-control.json` next to the
    /// telemetry file).
    pub fn control_file_path(&self) -> Result<PathBuf> {
        Ok(crate::telemetry::MiningControl::control_file_path(&self.telemetry_path()?))
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
