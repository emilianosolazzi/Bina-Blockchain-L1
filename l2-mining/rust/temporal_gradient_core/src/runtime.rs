use crate::chain::{LiveChallenge, LiveMiningClient, LiveSubmission};
use crate::config::MinerConfig;
use crate::crypto::{
    build_commitment_payload, contract_hash_message, has_leading_zero_bits, miner_address_from_signing_key,
    pre_filter_nonce, random_secret, MiningMaterial,
};
use crate::pqc::PqcMode;
use crate::seed::{decode_temporal_seed_timestamp, generate_temporal_seed};
use crate::telemetry::{MinerState, MiningPhase, PhaseTracker, TelemetrySnapshot};
use anyhow::{anyhow, Result};
use ethers::signers::LocalWallet;
use ethers::types::U256;
use k256::ecdsa::SigningKey;
use rand::rngs::OsRng;
use sha3::{Digest, Keccak256};
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;
use tokio::time;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
struct LiveWorkerCandidate {
    worker_id: usize,
    nonce: u64,
    temporal_seed: [u8; 8],
    reveal_signature: Vec<u8>,
    secret_value: [u8; 32],
    commit_hash: [u8; 32],
    solution_hash: [u8; 32],
}

#[derive(Debug)]
struct LiveWorkerSearchResult {
    hashes_checked: u64,
    candidate: Option<LiveWorkerCandidate>,
}

#[derive(Debug)]
struct RuntimeStats {
    state: MinerState,
    hashes: u64,
    solutions: u64,
    accepted_submissions: u64,
    rejected_submissions: u64,
    total_rewards_estimate: f64,
    last_solution_nonce: Option<u64>,
    last_solution_hash_hex: Option<String>,
    last_commit_hash_hex: Option<String>,
    last_output_hash_hex: Option<String>,
    temperature_c: Option<f32>,
}

impl Default for RuntimeStats {
    fn default() -> Self {
        Self {
            state: MinerState::Starting,
            hashes: 0,
            solutions: 0,
            accepted_submissions: 0,
            rejected_submissions: 0,
            total_rewards_estimate: 0.0,
            last_solution_nonce: None,
            last_solution_hash_hex: None,
            last_commit_hash_hex: None,
            last_output_hash_hex: None,
            temperature_c: Some(50.0),
        }
    }
}

pub struct MinerHandle {
    config: MinerConfig,
    started_at: Instant,
    stats: Arc<Mutex<RuntimeStats>>,
    telemetry_tx: broadcast::Sender<TelemetrySnapshot>,
    shutdown: CancellationToken,
    join_handle: JoinHandle<Result<()>>,
}

impl MinerHandle {
    pub fn subscribe(&self) -> broadcast::Receiver<TelemetrySnapshot> {
        self.telemetry_tx.subscribe()
    }

    pub async fn snapshot(&self) -> TelemetrySnapshot {
        snapshot_from_state(&self.config, self.started_at, &self.stats).await
    }

    pub fn shutdown(&self) {
        self.shutdown.cancel();
    }

    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    pub async fn wait(self) -> Result<()> {
        self.join_handle.await.map_err(|e| anyhow!("Miner task join error: {e}"))?
    }
}

pub fn spawn_miner(config: MinerConfig) -> Result<MinerHandle> {
    let (telemetry_tx, _) = broadcast::channel(256);
    let stats = Arc::new(Mutex::new(RuntimeStats::default()));
    let started_at = Instant::now();
    let shutdown = CancellationToken::new();
    let task_config = config.clone();
    let task_stats = Arc::clone(&stats);
    let task_tx = telemetry_tx.clone();
    let task_shutdown = shutdown.clone();

    let join_handle = tokio::spawn(async move {
        run_runtime(task_config, task_stats, started_at, task_tx, task_shutdown).await
    });

    Ok(MinerHandle {
        config,
        started_at,
        stats,
        telemetry_tx,
        shutdown,
        join_handle,
    })
}

async fn run_runtime(
    config: MinerConfig,
    stats: Arc<Mutex<RuntimeStats>>,
    started_at: Instant,
    telemetry_tx: broadcast::Sender<TelemetrySnapshot>,
    shutdown: CancellationToken,
) -> Result<()> {
    if let Some(live_client) = LiveMiningClient::connect(&config).await? {
        info!("Live chain submission mode enabled for contract {}", config.contract_address);
        return run_live_runtime(config, stats, started_at, telemetry_tx, shutdown, live_client).await;
    }

    run_simulated_runtime(config, stats, started_at, telemetry_tx, shutdown).await
}

async fn run_simulated_runtime(
    config: MinerConfig,
    stats: Arc<Mutex<RuntimeStats>>,
    started_at: Instant,
    telemetry_tx: broadcast::Sender<TelemetrySnapshot>,
    shutdown: CancellationToken,
) -> Result<()> {
    {
        let mut guard = stats.lock().await;
        guard.state = MinerState::Running;
    }

    let mut workers = Vec::new();
    for worker_id in 0..config.threads {
        workers.push(tokio::spawn(run_worker(
            worker_id,
            config.clone(),
            Arc::clone(&stats),
            started_at,
            telemetry_tx.clone(),
            shutdown.clone(),
        )));
    }

    let mut ticker = time::interval(config.stats_interval());
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                break;
            }
            _ = ticker.tick() => {
                let snapshot = snapshot_from_state(&config, started_at, &stats).await;
                let _ = telemetry_tx.send(snapshot);
            }
        }
    }

    {
        let mut guard = stats.lock().await;
        guard.state = MinerState::Stopping;
    }

    for worker in workers {
        match worker.await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => warn!("worker exited with error: {err}"),
            Err(err) => warn!("worker join failed: {err}"),
        }
    }

    {
        let mut guard = stats.lock().await;
        guard.state = MinerState::Stopped;
    }

    let final_snapshot = snapshot_from_state(&config, started_at, &stats).await;
    let _ = telemetry_tx.send(final_snapshot);
    info!("Miner runtime stopped cleanly");
    Ok(())
}

async fn run_live_runtime(
    config: MinerConfig,
    stats: Arc<Mutex<RuntimeStats>>,
    started_at: Instant,
    telemetry_tx: broadcast::Sender<TelemetrySnapshot>,
    shutdown: CancellationToken,
    live_client: LiveMiningClient,
) -> Result<()> {
    {
        let mut guard = stats.lock().await;
        guard.state = MinerState::Running;
    }

    let phase_tracker = PhaseTracker::new();

    // Background telemetry ticker — emits snapshots every second so the
    // display stays updated even while submit_solution is blocking.
    {
        let bg_stats = Arc::clone(&stats);
        let bg_tx = telemetry_tx.clone();
        let bg_shutdown = shutdown.clone();
        let bg_config = config.clone();
        let bg_phase = phase_tracker.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = bg_shutdown.cancelled() => break,
                    _ = interval.tick() => {
                        let snapshot = snapshot_with_phase(
                            &bg_config, started_at, &bg_stats, &bg_phase,
                        ).await;
                        let _ = bg_tx.send(snapshot);
                    }
                }
            }
        });
    }

    let pqc_mode = crate::pqc::PqcMode::parse(&config.pqc_mode);
    let mut retry_count = 0usize;
    let mut nonce_cursor = 0u64;

    // ── Resume a saved commitment from a previous run ──────────────
    if let Some(pending) = crate::pending::load(&config.private_key_path)? {
        info!(
            "Found saved commitment from block {} — attempting reveal",
            pending.commit_block
        );
        match live_client
            .reveal_pending(&pending, &phase_tracker, &config.private_key_path)
            .await
        {
            Ok(Some(receipt)) => {
                let on_chain = LiveMiningClient::extract_reward_from_receipt(&receipt).unwrap_or(0.0);
                let reward = if on_chain > 0.0 { on_chain } else { estimate_reward(config.difficulty_zero_bits) };
                phase_tracker.set(MiningPhase::RewardReceived, None);
                {
                    let mut guard = stats.lock().await;
                    guard.solutions = guard.solutions.saturating_add(1);
                    guard.accepted_submissions = guard.accepted_submissions.saturating_add(1);
                    guard.total_rewards_estimate += reward;
                    guard.last_solution_nonce = Some(pending.nonce);
                    guard.last_solution_hash_hex =
                        Some(hex_string(&pending.commit_hash));
                }
                info!(
                    "Resumed and revealed saved commitment nonce={} reward={reward}",
                    pending.nonce
                );
                if let Some(limit) = config.exit_after_solutions {
                    let solutions = stats.lock().await.solutions;
                    if solutions >= limit {
                        shutdown.cancel();
                    }
                }
            }
            Ok(None) => {
                info!("Saved commitment expired or already revealed, starting fresh");
            }
            Err(err) => {
                warn!("Failed to reveal saved commitment: {err:#}");
                // The pending file is still on disk; wait_for_commitment_clearance
                // will handle the expiry before the next commit.
            }
        }
    }

    while !shutdown.is_cancelled() {
        phase_tracker.set(MiningPhase::Searching, None);

        let challenge = match live_client.current_challenge().await {
            Ok(challenge) => {
                retry_count = 0;
                challenge
            }
            Err(err) => {
                retry_count += 1;
                warn!("Failed to fetch mining challenge: {err}");
                if retry_count > config.max_retries {
                    return Err(anyhow!("Exceeded live challenge retries"));
                }
                time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        if let Some(submission) = attempt_live_solution(
            &live_client,
            &config,
            &stats,
            &telemetry_tx,
            &shutdown,
            &challenge,
            pqc_mode,
            &mut nonce_cursor,
            started_at,
        ).await? {
            phase_tracker.set(MiningPhase::SolutionFound, None);

            match live_client.submit_solution(&submission, &phase_tracker, &config.private_key_path).await {
                Ok(receipt) => {
                    let on_chain = LiveMiningClient::extract_reward_from_receipt(&receipt).unwrap_or(0.0);
                    let reward = if on_chain > 0.0 { on_chain } else { estimate_reward(config.difficulty_zero_bits) };
                    let output_hash = LiveMiningClient::extract_output_hash_from_receipt(&receipt);
                    phase_tracker.set(MiningPhase::RewardReceived, None);
                    {
                        let mut guard = stats.lock().await;
                        guard.solutions = guard.solutions.saturating_add(1);
                        guard.accepted_submissions = guard.accepted_submissions.saturating_add(1);
                        guard.total_rewards_estimate += reward;
                        guard.last_solution_nonce = Some(submission.nonce);
                        guard.last_solution_hash_hex = Some(hex_string(&submission.commitment.commit_hash));
                        guard.last_commit_hash_hex = Some(hex_string(&submission.commitment.commit_hash));
                        guard.last_output_hash_hex = output_hash;
                    }

                    info!("Submitted live mining solution nonce={} reward={reward}", submission.nonce);

                    if let Some(limit) = config.exit_after_solutions {
                        let solutions = stats.lock().await.solutions;
                        if solutions >= limit {
                            shutdown.cancel();
                        }
                    }
                }
                Err(err) => {
                    let mut guard = stats.lock().await;
                    guard.rejected_submissions = guard.rejected_submissions.saturating_add(1);
                    warn!("Live submission failed: {err:#}");
                }
            }
        }

        time::sleep(Duration::from_millis(250)).await;
    }

    {
        let mut guard = stats.lock().await;
        guard.state = MinerState::Stopped;
    }

    let final_snapshot = snapshot_from_state(&config, started_at, &stats).await;
    let _ = telemetry_tx.send(final_snapshot);
    info!("Live miner runtime stopped cleanly");
    Ok(())
}

async fn attempt_live_solution(
    live_client: &LiveMiningClient,
    config: &MinerConfig,
    stats: &Arc<Mutex<RuntimeStats>>,
    telemetry_tx: &broadcast::Sender<TelemetrySnapshot>,
    shutdown: &CancellationToken,
    challenge: &LiveChallenge,
    _pqc_mode: crate::pqc::PqcMode,
    nonce_cursor: &mut u64,
    started_at: Instant,
) -> Result<Option<LiveSubmission>> {
    let miner_address = live_client.miner_address();
    let miner_address_bytes: [u8; 20] = miner_address.0;
    let step = config.threads.max(1) as u64;
    let solution_found = Arc::new(AtomicBool::new(false));
    let base_nonce = *nonce_cursor;
    let batch_size = config.batch_size.max(1);
    let worker_count = config.threads.max(1);
    let signer = live_client.signer_clone();

    let mut tasks = Vec::with_capacity(worker_count);
    for worker_id in 0..worker_count {
        let challenge = challenge.clone();
        let miner_address = miner_address;
        let contract_address = live_client.contract_address;
        let signer = signer.clone();
        let solution_found = Arc::clone(&solution_found);
        let shutdown = shutdown.clone();

        tasks.push(tokio::task::spawn_blocking(move || {
            search_live_worker_batch(
                worker_id,
                batch_size,
                base_nonce,
                step,
                challenge,
                miner_address,
                miner_address_bytes,
                contract_address,
                signer,
                solution_found,
                shutdown,
            )
        }));
    }

    let mut hashes_checked = 0u64;
    let mut winning_candidate: Option<LiveWorkerCandidate> = None;

    for task in tasks {
        let result = task.await.map_err(|err| anyhow!("Live worker join error: {err}"))??;
        hashes_checked = hashes_checked.saturating_add(result.hashes_checked);
        if winning_candidate.is_none() {
            winning_candidate = result.candidate;
        }
    }

    {
        let mut guard = stats.lock().await;
        guard.hashes = guard.hashes.saturating_add(hashes_checked);
        guard.temperature_c = Some(45.0);
    }

    *nonce_cursor = nonce_cursor.saturating_add((batch_size * worker_count) as u64);

    let Some(candidate) = winning_candidate else {
        return Ok(None);
    };

    let commit_nonce = live_client.next_commit_nonce().await?;
    let commitment = crate::crypto::DynamicMiningCommitment {
        commit_hash: candidate.commit_hash,
        pool_id: config.pool_id,
        nonce: commit_nonce,
        deadline: unix_secs().saturating_add(300),
    };

    let preview = {
        let mut guard = stats.lock().await;
        guard.last_solution_nonce = Some(candidate.nonce);
        guard.last_solution_hash_hex = Some(hex_string(&candidate.solution_hash));
        guard.last_commit_hash_hex = None;
        guard.last_output_hash_hex = None;
        guard.temperature_c = Some(45.0 + ((candidate.worker_id % 10) as f32));
        snapshot_from_guard(config, started_at, &guard)
    };
    let _ = telemetry_tx.send(preview);

    Ok(Some(LiveSubmission {
        commitment,
        previous_output: challenge.previous_output,
        temporal_seed: candidate.temporal_seed,
        nonce: candidate.nonce,
        reveal_signature: candidate.reveal_signature,
        secret_value: candidate.secret_value,
    }))
}

fn search_live_worker_batch(
    worker_id: usize,
    batch_size: usize,
    base_nonce: u64,
    step: u64,
    challenge: LiveChallenge,
    miner_address: ethers::types::Address,
    miner_address_bytes: [u8; 20],
    contract_address: ethers::types::Address,
    signer: LocalWallet,
    solution_found: Arc<AtomicBool>,
    shutdown: CancellationToken,
) -> Result<LiveWorkerSearchResult> {
    let temporal_seed = generate_temporal_seed()?;
    let seed_timestamp = decode_temporal_seed_timestamp(&temporal_seed)?;
    let time_based_entropy = keccak256_bytes(&[
        &challenge.block_timestamp.to_be_bytes(),
        challenge.prevrandao.as_slice(),
        &seed_timestamp.to_be_bytes(),
        contract_address.as_bytes(),
    ].concat());

    let mut hashes_checked = 0u64;
    for batch_index in 0..batch_size {
        if shutdown.is_cancelled() || solution_found.load(Ordering::Relaxed) {
            break;
        }

        let current_nonce = base_nonce
            .saturating_add(worker_id as u64)
            .saturating_add((batch_index as u64) * step);
        hashes_checked = hashes_checked.saturating_add(1);

        let secret_value = random_secret();
        let material = MiningMaterial {
            previous_output: challenge.previous_output,
            temporal_seed,
            nonce: current_nonce,
            miner_address: miner_address_bytes,
            time_based_entropy,
            secret_value,
        };

        let entropy_hash = crate::crypto::create_entropy_hash(&material);
        let reveal_signature = signer.sign_hash(ethers::types::H256::from(entropy_hash))?.to_vec();
        let solution_hash = quantum_resistant_hash_live(&reveal_signature, &entropy_hash, &secret_value);

        if meets_difficulty(&solution_hash, challenge.difficulty) {
            solution_found.store(true, Ordering::SeqCst);

            let commit_hash = keccak256_bytes(&[
                challenge.previous_output.as_slice(),
                temporal_seed.as_slice(),
                &current_nonce.to_be_bytes(),
                reveal_signature.as_slice(),
                secret_value.as_slice(),
                miner_address.as_bytes(),
            ].concat());

            return Ok(LiveWorkerSearchResult {
                hashes_checked,
                candidate: Some(LiveWorkerCandidate {
                    worker_id,
                    nonce: current_nonce,
                    temporal_seed,
                    reveal_signature,
                    secret_value,
                    commit_hash,
                    solution_hash,
                }),
            });
        }
    }

    Ok(LiveWorkerSearchResult {
        hashes_checked,
        candidate: None,
    })
}

async fn run_worker(
    worker_id: usize,
    config: MinerConfig,
    stats: Arc<Mutex<RuntimeStats>>,
    started_at: Instant,
    telemetry_tx: broadcast::Sender<TelemetrySnapshot>,
    shutdown: CancellationToken,
) -> Result<()> {
    let signing_key = SigningKey::random(&mut OsRng);
    let miner_address = miner_address_from_signing_key(&signing_key);
    let pqc_mode = PqcMode::parse(&config.pqc_mode);
    let mut nonce = worker_id as u64;
    let nonce_step = config.threads as u64;
    let target_divisor = (config.difficulty_zero_bits as u64).max(1);

    debug!("worker {worker_id} started");
    while !shutdown.is_cancelled() {
        let temporal_seed = generate_temporal_seed()?;
        let time_based_entropy = contract_hash_message(
            &[
                &unix_ms().to_be_bytes(),
                &(worker_id as u64).to_be_bytes(),
                temporal_seed.as_slice(),
                miner_address.as_slice(),
            ]
            .concat(),
        );
        let previous_output = contract_hash_message(&[&(worker_id as u64).to_be_bytes(), temporal_seed.as_slice()].concat());
        let pre_input = [
            previous_output.as_slice(),
            temporal_seed.as_slice(),
            miner_address.as_slice(),
            time_based_entropy.as_slice(),
        ]
        .concat();

        for _ in 0..config.batch_size {
            if shutdown.is_cancelled() {
                break;
            }

            let current_nonce = nonce;
            nonce = nonce.saturating_add(nonce_step);

            {
                let mut guard = stats.lock().await;
                guard.hashes = guard.hashes.saturating_add(1);
                guard.temperature_c = Some(45.0 + ((worker_id % 10) as f32));
            }

            if !pre_filter_nonce(current_nonce, &pre_input, target_divisor) {
                continue;
            }

            let secret_value = random_secret();
            let material = MiningMaterial {
                previous_output,
                temporal_seed,
                nonce: current_nonce,
                miner_address,
                time_based_entropy,
                secret_value,
            };
            let payload = build_commitment_payload(
                &signing_key,
                &material,
                0,
                unix_secs().saturating_add(300),
                pqc_mode,
            );

            if has_leading_zero_bits(&payload.solution_hash, config.difficulty_zero_bits) {
                let snapshot = {
                    let mut guard = stats.lock().await;
                    guard.solutions = guard.solutions.saturating_add(1);
                    guard.accepted_submissions = guard.accepted_submissions.saturating_add(1);
                    guard.total_rewards_estimate += estimate_reward(config.difficulty_zero_bits);
                    guard.last_solution_nonce = Some(current_nonce);
                    guard.last_solution_hash_hex = Some(hex_string(&payload.solution_hash));
                    snapshot_from_guard(&config, started_at, &guard)
                };

                let _ = telemetry_tx.send(snapshot);
                info!("worker {worker_id} found solution nonce={current_nonce}");

                if let Some(limit) = config.exit_after_solutions {
                    let solutions = stats.lock().await.solutions;
                    if solutions >= limit {
                        shutdown.cancel();
                        break;
                    }
                }
            }
        }

        time::sleep(Duration::from_millis(10)).await;
    }

    Ok(())
}

async fn snapshot_from_state(
    config: &MinerConfig,
    started_at: Instant,
    stats: &Arc<Mutex<RuntimeStats>>,
) -> TelemetrySnapshot {
    let guard = stats.lock().await;
    snapshot_from_guard(config, started_at, &guard)
}

fn snapshot_from_guard(
    config: &MinerConfig,
    started_at: Instant,
    guard: &RuntimeStats,
) -> TelemetrySnapshot {
    let uptime = started_at.elapsed().as_secs().max(1);
    TelemetrySnapshot {
        timestamp_unix_ms: unix_ms() as u128,
        state: guard.state,
        uptime_seconds: uptime,
        worker_count: config.threads,
        hashes: guard.hashes,
        hashrate_hs: guard.hashes as f64 / uptime as f64,
        solutions: guard.solutions,
        accepted_submissions: guard.accepted_submissions,
        rejected_submissions: guard.rejected_submissions,
        total_rewards_estimate: guard.total_rewards_estimate,
        last_solution_nonce: guard.last_solution_nonce,
        last_solution_hash_hex: guard.last_solution_hash_hex.clone(),
        last_commit_hash_hex: guard.last_commit_hash_hex.clone(),
        last_output_hash_hex: guard.last_output_hash_hex.clone(),
        temperature_c: guard.temperature_c,
        mining_phase: None,
        phase_blocks_remaining: None,
        phase_eta_seconds: None,
    }
}

async fn snapshot_with_phase(
    config: &MinerConfig,
    started_at: Instant,
    stats: &Arc<Mutex<RuntimeStats>>,
    phase_tracker: &PhaseTracker,
) -> TelemetrySnapshot {
    let ps = phase_tracker.get();
    let guard = stats.lock().await;
    let mut snapshot = snapshot_from_guard(config, started_at, &guard);
    snapshot.mining_phase = ps.phase;
    snapshot.phase_blocks_remaining = ps.blocks_remaining;
    snapshot.phase_eta_seconds = ps.eta_seconds;
    snapshot
}

fn estimate_reward(difficulty_zero_bits: u8) -> f64 {
    (difficulty_zero_bits as f64 / 8.0).max(1.0)
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn hex_string(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2 + 2);
    out.push_str("0x");
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn quantum_resistant_hash_live(
    signature: &[u8],
    entropy_hash: &[u8; 32],
    secret_value: &[u8; 32],
) -> [u8; 32] {
    let mut h = keccak256_bytes(&[signature, entropy_hash, secret_value].concat());
    for i in 0..3u8 {
        h = keccak256_bytes(xor_round_constant(h, i + 1).as_slice());
        h = rotate_hash_left(h, 7);
    }
    h
}

fn meets_difficulty(hash: &[u8; 32], difficulty: U256) -> bool {
    U256::from_big_endian(hash) < difficulty
}

fn keccak256_bytes(input: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(input);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn rotate_hash_left(input: [u8; 32], bits: u32) -> [u8; 32] {
    let value = U256::from_big_endian(&input);
    let rotated = (value << bits) | (value >> (256 - bits));
    let mut out = [0u8; 32];
    rotated.to_big_endian(&mut out);
    out
}

/// XOR the last byte of a 32-byte hash to match Solidity's `h ^ bytes32(uint256(value))`
/// which places the value at byte[31] (least-significant byte in big-endian uint256).
fn xor_round_constant(mut input: [u8; 32], value: u8) -> [u8; 32] {
    input[31] ^= value;
    input
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantum_resistant_hash_live_is_deterministic() {
        let signature = vec![0x11; 65];
        let entropy_hash = [0x22; 32];
        let secret_value = [0x33; 32];

        let first = quantum_resistant_hash_live(&signature, &entropy_hash, &secret_value);
        let second = quantum_resistant_hash_live(&signature, &entropy_hash, &secret_value);

        assert_eq!(first, second);
        assert_ne!(first, [0u8; 32]);
    }

    #[test]
    fn hex_string_formats_with_prefix() {
        assert_eq!(hex_string(&[0x12, 0xAB, 0x00]), "0x12ab00");
    }

    #[test]
    fn meets_difficulty_accepts_small_hash_under_large_target() {
        let hash = [0u8; 32];
        let difficulty = U256::from(1u64) << 255;

        assert!(meets_difficulty(&hash, difficulty));
    }
}
