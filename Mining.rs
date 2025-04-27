use ethers::{
    prelude::*,
    utils::{hex, keccak256},
    middleware::gas_oracle::{GasOracle, GasOracleMiddleware, Etherscan},
    types::{transaction::eip712::{EIP712Domain, TypedData, TypeField}, Address, U256, H256, Bytes},
    signers::LocalWallet,
};
use k256::ecdsa::{SigningKey, Signature, signature::Signer as _};
use rand::{rngs::OsRng, Rng};
use blake3;
use std::time::{Instant, SystemTime, UNIX_EPOCH, Duration};
use std::env;
use std::fs;
use std::path::Path;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::collections::VecDeque;
use tokio::sync::{Mutex, oneshot, broadcast};
use tracing::{info, error, warn, debug, Level};
use tracing_subscriber::FmtSubscriber;
use anyhow::{Result, Context, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zeroize::Zeroize;
use std::arch::x86_64::is_x86_feature_detected;

// Constants from MiningLib.sol
const QR_HASH_ITERATIONS: u8 = 3;
const QR_HASH_ROTATION: u8 = 7;
const BASE_WEIGHT: u64 = 1; // Adjust based on contract

// Placeholder modules (implement as needed)
mod cpu {
    pub struct CpuIdentity { pub vendor: String, pub brand: String, pub cores: u32, pub cache_size: u64, pub features: u64 }
    pub fn detect_cpu() -> CpuIdentity {
        CpuIdentity { vendor: "Unknown".to_string(), brand: "Unknown".to_string(), cores: 4, cache_size: 8192, features: 0 }
    }
    pub fn mask_cpu_identity() -> CpuIdentity { detect_cpu() }
}
mod network {
    use anyhow::Result;
    pub struct SecureChannel;
    impl SecureChannel {
        pub async fn write_all(&mut self, _buf: &[u8]) -> Result<()> { Ok(()) }
        pub fn check_rotation(&mut self) -> Result<()> { Ok(()) }
        pub async fn shutdown(&mut self) -> Result<()> { Ok(()) }
    }
    pub async fn secure_connect_pinned(_endpoint: &str, _cert_path: &str) -> Result<SecureChannel> {
        Err(anyhow!("Not implemented"))
    }
}
mod memory {
    use zeroize::Zeroize;
    pub struct SecureBuffer(Vec<u8>);
    impl SecureBuffer {
        pub fn new(size: usize) -> Self { Self(vec![0; size]) }
        pub fn as_slice(&self) -> &[u8] { &self.0 }
        pub fn as_mut_slice(&mut self) -> &mut [u8] { &mut self.0 }
    }
    impl Drop for SecureBuffer { fn drop(&mut self) { self.0.zeroize(); } }
}
mod update {
    use anyhow::Result;
    pub struct UpdateVerifier;
    impl UpdateVerifier {
        pub fn new(_pub_key: Vec<u8>, _version: &str) -> Result<Self> { Ok(Self) }
        pub async fn check_for_updates(&self, _server: &str) -> Result<Option<Manifest>> { Ok(None) }
        pub async fn download_update(&self, _manifest: &Manifest, _path: &std::path::Path) -> Result<()> { Ok(()) }
    }
    pub struct Manifest { pub version: String }
    pub async fn apply_update(_path: &std::path::Path) -> Result<()> { Ok(()) }
    pub fn restart_application() -> Result<()> { Ok(()) }
}

// Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MinerConfig {
    contract_address: String,
    rpc_url: String,
    private_key_path: Option<String>,
    threads: usize,
    gas_price_multiplier: f64,
    retry_delay: Duration,
    log_level: Level,
    stats_interval: Duration,
    exit_after_blocks: Option<usize>,
    max_retries: usize,
    prefetch_distance: usize,
    batch_size: usize,
    l3_cache_optimized: bool,
    update_server: String,
    update_check_interval: Duration,
    update_public_key_path: String,
    update_enabled: bool,
    stats_server_required: bool,
    stats_server_endpoint: String,
    stats_server_cert_path: String,
}

fn default_update_server() -> String { "https://updates.example.com/v1".to_string() }
fn default_update_check_interval() -> Duration { Duration::from_secs(4 * 3600) }
fn default_update_public_key_path() -> String { "update_pub.der".to_string() }
fn default_update_enabled() -> bool { true }
fn default_stats_server_required() -> bool { false }
fn default_stats_server_endpoint() -> String { "localhost:9999".to_string() }
fn default_stats_server_cert_path() -> String { "certs/stats_server.der".to_string() }

impl Default for MinerConfig {
    fn default() -> Self {
        Self {
            contract_address: "0xYourContractAddress".to_string(),
            rpc_url: "http://localhost:8545".to_string(),
            private_key_path: None,
            threads: 4,
            gas_price_multiplier: 1.1,
            retry_delay: Duration::from_secs(5),
            log_level: Level::INFO,
            stats_interval: Duration::from_secs(60),
            exit_after_blocks: None,
            max_retries: 5,
            prefetch_distance: 4,
            batch_size: 16,
            l3_cache_optimized: true,
            update_server: default_update_server(),
            update_check_interval: default_update_check_interval(),
            update_public_key_path: default_update_public_key_path(),
            update_enabled: default_update_enabled(),
            stats_server_required: default_stats_server_required(),
            stats_server_endpoint: default_stats_server_endpoint(),
            stats_server_cert_path: default_stats_server_cert_path(),
        }
    }
}

// Mining statistics
#[derive(Debug, Clone, Default)]
struct MiningStats {
    hashes: u64,
    solutions: u32,
    start_time: SystemTime,
    failed_submissions: usize,
    successful_submissions: usize,
    total_rewards: f64,
}

// Shutdown manager
#[derive(Clone)]
struct ShutdownManager {
    sender: broadcast::Sender<()>,
}

impl ShutdownManager {
    fn new() -> Self {
        let (sender, _) = broadcast::channel(1);
        Self { sender }
    }
    fn trigger(&self) { let _ = self.sender.send(()); }
    fn subscribe(&self) -> broadcast::Receiver<()> { self.sender.subscribe() }
}

// Thermal monitor
struct ThermalMonitor {
    readings: VecDeque<f32>,
    max_readings: usize,
}

impl ThermalMonitor {
    fn new(max_readings: usize) -> Self {
        Self { readings: VecDeque::with_capacity(max_readings.max(1)), max_readings: max_readings.max(1) }
    }
    fn add_reading(&mut self, temp: f32) {
        if self.readings.len() >= self.max_readings { self.readings.pop_front(); }
        self.readings.push_back(temp);
    }
    fn average_temp(&self) -> f32 {
        if self.readings.is_empty() { 50.0 } else { self.readings.iter().sum::<f32>() / self.readings.len() as f32 }
    }
}

// Thermal controller
struct ThermalNotification { temperature: f32, timestamp: SystemTime }

struct ThermalController {
    monitor: ThermalMonitor,
    max_temp: f32,
    min_throttle_factor: f32,
    notification_tx: broadcast::Sender<ThermalNotification>,
}

impl ThermalController {
    fn new(max_readings: usize, max_temp: f32, min_throttle_factor: f32) -> Self {
        let (notification_tx, _) = broadcast::channel(32);
        Self {
            monitor: ThermalMonitor::new(max_readings),
            max_temp,
            min_throttle_factor: min_throttle_factor.max(0.0).min(1.0),
            notification_tx,
        }
    }
    fn subscribe(&self) -> broadcast::Receiver<ThermalNotification> { self.notification_tx.subscribe() }
    async fn update(&mut self, current_temp: f32) -> f32 {
        self.monitor.add_reading(current_temp);
        let avg_temp = self.monitor.average_temp();
        let factor = if avg_temp > self.max_temp {
            let excess = (avg_temp - self.max_temp).max(0.0);
            let throttle_reduction = (excess / 10.0).min(1.0);
            let factor = 1.0 - throttle_reduction * (1.0 - self.min_throttle_factor);
            let _ = self.notification_tx.send(ThermalNotification { temperature: avg_temp, timestamp: SystemTime::now() });
            warn!("Thermal throttling: Avg Temp: {:.1}°C (Max: {}°C), Factor: {:.2}", avg_temp, self.max_temp, factor);
            factor.max(self.min_throttle_factor)
        } else { 1.0 };
        if factor < 1.0 {
            tokio::time::sleep(Duration::from_millis(((1.0 - factor) * 100.0) as u64)).await;
        }
        factor
    }
}

// CPU temperature reading
#[cfg(target_os = "linux")]
fn read_cpu_temperature() -> Result<f32> {
    let temp_str = fs::read_to_string("/sys/class/thermal/thermal_zone0/temp")?;
    Ok(temp_str.trim().parse::<f32>()? / 1000.0)
}

#[cfg(not(target_os = "linux"))]
fn read_cpu_temperature() -> Result<f32> { Ok(50.0) }

// Stealth address
#[derive(Debug, Clone)]
struct StealthAddress {
    spending_key: LocalWallet,
    viewing_key: SigningKey,
    ephemeral_keys: VecDeque<(SigningKey, SystemTime)>,
}

impl StealthAddress {
    fn generate_ephemeral_key(&mut self) -> Result<PublicKey> {
        let ephemeral_key = SigningKey::random(&mut OsRng);
        let expiry = SystemTime::now() + Duration::from_secs(3600);
        self.ephemeral_keys.push_back((ephemeral_key.clone(), expiry));
        while let Some((_, exp)) = self.ephemeral_keys.front() {
            if exp < &SystemTime::now() { self.ephemeral_keys.pop_front(); } else { break; }
        }
        Ok(ephemeral_key.verifying_key().into())
    }
    fn find_claiming_key(&self, stealth_meta: &[u8]) -> Option<SigningKey> {
        self.ephemeral_keys.iter().find(|(key, expiry)| {
            if expiry < &SystemTime::now() { return false; }
            let public = key.verifying_key();
            let point = public.as_affine().to_encoded_point(true);
            stealth_meta.starts_with(point.as_bytes())
        }).map(|(key, _)| key.clone())
    }
}

// EIP-712 commitment
#[derive(Debug, Clone)]
pub struct DynamicMiningCommitment {
    pub commit_hash: [u8; 32],
    pub pool_id: u8,
    pub nonce: u64,
    pub deadline: u64,
}

impl DynamicMiningCommitment {
    fn new(
        commit_hash: [u8; 32], pool_id: u8, nonce: u64, deadline: u64,
    ) -> Self {
        Self { commit_hash, pool_id, nonce, deadline }
    }

    fn to_eip712_types(&self, chain_id: u64, contract_address: Address) -> TypedData {
        TypedData {
            domain: EIP712Domain {
                name: Some("TemporalGradientBeacon".to_string()),
                version: Some("1".to_string()),
                chain_id: Some(chain_id.into()),
                verifying_contract: Some(contract_address),
                salt: None,
            },
            primary_type: "MiningCommitment".to_string(),
            types: {
                let mut types = BTreeMap::new();
                types.insert(
                    "MiningCommitment".to_string(),
                    vec![
                        TypeField { name: "commitHash".to_string(), r#type: "bytes32".to_string() },
                        TypeField { name: "poolId".to_string(), r#type: "uint8".to_string() },
                        TypeField { name: "nonce".to_string(), r#type: "uint64".to_string() },
                        TypeField { name: "deadline".to_string(), r#type: "uint64".to_string() },
                    ],
                );
                types
            },
            message: {
                let mut map = BTreeMap::new();
                map.insert("commitHash".to_string(), Value::from(format!("0x{}", hex::encode(self.commit_hash))));
                map.insert("poolId".to_string(), Value::from(self.pool_id));
                map.insert("nonce".to_string(), Value::from(self.nonce));
                map.insert("deadline".to_string(), Value::from(self.deadline));
                map
            },
        }
    }

    async fn sign(&self, wallet: &LocalWallet, chain_id: u64, contract_address: Address) -> Result<Bytes> {
        let typed_data = self.to_eip712_types(chain_id, contract_address);
        let signature = wallet.sign_typed_data(&typed_data).await
            .map_err(|e| anyhow!("Failed to sign commitment: {}", e))?;
        Ok(signature.to_vec().into())
    }
}

// Contract ABI
abigen!(
    MiningContract,
    r#"[
        function submitMiningCommitment(
            bytes32 commitHash,
            uint8 poolId,
            uint256 nonce,
            uint256 deadline,
            bytes calldata signature
        ) external returns (bool);
        
        function revealMiningCommitment(
            bytes32 previousOutput,
            bytes calldata temporalSeed,
            uint64 nonce,
            bytes calldata signature,
            bytes32 secretValue,
            uint8 poolId
        ) external;
        
        function minCommitmentAge() external view returns (uint256);
        function getMiningChallenge(uint8 poolId) external view returns (bytes32[] memory outputs, uint256 difficulty);
    ]"#
);

// Hybrid hashing functions
#[inline(always)]
fn contract_hash_message(message: &[u8]) -> [u8; 32] {
    keccak256(message) // Direct keccak256 for contract compatibility
}

#[inline(always)]
fn pre_filter_nonce(nonce: u64, input: &[u8], difficulty: U256) -> bool {
    let hash = blake3::hash(&[input, &nonce.to_be_bytes()].concat());
    U256::from_big_endian(hash.as_bytes()) < difficulty / U256::from(100) // Tunable threshold
}

fn quantum_resistant_hash_inner(input: &[u8], block_timestamp: u64) -> [u8; 32] {
    let mut h = keccak256(input);
    for i in 0..QR_HASH_ITERATIONS {
        let mut xor_h = h;
        xor_h[0] ^= (i + 1) as u8;
        let input = [&xor_h, &block_timestamp.to_be_bytes()].concat();
        h = keccak256(&input);
        let mut rotated = [0u8; 32];
        for j in 0..32 {
            let left = (h[j] as u32) << QR_HASH_ROTATION;
            let right = (h[j] as u32) >> (8 - QR_HASH_ROTATION);
            rotated[j] = ((left | right) & 0xFF) as u8;
        }
        h = rotated;
    }
    h
}

fn quantum_resistant_hash(signature: &Signature, entropy_hash: &[u8; 32], secret: &[u8], block_timestamp: u64) -> [u8; 32] {
    let packed = [
        signature.to_der().as_bytes(),
        entropy_hash,
        secret,
    ].concat();
    quantum_resistant_hash_inner(&packed, block_timestamp)
}

fn create_entropy_hash(
    previous_output: &[u8; 32],
    temporal_seed: &[u8],
    nonce: u64,
    sender: &Address,
    time_based_entropy: &[u8; 32],
    secret_value: &[u8; 32],
) -> [u8; 32] {
    keccak256(&[
        previous_output.as_slice(),
        temporal_seed,
        &nonce.to_be_bytes(),
        sender.as_bytes(),
        time_based_entropy,
        secret_value,
    ].concat())
}

fn generate_temporal_seed() -> Vec<u8> {
    let mut seed = Vec::with_capacity(64);
    seed.extend_from_slice(&rand::random::<[u8; 32]>());
    seed.extend_from_slice(&SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos().to_le_bytes());
    blake3::hash(&seed).as_bytes().to_vec()
}

#[inline]
fn meets_difficulty(hmac: &[u8; 32], target: U256) -> bool {
    U256::from_big_endian(hmac) < target
}

async fn get_current_challenge(provider: &Provider<Http>, contract_address: &str) -> Result<([u8; 32], U256)> {
    let contract_addr = contract_address.parse::<Address>()?;
    let contract = MiningContract::new(contract_addr, Arc::new(provider.clone()));
    let pool_id = 0u8;
    let (outputs, difficulty) = contract.get_mining_challenge(pool_id).call().await?;
    let previous_output = outputs.first()
        .ok_or_else(|| anyhow!("No outputs returned"))?
        .to_fixed_bytes();
    Ok((previous_output, difficulty))
}

async fn submit_solution(
    client: &SignerMiddleware<GasOracleMiddleware<Provider<Http>, Etherscan>, LocalWallet>,
    address: Address,
    commitment: &DynamicMiningCommitment,
    previous_output: &[u8; 32],
    temporal_seed: &[u8],
    nonce: u64,
    signature: &Signature,
    secret_value: &[u8; 32],
) -> Result<TransactionReceipt> {
    let contract = MiningContract::new(address, Arc::new(client.clone()));
    let sig_bytes = Bytes::from(signature.to_der().as_bytes().to_vec());
    let commit_tx = contract.submit_mining_commitment(
        commitment.commit_hash.into(),
        commitment.pool_id,
        commitment.nonce,
        commitment.deadline,
        commitment.sign(&client.signer(), client.chain_id().await?.as_u64(), address).await?,
    );
    let pending_tx = commit_tx.send().await.context("Failed to send commitment")?;
    let commit_receipt = pending_tx.await?.ok_or_else(|| anyhow!("No commitment receipt"))?;
    let min_blocks = contract.min_commitment_age().call().await?;
    loop {
        let current_block = client.get_block_number().await?;
        if current_block >= commit_receipt.block_number.unwrap() + min_blocks.into() { break; }
        tokio::time::sleep(Duration::from_secs(12)).await;
    }
    let reveal_tx = contract.reveal_mining_commitment(
        (*previous_output).into(),
        Bytes::from(temporal_seed.to_vec()),
        nonce,
        sig_bytes,
        (*secret_value).into(),
        commitment.pool_id,
    );
    let pending_tx = reveal_tx.send().await.context("Failed to send reveal")?;
    let receipt = pending_tx.await?.ok_or_else(|| anyhow!("No reveal receipt"))?;
    Ok(receipt)
}

fn extract_reward_from_receipt(receipt: &TransactionReceipt) -> Option<f64> {
    let event_signature_hash = keccak256("BeaconBlockMined(address,bytes32,uint256,uint64,uint64,uint8)");
    for log in &receipt.logs {
        if log.topics.len() >= 1 && log.topics[0] == H256(event_signature_hash) && log.data.len() >= 32 {
            let reward_u256 = U256::from_big_endian(&log.data[0..32]);
            return ethers::utils::format_units(reward_u256, 18).ok().and_then(|s| s.parse().ok());
        }
    }
    None
}

async fn print_stats(stats_arc: &Arc<Mutex<MiningStats>>) {
    let stats = stats_arc.lock().await;
    let elapsed = stats.start_time.elapsed().unwrap_or_default().as_secs_f64();
    let hashrate = if elapsed > 0.0 { stats.hashes as f64 / elapsed } else { 0.0 };
    info!("┌─── Mining Statistics ───────────────────────");
    info!("│ Solutions: {}", stats.solutions);
    info!("│ Total Hashes: {}", stats.hashes);
    info!("│ Hashrate: {:.2} H/s", hashrate);
    info!("│ Running time: {:.2} minutes", elapsed / 60.0);
    info!("│ Total rewards: {:.6} tokens", stats.total_rewards);
    info!("│ Successful Submissions: {}", stats.successful_submissions);
    info!("│ Failed Submissions: {}", stats.failed_submissions);
    info!("└────────────────────────────────────────────");
}

fn load_config() -> Result<MinerConfig> {
    let config_path = env::var("CONFIG_PATH").unwrap_or("miner_config.json".to_string());
    let defaults = MinerConfig::default();
    let config: MinerConfig = if Path::new(&config_path).exists() {
        let config_data = fs::read_to_string(&config_path)?;
        let loaded_config: Value = serde_json::from_str(&config_data)?;
        let default_config_json = serde_json::to_value(defaults)?;
        let mut merged_config = default_config_json;
        json_patch::merge(&mut merged_config, &loaded_config);
        serde_json::from_value(merged_config)?
    } else {
        MinerConfig {
            contract_address: env::var("CONTRACT_ADDRESS").unwrap_or(defaults.contract_address),
            rpc_url: env::var("RPC_URL").unwrap_or(defaults.rpc_url),
            private_key_path: env::var("PRIVATE_KEY_PATH").ok(),
            threads: env::var("MINER_THREADS").ok().and_then(|s| s.parse().ok()).unwrap_or(defaults.threads),
            gas_price_multiplier: env::var("GAS_PRICE_MULTIPLIER").ok().and_then(|s| s.parse().ok()).unwrap_or(defaults.gas_price_multiplier),
            retry_delay: Duration::from_secs(env::var("RETRY_DELAY_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(defaults.retry_delay.as_secs())),
            log_level: env::var("LOG_LEVEL").ok().and_then(|s| match s.to_uppercase().as_str() {
                "TRACE" => Some(Level::TRACE), "DEBUG" => Some(Level::DEBUG), "INFO" => Some(Level::INFO),
                "WARN" => Some(Level::WARN), "ERROR" => Some(Level::ERROR), _ => None,
            }).unwrap_or(defaults.log_level),
            stats_interval: Duration::from_secs(env::var("STATS_INTERVAL_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(defaults.stats_interval.as_secs())),
            exit_after_blocks: env::var("EXIT_AFTER_BLOCKS").ok().and_then(|s| s.parse().ok()),
            max_retries: env::var("MAX_RETRIES").ok().and_then(|s| s.parse().ok()).unwrap_or(defaults.max_retries),
            prefetch_distance: env::var("PREFETCH_DISTANCE").ok().and_then(|s| s.parse().ok()).unwrap_or(defaults.prefetch_distance),
            batch_size: env::var("BATCH_SIZE").ok().and_then(|s| s.parse().ok()).unwrap_or(defaults.batch_size),
            l3_cache_optimized: env::var("L3_CACHE_OPTIMIZED").ok().and_then(|s| s.parse().ok()).unwrap_or(defaults.l3_cache_optimized),
            update_server: env::var("UPDATE_SERVER").unwrap_or(defaults.update_server),
            update_check_interval: Duration::from_secs(env::var("UPDATE_CHECK_INTERVAL_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(defaults.update_check_interval.as_secs())),
            update_public_key_path: env::var("UPDATE_PUBLIC_KEY_PATH").unwrap_or(defaults.update_public_key_path),
            update_enabled: env::var("UPDATE_ENABLED").ok().and_then(|s| s.parse().ok()).unwrap_or(defaults.update_enabled),
            stats_server_required: env::var("STATS_SERVER_REQUIRED").ok().and_then(|s| s.parse().ok()).unwrap_or(defaults.stats_server_required),
            stats_server_endpoint: env::var("STATS_SERVER_ENDPOINT").unwrap_or(defaults.stats_server_endpoint),
            stats_server_cert_path: env::var("STATS_SERVER_CERT_PATH").unwrap_or(defaults.stats_server_cert_path),
        }
    };
    let mut config = config;
    if config.threads < 1 { config.threads = 1; warn!("Threads set to 1"); }
    if config.batch_size < 1 { config.batch_size = 1; warn!("Batch size set to 1"); }
    if config.contract_address == "0xYourContractAddress" {
        warn!("Using default contract address. Set CONTRACT_ADDRESS or update config");
    }
    if !Path::new(&config_path).exists() {
        let config_json = serde_json::to_string_pretty(&config)?;
        fs::write(&config_path, config_json)?;
    }
    Ok(config)
}

fn setup_logging(level: Level) -> Result<()> {
    let subscriber = FmtSubscriber::builder().with_max_level(level).with_ansi(true).with_file(true).with_line_number(true).finish();
    tracing::subscriber::set_global_default(subscriber).context("Failed to set logging")
}

fn load_or_generate_key_secure(config: &MinerConfig) -> Result<memory::SecureBuffer> {
    let key_bytes = if let Some(key_path) = &config.private_key_path {
        let key_path = Path::new(key_path);
        if key_path.exists() {
            let key_data = fs::read_to_string(key_path)?;
            hex::decode(key_data.trim())?
        } else {
            let new_key = SigningKey::random(&mut OsRng);
            let bytes = new_key.to_bytes();
            let key_hex = hex::encode(&bytes);
            fs::write(key_path, key_hex)?;
            bytes.to_vec()
        }
    } else {
        let new_key = SigningKey::random(&mut OsRng);
        new_key.to_bytes().to_vec()
    };
    if key_bytes.len() != 32 { return Err(anyhow!("Invalid key length: {}", key_bytes.len())); }
    let mut secure_buffer = memory::SecureBuffer::new(32);
    secure_buffer.as_mut_slice().copy_from_slice(&key_bytes);
    key_bytes.zeroize();
    Ok(secure_buffer)
}

fn create_wallet_from_secure_buffer(key_buffer: &memory::SecureBuffer, _config: &MinerConfig) -> Result<LocalWallet> {
    let mut key_bytes = [0u8; 32];
    key_bytes.copy_from_slice(key_buffer.as_slice());
    let wallet = LocalWallet::from_bytes(&key_bytes).map_err(|e| anyhow!("Failed to create wallet: {}", e))?;
    key_bytes.zeroize();
    let chain_id = env::var("CHAIN_ID").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    Ok(wallet.with_chain_id(chain_id))
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = load_config()?;
    setup_logging(config.log_level)?;
    let app_version = env!("CARGO_PKG_VERSION");
    info!("Starting Temporal Gradient Beacon Miner v{}", app_version);
    let cpu_info = cpu::detect_cpu();
    info!("Detected CPU: Vendor={}, Brand={}, Cores={}, Cache={}KB", cpu_info.vendor, cpu_info.brand, cpu_info.cores, cpu_info.cache_size / 1024);

    let shutdown_manager = ShutdownManager::new();
    let shutdown_manager_for_signal = shutdown_manager.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.expect("Failed to install Ctrl+C handler");
        warn!("Ctrl+C received, initiating shutdown...");
        shutdown_manager_for_signal.trigger();
    });

    let secure_channel = if config.stats_server_required || config.stats_server_endpoint != default_stats_server_endpoint() {
        match network::secure_connect_pinned(&config.stats_server_endpoint, &config.stats_server_cert_path).await {
            Ok(channel) => {
                info!("Connected to stats server at {}", config.stats_server_endpoint);
                Some(Arc::new(Mutex::new(channel)))
            }
            Err(e) => {
                let msg = format!("Failed to connect to stats server: {}", e);
                if config.stats_server_required { return Err(anyhow!(msg)); }
                warn!("{}", msg);
                None
            }
        }
    } else { None };

    let secure_key_buffer = Arc::new(Mutex::new(load_or_generate_key_secure(&config)?));
    let wallet = {
        let key_buffer_guard = secure_key_buffer.lock().await;
        create_wallet_from_secure_buffer(&key_buffer_guard, &config)?
    };
    {
        let mut temp_key_bytes = [0u8; 32];
        {
            let key_buffer_guard = secure_key_buffer.lock().await;
            temp_key_bytes.copy_from_slice(key_buffer_guard.as_slice());
        }
        let temp_signing_key = SigningKey::from_bytes(&temp_key_bytes)?;
        let public_key = temp_signing_key.verifying_key();
        info!("Mining with public key: {}", hex::encode(public_key.to_encoded_point(false).as_bytes()));
        temp_key_bytes.zeroize();
    }

    let provider = Provider::<Http>::try_from(&config.rpc_url)?;
    let chain_id = provider.get_chainid().await?.as_u64();
    let api_key = env::var("ETHERSCAN_API_KEY").context("ETHERSCAN_API_KEY not set")?;
    let gas_oracle = Etherscan::new(chain_id.try_into()?, api_key);
    let gas_provider = GasOracleMiddleware::new(provider.clone(), gas_oracle, config.gas_price_multiplier);
    let client = SignerMiddleware::new(gas_provider, wallet.clone());

    let stats = Arc::new(Mutex::new(MiningStats { start_time: SystemTime::now(), ..Default::default() }));
    let stats_clone = Arc::clone(&stats);
    let stats_interval = config.stats_interval;
    let secure_channel_clone = secure_channel.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(stats_interval).await;
            print_stats(&stats_clone).await;
            if let Some(channel_arc) = &secure_channel_clone {
                let stats_data = {
                    let stats_guard = stats_clone.lock().await;
                    serde_json::to_string(&*stats_guard).unwrap_or("{}".to_string())
                };
                let mut channel_guard = channel_arc.lock().await;
                if let Err(e) = channel_guard.write_all(stats_data.as_bytes()).await {
                    warn!("Failed to send stats: {}", e);
                }
            }
        }
    });

    let contract_address = config.contract_address.parse::<Address>()?;
    let stealth = Arc::new(Mutex::new(StealthAddress {
        spending_key: wallet.clone(),
        viewing_key: SigningKey::random(&mut OsRng),
        ephemeral_keys: VecDeque::new(),
    }));

    let mut consecutive_errors = 0;
    let mut blocks_mined_count = 0;
    let mut main_shutdown_receiver = shutdown_manager.subscribe();

    'main_loop: loop {
        tokio::select! {
            _ = main_shutdown_receiver.recv() => {
                info!("Main loop shutdown");
                break 'main_loop;
            }
            _ = tokio::time::sleep(Duration::ZERO) => {
                if let Some(exit_after) = config.exit_after_blocks {
                    if blocks_mined_count >= exit_after {
                        info!("Mined {} solutions, shutting down", exit_after);
                        shutdown_manager.trigger();
                        break 'main_loop;
                    }
                }
            }
        }

        let (previous_output, difficulty_target) = match get_current_challenge(&provider, &config.contract_address).await {
            Ok(challenge) => { consecutive_errors = 0; challenge },
            Err(e) => {
                error!("Failed to get challenge: {}", e);
                consecutive_errors += 1;
                if consecutive_errors > config.max_retries { return Err(anyhow!("Too many failures")); }
                tokio::time::sleep(config.retry_delay).await;
                continue;
            }
        };
        info!("Mining new block. Difficulty: {}", difficulty_target);

        let solution_found = Arc::new(AtomicBool::new(false));
        let mut handles = vec![];

        for thread_id in 0..config.threads {
            let client = client.clone();
            let stats = Arc::clone(&stats);
            let solution_found = Arc::clone(&solution_found);
            let previous_output = previous_output;
            let config = config.clone();
            let secure_key_buffer_clone = Arc::clone(&secure_key_buffer);
            let difficulty_target = difficulty_target;
            let stealth = Arc::clone(&stealth);
            let mut thread_shutdown_receiver = shutdown_manager.subscribe();
            let thermal_controller = ThermalController::new(10, 85.0, 0.2);

            handles.push(tokio::spawn(async move {
                let mut nonce_base = thread_id as u64;
                let nonce_step_all_threads = config.threads as u64;
                let nonce_increment_batch = nonce_step_all_threads * config.batch_size as u64;
                let mut thermal_controller = thermal_controller;
                let thermal_rx = thermal_controller.subscribe();
                let mut throttle_factor = 1.0;

                let latest_block = provider.get_block(BlockNumber::Latest).await?.ok_or_else(|| anyhow!("No latest block"))?;
                let block_timestamp = latest_block.timestamp.as_u64();
                let prevrandao = latest_block.prevrandao.unwrap_or(H256::zero()).0;
                let time_based_entropy = keccak256(&[
                    &block_timestamp.to_be_bytes(),
                    &prevrandao,
                    &generate_temporal_seed(),
                    client.address().as_bytes(),
                ].concat());

                'mining_loop: loop {
                    tokio::select! {
                        _ = thread_shutdown_receiver.recv() => {
                            debug!("Thread {} shutdown", thread_id);
                            break 'mining_loop;
                        }
                        Ok(notification) = thermal_rx.recv() => {
                            if SystemTime::now().duration_since(notification.timestamp).unwrap_or_default() < Duration::from_secs(1) {
                                throttle_factor = thermal_controller.update(notification.temperature).await;
                            }
                        }
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {
                            match read_cpu_temperature() {
                                Ok(temp) => { throttle_factor = thermal_controller.update(temp).await; }
                                Err(e) => { warn!("Failed to read temperature: {}", e); }
                            }
                        }
                    }

                    if solution_found.load(Ordering::SeqCst) { break 'mining_loop; }

                    let secret_value = rand::random::<[u8; 32]>();
                    let temporal_seed = generate_temporal_seed();
                    let pre_input = [
                        &previous_output,
                        &temporal_seed,
                        client.address().as_bytes(),
                        &time_based_entropy,
                        &secret_value,
                    ].concat();

                    let mut found_in_batch = false;
                    let mut solution_details = None;

                    for i in 0..config.batch_size {
                        let current_nonce = nonce_base + (i as u64 * nonce_step_all_threads);
                        if !pre_filter_nonce(current_nonce, &pre_input, difficulty_target) { continue; }

                        let entropy_hash = create_entropy_hash(
                            &previous_output, &temporal_seed, current_nonce, &client.address(), &time_based_entropy, &secret_value,
                        );

                        let mut temp_key_bytes = [0u8; 32];
                        {
                            let key_buffer_guard = secure_key_buffer_clone.lock().await;
                            temp_key_bytes.copy_from_slice(key_buffer_guard.as_slice());
                        }
                        let temp_signing_key = SigningKey::from_bytes(&temp_key_bytes)?;
                        let signature: Signature = temp_signing_key.sign(&entropy_hash);
                        temp_key_bytes.zeroize();

                        let solution_hash = quantum_resistant_hash(&signature, &entropy_hash, &secret_value, block_timestamp);
                        {
                            let mut stats_guard = stats.lock().await;
                            stats_guard.hashes += 1;
                        }

                        if meets_difficulty(&solution_hash, difficulty_target) {
                            if solution_found.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                                info!("Thread {} found solution! Nonce: {}", thread_id, current_nonce);
                                found_in_batch = true;
                                solution_details = Some((current_nonce, temporal_seed.clone(), signature, solution_hash, secret_value));
                                break;
                            } else {
                                debug!("Thread {} found solution, but another claimed it", thread_id);
                                break 'mining_loop;
                            }
                        }
                    }

                    if found_in_batch {
                        if let Some((nonce, temporal_seed, signature, solution_hash, secret_value)) = solution_details {
                            let commit_hash = contract_hash_message(&[
                                &previous_output,
                                &temporal_seed,
                                &nonce.to_be_bytes(),
                                &signature.to_der().as_bytes(),
                                &secret_value,
                                client.address().as_bytes(),
                            ].concat());

                            let deadline = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() + 300;
                            let commitment = DynamicMiningCommitment::new(
                                commit_hash, 0, nonce, deadline,
                            );

                            match submit_solution(&client, contract_address, &commitment, &previous_output, &temporal_seed, nonce, &signature, &secret_value).await {
                                Ok(receipt) => {
                                    info!("Solution submitted by thread {}: {:?}", thread_id, receipt.transaction_hash);
                                    let reward = extract_reward_from_receipt(&receipt).unwrap_or(0.0);
                                    let mut stats_guard = stats.lock().await;
                                    stats_guard.solutions += 1;
                                    stats_guard.successful_submissions += 1;
                                    stats_guard.total_rewards += reward;
                                    break 'mining_loop;
                                }
                                Err(e) => {
                                    error!("Thread {} failed to submit: {}", thread_id, e);
                                    let mut stats_guard = stats.lock().await;
                                    stats_guard.failed_submissions += 1;
                                    solution_found.store(false, Ordering::SeqCst);
                                    tokio::time::sleep(Duration::from_millis(100)).await;
                                }
                            }
                        }
                    }

                    nonce_base += nonce_increment_batch;
                    if throttle_factor < 1.0 {
                        tokio::time::sleep(Duration::from_millis(((1.0 - throttle_factor) * 100.0) as u64)).await;
                    }
                    tokio::task::yield_now().await;
                }
                Ok::<(), anyhow::Error>(())
            }));
        }

        futures::future::join_all(handles).await;
        if solution_found.load(Ordering::SeqCst) {
            blocks_mined_count += 1;
            info!("Solution found. Total solutions: {}", blocks_mined_count);
        } else {
            warn!("No solution found, fetching new challenge...");
            tokio::time::sleep(config.retry_delay).await;
        }
    }

    if let Some(channel_arc) = secure_channel {
        info!("Shutting down stats connection...");
        let mut channel_guard = channel_arc.lock().await;
        if let Err(e) = channel_guard.shutdown().await { warn!("Error shutting down stats: {}", e); }
    }
    info!("Miner shutting down");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_quantum_resistant_hash() {
        let signing_key = SigningKey::random(&mut OsRng);
        let entropy_hash = [0u8; 32];
        let secret = [0u8; 32];
        let signature: Signature = signing_key.sign(&entropy_hash);
        let block_timestamp = 1735689600u64;
        let result = quantum_resistant_hash(&signature, &entropy_hash, &secret, block_timestamp);
        assert_eq!(result.len(), 32, "Hash length mismatch");
    }

    #[tokio::test]
    #[ignore]
    async fn test_full_mining_cycle() {
        let mut config = MinerConfig::default();
        let key_file = NamedTempFile::new().unwrap();
        let dummy_key = SigningKey::random(&mut OsRng);
        fs::write(key_file.path(), hex::encode(dummy_key.to_bytes())).unwrap();
        config.threads = 1;
        config.batch_size = 2;
        config.exit_after_blocks = Some(1);
        config.rpc_url = "http://localhost:8545".to_string();
        config.contract_address = "0x5FbDB2315678afecb367f032d93F642f64180aa3".to_string();
        config.private_key_path = Some(key_file.path().to_str().unwrap().to_string());
        config.log_level = Level::DEBUG;
        config.retry_delay = Duration::from_millis(100);
        config.stats_interval = Duration::from_secs(1);
        // Requires Anvil node and deployed contract
        // Run: anvil --port 8545
        // Deploy contract with Forge and update address
        let result = main().await;
        assert!(result.is_ok(), "Mining cycle failed: {:?}", result.err());
    }

    #[test]
    fn benchmark_hybrid_hashing() {
        let test_data = vec![0u8; 128];
        let difficulty = U256::from(1000);
        let nonce = 0u64;
        for _ in 0..1000 { let _ = contract_hash_message(&test_data); let _ = pre_filter_nonce(nonce, &test_data, difficulty); }
        let keccak_start = Instant::now();
        for _ in 0..10000 { let _ = contract_hash_message(&test_data); }
        let keccak_time = keccak_start.elapsed();
        let blake_start = Instant::now();
        for _ in 0..10000 { let _ = pre_filter_nonce(nonce, &test_data, difficulty); }
        let blake_time = blake_start.elapsed();
        println!("Keccak256: {:?}", keccak_time);
        println!("BLAKE3: {:?}", blake_time);
        println!("Speed improvement: {:.2}x", keccak_time.as_secs_f64() / blake_time.as_secs_f64());
    }
}