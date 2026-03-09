use crate::chain::{LiveChallenge, LiveMiningClient, LiveSubmission};
use crate::config::MinerConfig;
use crate::crypto::{
    build_commitment_payload, contract_hash_message, has_leading_zero_bits, miner_address_from_signing_key,
    pre_filter_nonce, random_secret, MiningMaterial,
};
use crate::pqc::PqcMode;
use crate::seed::{decode_temporal_seed_timestamp, generate_temporal_seed};
use crate::telemetry::{MinerState, TelemetrySnapshot};
use anyhow::{anyhow, Result};
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

    let pqc_mode = crate::pqc::PqcMode::parse(&config.pqc_mode);
    let mut retry_count = 0usize;
    let mut nonce_cursor = 0u64;

    while !shutdown.is_cancelled() {
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
            match live_client.submit_solution(&submission).await {
                Ok(receipt) => {
                    let reward = LiveMiningClient::extract_reward_from_receipt(&receipt).unwrap_or(0.0);
                    {
                        let mut guard = stats.lock().await;
                        guard.solutions = guard.solutions.saturating_add(1);
                        guard.accepted_submissions = guard.accepted_submissions.saturating_add(1);
                        guard.total_rewards_estimate += reward;
                        guard.last_solution_nonce = Some(submission.nonce);
                        guard.last_solution_hash_hex = Some(hex_string(&submission.commitment.commit_hash));
                    }

                    let snapshot = snapshot_from_state(&config, started_at, &stats).await;
                    let _ = telemetry_tx.send(snapshot);
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
                    warn!("Live submission failed: {err}");
                }
            }
        }

        let snapshot = snapshot_from_state(&config, started_at, &stats).await;
        let _ = telemetry_tx.send(snapshot);
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
    let solution_found = AtomicBool::new(false);

    for worker_id in 0..config.threads.max(1) {
        let temporal_seed = generate_temporal_seed()?;
        let seed_timestamp = decode_temporal_seed_timestamp(&temporal_seed)?;
        let time_based_entropy = keccak256_bytes(&[
            &challenge.block_timestamp.to_be_bytes(),
            challenge.prevrandao.as_slice(),
            &seed_timestamp.to_be_bytes(),
            live_client.contract_address.as_bytes(),
        ].concat());
        let pre_input = [
            challenge.previous_output.as_slice(),
            temporal_seed.as_slice(),
            miner_address.as_bytes(),
            time_based_entropy.as_slice(),
        ].concat();

        for batch_index in 0..config.batch_size {
            if shutdown.is_cancelled() || solution_found.load(Ordering::SeqCst) {
                return Ok(None);
            }

            let current_nonce = (*nonce_cursor)
                .saturating_add(worker_id as u64)
                .saturating_add((batch_index as u64) * step);

            {
                let mut guard = stats.lock().await;
                guard.hashes = guard.hashes.saturating_add(1);
                guard.temperature_c = Some(45.0 + ((worker_id % 10) as f32));
            }

            if !pre_filter_nonce_live(current_nonce, &pre_input, challenge.difficulty) {
                continue;
            }

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
            let reveal_signature = live_client.sign_entropy_hash(entropy_hash)?;
            let solution_hash = quantum_resistant_hash_live(
                &reveal_signature,
                &entropy_hash,
                &secret_value,
                challenge.block_timestamp,
            );

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
                let commit_nonce = live_client.next_commit_nonce().await?;
                let commitment = crate::crypto::DynamicMiningCommitment {
                    commit_hash,
                    pool_id: config.pool_id,
                    nonce: commit_nonce,
                    deadline: unix_secs().saturating_add(300),
                };

                let preview = {
                    let mut guard = stats.lock().await;
                    guard.last_solution_nonce = Some(current_nonce);
                    guard.last_solution_hash_hex = Some(hex_string(&solution_hash));
                    snapshot_from_guard(config, started_at, &guard)
                };
                let _ = telemetry_tx.send(preview);
                *nonce_cursor = current_nonce.saturating_add(step);

                return Ok(Some(LiveSubmission {
                    commitment,
                    previous_output: challenge.previous_output,
                    temporal_seed,
                    nonce: current_nonce,
                    reveal_signature,
                    secret_value,
                }));
            }
        }
    }

    *nonce_cursor = nonce_cursor.saturating_add((config.batch_size.max(1) * config.threads.max(1)) as u64);
    Ok(None)
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
        temperature_c: guard.temperature_c,
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
    block_timestamp: u64,
) -> [u8; 32] {
    let mut h = keccak256_bytes(&[signature, entropy_hash, secret_value].concat());
    for i in 0..3u8 {
        h = keccak256_bytes(&[
            xor_first_byte(h, i + 1).as_slice(),
            block_timestamp.to_be_bytes().as_slice(),
        ].concat());
        h = rotate_hash_left(h, 7);
    }
    h
}

fn pre_filter_nonce_live(nonce: u64, input: &[u8], difficulty: U256) -> bool {
    let hash = blake3::hash(&[input, &nonce.to_be_bytes()].concat());
    U256::from_big_endian(hash.as_bytes()) < difficulty / U256::from(100u64)
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

fn xor_first_byte(mut input: [u8; 32], value: u8) -> [u8; 32] {
    input[0] ^= value;
    input
}
