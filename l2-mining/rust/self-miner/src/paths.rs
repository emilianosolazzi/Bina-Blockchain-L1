//! Self-miner–specific AppData paths.
//!
//! Uses a **separate** app identity (`TGSelfMiner`) so the self-miner never
//! collides with the original `TemporalGradientMiner` stack.  Each self-miner
//! instance gets its own config, key, telemetry, and trust-seal files.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

const APP_QUALIFIER: &str = "com";
const APP_ORGANIZATION: &str = "entropy";
const APP_NAME: &str = "TGSelfMiner";

#[derive(Debug, Clone)]
pub struct SelfMinerPaths {
    pub install_root: PathBuf,
    pub bin_dir: PathBuf,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub key_dir: PathBuf,
    pub config_file: PathBuf,
    pub telemetry_file: PathBuf,
}

pub fn self_miner_paths() -> Result<SelfMinerPaths> {
    let dirs = directories::ProjectDirs::from(APP_QUALIFIER, APP_ORGANIZATION, APP_NAME)
        .context("Failed to determine per-user self-miner directories")?;

    let install_root = dirs.data_local_dir().to_path_buf();
    let bin_dir = install_root.join("bin");
    let config_dir = dirs.config_dir().to_path_buf();
    let data_dir = dirs.data_dir().to_path_buf();
    let log_dir = install_root.join("logs");
    let key_dir = data_dir.join("keys");
    let config_file = config_dir.join("miner-config.json");
    let telemetry_file = log_dir.join("telemetry.jsonl");

    Ok(SelfMinerPaths {
        install_root,
        bin_dir,
        config_dir,
        data_dir,
        log_dir,
        key_dir,
        config_file,
        telemetry_file,
    })
}

pub fn ensure_self_miner_layout() -> Result<SelfMinerPaths> {
    let paths = self_miner_paths()?;
    for dir in [
        &paths.install_root,
        &paths.bin_dir,
        &paths.config_dir,
        &paths.data_dir,
        &paths.log_dir,
        &paths.key_dir,
    ] {
        fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create {}", dir.display()))?;
    }
    Ok(paths)
}
