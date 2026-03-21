use crate::chain::{LiveChallenge, LiveMiningClient, LiveSubmission};
use crate::config::MinerConfig;
use crate::cpu::get_cpu_temperature;
use crate::crypto::{
    build_commitment_payload, contract_hash_message, has_leading_zero_bits, miner_address_from_signing_key,
    pre_filter_nonce, random_secret, MiningMaterial,
};
use crate::memory::SecureBuffer;
use crate::pqc::PqcMode;
use crate::seed::{decode_temporal_seed_timestamp, generate_temporal_seed};
use crate::telemetry::{MinerState, MiningPhase, PhaseTracker, TelemetrySnapshot};
use crate::tg_output_filter::{MemoryBackend, Ready, SledBackend, TgOutputFilter};
use anyhow::{anyhow, Result};
use ethers::signers::LocalWallet;
use ethers::types::U256;
use k256::ecdsa::SigningKey;
use rand::rngs::OsRng;
use sha3::{Digest, Keccak256};
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{atomic::{AtomicBool, AtomicU64, Ordering}, Arc, Mutex as StdMutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;
use tokio::time;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use zeroize::{Zeroize, ZeroizeOnDrop};

type SharedOutputFilter = Arc<RwLock<TgOutputFilter<Ready>>>;

#[derive(Debug, Clone, Default)]
struct OutputFilterMetrics {
    output_count: u64,
    filter_fp_rate: Option<f64>,
    filter_memory_kb: Option<u64>,
    epoch_stats: std::collections::HashMap<u64, u64>,
}

#[derive(Debug, Clone, Zeroize, ZeroizeOnDrop)]
struct LiveWorkerCandidate {
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
struct HashrateWindow {
    samples: VecDeque<(Instant, u64)>,
    window: Duration,
}

impl Default for HashrateWindow {
    fn default() -> Self {
        Self {
            samples: VecDeque::new(),
            window: Duration::from_secs(10),
        }
    }
}

impl HashrateWindow {
    fn record(&mut self, now: Instant, hashes: u64) -> f64 {
        self.samples.push_back((now, hashes));
        self.prune(now);

        let Some((first_at, first_hashes)) = self.samples.front().copied() else {
            return 0.0;
        };

        let elapsed = now.saturating_duration_since(first_at).as_secs_f64();
        if elapsed <= f64::EPSILON {
            return 0.0;
        }

        let delta_hashes = hashes.saturating_sub(first_hashes);
        delta_hashes as f64 / elapsed
    }

    fn prune(&mut self, now: Instant) {
        while self.samples.len() > 1 {
            let Some((sample_at, _)) = self.samples.front().copied() else {
                break;
            };

            if now.saturating_duration_since(sample_at) <= self.window {
                break;
            }

            self.samples.pop_front();
        }
    }
}

fn secure_random_secret_array() -> Result<[u8; 32]> {
    let mut raw = random_secret();
    let secure = SecureBuffer::from_slice(&raw)
        .map_err(|err| anyhow!("Failed to protect mining secret in memory: {err}"))?;
    raw.zeroize();
    secure
        .to_array::<32>()
        .map_err(|err| anyhow!("Failed to extract mining secret from secure memory: {err}"))
}

#[derive(Debug, Clone)]
struct RuntimeStats {
    state: MinerState,
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
            solutions: 0,
            accepted_submissions: 0,
            rejected_submissions: 0,
            total_rewards_estimate: 0.0,
            last_solution_nonce: None,
            last_solution_hash_hex: None,
            last_commit_hash_hex: None,
            last_output_hash_hex: None,
            temperature_c: None,
        }
    }
}

pub struct MinerHandle {
    config: MinerConfig,
    started_at: Instant,
    stats: Arc<Mutex<RuntimeStats>>,
    output_filter: SharedOutputFilter,
    hash_counter: Arc<AtomicU64>,
    hashrate_window: Arc<StdMutex<HashrateWindow>>,
    telemetry_tx: broadcast::Sender<TelemetrySnapshot>,
    shutdown: CancellationToken,
    join_handle: JoinHandle<Result<()>>,
}

impl MinerHandle {
    pub fn subscribe(&self) -> broadcast::Receiver<TelemetrySnapshot> {
        self.telemetry_tx.subscribe()
    }

    pub async fn snapshot(&self) -> TelemetrySnapshot {
        snapshot_from_state(
            &self.config,
            self.started_at,
            &self.stats,
            &self.hash_counter,
            &self.hashrate_window,
            &self.output_filter,
        ).await
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
    let output_filter = build_output_filter(&config)?;
    let hash_counter = Arc::new(AtomicU64::new(0));
    let hashrate_window = Arc::new(StdMutex::new(HashrateWindow::default()));
    let started_at = Instant::now();
    let shutdown = CancellationToken::new();
    let task_config = config.clone();
    let task_stats = Arc::clone(&stats);
    let task_output_filter = Arc::clone(&output_filter);
    let task_hash_counter = Arc::clone(&hash_counter);
    let task_hashrate_window = Arc::clone(&hashrate_window);
    let task_tx = telemetry_tx.clone();
    let task_shutdown = shutdown.clone();

    let join_handle = tokio::spawn(async move {
        run_runtime(
            task_config,
            task_stats,
            task_output_filter,
            task_hash_counter,
            task_hashrate_window,
            started_at,
            task_tx,
            task_shutdown,
        ).await
    });

    Ok(MinerHandle {
        config,
        started_at,
        stats,
        output_filter,
        hash_counter,
        hashrate_window,
        telemetry_tx,
        shutdown,
        join_handle,
    })
}

async fn run_runtime(
    config: MinerConfig,
    stats: Arc<Mutex<RuntimeStats>>,
    output_filter: SharedOutputFilter,
    hash_counter: Arc<AtomicU64>,
    hashrate_window: Arc<StdMutex<HashrateWindow>>,
    started_at: Instant,
    telemetry_tx: broadcast::Sender<TelemetrySnapshot>,
    shutdown: CancellationToken,
) -> Result<()> {
    if let Some(live_client) = LiveMiningClient::connect(&config).await? {
        info!("Live chain submission mode enabled for contract {}", config.contract_address);
        return run_live_runtime(
            config,
            stats,
            output_filter,
            hash_counter,
            hashrate_window,
            started_at,
            telemetry_tx,
            shutdown,
            live_client,
        ).await;
    }

    run_simulated_runtime(
        config,
        stats,
        output_filter,
        hash_counter,
        hashrate_window,
        started_at,
        telemetry_tx,
        shutdown,
    ).await
}

async fn run_simulated_runtime(
    config: MinerConfig,
    stats: Arc<Mutex<RuntimeStats>>,
    output_filter: SharedOutputFilter,
    hash_counter: Arc<AtomicU64>,
    hashrate_window: Arc<StdMutex<HashrateWindow>>,
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
            Arc::clone(&output_filter),
            Arc::clone(&hash_counter),
            Arc::clone(&hashrate_window),
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
                let snapshot = snapshot_from_state(
                    &config,
                    started_at,
                    &stats,
                    &hash_counter,
                    &hashrate_window,
                    &output_filter,
                ).await;
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

    let final_snapshot = snapshot_from_state(
        &config,
        started_at,
        &stats,
        &hash_counter,
        &hashrate_window,
        &output_filter,
    ).await;
    let _ = telemetry_tx.send(final_snapshot);
    info!("Miner runtime stopped cleanly");
    Ok(())
}

async fn run_live_runtime(
    config: MinerConfig,
    stats: Arc<Mutex<RuntimeStats>>,
    output_filter: SharedOutputFilter,
    hash_counter: Arc<AtomicU64>,
    hashrate_window: Arc<StdMutex<HashrateWindow>>,
    started_at: Instant,
    telemetry_tx: broadcast::Sender<TelemetrySnapshot>,
    shutdown: CancellationToken,
    live_client: LiveMiningClient,
) -> Result<()> {
    {
        let mut guard = stats.lock().await;
        guard.state = MinerState::Running;
    }

    let phase_tracker = PhaseTracker::with_block_time_millis(config.block_time_millis);

    // Background telemetry ticker — emits snapshots every second so the
    // display stays updated even while submit_solution is blocking.
    {
        let bg_stats = Arc::clone(&stats);
        let bg_filter = Arc::clone(&output_filter);
        let bg_hashes = Arc::clone(&hash_counter);
        let bg_hashrate_window = Arc::clone(&hashrate_window);
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
                            &bg_config,
                            started_at,
                            &bg_stats,
                            &bg_hashes,
                            &bg_hashrate_window,
                            &bg_filter,
                            &bg_phase,
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
    let _ = try_resume_pending_commitment(
        &live_client,
        &config,
        &stats,
        &output_filter,
        &phase_tracker,
        &shutdown,
    )
    .await?;

    while !shutdown.is_cancelled() {
        // ── Pre-check: wait out any stale on-chain commitment before searching ──
        match live_client.has_pending_commitment().await {
            Ok(true) => {
                tracing::info!("Stale on-chain commitment detected — waiting for clearance before searching");
                if let Err(err) = live_client.wait_for_commitment_clearance_public(&phase_tracker).await {
                    tracing::warn!("Clearance wait failed: {err:#}");
                    time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
                tracing::info!("Commitment cleared — resuming mining");
            }
            Ok(false) => {} // no pending commitment, proceed normally
            Err(err) => {
                tracing::warn!("Failed to check commitment status: {err:#}");
                // Don't block — proceed and let submit_solution handle it
            }
        }

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
            &hash_counter,
            &output_filter,
            &telemetry_tx,
            &shutdown,
            &challenge,
            pqc_mode,
            &hashrate_window,
            &mut nonce_cursor,
            started_at,
        ).await? {
            phase_tracker.set(MiningPhase::SolutionFound, None);

            match live_client.submit_solution(&submission, &phase_tracker, &config.private_key_path).await {
                Ok(receipt) => {
                    let reward = reward_from_receipt_or_estimate(&receipt, config.difficulty_zero_bits);
                    let output_hash = LiveMiningClient::extract_output_hash_from_receipt(&receipt);
                    if let Some(output_hash_hex) = output_hash.as_deref() {
                        if let Some(output_hash_bytes) = parse_hex_bytes32(output_hash_hex) {
                            let wallet_addr = hex_string(live_client.miner_address().as_bytes());
                            if let Err(err) = record_output_solution(
                                &output_filter,
                                output_hash_bytes,
                                submission.nonce,
                                &wallet_addr,
                            ) {
                                warn!("Failed to record live output in filter: {err:#}");
                            }
                        } else {
                            warn!("Could not parse live output hash {output_hash_hex} for filter recording");
                        }
                    }
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
                    {
                        let mut guard = stats.lock().await;
                        guard.rejected_submissions = guard.rejected_submissions.saturating_add(1);
                    }
                    warn!("Live submission failed: {err:#}");

                    if let Err(recovery_err) = try_resume_pending_commitment(
                        &live_client,
                        &config,
                        &stats,
                        &output_filter,
                        &phase_tracker,
                        &shutdown,
                    )
                    .await
                    {
                        warn!("Pending reveal recovery failed after live submission error: {recovery_err:#}");
                    }
                }
            }
        }

        tokio::task::yield_now().await;
    }

    {
        let mut guard = stats.lock().await;
        guard.state = MinerState::Stopped;
    }

    let final_snapshot = snapshot_from_state(
        &config,
        started_at,
        &stats,
        &hash_counter,
        &hashrate_window,
        &output_filter,
    ).await;
    let _ = telemetry_tx.send(final_snapshot);
    info!("Live miner runtime stopped cleanly");
    Ok(())
}

async fn try_resume_pending_commitment(
    live_client: &LiveMiningClient,
    config: &MinerConfig,
    stats: &Arc<Mutex<RuntimeStats>>,
    output_filter: &SharedOutputFilter,
    phase_tracker: &PhaseTracker,
    shutdown: &CancellationToken,
) -> Result<bool> {
    let Some(pending) = crate::pending::load(&config.private_key_path)? else {
        return Ok(false);
    };

    info!(
        "Found saved commitment from block {} — attempting reveal",
        pending.commit_block
    );

    match live_client
        .reveal_pending(&pending, phase_tracker, &config.private_key_path)
        .await
    {
        Ok(Some(receipt)) => {
            let reward = reward_from_receipt_or_estimate(&receipt, config.difficulty_zero_bits);
            let output_hash_hex = LiveMiningClient::extract_output_hash_from_receipt(&receipt);
            phase_tracker.set(MiningPhase::RewardReceived, None);

            {
                let mut guard = stats.lock().await;
                guard.solutions = guard.solutions.saturating_add(1);
                guard.accepted_submissions = guard.accepted_submissions.saturating_add(1);
                guard.total_rewards_estimate += reward;
                guard.last_solution_nonce = Some(pending.nonce);
                guard.last_solution_hash_hex = Some(hex_string(&pending.commit_hash));
                guard.last_commit_hash_hex = Some(hex_string(&pending.commit_hash));
                guard.last_output_hash_hex = output_hash_hex.clone();
            }

            if let Some(output_hash_hex) = output_hash_hex {
                if let Some(output_hash) = parse_hex_bytes32(&output_hash_hex) {
                    let wallet_addr = hex_string(live_client.miner_address().as_bytes());
                    if let Err(err) = record_output_solution(output_filter, output_hash, pending.nonce, &wallet_addr) {
                        warn!("Failed to record resumed output in filter: {err:#}");
                    }
                } else {
                    warn!("Could not parse resumed output hash {output_hash_hex} for filter recording");
                }
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

            Ok(true)
        }
        Ok(None) => {
            info!("Saved commitment expired or already revealed, starting fresh");
            Ok(false)
        }
        Err(err) => {
            warn!("Failed to reveal saved commitment: {err:#}");
            Ok(false)
        }
    }
}

async fn attempt_live_solution(
    live_client: &LiveMiningClient,
    config: &MinerConfig,
    stats: &Arc<Mutex<RuntimeStats>>,
    hash_counter: &Arc<AtomicU64>,
    output_filter: &SharedOutputFilter,
    telemetry_tx: &broadcast::Sender<TelemetrySnapshot>,
    shutdown: &CancellationToken,
    challenge: &LiveChallenge,
    _pqc_mode: crate::pqc::PqcMode,
    hashrate_window: &Arc<StdMutex<HashrateWindow>>,
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
        let output_filter = Arc::clone(output_filter);
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
                output_filter,
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
        hash_counter.fetch_add(hashes_checked, Ordering::Relaxed);
        guard.temperature_c = get_cpu_temperature();
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
        guard.temperature_c = get_cpu_temperature().or(guard.temperature_c);
        let metrics = output_filter_metrics(output_filter);
        snapshot_from_guard(
            config,
            started_at,
            &guard,
            hash_counter.load(Ordering::Relaxed),
            hashrate_window,
            &metrics,
        )
    };
    let _ = telemetry_tx.send(preview);

    Ok(Some(LiveSubmission {
        commitment,
        previous_output: challenge.previous_output,
        temporal_seed: candidate.temporal_seed,
        nonce: candidate.nonce,
        reveal_signature: candidate.reveal_signature.clone(),
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
    output_filter: SharedOutputFilter,
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
    let secret_value = secure_random_secret_array()?;

    let mut hashes_checked = 0u64;
    for batch_index in 0..batch_size {
        if shutdown.is_cancelled() || solution_found.load(Ordering::Relaxed) {
            break;
        }

        let current_nonce = base_nonce
            .saturating_add(worker_id as u64)
            .saturating_add((batch_index as u64) * step);
        hashes_checked = hashes_checked.saturating_add(1);
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

        {
            let filter = output_filter
                .read()
                .map_err(|_| anyhow!("output filter read lock poisoned"))?;
            if !filter.is_candidate(&solution_hash) {
                continue;
            }
        }

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
    output_filter: SharedOutputFilter,
    hash_counter: Arc<AtomicU64>,
    hashrate_window: Arc<StdMutex<HashrateWindow>>,
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
    let mut pending_hashes = 0u64;
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
            pending_hashes = pending_hashes.saturating_add(1);

            if pending_hashes >= 256 {
                hash_counter.fetch_add(pending_hashes, Ordering::Relaxed);
                pending_hashes = 0;
            }

            if !pre_filter_nonce(current_nonce, &pre_input, target_divisor) {
                continue;
            }

            let secret_value = secure_random_secret_array()?;
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

            {
                let filter = output_filter
                    .read()
                    .map_err(|_| anyhow!("output filter read lock poisoned"))?;
                if !filter.is_candidate(&payload.solution_hash) {
                    continue;
                }
            }

            if has_leading_zero_bits(&payload.solution_hash, config.difficulty_zero_bits) {
                if pending_hashes > 0 {
                    hash_counter.fetch_add(pending_hashes, Ordering::Relaxed);
                    pending_hashes = 0;
                }
                let wallet_addr = hex_string(&miner_address);
                if let Err(err) = record_output_solution(
                    &output_filter,
                    payload.solution_hash,
                    current_nonce,
                    &wallet_addr,
                ) {
                    warn!("worker {worker_id} failed to record output in filter: {err:#}");
                }
                let snapshot = {
                    let mut guard = stats.lock().await;
                    guard.solutions = guard.solutions.saturating_add(1);
                    guard.accepted_submissions = guard.accepted_submissions.saturating_add(1);
                    guard.total_rewards_estimate += estimate_reward(config.difficulty_zero_bits);
                    guard.last_solution_nonce = Some(current_nonce);
                    guard.last_solution_hash_hex = Some(hex_string(&payload.solution_hash));
                    guard.last_output_hash_hex = Some(hex_string(&payload.solution_hash));
                    guard.temperature_c = get_cpu_temperature().or(guard.temperature_c);
                    let metrics = output_filter_metrics(&output_filter);
                    snapshot_from_guard(
                        &config,
                        started_at,
                        &guard,
                        hash_counter.load(Ordering::Relaxed),
                        &hashrate_window,
                        &metrics,
                    )
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

    if pending_hashes > 0 {
        hash_counter.fetch_add(pending_hashes, Ordering::Relaxed);
    }

    Ok(())
}

async fn snapshot_from_state(
    config: &MinerConfig,
    started_at: Instant,
    stats: &Arc<Mutex<RuntimeStats>>,
    hash_counter: &Arc<AtomicU64>,
    hashrate_window: &Arc<StdMutex<HashrateWindow>>,
    output_filter: &SharedOutputFilter,
) -> TelemetrySnapshot {
    let guard = stats.lock().await.clone();
    let metrics = output_filter_metrics(output_filter);
    snapshot_from_guard(
        config,
        started_at,
        &guard,
        hash_counter.load(Ordering::Relaxed),
        hashrate_window,
        &metrics,
    )
}

fn snapshot_from_guard(
    config: &MinerConfig,
    started_at: Instant,
    guard: &RuntimeStats,
    hashes: u64,
    hashrate_window: &Arc<StdMutex<HashrateWindow>>,
    filter_metrics: &OutputFilterMetrics,
) -> TelemetrySnapshot {
    let now = Instant::now();
    let uptime = started_at.elapsed().as_secs().max(1);
    let hashrate_hs = hashrate_window
        .lock()
        .map(|mut window| window.record(now, hashes))
        .unwrap_or_else(|_| hashes as f64 / uptime as f64);
    TelemetrySnapshot {
        timestamp_unix_ms: unix_ms() as u128,
        state: guard.state,
        uptime_seconds: uptime,
        worker_count: config.threads,
        hashes,
        hashrate_hs,
        solutions: guard.solutions,
        accepted_submissions: guard.accepted_submissions,
        rejected_submissions: guard.rejected_submissions,
        total_rewards_estimate: guard.total_rewards_estimate,
        output_count: filter_metrics.output_count,
        last_solution_nonce: guard.last_solution_nonce,
        last_solution_hash_hex: guard.last_solution_hash_hex.clone(),
        last_commit_hash_hex: guard.last_commit_hash_hex.clone(),
        last_output_hash_hex: guard.last_output_hash_hex.clone(),
        filter_fp_rate: filter_metrics.filter_fp_rate,
        filter_memory_kb: filter_metrics.filter_memory_kb,
        epoch_stats: filter_metrics.epoch_stats.clone(),
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
    hash_counter: &Arc<AtomicU64>,
    hashrate_window: &Arc<StdMutex<HashrateWindow>>,
    output_filter: &SharedOutputFilter,
    phase_tracker: &PhaseTracker,
) -> TelemetrySnapshot {
    let ps = phase_tracker.get();
    let guard = stats.lock().await.clone();
    let metrics = output_filter_metrics(output_filter);
    let mut snapshot = snapshot_from_guard(
        config,
        started_at,
        &guard,
        hash_counter.load(Ordering::Relaxed),
        hashrate_window,
        &metrics,
    );
    snapshot.mining_phase = ps.phase;
    snapshot.phase_blocks_remaining = ps.blocks_remaining;
    snapshot.phase_eta_seconds = ps.eta_seconds;
    snapshot
}

fn build_output_filter(config: &MinerConfig) -> Result<SharedOutputFilter> {
    let filter = match build_persistent_output_filter(config) {
        Ok(filter) => filter,
        Err(err) => {
            warn!("Falling back to in-memory output filter: {err:#}");
            TgOutputFilter::new()
                .with_storage(Arc::new(MemoryBackend::default()))
                .build()
                .map_err(|filter_err| anyhow!("Failed to build in-memory output filter: {filter_err}"))?
        }
    };

    Ok(Arc::new(RwLock::new(filter)))
}

fn build_persistent_output_filter(config: &MinerConfig) -> Result<TgOutputFilter<Ready>> {
    let db_path = output_filter_path(&config.private_key_path);
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let backend = Arc::new(
        SledBackend::new(&db_path)
            .map_err(|err| anyhow!("Failed to open output filter db at {}: {err}", db_path.display()))?,
    );

    let mut filter = TgOutputFilter::new()
        .with_storage(backend)
        .build()
        .map_err(|err| anyhow!("Failed to build persistent output filter: {err}"))?;

    let removed = filter.garbage_collect();
    if removed > 0 {
        info!("Output filter garbage-collected {removed} expired records");
    }
    filter.log_summary();
    Ok(filter)
}

fn output_filter_path(key_path: &str) -> PathBuf {
    let path = Path::new(key_path);
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let parent = path.parent().unwrap_or(Path::new("."));
    parent.join(format!("{stem}.output-filter.db"))
}

fn output_filter_metrics(output_filter: &SharedOutputFilter) -> OutputFilterMetrics {
    let filter = match output_filter.read() {
        Ok(filter) => filter,
        Err(_) => {
            warn!("Output filter read lock poisoned while building telemetry snapshot");
            return OutputFilterMetrics::default();
        }
    };

    OutputFilterMetrics {
        output_count: filter.output_count(),
        filter_fp_rate: Some(filter.false_positive_rate()),
        filter_memory_kb: Some((filter.memory_bytes() / 1024) as u64),
        epoch_stats: filter.epoch_stats(),
    }
}

fn record_output_solution(
    output_filter: &SharedOutputFilter,
    output_hash: [u8; 32],
    nonce: u64,
    wallet_addr: &str,
) -> Result<bool> {
    let mut filter = output_filter
        .write()
        .map_err(|_| anyhow!("output filter write lock poisoned"))?;
    filter
        .record_solution(output_hash, nonce, wallet_addr)
        .map_err(|err| anyhow!("failed to record output in filter: {err}"))
}

fn parse_hex_bytes32(value: &str) -> Option<[u8; 32]> {
    let trimmed = value.strip_prefix("0x").unwrap_or(value);
    let bytes = hex::decode(trimmed).ok()?;
    bytes.try_into().ok()
}

fn reward_from_receipt_or_estimate(receipt: &ethers::types::TransactionReceipt, difficulty_zero_bits: u8) -> f64 {
    match LiveMiningClient::extract_reward_from_receipt(receipt) {
        Some(on_chain) if on_chain > 0.0 => on_chain,
        _ => {
            let estimated = estimate_reward(difficulty_zero_bits);
            warn!(
                tx_hash = ?receipt.transaction_hash,
                estimated_reward = estimated,
                "Receipt did not expose an on-chain reward event; using estimated reward fallback"
            );
            estimated
        }
    }
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
