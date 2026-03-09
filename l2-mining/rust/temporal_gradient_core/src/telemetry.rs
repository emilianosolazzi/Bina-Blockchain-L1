use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MinerState {
    Starting,
    Running,
    Stopping,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetrySnapshot {
    pub timestamp_unix_ms: u128,
    pub state: MinerState,
    pub uptime_seconds: u64,
    pub worker_count: usize,
    pub hashes: u64,
    pub hashrate_hs: f64,
    pub solutions: u64,
    pub accepted_submissions: u64,
    pub rejected_submissions: u64,
    pub total_rewards_estimate: f64,
    pub last_solution_nonce: Option<u64>,
    pub last_solution_hash_hex: Option<String>,
    pub temperature_c: Option<f32>,
}
