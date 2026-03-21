use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::sync::MutexGuard;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MinerState {
    Starting,
    Running,
    Stopping,
    Stopped,
}

/// Describes the current phase of the live commit-reveal mining cycle.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MiningPhase {
    Searching,
    SolutionFound,
    WaitingForClearance,
    Committing,
    CommitmentLocked,
    Revealing,
    RewardReceived,
}

impl std::fmt::Display for MiningPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Searching => write!(f, "Searching for solution..."),
            Self::SolutionFound => write!(f, "Solution found!"),
            Self::WaitingForClearance => write!(f, "Waiting for previous commitment to expire"),
            Self::Committing => write!(f, "Submitting commitment to blockchain..."),
            Self::CommitmentLocked => write!(f, "Commitment locked \u{2014} awaiting reveal window"),
            Self::Revealing => write!(f, "Revealing solution on-chain..."),
            Self::RewardReceived => write!(f, "Reward received!"),
        }
    }
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
    #[serde(default)]
    pub output_count: u64,
    pub last_solution_nonce: Option<u64>,
    pub last_solution_hash_hex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_commit_hash_hex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_output_hash_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_fp_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_memory_kb: Option<u64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub epoch_stats: HashMap<u64, u64>,
    pub temperature_c: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mining_phase: Option<MiningPhase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase_blocks_remaining: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase_eta_seconds: Option<u64>,
}

// ---------- Phase tracking for cross-module communication ----------

/// Snapshot of the current mining phase, used by the display layer.
#[derive(Debug, Clone, Default)]
pub struct PhaseState {
    pub phase: Option<MiningPhase>,
    pub blocks_remaining: Option<u64>,
    pub eta_seconds: Option<u64>,
}

/// Thread-safe handle that [`chain`] writes to during the commit-reveal
/// cycle and the telemetry ticker reads from every second.
#[derive(Debug, Clone)]
pub struct PhaseTracker {
    inner: Arc<Mutex<PhaseState>>,
    block_time_millis: u64,
}

impl PhaseTracker {
    pub fn new() -> Self {
        Self::with_block_time_millis(12_000)
    }

    pub fn with_block_time_millis(block_time_millis: u64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(PhaseState::default())),
            block_time_millis: block_time_millis.max(1),
        }
    }

    pub fn set(&self, phase: MiningPhase, blocks_remaining: Option<u64>) {
        let mut s = self.lock_state();
        s.phase = Some(phase);
        s.blocks_remaining = blocks_remaining;
        s.eta_seconds = blocks_remaining.map(|b| {
            let eta_millis = b.saturating_mul(self.block_time_millis);
            eta_millis.saturating_add(999) / 1000
        });
    }

    pub fn get(&self) -> PhaseState {
        self.lock_state().clone()
    }

    pub fn clear(&self) {
        let mut s = self.lock_state();
        *s = PhaseState::default();
    }

    fn lock_state(&self) -> MutexGuard<'_, PhaseState> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl Default for PhaseTracker {
    fn default() -> Self {
        Self::new()
    }
}
