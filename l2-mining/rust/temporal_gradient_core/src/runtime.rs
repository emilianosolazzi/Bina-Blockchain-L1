use crate::chain::{LiveChallenge, LiveMiningClient, LiveSubmission};
use crate::config::MinerConfig;
use crate::cpu::get_cpu_temperature;
use crate::crypto::{
    build_commitment_payload, contract_hash_message, has_leading_zero_bits, miner_address_from_signing_key,
    pre_filter_nonce, random_secret, MiningMaterial,
};
use crate::memory::SecureBuffer;
use crate::seed::{decode_temporal_seed_timestamp, generate_temporal_seed};
use crate::telemetry::{MinerState, MiningPhase, PhaseTracker, TelemetrySnapshot};
use crate::tg_output_filter::{MemoryBackend, Ready, SledBackend, TgOutputFilter};
use anyhow::{anyhow, Result};
use ethers::signers::LocalWallet;
use ethers::types::U256;
use k256::ecdsa::SigningKey;
use rand::rngs::OsRng;
#[cfg(feature = "stale-mining")]
use serde_json::Value;
#[cfg(feature = "stale-mining")]
use futures_util::{SinkExt, StreamExt};
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
    let live_client = LiveMiningClient::connect(&config).await?;

    // ── Optional stale-block mining sidecar ──────────────────────
    #[cfg(feature = "stale-mining")]
    let _stale_handle: Option<JoinHandle<()>> = if config.stale_mining_enabled() {
        let sb_cfg = config.stale_block.as_ref().unwrap().to_miner_config();
        let sb_shutdown = shutdown.clone();
        info!(
            "Stale-block mining enabled — WebSocket stream primary, {}s fallback poll | {}",
            sb_cfg.poll_interval_secs, sb_cfg.bitcoin_api_url
        );
        Some(tokio::spawn(run_stale_block_loop(sb_cfg, live_client.clone(), sb_shutdown)))
    } else {
        debug!("Stale-block mining disabled (not configured or enabled: false)");
        None
    };

    if let Some(live_client) = live_client {
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
        // ── Pre-check: read on-chain commitment state and handle intelligently ──
        match live_client.get_onchain_commitment().await {
            Ok(Some(commitment)) if !commitment.expired => {
                let same_pool = commitment.pool_id == config.pool_id;

                if same_pool {
                    // Commitment is for OUR pool — try to reveal with saved pending data
                    match try_resume_pending_commitment(
                        &live_client,
                        &config,
                        &stats,
                        &output_filter,
                        &phase_tracker,
                        &shutdown,
                    ).await {
                        Ok(true) => {
                            tracing::info!("Successfully revealed pending commitment — resuming mining");
                            continue;
                        }
                        Ok(false) => {
                            tracing::info!("No saved reveal data for pool {} commitment — waiting for expiry", commitment.pool_id);
                        }
                        Err(err) => {
                            tracing::warn!("Pending reveal attempt failed: {err:#} — waiting for expiry");
                        }
                    }
                } else {
                    // Commitment is for a DIFFERENT pool — no point trying to reveal,
                    // just wait for it to expire cleanly
                    tracing::warn!(
                        "On-chain commitment is for POOL {} but miner is configured for POOL {} — \
                         cannot reveal, waiting for expiry at block {}",
                        commitment.pool_id, config.pool_id, commitment.expires_at
                    );
                }

                // Wait for the stale commitment to expire
                if let Err(err) = live_client.wait_for_commitment_clearance_public(&phase_tracker).await {
                    tracing::warn!("Clearance wait failed: {err:#}");
                    time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
                tracing::info!("Commitment cleared — resuming mining");
            }
            Ok(Some(commitment)) if commitment.expired => {
                // Already expired — log and proceed immediately, no waiting needed
                tracing::info!(
                    "Stale on-chain commitment (pool {}, block {}) already expired — mining immediately",
                    commitment.pool_id, commitment.commit_block
                );
            }
            Ok(Some(_)) => {} // unreachable but satisfy match
            Ok(None) => {} // no commitment at all, proceed normally
            Err(err) => {
                tracing::warn!("Failed to check commitment status: {err:#}");
            }
        }

        // ── Pre-flight: verify pool has remaining emission before spending CPU ──
        match live_client.check_pool_emission().await {
            Ok((remaining, _mined, active)) => {
                if !active {
                    tracing::error!("Pool {} is not active! Stopping miner.", config.pool_id);
                    shutdown.cancel();
                    continue;
                }
                if remaining <= 0.0 {
                    tracing::error!(
                        "Pool {} has ZERO remaining emission — cannot mine. \
                         Create a new pool or switch pool_id in config.",
                        config.pool_id
                    );
                    shutdown.cancel();
                    continue;
                }
                tracing::info!("Pool {} has {remaining:.2} TGBT remaining", config.pool_id);
            }
            Err(err) => {
                tracing::warn!("Could not verify pool emission: {err:#} — proceeding anyway");
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
                    let err_msg = format!("{err:#}");
                    // Only count as "rejected" if the TX actually reached the chain.
                    // Pre-chain failures (gas estimation, race detection) are retryable.
                    let is_prechain = err_msg.contains("estimate commitment gas")
                        || err_msg.contains("Failed to send commitment")
                        || err_msg.contains("race detected")
                        || err_msg.contains("No commitment receipt");
                    if !is_prechain {
                        let mut guard = stats.lock().await;
                        guard.rejected_submissions = guard.rejected_submissions.saturating_add(1);
                    }
                    warn!("Live submission failed{}: {err:#}", if is_prechain { " (pre-chain, not counted as rejection)" } else { "" });

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

        // ── Throttle: sleep between mining cycles to stay under RPC rate limits ──
        let delay = config.cycle_delay_secs;
        if delay > 0 && !shutdown.is_cancelled() {
            tracing::debug!("Cycle delay: sleeping {delay}s before next mining cycle");
            tokio::time::sleep(Duration::from_secs(delay)).await;
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

// ────────────────────────────────────────────────────────────────────
// Optional stale-block mining loop (only compiled with `stale-mining`)
//
// PRIMARY MODE: WebSocket stream from NativeBTC API.
//   - Subscribes to `subscribe:blocks` and `subscribe:stats`.
//   - Receives real-time new-block and mempool stats push notifications.
//   - Zero-latency orphan detection — no 30s polling delay.
//
// FALLBACK MODE: HTTP polling at `poll_interval_secs` if WS drops.
//   - Uses lightweight `/v1/block-height` for tip height.
//   - Falls back to `/v1/blocks` for full block list when height changes.
//
// ENRICHMENT: On every new block event, fetches `/v1/mempool/fees`
//   and `/v1/mempool/stats` to attach congestion data to proofs.
// ────────────────────────────────────────────────────────────────────
#[cfg(feature = "stale-mining")]
async fn run_stale_block_loop(
    config: crate::stale_block_miner::StaleBlockMinerConfig,
    live_client: Option<LiveMiningClient>,
    shutdown: CancellationToken,
) {
    use crate::stale_block_miner::{StaleBlockMiner, MempoolStats};

    let mut miner = StaleBlockMiner::new(config.clone());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("TGBT-StaleBlockMiner/0.2")
        .build()
        .expect("failed to build HTTP client for stale mining");

    if config.auto_submit && live_client.is_none() {
        warn!("Stale-block auto-submit is enabled, but no live mining client is available; proofs will be queued only");
    }

    let mut latest_mempool = MempoolStats::default();

    info!("Stale-block mining loop started (WebSocket primary)");

    loop {
        // ── Try WebSocket stream first ──────────────────────────────
        let ws_url = build_ws_url(&config);
        info!("Stale-block: connecting WebSocket stream → {}", redact_key(&ws_url));

        match connect_ws_stream(&ws_url, &shutdown).await {
            Ok(ws_stream) => {
                info!("Stale-block: WebSocket connected — subscribing to blocks + stats");
                run_ws_event_loop(
                    ws_stream,
                    &client,
                    &config,
                    &mut miner,
                    &live_client,
                    &mut latest_mempool,
                    &shutdown,
                ).await;
                // If we get here, the WS dropped. Fall through to reconnect.
                if shutdown.is_cancelled() {
                    break;
                }
                warn!("Stale-block: WebSocket disconnected — will reconnect in 5s");
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                }
            }
            Err(err) => {
                warn!("Stale-block: WebSocket connection failed: {err:#} — falling back to HTTP polling");
                run_http_fallback_loop(
                    &client,
                    &config,
                    &mut miner,
                    &live_client,
                    &mut latest_mempool,
                    &shutdown,
                ).await;
                if shutdown.is_cancelled() {
                    break;
                }
                // After poll loop exits (e.g. repeated failures), retry WS
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                }
            }
        }
    }

    // Drain pending proofs on shutdown
    let pending = miner.drain_pending_proofs();
    if !pending.is_empty() {
        info!("Stale-block mining: {} pending proof(s) drained on shutdown", pending.len());
    }
}

/// Build the WebSocket URL with API key.
#[cfg(feature = "stale-mining")]
fn build_ws_url(config: &crate::stale_block_miner::StaleBlockMinerConfig) -> String {
    let base = config.bitcoin_api_url.trim_end_matches('/');
    // Convert https:// → wss://, http:// → ws://
    let ws_base = if base.starts_with("https://") {
        base.replacen("https://", "wss://", 1)
    } else if base.starts_with("http://") {
        base.replacen("http://", "ws://", 1)
    } else {
        format!("wss://{}", base.trim_start_matches("wss://").trim_start_matches("ws://"))
    };

    match &config.api_key {
        Some(key) if !key.is_empty() => format!("{ws_base}/v1/mempool/stream?key={key}"),
        _ => format!("{ws_base}/v1/mempool/stream"),
    }
}

/// Redact API key in log output.
#[cfg(feature = "stale-mining")]
fn redact_key(url: &str) -> String {
    if let Some(idx) = url.find("key=") {
        let prefix = &url[..idx + 4];
        format!("{prefix}[REDACTED]")
    } else {
        url.to_string()
    }
}

/// Build an authenticated REST URL for NativeBTC API.
#[cfg(feature = "stale-mining")]
fn api_url(config: &crate::stale_block_miner::StaleBlockMinerConfig, path: &str) -> String {
    let base = config.bitcoin_api_url.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    match &config.api_key {
        Some(key) if !key.is_empty() => format!("{base}/{path}?key={key}"),
        _ => format!("{base}/{path}"),
    }
}

/// Connect to the NativeBTC WebSocket stream.
#[cfg(feature = "stale-mining")]
async fn connect_ws_stream(
    url: &str,
    shutdown: &CancellationToken,
) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>> {
    use tokio_tungstenite::connect_async;

    let connect_fut = connect_async(url);
    tokio::select! {
        _ = shutdown.cancelled() => Err(anyhow!("shutdown during WS connect")),
        result = connect_fut => {
            let (stream, _response) = result.map_err(|e| anyhow!("WS connect error: {e}"))?;
            Ok(stream)
        }
    }
}

/// Main WebSocket event loop.  Subscribes to blocks + stats, processes push
/// messages in real time, and triggers stale-block harvesting on new blocks.
#[cfg(feature = "stale-mining")]
async fn run_ws_event_loop(
    ws_stream: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    client: &reqwest::Client,
    config: &crate::stale_block_miner::StaleBlockMinerConfig,
    miner: &mut crate::stale_block_miner::StaleBlockMiner,
    live_client: &Option<LiveMiningClient>,
    latest_mempool: &mut crate::stale_block_miner::MempoolStats,
    shutdown: &CancellationToken,
) {
    use tokio_tungstenite::tungstenite::Message;

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Subscribe to block events and mempool stats
    let subscribe_cmds = ["subscribe:blocks", "subscribe:stats"];
    for cmd in subscribe_cmds {
        if let Err(err) = ws_tx.send(Message::Text(cmd.to_string())).await {
            warn!("Stale-block WS: failed to send '{cmd}': {err}");
            return;
        }
    }

    // Also set up a keepalive ping every 30s to prevent idle disconnects
    let mut keepalive = time::interval(Duration::from_secs(30));
    keepalive.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

    let mut consecutive_errors = 0u32;

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                info!("Stale-block WS: shutting down");
                let _ = ws_tx.send(Message::Close(None)).await;
                break;
            }
            _ = keepalive.tick() => {
                if let Err(err) = ws_tx.send(Message::Ping(vec![0x54, 0x47])).await {
                    warn!("Stale-block WS: keepalive ping failed: {err}");
                    break; // Let reconnect logic handle it
                }
            }
            msg = ws_rx.next() => {
                match msg {
                    None => {
                        info!("Stale-block WS: stream ended (server closed)");
                        break;
                    }
                    Some(Err(err)) => {
                        warn!("Stale-block WS: read error: {err}");
                        consecutive_errors += 1;
                        if consecutive_errors > 5 {
                            warn!("Stale-block WS: too many consecutive errors, disconnecting");
                            break;
                        }
                        continue;
                    }
                    Some(Ok(Message::Text(text))) => {
                        consecutive_errors = 0;
                        handle_ws_message(
                            &text, client, config, miner, live_client,
                            latest_mempool,
                        ).await;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = ws_tx.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) => {
                        info!("Stale-block WS: received close frame");
                        break;
                    }
                    Some(Ok(_)) => {} // Binary frames, pongs, etc.
                }
            }
        }
    }
}

/// Handle a single WebSocket text message from the NativeBTC stream.
#[cfg(feature = "stale-mining")]
async fn handle_ws_message(
    text: &str,
    client: &reqwest::Client,
    config: &crate::stale_block_miner::StaleBlockMinerConfig,
    miner: &mut crate::stale_block_miner::StaleBlockMiner,
    live_client: &Option<LiveMiningClient>,
    latest_mempool: &mut crate::stale_block_miner::MempoolStats,
) {
    let msg: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(err) => {
            debug!("Stale-block WS: ignoring unparseable message: {err}");
            return;
        }
    };

    let msg_type = msg.get("type").and_then(Value::as_str).unwrap_or("");

    match msg_type {
        // ── New block event ─────────────────────────────────────────
        "new_block" | "block" => {
            info!("Stale-block WS: new block event received — harvesting orphans");
            // Enrich with fee/congestion data
            if let Err(err) = refresh_mempool_stats(client, config, latest_mempool).await {
                debug!("Stale-block: fee enrichment failed: {err:#}");
            }
            // Harvest stale proofs from the blocks endpoint
            match harvest_mempool_stale_proofs(client, config, miner).await {
                Ok(new_count) => {
                    if new_count > 0 {
                        info!(
                            "Stale-block WS: harvested {new_count} proof(s) | mempool={} txs, fastest_fee={} sat/vB, congestion={:.0}%",
                            latest_mempool.mempool_size,
                            latest_mempool.fastest_fee,
                            latest_mempool.congestion_ratio() * 100.0,
                        );
                    }
                }
                Err(err) => warn!("Stale-block WS: harvest failed: {err:#}"),
            }
            submit_pending_proofs(miner, live_client, config).await;
        }

        // ── Mempool stats push ──────────────────────────────────────
        "stats" => {
            if let Some(data) = msg.get("data") {
                parse_ws_mempool_stats(data, latest_mempool);
                debug!(
                    "Stale-block WS: mempool stats update — {} txs, fastest={} sat/vB",
                    latest_mempool.mempool_size, latest_mempool.fastest_fee,
                );
            }
        }

        // ── New transactions (useful for future work) ───────────────
        "new_txs" => {
            let count = msg.get("count").and_then(Value::as_u64).unwrap_or(0);
            debug!("Stale-block WS: {count} new mempool txs (observed)");
        }

        _ => {
            debug!("Stale-block WS: unhandled message type '{msg_type}'");
        }
    }
}

/// Parse mempool stats from WS push `data` field.
///
/// NativeBTC actual WS stats push shape:
/// ```json
/// { "success": true,
///   "mempool": { "size": 1996, "bytes": 756376, ... },
///   "feeEstimates": { "1_block": 2, "3_block": 2, "6_block": 2, ... },
///   "congestion": "low" }
/// ```
#[cfg(feature = "stale-mining")]
fn parse_ws_mempool_stats(data: &Value, stats: &mut crate::stale_block_miner::MempoolStats) {
    // Mempool size/bytes are nested under `.mempool`
    if let Some(mp) = data.get("mempool") {
        stats.mempool_size = mp.get("size").and_then(Value::as_u64).unwrap_or(stats.mempool_size);
        stats.mempool_vbytes = mp.get("bytes").and_then(Value::as_u64)
            .or_else(|| mp.get("usage").and_then(Value::as_u64))
            .unwrap_or(stats.mempool_vbytes);
    }
    // Fee estimates use `N_block` keys under `.feeEstimates`
    if let Some(fee_est) = data.get("feeEstimates") {
        stats.fastest_fee = fee_est.get("1_block").and_then(Value::as_u64).unwrap_or(stats.fastest_fee);
        stats.half_hour_fee = fee_est.get("3_block").and_then(Value::as_u64).unwrap_or(stats.half_hour_fee);
        stats.hour_fee = fee_est.get("6_block").and_then(Value::as_u64).unwrap_or(stats.hour_fee);
        stats.economy_fee = fee_est.get("25_block").and_then(Value::as_u64).unwrap_or(stats.economy_fee);
        stats.minimum_fee = fee_est.get("144_block").and_then(Value::as_u64).unwrap_or(stats.minimum_fee);
    }
    stats.captured_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
}

/// Fetch mempool fee estimates + stats from REST endpoints.
///
/// NativeBTC actual REST response shapes:
///
/// `/v1/mempool/fees`:
/// ```json
/// { "success": true, "recommended": { "satPerVb": 2 },
///   "estimates": { "1": { "satPerVb": 2 }, "6": { "satPerVb": 2 }, ... } }
/// ```
///
/// `/v1/mempool/stats`:
/// ```json
/// { "success": true,
///   "mempool": { "size": 1979, "bytes": 753635, ... },
///   "feeEstimates": { "1_block": 2, "3_block": 2, ... } }
/// ```
#[cfg(feature = "stale-mining")]
async fn refresh_mempool_stats(
    client: &reqwest::Client,
    config: &crate::stale_block_miner::StaleBlockMinerConfig,
    stats: &mut crate::stale_block_miner::MempoolStats,
) -> Result<()> {
    // Fetch fees and stats in parallel
    let fees_url = api_url(config, "v1/mempool/fees");
    let stats_url = api_url(config, "v1/mempool/stats");

    let (fees_res, stats_res) = tokio::join!(
        client.get(&fees_url).send(),
        client.get(&stats_url).send(),
    );

    // Parse fees: NativeBTC nests under `.estimates.{N}.satPerVb` and `.recommended.satPerVb`
    if let Ok(resp) = fees_res {
        if let Ok(fees) = resp.json::<Value>().await {
            // Best single-number: recommended.satPerVb
            if let Some(rec) = fees.get("recommended") {
                stats.fastest_fee = rec.get("satPerVb").and_then(Value::as_u64).unwrap_or(stats.fastest_fee);
            }
            // Per-block estimates
            if let Some(est) = fees.get("estimates") {
                let get_est = |key: &str| -> Option<u64> {
                    est.get(key).and_then(|e| e.get("satPerVb")).and_then(Value::as_u64)
                };
                if let Some(v) = get_est("1")   { stats.fastest_fee = v; }
                if let Some(v) = get_est("3")   { stats.half_hour_fee = v; }
                if let Some(v) = get_est("6")   { stats.hour_fee = v; }
                if let Some(v) = get_est("25")  { stats.economy_fee = v; }
                if let Some(v) = get_est("144") { stats.minimum_fee = v; }
            }
        }
    }

    // Parse mempool stats: NativeBTC nests under `.mempool.size`, `.mempool.bytes`
    if let Ok(resp) = stats_res {
        if let Ok(body) = resp.json::<Value>().await {
            if let Some(mp) = body.get("mempool") {
                stats.mempool_size = mp.get("size").and_then(Value::as_u64)
                    .unwrap_or(stats.mempool_size);
                stats.mempool_vbytes = mp.get("bytes").and_then(Value::as_u64)
                    .or_else(|| mp.get("usage").and_then(Value::as_u64))
                    .unwrap_or(stats.mempool_vbytes);
            }
            // Also grab feeEstimates from stats endpoint as secondary source
            if let Some(fee_est) = body.get("feeEstimates") {
                if stats.fastest_fee == 0 {
                    stats.fastest_fee = fee_est.get("1_block").and_then(Value::as_u64).unwrap_or(0);
                }
                if stats.economy_fee == 0 {
                    stats.economy_fee = fee_est.get("25_block").and_then(Value::as_u64).unwrap_or(0);
                }
            }
        }
    }

    stats.captured_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    Ok(())
}

/// Lightweight block-height check via `/v1/block-height`.
///
/// NativeBTC returns JSON: `{"success":true,"height":941815,"hex":"0xe5ef7"}`
#[cfg(feature = "stale-mining")]
async fn fetch_block_height(
    client: &reqwest::Client,
    config: &crate::stale_block_miner::StaleBlockMinerConfig,
) -> Result<u64> {
    let url = api_url(config, "v1/block-height");
    let body = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    body.get("height")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("missing 'height' in block-height response: {body}"))
}

/// HTTP fallback loop — used when WebSocket is unavailable.
/// Polls `/v1/block-height` every tick. Only fetches full block data when
/// the tip height changes, avoiding unnecessary traffic.
#[cfg(feature = "stale-mining")]
async fn run_http_fallback_loop(
    client: &reqwest::Client,
    config: &crate::stale_block_miner::StaleBlockMinerConfig,
    miner: &mut crate::stale_block_miner::StaleBlockMiner,
    live_client: &Option<LiveMiningClient>,
    latest_mempool: &mut crate::stale_block_miner::MempoolStats,
    shutdown: &CancellationToken,
) {
    let mut poll = time::interval(Duration::from_secs(config.poll_interval_secs));
    poll.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    let mut last_known_height: u64 = 0;
    let mut consecutive_failures = 0u32;

    info!("Stale-block: HTTP fallback polling active (every {}s)", config.poll_interval_secs);

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                info!("Stale-block: HTTP fallback loop shutting down");
                break;
            }
            _ = poll.tick() => {
                // Step 1: Lightweight tip height check
                let current_height = match fetch_block_height(client, config).await {
                    Ok(h) => {
                        consecutive_failures = 0;
                        h
                    }
                    Err(err) => {
                        consecutive_failures += 1;
                        warn!("Stale-block: block-height fetch failed ({consecutive_failures}x): {err:#}");
                        if consecutive_failures >= 10 {
                            warn!("Stale-block: 10 consecutive failures — yielding to WS reconnect");
                            break;
                        }
                        continue;
                    }
                };

                if current_height == last_known_height {
                    debug!("Stale-block: tip unchanged at {current_height}");
                    continue;
                }

                info!("Stale-block: new tip height {current_height} (was {last_known_height})");
                last_known_height = current_height;

                // Step 2: Fetch fee/stats enrichment
                if let Err(err) = refresh_mempool_stats(client, config, latest_mempool).await {
                    debug!("Stale-block: fee enrichment failed: {err:#}");
                }

                // Step 3: Full harvest
                match harvest_mempool_stale_proofs(client, config, miner).await {
                    Ok(new_count) => {
                        if new_count > 0 {
                            info!(
                                "Stale-block (HTTP): harvested {new_count} proof(s) | mempool={} txs, fastest_fee={} sat/vB",
                                latest_mempool.mempool_size, latest_mempool.fastest_fee,
                            );
                        }
                    }
                    Err(err) => warn!("Stale-block (HTTP): harvest failed: {err:#}"),
                }
                submit_pending_proofs(miner, live_client, config).await;
            }
        }
    }
}

/// Drain pending proofs and submit them (or requeue on failure).
#[cfg(feature = "stale-mining")]
async fn submit_pending_proofs(
    miner: &mut crate::stale_block_miner::StaleBlockMiner,
    live_client: &Option<LiveMiningClient>,
    config: &crate::stale_block_miner::StaleBlockMinerConfig,
) {
    let pending = miner.drain_pending_proofs();
    if pending.is_empty() {
        return;
    }

    if config.auto_submit {
        if let Some(live_client) = live_client {
            let mut retry = Vec::new();
            for proof in pending {
                if let Err(err) = live_client.submit_stale_proof(&proof).await {
                    warn!(proof_id = %proof.proof_id, "Stale-block: failed to auto-submit proof: {err:#}");
                    retry.push(proof);
                }
            }
            if !retry.is_empty() {
                warn!("Stale-block: requeued {} proof(s) after submission failures", retry.len());
                miner.requeue_pending_proofs(retry);
            }
        } else {
            miner.requeue_pending_proofs(pending);
        }
    } else {
        info!("Stale-block: queued {} proof(s) for manual submission", pending.len());
        miner.requeue_pending_proofs(pending);
    }
}

#[cfg(feature = "stale-mining")]
fn decode_bitcoin_display_hash(value: &str) -> Option<[u8; 32]> {
    let trimmed = value.trim();
    if trimmed.len() != 64 {
        return None;
    }

    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&hex::decode(trimmed).ok()?);
    bytes.reverse();
    Some(bytes)
}

/// Fetch 80-byte raw header for a block via `/v1/block/:hash/header`.
/// Returns (raw_header_bytes, height).
#[cfg(feature = "stale-mining")]
async fn fetch_block_header_json(
    client: &reqwest::Client,
    config: &crate::stale_block_miner::StaleBlockMinerConfig,
    block_hash: &str,
) -> Result<(Vec<u8>, u64)> {
    let path = format!("v1/block/{}/header", block_hash);
    let url = api_url(config, &path);
    let resp: Value = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let header_hex = resp
        .get("headerHex")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing headerHex in /v1/block/{}/header response", block_hash))?;
    let height = resp
        .get("height")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("missing height in /v1/block/{}/header response", block_hash))?;

    let bytes = hex::decode(header_hex.trim())
        .map_err(|e| anyhow!("bad headerHex for {}: {e}", block_hash))?;
    if bytes.len() != 80 {
        return Err(anyhow!(
            "unexpected header length {} for {} (expected 80)",
            bytes.len(),
            block_hash
        ));
    }
    Ok((bytes, height))
}

/// Fetch the canonical block hash at a given height via `/v1/blocks/:height`.
#[cfg(feature = "stale-mining")]
async fn fetch_canonical_hash_at_height(
    client: &reqwest::Client,
    config: &crate::stale_block_miner::StaleBlockMinerConfig,
    height: u64,
) -> Result<[u8; 32]> {
    let path = format!("v1/blocks/{}", height);
    let url = api_url(config, &path);
    let resp: Value = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    // The response may be a wrapped object { success, hash, ... } or a direct hash string (verbosity 0).
    let hash_str = resp
        .as_str()                                         // verbosity 0: plain hash string
        .or_else(|| resp.get("hash").and_then(Value::as_str))   // { hash: "0000..." }
        .or_else(|| resp.get("id").and_then(Value::as_str))     // mempool-style { id: "0000..." }
        .ok_or_else(|| anyhow!("no hash found in /v1/blocks/{} response", height))?;

    decode_bitcoin_display_hash(hash_str)
        .ok_or_else(|| anyhow!("invalid canonical hash at height {}: {}", height, hash_str))
}

/// Harvest stale-block proofs via `/v1/chain/tips`.
///
/// 1. GET `/v1/chain/tips` → filter tips where `isOrphan == true`
/// 2. For each orphan tip: fetch its raw 80-byte header via `/v1/block/:hash/header`
/// 3. Resolve the canonical hash at the same height via `/v1/blocks/:height`
/// 4. Submit to the miner's `submit_stale_header()`
#[cfg(feature = "stale-mining")]
async fn harvest_mempool_stale_proofs(
    client: &reqwest::Client,
    config: &crate::stale_block_miner::StaleBlockMinerConfig,
    miner: &mut crate::stale_block_miner::StaleBlockMiner,
) -> Result<usize> {
    // ── 1. Fetch chain tips ──────────────────────────────────────────────
    let url = api_url(config, "v1/chain/tips");
    let tips_resp: Value = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if !tips_resp
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(anyhow!("chain/tips returned success=false"));
    }

    let tips = tips_resp
        .get("tips")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("chain/tips response missing tips array"))?;

    let orphan_count = tips_resp
        .get("orphanCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    if orphan_count == 0 {
        debug!("Stale-block: chain/tips reports 0 orphan tips – nothing to harvest");
        return Ok(0);
    }

    info!("Stale-block: chain/tips reports {} orphan tip(s), processing…", orphan_count);

    let mut harvested = 0usize;

    for tip in tips {
        // ── 2. Skip non-orphan tips ──────────────────────────────────────
        let is_orphan = tip
            .get("isOrphan")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !is_orphan {
            continue;
        }

        let Some(orphan_hash) = tip.get("hash").and_then(Value::as_str) else {
            continue;
        };
        let Some(orphan_height) = tip.get("height").and_then(Value::as_u64) else {
            continue;
        };
        let branch_len = tip
            .get("branchLen")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1); // floor at 1

        // ── 3. Fetch the orphan's raw 80-byte header ────────────────────
        let (raw_header, _hdr_height) =
            match fetch_block_header_json(client, config, orphan_hash).await {
                Ok(r) => r,
                Err(err) => {
                    warn!(
                        "Stale-block: failed to fetch header for orphan {} at height {}: {err:#}",
                        orphan_hash, orphan_height
                    );
                    continue;
                }
            };

        // ── 4. Resolve canonical hash at the same height ────────────────
        let canonical_hash =
            match fetch_canonical_hash_at_height(client, config, orphan_height).await {
                Ok(h) => h,
                Err(err) => {
                    warn!(
                        "Stale-block: failed to fetch canonical hash at height {}: {err:#}",
                        orphan_height
                    );
                    continue;
                }
            };

        // ── 5. Submit to miner ──────────────────────────────────────────
        match miner.submit_stale_header(
            &raw_header,
            orphan_height,
            canonical_hash,
            branch_len as u32,
        ) {
            Ok(proof) => {
                harvested += 1;
                info!(
                    proof_id = %proof.proof_id,
                    height = proof.height,
                    leading_zeros = proof.leading_zeros,
                    reorg_depth = branch_len,
                    "Harvested stale block proof via chain/tips"
                );
            }
            Err(crate::stale_block_miner::StaleBlockError::AlreadyHarvested(_)) => {
                debug!("Stale-block: orphan {} already harvested", orphan_hash);
            }
            Err(crate::stale_block_miner::StaleBlockError::NotStale(_)) => {
                debug!(
                    "Stale-block: orphan {} at height {} matches canonical chain",
                    orphan_hash, orphan_height
                );
            }
            Err(err) => {
                warn!(
                    "Stale-block: failed to build proof for orphan {} at height {}: {err}",
                    orphan_hash, orphan_height
                );
            }
        }
    }

    Ok(harvested)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "stale-mining")]
    #[test]
    fn decode_bitcoin_display_hash_reverses_bytes() {
        let decoded = decode_bitcoin_display_hash("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f")
            .expect("hash should decode");

        assert_eq!(decoded[0], 0x1f);
        assert_eq!(decoded[31], 0x00);
    }

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
