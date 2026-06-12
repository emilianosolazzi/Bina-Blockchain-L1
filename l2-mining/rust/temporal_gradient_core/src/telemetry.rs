use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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

    // ── Queue telemetry ──
    #[serde(default)]
    pub pending_solutions: u64,
    #[serde(default)]
    pub approved_solutions: u64,
    #[serde(default)]
    pub rejected_solutions: u64,

    // ── Mining control state ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mining_paused: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mining_power_pct: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tamper_locked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tamper_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tamper_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tamper_triggered_at_unix_ms: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tamper_seal_hash: Option<String>,

    // ── Stale block mining telemetry ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_block_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_fork_depth: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_zero_bits: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_quality: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_xor_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitcoin_tip_height: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_pending_proofs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_proof_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_raw_header_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_block_hash_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_canonical_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_entropy_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_submitter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_created_at: Option<u64>,
}

// ---------- Stale block telemetry shared state ----------

/// Snapshot of stale block mining status, updated by the stale block loop
/// and read by the telemetry ticker.
#[derive(Debug, Clone, Default)]
pub struct StaleBlockTelemetryState {
    pub stale_block_count: u64,
    pub max_fork_depth: u32,
    pub max_leading_zeros: u32,
    pub average_quality: u32,
    pub cumulative_xor_hex: String,
    pub bitcoin_tip_height: u64,
    pub pending_proofs: u64,
    pub latest_proof_id: Option<String>,
    pub latest_raw_header_hex: Option<String>,
    pub latest_block_hash_hex: Option<String>,
    pub latest_canonical_hash_hex: Option<String>,
    pub latest_entropy_digest_hex: Option<String>,
    pub latest_submitter: Option<String>,
    pub latest_created_at: Option<u64>,
}

/// Thread-safe handle for sharing stale block telemetry between the stale
/// block mining loop and the main telemetry system.
#[derive(Debug, Clone)]
pub struct StaleBlockTelemetry {
    inner: Arc<Mutex<StaleBlockTelemetryState>>,
}

impl StaleBlockTelemetry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(StaleBlockTelemetryState::default())),
        }
    }

    pub fn update(&self, state: StaleBlockTelemetryState) {
        let mut s = self.lock_state();
        let tip = s.bitcoin_tip_height; // preserve tip height set independently
        *s = state;
        if s.bitcoin_tip_height == 0 {
            s.bitcoin_tip_height = tip;
        }
    }

    pub fn get(&self) -> StaleBlockTelemetryState {
        self.lock_state().clone()
    }

    pub fn set_tip_height(&self, height: u64) {
        self.lock_state().bitcoin_tip_height = height;
    }

    fn lock_state(&self) -> MutexGuard<'_, StaleBlockTelemetryState> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl Default for StaleBlockTelemetry {
    fn default() -> Self {
        Self::new()
    }
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

// ---------- Mining control (pause / power throttle) ----------

/// External mining control state, read from a JSON file written by the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MiningControl {
    #[serde(default)]
    pub paused: bool,
    /// Mining power percentage: 25, 50, 75, or 100. Clamped to these values.
    #[serde(default = "default_power_pct")]
    pub power_pct: u8,
    /// One-shot trigger: request an immediate stale-proof submission attempt.
    #[serde(default)]
    pub submit_stale_now: bool,
    /// One-shot trigger: reseal and clear the local tamper-lock state.
    #[serde(default)]
    pub tamper_reseal_now: bool,
    /// One-shot trigger: approve a queued solution hash for submission.
    #[serde(default)]
    pub approve_solution_hash: Option<String>,
}

fn default_power_pct() -> u8 { 100 }

impl Default for MiningControl {
    fn default() -> Self {
        Self { paused: false, power_pct: 100, submit_stale_now: false, tamper_reseal_now: false, approve_solution_hash: None }
    }
}

impl MiningControl {
    /// Clamp power_pct to nearest valid tier (25/50/75/100).
    pub fn normalized_power_pct(&self) -> u8 {
        match self.power_pct {
            0..=37 => 25,
            38..=62 => 50,
            63..=87 => 75,
            _ => 100,
        }
    }

    /// Effective worker count for a given max thread count.
    pub fn effective_workers(&self, max_threads: usize) -> usize {
        let pct = self.normalized_power_pct() as usize;
        (max_threads * pct / 100).max(1)
    }

    /// Read control file. Returns default if file doesn't exist or is invalid.
    pub fn read_from_file(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Write control file.
    pub fn write_to_file(&self, path: &Path) -> std::io::Result<()> {
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)
    }

    /// Standard control file path next to the telemetry file.
    pub fn control_file_path(telemetry_path: &Path) -> PathBuf {
        telemetry_path.with_file_name("miner-control.json")
    }

    /// Consume the one-shot stale submit trigger, resetting it to `false`.
    pub fn take_submit_stale_now(path: &Path) -> bool {
        let mut control = Self::read_from_file(path);
        if !control.submit_stale_now {
            return false;
        }
        control.submit_stale_now = false;
        let _ = control.write_to_file(path);
        true
    }

    /// Consume the one-shot tamper reseal trigger, resetting it to `false`.
    pub fn take_tamper_reseal_now(path: &Path) -> bool {
        let mut control = Self::read_from_file(path);
        if !control.tamper_reseal_now {
            return false;
        }
        control.tamper_reseal_now = false;
        let _ = control.write_to_file(path);
        true
    }

    /// Consume the one-shot solution approval trigger, returning the hash if set.
    pub fn take_approve_solution_hash(path: &Path) -> Option<String> {
        let mut control = Self::read_from_file(path);
        if let Some(hash) = control.approve_solution_hash.take() {
            let _ = control.write_to_file(path);
            Some(hash)
        } else {
            None
        }
    }
}
