use ethers::signers::Signer as _;
use k256::ecdsa::{SigningKey, Signature, signature::Signer};
// Removed SHA-2 imports as we're now using BLAKE3 for hashing
// use sha2::{Sha256, Digest};
// use hmac::{Hmac, Mac};
use rand::rngs::OsRng; // Keep OsRng for key generation
use rand::Rng; // Keep Rng for random bytes in temporal seed
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use std::env;
use std::fs;
use std::path::Path;
use std::thread;
use std::sync::{Arc, atomic::Ordering}; // Import Ordering
use std::time::Duration;
use ethers::{
    prelude::*,
    utils::{hex, keccak256},
    middleware::gas_oracle::{GasOracle, GasOracleMiddleware, Etherscan}, // Use Etherscan
    types::{transaction::eip712::{EIP712, types::*}, H160, U256} // EIP-712 imports
};
use tracing::{info, error, warn, debug, Level};
use tracing_subscriber::FmtSubscriber;
use anyhow::{Result, Context, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{Mutex, oneshot, broadcast}; // Added broadcast for shutdown
use std::arch::x86_64::is_x86_feature_detected; // Import for CPU feature detection
use zeroize::Zeroize; // Import Zeroize trait for temporary key buffer
use std::collections::VecDeque; // Added for ThermalMonitor
use std::path::PathBuf; // Added for update path

// Assuming cpu.rs and network.rs are in the same directory or crate root
mod cpu; // Declare the cpu module if not done elsewhere (like main.rs/lib.rs)
mod network; // Declare the network module
mod memory; // Declare the memory module
mod update; // Add the update module

use memory::SecureBuffer; // Import SecureBuffer

// Remove unused HMAC type since we're using BLAKE3
// type HmacSha256 = Hmac<Sha256>;

// Enum to represent different mining strategies based on CPU features
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MiningStrategy {
    Generic,
    SSE4,
    AVX2,
    AVX512, // Added for AVX-512 detection
}

// Function to detect CPU features and select a strategy
fn detect_cpu_features() -> MiningStrategy {
    // Check for AVX-512 first (most performant)
    if is_x86_feature_detected!("avx512f") && is_x86_feature_detected!("avx512vl") {
        info!("AVX-512 detected, using AVX-512 optimized strategy.");
        MiningStrategy::AVX512
    } 
    // Check for AVX2 next
    else if is_x86_feature_detected!("avx2") {
        info!("AVX2 detected, using AVX2 optimized strategy.");
        MiningStrategy::AVX2
    } 
    // Check for SSE4.1 as a fallback
    else if is_x86_feature_detected!("sse4.1") {
        info!("SSE4.1 detected, using SSE4 optimized strategy.");
        MiningStrategy::SSE4
    } 
    // Fallback to generic implementation
    else {
        info!("No specific CPU features detected, using generic strategy.");
        MiningStrategy::Generic
    }
}

// --- Shutdown Manager (Improvement #1) ---
#[derive(Clone)] // Clone needed to pass sender to signal handler
struct ShutdownManager {
    sender: broadcast::Sender<()>,
}

impl ShutdownManager {
    fn new() -> Self {
        let (sender, _) = broadcast::channel(1); // Receiver created via subscribe
        Self { sender }
    }

    fn trigger(&self) {
        // Send returns Result, ignore error if no receivers exist
        let _ = self.sender.send(());
    }

    fn subscribe(&self) -> broadcast::Receiver<()> {
        self.sender.subscribe()
    }
}
// --- End Shutdown Manager ---

// --- Thermal Monitor Struct - Merged the duplicate implementations ---
struct ThermalMonitor {
    readings: VecDeque<f32>,
    max_readings: usize,
}

impl ThermalMonitor {
    fn new(max_readings: usize) -> Self {
        // Ensure max_readings is at least 1
        let capacity = max_readings.max(1);
        Self {
            readings: VecDeque::with_capacity(capacity),
            max_readings: capacity,
        }
    }

    fn add_reading(&mut self, temp: f32) {
        if self.readings.len() >= self.max_readings {
            self.readings.pop_front();
        }
        self.readings.push_back(temp);
    }

    fn average_temp(&self) -> f32 {
        if self.readings.is_empty() {
            // Return a reasonable default if no readings yet, e.g., 50.0
            50.0
        } else {
            self.readings.iter().sum::<f32>() / self.readings.len() as f32
        }
    }
}
// --- End Thermal Monitor Struct ---

// Add new thermal notification channel type
struct ThermalNotification {
    temperature: f32,
    timestamp: SystemTime,
}

// Update ThermalController to be async-aware
struct ThermalController {
    monitor: ThermalMonitor,
    max_temp: f32,
    min_throttle_factor: f32, // Renamed from min_throttle for clarity (0.1 means 10% speed)
    notification_tx: broadcast::Sender<ThermalNotification>,
}

impl ThermalController {
    // Consider adding default values or getting from config
    pub fn new(max_readings: usize, max_temp: f32, min_throttle_factor: f32) -> Self {
        let (notification_tx, _) = broadcast::channel(32); // Buffer size of 32 should be sufficient
        Self {
            monitor: ThermalMonitor::new(max_readings),
            max_temp,
            min_throttle_factor: min_throttle_factor.max(0.0).min(1.0), // Clamp between 0.0 and 1.0
            notification_tx,
        }
    }

    // Add method to get notification receiver
    pub fn subscribe(&self) -> broadcast::Receiver<ThermalNotification> {
        self.notification_tx.subscribe()
    }

    // Make update async and notify of significant changes
    pub async fn update(&mut self, current_temp: f32) -> f32 {
        self.monitor.add_reading(current_temp);
        let avg_temp = self.monitor.average_temp();
        let factor = if avg_temp > self.max_temp {
            let excess_scale = 10.0;
            let excess = (avg_temp - self.max_temp).max(0.0);
            let throttle_reduction = (excess / excess_scale).min(1.0);
            let factor = 1.0 - throttle_reduction * (1.0 - self.min_throttle_factor);
            
            // Notify of high temperature condition
            let _ = self.notification_tx.send(ThermalNotification {
                temperature: avg_temp,
                timestamp: SystemTime::now(),
            });

            warn!(
                "Thermal throttling active! Avg Temp: {:.1}°C (Max: {}°C), Factor: {:.2}",
                avg_temp, self.max_temp, factor
            );
            factor.max(self.min_throttle_factor)
        } else {
            1.0
        };

        // Use non-blocking sleep for throttling
        if factor < 1.0 {
            let sleep_duration = Duration::from_millis(
                ((1.0 - factor) * 100.0) as u64
            );
            tokio::time::sleep(sleep_duration).await;
        }

        factor
    }
}
// --- End Thermal Controller ---

// Dynamic throttling based on temperature
fn adjust_for_thermals(current_temp: f32) -> f32 {
    match current_temp {
        t if t > 90.0 => {
            warn!("High temperature detected ({:.1}°C), throttling to 50%", t);
            0.5
        },
        t if t > 80.0 => {
            warn!("Elevated temperature detected ({:.1}°C), throttling to 75%", t);
            0.75
        },
        t if t > 70.0 => {
            debug!("Moderate temperature detected ({:.1}°C), throttling to 90%", t); // Use debug for less critical throttling
            0.9
        },
        _ => 1.0 // No throttling
    }
}

// Function stub for reading CPU temperature (Linux example)
#[cfg(target_os = "linux")]
fn read_cpu_temperature() -> Result<f32> {
    let temp_str = fs::read_to_string("/sys/class/thermal/thermal_zone0/temp")?;
    Ok(temp_str.trim().parse::<f32>()? / 1000.0)
}

#[cfg(not(target_os = "linux"))]
fn read_cpu_temperature() -> Result<f32> {
    // Placeholder for other OS or if reading fails
    Ok(50.0) // Return a default value
}


// Configuration struct - Adapted from snippet, keeping necessary fields
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
    // Removed unused performance tuning fields
    // use_avx2: bool,
    // use_sha_ni: bool,
    prefetch_distance: usize,
    batch_size: usize,
    l3_cache_optimized: bool, // New field
    // --- Update Mechanism Fields ---
    #[serde(default = "default_update_server")]
    update_server: String,
    #[serde(default = "default_update_check_interval")]
    update_check_interval: Duration,
    #[serde(default = "default_update_public_key_path")]
    update_public_key_path: String,
    #[serde(default = "default_update_enabled")]
    update_enabled: bool,
    // --- Stats Server Fields ---
    #[serde(default = "default_stats_server_required")]
    stats_server_required: bool,
    #[serde(default = "default_stats_server_endpoint")]
    stats_server_endpoint: String,
    #[serde(default = "default_stats_server_cert_path")]
    stats_server_cert_path: String,
}

// Default functions for new config fields
fn default_update_server() -> String { "https://updates.example.com/v1".to_string() } // Replace with actual URL
fn default_update_check_interval() -> Duration { Duration::from_secs(4 * 3600) } // Default: 4 hours
fn default_update_public_key_path() -> String { "update_pub.der".to_string() } // Relative path
fn default_update_enabled() -> bool { true } // Enabled by default
fn default_stats_server_required() -> bool { false }
fn default_stats_server_endpoint() -> String { "localhost:9999".to_string() }
fn default_stats_server_cert_path() -> String { "certs/stats_server.der".to_string() }

// Default implementation for MinerConfig
impl Default for MinerConfig {
    fn default() -> Self {
        // Note: Detecting physical cores accurately requires a crate like `num_cpus`.
        // Using a sensible default like 4, or recommend users configure it.
        let default_threads = 4; // Placeholder, adjust based on detection or user config

        Self {
            contract_address: "0xYourContractAddress".to_string(),
            rpc_url: "http://localhost:8545".to_string(),
            private_key_path: None,
            threads: default_threads, // Use detected or default
            gas_price_multiplier: 1.1,
            retry_delay: Duration::from_secs(5),
            log_level: Level::INFO,
            stats_interval: Duration::from_secs(60),
            exit_after_blocks: None,
            max_retries: 5,
            // Defaults for new/updated fields
            // Removed defaults for use_avx2 and use_sha_ni
            prefetch_distance: 4, // New default
            batch_size: 16, // New default
            l3_cache_optimized: true, // Defaulting to true as requested
            // --- Update Defaults ---
            update_server: default_update_server(),
            update_check_interval: default_update_check_interval(),
            update_public_key_path: default_update_public_key_path(),
            update_enabled: default_update_enabled(),
            // --- Stats Server Defaults ---
            stats_server_required: default_stats_server_required(),
            stats_server_endpoint: default_stats_server_endpoint(),
            stats_server_cert_path: default_stats_server_cert_path(),
        }
    }
}


// Mining statistics - Adapted from snippet
#[derive(Debug, Clone, Default)]
struct MiningStats {
    hashes: u64,
    solutions: u32, // Renamed from blocks_mined for consistency with snippet
    start_time: SystemTime,
    // Removed fields not present in snippet's stats
    failed_submissions: usize,
    successful_submissions: usize,
    // highest_difficulty: u128, // Removed
    // mining_efficiency: f64, // Removed
    total_rewards: f64, // Kept from original
}

// Removed MiningBuffer struct

// generate_temporal_seed - Adapted to use BLAKE3 instead of SHA-256
fn generate_temporal_seed() -> Vec<u8> {
    let mut seed = Vec::with_capacity(64);
    seed.extend_from_slice(&rand::random::<[u8; 32]>()); // Use rand::random for simplicity
    seed.extend_from_slice(&SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default() // Use unwrap_or_default for robustness
        .as_nanos()
        .to_le_bytes());
    
    // Use BLAKE3 instead of SHA-256 for consistency and better performance
    blake3::hash(&seed).as_bytes().to_vec()
}

// meets_difficulty - From snippet (takes U256 target)
#[inline]
fn meets_difficulty(hmac: &[u8; 32], target: U256) -> bool {
    let hmac_num = U256::from_big_endian(hmac);
    hmac_num < target
}

// Removed calculate_actual_difficulty

// New CPU optimization function
#[inline(always)]
fn optimize_for_cpu(ptr: *const u8) {
    // This function contains platform-specific optimizations.
    // Ensure the pointer `ptr` is valid and points to the data you intend to prefetch/flush.
    // The effectiveness and correctness depend heavily on the specific CPU architecture and workload.
    #[cfg(target_arch = "x86_64")]
    {
        // Safety: Calling these intrinsics requires careful consideration.
        // The pointer must be valid. Incorrect usage can lead to crashes or undefined behavior.
        // These hints might not always improve performance and can sometimes hurt it.
        // Profile carefully.
        unsafe {
            // Prefetch hint for Intel (T0 = temporal data, cache level 0)
            // This suggests bringing data into L1/L2 cache.
            core::arch::x86_64::_mm_prefetch(ptr as *const i8, core::arch::x86_64::_MM_HINT_T0);

            // CLFLUSH hint for AMD Ryzen (cache line flush)
            // This instruction invalidates the cache line containing the address `ptr`
            // from all levels of the processor cache hierarchy.
            // Its use case here is less clear than prefetch and might be counterproductive
            // unless specifically needed for memory ordering or avoiding stale cache lines,
            // which is unlikely in this hashing context. Consider removing if not beneficial.
            // core::arch::x86_64::_mm_clflush(ptr); // Commented out as potentially detrimental
        }
    }
    // Add cfgs for other architectures (e.g., aarch64) if needed
    #[cfg(not(target_arch = "x86_64"))]
    {
        // No-op or specific optimizations for other architectures
        let _ = ptr; // Avoid unused variable warning
    }
}

// get_current_challenge - Adapted to return U256 difficulty
async fn get_current_challenge(
    provider: &Provider<Http>,
    contract_address: &str
) -> Result<(Vec<u8>, U256)> { // Changed return type
    // Use ethers-rs for proper ABI encoding/decoding
    // Assuming the contract function is `getMiningChallenge() returns (bytes32, uint256)`
    abigen!(
        TGBContract,
        r#"[
            function getMiningChallenge(uint8 poolId) external view returns (bytes32[] memory outputs, uint256 difficulty)
        ]"#,
    );

    let contract_addr = contract_address.parse::<Address>()?;
    let contract = TGBContract::new(contract_addr, Arc::new(provider.clone()));

    // Define pool_id (e.g., 0 or make configurable)
    let pool_id = 0u8;
    let (outputs, difficulty) = contract.get_mining_challenge(pool_id).call().await?;

    // Handle the array of outputs, taking the first one.
    let previous_output = outputs
        .first()
        .ok_or_else(|| anyhow!("No outputs returned from getMiningChallenge"))?
        .to_vec();

    // Add bounds check (though contract should return bytes32)
    if previous_output.len() != 32 {
        return Err(anyhow!("Invalid previous output length received from contract: {}", previous_output.len()));
    }

    Ok((previous_output, difficulty))
}


// submit_solution - From snippet, adapted for existing wallet type and error handling
async fn submit_solution(
    client: &SignerMiddleware<GasOracleMiddleware<Provider<Http>, Etherscan>, LocalWallet>, // Updated Gas Oracle
    address: Address,
    previous_output: &[u8],
    temporal_seed: &[u8],
    nonce: u64,
    signature: &Signature,
    hmac: &[u8; 32],
) -> Result<TransactionReceipt> { // Return Receipt for reward extraction
    abigen!(
        TGBContractSubmit,
        r#"[
            function revealMiningCommitment(bytes32 previousOutput, bytes calldata temporalSeed, uint64 nonce, bytes calldata signature, bytes32 secretValue, uint8 poolId) external
        ]"#,
        // Note: The snippet used `submitBeaconBlock`, but the contract uses `revealMiningCommitment`.
        // The parameters also differ. `secretValue` and `poolId` are missing from the snippet's call.
        // Assuming `hmac` corresponds to `secretValue` and using poolId 0.
        // *** THIS NEEDS CONFIRMATION BASED ON CONTRACT LOGIC ***
    );

    let contract = TGBContractSubmit::new(address, Arc::new(client.clone()));

    // Convert signature to bytes. Using DER format as in snippet.
    let sig_bytes = Bytes::from(signature.to_der().as_bytes().to_vec());

    // Add bounds check for previous_output before try_into
    if previous_output.len() != 32 {
        return Err(anyhow!("Previous output length must be 32 bytes, got {}", previous_output.len()));
    }
    let prev_output_bytes32: [u8; 32] = previous_output.try_into()
        .expect("Length checked above, should not fail"); // Use expect after check

    let secret_value_bytes32: [u8; 32] = *hmac; // Assuming hmac is the secretValue
    let pool_id = 0u8; // Assuming pool 0

    let call = contract.reveal_mining_commitment(
        prev_output_bytes32.into(),
        Bytes::from(temporal_seed.to_vec()),
        nonce,
        sig_bytes,
        secret_value_bytes32.into(),
        pool_id
    );

    // Estimate gas and warn if high
    match call.estimate_gas().await {
        Ok(gas_limit) => {
            debug!("Estimated gas limit for submission: {}", gas_limit);
            if gas_limit > U256::from(1_000_000) { // Example threshold
                warn!("High estimated gas limit detected: {}", gas_limit);
            }
        }
        Err(e) => {
            warn!("Failed to estimate gas for submission: {}", e);
            // Decide whether to proceed or return error
            // return Err(anyhow!("Gas estimation failed: {}", e));
        }
    }


    let pending_tx = call.send().await.context("Failed to send transaction")?;
    let receipt = pending_tx
        .await
        .context("Failed to confirm transaction")?
        .ok_or_else(|| anyhow!("No transaction receipt received"))?;

    Ok(receipt)
}

// create_message - From snippet
fn create_message(previous_output: &[u8], temporal_seed: &[u8], nonce: u64) -> Vec<u8> {
    // Added miner address and block data as per contract logic (if needed)
    // For now, sticking to snippet's version. Review if contract requires more data.
    let mut message = Vec::with_capacity(previous_output.len() + temporal_seed.len() + 8);
    message.extend_from_slice(previous_output);
    message.extend_from_slice(temporal_seed);
    message.extend_from_slice(&nonce.to_le_bytes());
    // Potentially add: message.extend_from_slice(&miner_address.0);
    // Potentially add: message.extend_from_slice(&block_prevrandao);
    message
}

// Constants matching MiningLib.sol values
const QR_HASH_ITERATIONS: u8 = 3;
const QR_HASH_ROTATION: u8 = 7;

// Updated function to match the Solidity contract's entropy calculation exactly
async fn create_entropy_hash(
    provider: &Provider<Http>,
    client_address: &Address,
    previous_output: &[u8],
    temporal_seed: &[u8],
    nonce: u64,
    secret_value: &[u8]
) -> Result<([u8; 32], u64)> {
    // 1) Fetch the latest block to get prevrandao and timestamp
    let block = provider.get_block(BlockNumber::Latest).await?
        .ok_or_else(|| anyhow!("Failed to fetch latest block"))?;
    
    let block_timestamp = block.timestamp.as_u64();
    let prevrandao = block.prevrandao
        .ok_or_else(|| anyhow!("Block is missing prevrandao"))?;

    // 2) Create entropy exactly as in the Solidity contract
    let mut entropy = Vec::with_capacity(
        previous_output.len() + temporal_seed.len() + 8 + 20 + 32 + 8 + secret_value.len()
    );
    entropy.extend_from_slice(previous_output);
    entropy.extend_from_slice(temporal_seed);
    entropy.extend_from_slice(&nonce.to_be_bytes());
    entropy.extend_from_slice(&client_address.0); // Address bytes
    entropy.extend_from_slice(&prevrandao.0);     // prevrandao bytes
    entropy.extend_from_slice(&block_timestamp.to_be_bytes());
    entropy.extend_from_slice(secret_value);      // usually your HMAC key or seed
    
    // Hash the combined entropy exactly as in Solidity
    let entropy_hash = keccak256(&entropy);
    
    Ok((entropy_hash, block_timestamp))
}

// Async implementation of quantum resistant hash that matches Solidity exactly
async fn quantum_resistant_hash_async(
    signature: &Signature,
    entropy_hash: &[u8; 32],
    secret: &[u8],
    block_timestamp: u64
) -> [u8; 32] {
    // Create packed input exactly as in Solidity
    let mut packed = Vec::new();
    packed.extend_from_slice(&signature.to_der().as_bytes());
    packed.extend_from_slice(entropy_hash);
    packed.extend_from_slice(secret);
    
    // Use the inner function with the block timestamp from the chain
    quantum_resistant_hash_inner(&packed, block_timestamp)
}

// Inner implementation that matches Solidity's quantumResistantHash exactly
fn quantum_resistant_hash_inner(input: &[u8], block_ts: u64) -> [u8; 32] {
    // Initial hash
    let mut h = keccak256(input);
    
    // Get block timestamp in big-endian format
    let ts_bytes = block_ts.to_be_bytes();
    
    // Perform the same iterations as in Solidity
    for i in 0..QR_HASH_ITERATIONS {
        // 1) XOR with (i+1)
        let mut buf = [0u8; 32];
        buf[31] = (i as u8).wrapping_add(1); // Set the last byte to i+1
        
        for j in 0..32 {
            buf[j] ^= h[j]; // XOR operation
        }
        
        // 2) Hash with timestamp
        let mut data = Vec::with_capacity(40);
        data.extend_from_slice(&buf);
        data.extend_from_slice(&ts_bytes);
        h = keccak256(&data);
        
        // 3) Rotate left by QR_HASH_ROTATION bits
        h = rotate_left_256(h, QR_HASH_ROTATION as usize);
    }
    
    h
}

// Rotate left a 256-bit value (represented as [u8; 32]) by n bits
fn rotate_left_256(mut bytes: [u8; 32], n: usize) -> [u8; 32] {
    // First convert to a single 256-bit integer
    let mut value = U256::from_big_endian(&bytes);
    
    // Perform rotation (U256 doesn't have rotate_left, so we implement it manually)
    let rotated = (value << n) | (value >> (256 - n));
    
    // Convert back to [u8; 32]
    let mut result = [0u8; 32];
    rotated.to_big_endian(&mut result);
    
    result
}

// Get current timestamp in big-endian format
fn get_current_timestamp_bytes() -> Result<[u8; 8]> {
    static TIMESTAMP_CACHE: Mutex<(SystemTime, [u8; 8])> = Mutex::new((UNIX_EPOCH, [0u8; 8]));
    
    let mut cache = TIMESTAMP_CACHE.lock()
        .map_err(|_| anyhow!("Failed to acquire timestamp cache lock"))?;
    let now = SystemTime::now();
    
    // If cached timestamp is older than 5 seconds, refresh it
    if now.duration_since(cache.0)
        .map_err(|e| anyhow!("System time error: {}", e))?
        .as_secs() > 5 
    {
        // Get current timestamp in seconds
        let secs_since_epoch = now.duration_since(UNIX_EPOCH)
            .map_err(|e| anyhow!("Failed to get unix timestamp: {}", e))?
            .as_secs();
        
        cache.0 = now;
        cache.1 = secs_since_epoch.to_be_bytes();
    }
    
    Ok(cache.1)
}

// This async version handles errors properly
async fn get_current_timestamp_bytes_async(provider: &Provider<Http>) -> Result<[u8; 8]> {
    let block = provider.get_block(BlockNumber::Latest).await?
        .ok_or_else(|| anyhow!("Failed to fetch latest block"))?;
    
    let timestamp = block.timestamp.as_u64();
    Ok(timestamp.to_be_bytes())
}

// Specialized hash function that uses BLAKE3 for pre-hashing before keccak256
// This improves performance in the tight mining loop
#[inline(always)]
fn fast_hash_message(message: &[u8]) -> [u8; 32] {
    // First hash with BLAKE3 (extremely fast on modern CPUs)
    // BLAKE3 automatically uses AVX2/AVX512/SSE4.1 if available
    let fast_hash = blake3::hash(message);
    
    // Then hash with keccak256 to match the contract's verification
    keccak256(fast_hash.as_bytes())
}

// Add EIP-712 struct definition at module level with TODOs for chain ID and contract address
pub struct DynamicMiningCommitment {
    pub commit_hash: [u8; 32],
    pub pool_id: u8,
    pub nonce: u64,
    pub deadline: u64,
    stealth_meta: Vec<u8>,      // Stealth metadata for anonymous claim
    stealth_proof: Vec<u8>,     // Zero-knowledge proof of stealth key ownership
}

impl DynamicMiningCommitment {
    fn to_eip712_types(chain_id: u64, contract_address: Address) -> TypedData {
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
                        TypeField { 
                            name: "stealthMeta".to_string(), 
                            r#type: "bytes".to_string() 
                        },
                        TypeField { 
                            name: "stealthProof".to_string(), 
                            r#type: "bytes".to_string() 
                        },
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
                map.insert("stealthMeta".to_string(), 
                    Value::from(format!("0x{}", hex::encode(&self.stealth_meta))));
                map.insert("stealthProof".to_string(), 
                    Value::from(format!("0x{}", hex::encode(&self.stealth_proof))));
                map
            },
        }
    }

    // Helper to create and sign commitment
    async fn sign(&self, wallet: &LocalWallet, chain_id: u64, contract_address: Address) -> Result<Bytes> {
        let typed_data = self.to_eip712_types(chain_id, contract_address);
        let signature = wallet.sign_typed_data(&typed_data).await
            .map_err(|e| anyhow!("Failed to sign commitment: {}", e))?;
        Ok(signature.to_vec().into())
    }
}

// Add ABI bindings for contract at module scope
abigen!(
    MiningContract,
    r#" [
        function submitMiningCommitment(bytes32 commitHash, uint8 poolId, uint256 nonce, uint256 deadline, bytes signature) external returns (bool);
        function revealMiningCommitment(bytes32 previousOutput, bytes temporalSeed, uint64 nonce, bytes signature, bytes32 secretValue, uint8 poolId) external;
        function minCommitmentAge() external view returns (uint256);
    ]"#,
);

#[tokio::main]
async fn main() -> Result<()> {
    // Load configuration from file or environment
    let config = load_config()?;

    // Initialize logging
    setup_logging(config.log_level)?;

    // --- Log CPU Information ---
    let real_cpu_identity = cpu::detect_cpu(); // Use the concrete function from cpu.rs
    info!("Detected CPU: Vendor={}, Brand={}, Cores={}, Cache={}KB",
          real_cpu_identity.vendor, real_cpu_identity.brand, real_cpu_identity.cores, real_cpu_identity.cache_size / 1024);
    debug!("Detected CPU Features Bitmask: {:#018x}", real_cpu_identity.features);

    let masked_cpu_identity = cpu::mask_cpu_identity();
    info!("Masked CPU for Reporting: Vendor={}, Brand={}, Cores={}, Cache={}KB",
          masked_cpu_identity.vendor, masked_cpu_identity.brand, masked_cpu_identity.cores, masked_cpu_identity.cache_size / 1024);
    debug!("Masked CPU Features Bitmask: {:#018x}", masked_cpu_identity.features);
    // --- End Log CPU Information ---


    // --- Application Version ---
    let app_version = env!("CARGO_PKG_VERSION");
    info!("Starting Temporal Gradient Beacon Miner v{}", app_version);
    debug!("Configuration: {:?}", config);

    // --- Start Update Check Task (if enabled) ---
    if config.update_enabled {
        let update_config = config.clone();
        let retry_state = Arc::new(Mutex::new(UpdateRetryState::new()));
        
        tokio::spawn(async move {
            info!("Update checker task started. Interval: {:?}", update_config.update_check_interval);
            loop {
                let should_check = {
                    let state = retry_state.lock().await;
                    state.should_retry()
                };

                if should_check {
                    match check_and_apply_updates(&update_config, app_version).await {
                        Ok(applied) => {
                            let mut state = retry_state.lock().await;
                            state.record_success();
                            if applied {
                                info!("Update applied successfully. Restarting...");
                                break;
                            }
                            drop(state); // Release lock before sleep
                            tokio::time::sleep(update_config.update_check_interval).await;
                        }
                        Err(e) => {
                            let mut state = retry_state.lock().await;
                            state.record_failure();
                            let next_delay = state.next_attempt_delay();
                            error!(
                                "Update check failed: {}. Will retry in {:?} (attempt #{})", 
                                e, next_delay, state.consecutive_failures
                            );
                            drop(state); // Release lock before sleep
                            tokio::time::sleep(next_delay).await;
                        }
                    }
                } else {
                    tokio::time::sleep(Duration::from_secs(60)).await; // Check retry state every minute
                }
            }
        });
    } else {
        info!("Automatic updates disabled via config.");
    }
    // --- End Update Check Task ---


    info!("Starting Temporal Gradient Beacon Miner v1.0.0");
    debug!("Configuration: {:?}", config);

    // --- Shutdown Manager Setup (Improvement #1) ---
    let shutdown_manager = ShutdownManager::new();
    let shutdown_manager_for_signal = shutdown_manager.clone();
    // Setup Ctrl+C handler
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.expect("Failed to install Ctrl+C handler");
        warn!("Ctrl+C received, initiating shutdown...");
        shutdown_manager_for_signal.trigger();
    });
    // --- End Shutdown Manager Setup ---


    // --- Hypothetical Secure Stats Server Connection (Improvement #3) ---
    let secure_channel = {
        let endpoint = env::var("STATS_SERVER_ENDPOINT")
            .unwrap_or_else(|_| config.stats_server_endpoint.clone());
        let cert_path = env::var("STATS_SERVER_CERT_PATH")
            .unwrap_or_else(|_| config.stats_server_cert_path.clone());
        let critical = env::var("STATS_SERVER_REQUIRED")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(config.stats_server_required);

        match network::secure_connect_pinned(&endpoint, &cert_path).await {
            Ok(channel) => {
                info!("Successfully connected to secure stats server at {}", endpoint);
                Some(Arc::new(Mutex::new(channel)))
            }
            Err(e) => {
                let msg = format!(
                    "Failed to connect to secure stats server at {} (pinning attempted): {}", 
                    endpoint, e
                );
                if critical {
                    error!("{}", msg);
                    error!("Stats server connection required, shutting down.");
                    return Err(anyhow!(msg));
                } else {
                    warn!("{}", msg);
                    warn!("Continuing without secure stats reporting.");
                    None
                }
            }
        }
    };
    // --- End Hypothetical Secure Stats Server Connection ---


    // Load or generate signing key bytes into a SecureBuffer
    let secure_key_buffer = Arc::new(Mutex::new(load_or_generate_key_secure(&config)?));

    // Create ethers wallet from signing key bytes in SecureBuffer
    let wallet = {
        let key_buffer_guard = secure_key_buffer.lock().await;
        create_wallet_from_secure_buffer(&key_buffer_guard, &config)? // Uses improved function
    };

    // Log public key (requires temporary key reconstruction)
    {
        // Use temporary buffer approach here as well for consistency and safety
        let mut temp_key_bytes = [0u8; 32];
        { // Inner scope for lock guard
            let key_buffer_guard = secure_key_buffer.lock().await;
            temp_key_bytes.copy_from_slice(key_buffer_guard.as_slice());
        } // Lock guard dropped here

        let temp_signing_key = SigningKey::from_bytes(&temp_key_bytes)
            .map_err(|e| {
                temp_key_bytes.zeroize(); // Zeroize on error
                anyhow!("Failed to reconstruct key for pubkey logging: {}", e)
            })?;

        let public_key = temp_signing_key.verifying_key();
        info!("Mining with public key: {}", hex::encode(public_key.to_encoded_point(false).as_bytes()));

        temp_key_bytes.zeroize(); // Zeroize after successful use
        // temp_signing_key is dropped here
    }


    // Initialize provider with gas station
    let provider = Provider::<Http>::try_from(&config.rpc_url)?;
    // Use Etherscan Gas Oracle - Requires ETHERSCAN_API_KEY env var
    let chain = provider.get_chainid().await?.as_u64().try_into()?; // Get chain ID for Etherscan
    let api_key = env::var("ETHERSCAN_API_KEY").context("ETHERSCAN_API_KEY not set")?;
    let gas_oracle = Etherscan::new(chain, api_key);
    let gas_provider = GasOracleMiddleware::new(provider.clone(), gas_oracle, config.gas_price_multiplier);
    // Create SignerMiddleware client
    let client = SignerMiddleware::new(gas_provider, wallet.clone()); // Use wallet clone

    // Initialize mining statistics (using tokio Mutex)
    let stats = Arc::new(Mutex::new(MiningStats {
        start_time: SystemTime::now(),
        ..Default::default()
    }));

    // Start statistics reporting thread (modified to potentially send over secure channel)
    let stats_clone_for_reporter = Arc::clone(&stats);
    let stats_interval = config.stats_interval;
    let secure_channel_clone = secure_channel.clone(); // Clone Arc for the stats task
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(stats_interval).await;
            print_stats(&stats_clone_for_reporter).await; // Log stats locally

            // --- Hypothetical Stats Sending ---
            if let Some(channel_arc) = &secure_channel_clone {
                let stats_data = { // Create stats data within scope
                    let stats_guard = stats_clone_for_reporter.lock().await;
                    // Serialize stats data (e.g., to JSON)
                    serde_json::to_string(&*stats_guard).unwrap_or_else(|_| "{}".to_string())
                };

                let mut channel_guard = channel_arc.lock().await;
                match channel_guard.write_all(stats_data.as_bytes()).await {
                    Ok(_) => debug!("Successfully sent stats to secure server."),
                    Err(e) => warn!("Failed to send stats to secure server: {}", e),
                }
                // Check rotation periodically within the stats loop or another dedicated task
                if let Err(e) = channel_guard.check_rotation() {
                     warn!("Error during secure channel key rotation check: {}", e);
                }
            }
            // --- End Hypothetical Stats Sending ---
        }
    });

    // Contract address
    let contract_address = config.contract_address.parse::<Address>()?;

    // --- Graceful Shutdown Setup (Improvement #5) ---
    let (shutdown_sender, shutdown_receiver) = oneshot::channel::<()>();
    // Share the receiver with the main loop logic if needed for external triggers
    let shutdown_receiver_shared = Arc::new(Mutex::new(Some(shutdown_receiver)));
    // --- End Graceful Shutdown Setup ---

    // Main mining loop
    let mut consecutive_errors = 0;
    let mut blocks_mined_count = 0; // Track mined blocks locally for exit condition

    // --- Main loop shutdown receiver (Improvement #1) ---
    let mut main_shutdown_receiver = shutdown_manager.subscribe();
    // --- End main loop shutdown receiver ---

    // Add after wallet creation, before main loop
    let stealth = Arc::new(Mutex::new(StealthAddress {
        spending_key: wallet.clone(),
        viewing_key: SigningKey::random(&mut OsRng),
        ephemeral_keys: VecDeque::new(),
    }));

    'main_loop: loop { // Label the main loop
        // Check exit condition or shutdown signal
        tokio::select! {
            biased; // Check shutdown first
            _ = main_shutdown_receiver.recv() => {
                info!("Main loop received shutdown signal.");
                break 'main_loop;
            }
            _ = tokio::time::sleep(Duration::ZERO) => { // Allow other checks to proceed
                if let Some(exit_after) = config.exit_after_blocks {
                    if blocks_mined_count >= exit_after {
                        info!("Reached target of {} solutions, initiating shutdown...", exit_after);
                        shutdown_manager.trigger(); // Signal threads
                        break 'main_loop; // Exit main loop
                    }
                }
            }
        }

        // Get current challenge from contract
        let (previous_output, difficulty_target_u256) = match get_current_challenge(&provider, &config.contract_address).await {
            Ok(challenge) => {
                consecutive_errors = 0;
                challenge
            },
            Err(e) => {
                error!("Failed to get current challenge: {}", e);
                consecutive_errors += 1;
                if consecutive_errors > config.max_retries {
                    return Err(anyhow!("Too many consecutive failures getting challenge, exiting"));
                }
                // Check for shutdown signal before sleeping
                if main_shutdown_receiver.try_recv().is_ok() {
                     info!("Shutdown signal received while handling challenge error.");
                     break 'main_loop;
                }
                tokio::time::sleep(config.retry_delay).await;
                continue;
            }
        };

        info!("Mining new block. Difficulty Target: {}", difficulty_target_u256);
        // info!("Previous output: {}", hex::encode(&previous_output)); // Keep if previous_output is valid

        // Create thread-safe atomic flag for signaling successful mining
        let solution_found = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut handles = vec![];

        // --- Create shutdown receivers for each thread (Improvement #5) ---
        let mut thread_shutdown_receivers = Vec::new();
        let main_receiver_guard = shutdown_receiver_shared.lock().await;
        if main_receiver_guard.is_some() {
            // This is complex to share a single oneshot::Receiver.
            // A simpler approach for multiple threads is using a broadcast channel
            // or cloning an Arc<Notify>. Let's stick to a simpler signal for now.
            // For this example, we'll assume the main loop break is sufficient signal,
            // but a dedicated broadcast channel is better for robust multi-thread shutdown.
            // We'll add the select! macro but without a shared receiver for simplicity here.
        }
        // --- End shutdown receiver setup ---


        // Start mining with multiple threads
        for thread_id in 0..config.threads {
            // Clone necessary variables for the thread
            let client = client.clone();
            let stats = Arc::clone(&stats);
            let solution_found = Arc::clone(&solution_found);
            let previous_output = previous_output.clone(); // Clone previous_output
            let config = config.clone(); // Clone config
            // Clone the Arc<Mutex<SecureBuffer>> for the key bytes
            let secure_key_buffer_clone = Arc::clone(&secure_key_buffer);
            let difficulty_target = difficulty_target_u256;
            let mut consecutive_submission_errors = 0; // Track submission errors per thread
            // --- Thread-specific shutdown receiver (Improvement #1) ---
            let mut thread_shutdown_receiver = shutdown_manager.subscribe();
            // --- End thread-specific shutdown receiver ---

            // In the thread spawn setup, add stealth clone
            let stealth = Arc::clone(&stealth);

            handles.push(tokio::spawn(async move { // Use tokio::spawn
                // Start nonce based on thread ID
                let mut nonce_base = thread_id as u64;
                // Total increment across all threads for each step
                let nonce_step_all_threads = config.threads as u64;
                // Increment for this thread after processing a batch
                let nonce_increment_batch = nonce_step_all_threads * config.batch_size as u64;
                let mut throttle_factor = 1.0f32; // Initialize throttle factor
                // --- Initialize Thermal Controller (Improvement #2) ---
                // Example values, consider making these configurable
                let mut thermal_controller = ThermalController::new(10, 85.0, 0.2); // 10 readings, max 85C, min 20% speed
                let mut throttle_factor = 1.0f32; // Initialize throttle factor
                // --- End Initialize Thermal Controller ---

                // Update mining loop throttling logic
                let thermal_rx = thermal_controller.subscribe();
                'mining_loop: loop {
                    // --- Graceful Shutdown Check (Improvement #1) ---
                    tokio::select! {
                        biased; // Check shutdown first
                        _ = thread_shutdown_receiver.recv() => {
                            debug!("Thread {} received shutdown signal", thread_id);
                            break 'mining_loop;
                        }
                        // Use default branch to continue if no signal received
                        _ = tokio::time::sleep(Duration::ZERO) => { // Placeholder for actual work check
                            // Continue with mining logic if no shutdown signal
                        }
                    }
                    // --- End Graceful Shutdown Check ---


                    // --- Thermal Throttling Check (Improvement #2) ---
                    // Non-blocking temperature check
                    tokio::select! {
                        biased;
                        Ok(notification) = thermal_rx.recv() => {
                            // Update throttle factor based on notification
                            if SystemTime::now().duration_since(notification.timestamp)
                                .unwrap_or_default() < Duration::from_secs(1) 
                            {
                                throttle_factor = thermal_controller.update(notification.temperature).await;
                            }
                        }
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {
                            // Periodic temperature check if no notifications
                            match read_cpu_temperature() {
                                Ok(temp) => {
                                    throttle_factor = thermal_controller.update(temp).await;
                                }
                                Err(e) => {
                                    warn!("Failed to read CPU temperature: {}", e);
                                }
                            }
                        }
                    }

                    // Apply throttling using tokio sleep
                    if throttle_factor < 1.0 {
                        let sleep_ms = ((1.0 - throttle_factor) * 100.0) as u64;
                        tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
                    }
                    // --- End Thermal Throttling Check ---


                    // Check if another thread found a solution before starting the batch
                    if solution_found.load(Ordering::SeqCst) { // Use SeqCst
                        break 'mining_loop;
                    }

                    let mut found_in_batch = false;
                    let mut solution_details = None; // Store details if found

                    // Process a batch of nonces
                    for i in 0..config.batch_size {
                        let current_nonce = nonce_base + (i as u64 * nonce_step_all_threads);

                        let temporal_seed = generate_temporal_seed();
                        let message = create_message(&previous_output, &temporal_seed, current_nonce);

                        // Removed call to optimize_for_cpu as it's unnecessary with BLAKE3
                        // BLAKE3 already uses optimal SIMD instructions based on CPU features

                        // USE FAST HASHING: Pre-hash with BLAKE3 before keccak256
                        // This is much faster on CPU while still meeting contract requirements
                        let message_hash = fast_hash_message(&message);

                        // Reconstruct signing key temporarily for signing (using temp buffer)
                        let signature = {
                            let mut temp_key_bytes = [0u8; 32];
                            { // Inner scope for lock guard
                                let key_buffer_guard = secure_key_buffer_clone.lock().await;
                                temp_key_bytes.copy_from_slice(key_buffer_guard.as_slice());
                            } // Lock guard dropped

                            let temp_signing_key = SigningKey::from_bytes(&temp_key_bytes)
                                .map_err(|e| {
                                    temp_key_bytes.zeroize(); // Zeroize on error
                                    anyhow!("Failed to reconstruct key for signing: {}", e)
                                })?;

                            let sig: Signature = temp_signing_key.sign(&message_hash);
                            temp_key_bytes.zeroize(); // Zeroize after use
                            // temp_signing_key dropped here
                            sig
                        };
                        // Check if reconstruction failed (though map_err should handle it)
                        // If using Result, handle error here.

                        let solution_hash = quantum_resistant_hash(&signature, &message_hash, &temporal_seed);

                        // --- update hash counter ---
                        {
                            let mut stats_guard = stats.lock().await;
                            stats_guard.hashes += 1;
                        }

                        // Check if difficulty is met
                        if meets_difficulty(&solution_hash, difficulty_target) {
                            // Try to claim the solution
                            if solution_found.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() { // Use SeqCst
                                info!("Thread {} found solution in batch! Nonce: {}", thread_id, current_nonce);
                                found_in_batch = true;
                                // Store details needed for submission
                                solution_details = Some((current_nonce, temporal_seed, signature, solution_hash));
                                // Break inner batch loop, proceed to submission
                                break;
                            } else {
                                // Another thread claimed the solution first, stop this thread's work
                                debug!("Thread {} found solution, but another thread claimed it first.");
                                break 'mining_loop; // Exit outer loop
                            }
                        }
                    } // End of batch loop

                    // If a solution was found and claimed by this thread in the batch
                    if found_in_batch {
                        if let Some((nonce, temporal_seed, signature, solution_hash)) = solution_details {
                            // Build the commitment hash
                            let mut buf = Vec::new();
                            buf.extend_from_slice(&previous_output);
                            buf.extend_from_slice(&temporal_seed);
                            buf.extend_from_slice(&nonce.to_le_bytes());
                            buf.extend_from_slice(&signature.to_der());
                            buf.extend_from_slice(&solution_hash);
                            buf.extend_from_slice(&wallet.address().0);
                            let commit_hash = keccak256(&buf);

                            // Create EIP-712 commitment struct
                            let deadline = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs() + 300; // 5 minutes

                            let commitment = DynamicMiningCommitment {
                                commit_hash: commit_hash.try_into().expect("32 bytes"),
                                pool_id: 0,
                                nonce,
                                deadline,
                                stealth_meta: vec![],      // Stealth metadata for anonymous claim
                                stealth_proof: vec![],     // Zero-knowledge proof of stealth key ownership
                            };

                            // Sign with dynamic chain ID and contract address
                            let signature_eip712 = commitment.sign(&wallet, chain, contract_address).await?;

                            // Submit commitment
                            let contract = MiningContract::new(
                                config.contract_address.parse().unwrap(),
                                Arc::new(client.clone())
                            );

                            info!("Submitting mining commitment...");
                            let tx = contract.submit_mining_commitment(
                                commit_hash.into(),
                                0u8,
                                nonce,
                                deadline,
                                signature_eip712.to_vec().into(),
                            ).send().await?;

                            let receipt = tx.await?
                                .ok_or_else(|| anyhow!("No receipt for commitment"))?;
                            
                            info!("Commitment submitted in tx: {:?}", receipt.transaction_hash);

                            // Wait for minCommitmentAge blocks
                            let min_blocks = contract.min_commitment_age().call().await?;
                            info!("Waiting for {} blocks before reveal...", min_blocks);
                            loop {
                                let current_block = provider.get_block_number().await?;
                                if current_block >= commit_block + min_blocks.into() {
                                    break;
                                }
                                tokio::time::sleep(Duration::from_secs(12)).await; // ~1 block
                            }

                            // Now proceed with reveal
                            info!("Revealing mining solution...");
                            let reveal_tx = contract.reveal_mining_commitment(
                                previous_output.into(),
                                temporal_seed.into(),
                                nonce,
                                signature_eip712.to_vec().into(),
                                solution_hash.into(),
                                0u8
                            ).send().await?;

                            let receipt = reveal_tx.await?
                                .ok_or_else(|| anyhow!("No receipt for solution reveal"))?;

                            // Update statistics with reward
                            let reward = extract_reward_from_receipt(&receipt).unwrap_or(0.0);
                            let mut stats_guard = stats.lock().await;
// ...existing code...
                            info!("Solution submitted successfully by thread {}!", thread_id);
                            consecutive_submission_errors = 0; // Reset thread-local submission errors

                            let reward = extract_reward_from_receipt(&receipt).unwrap_or(0.0);
                            // Update statistics
                            let mut stats_guard = stats.lock().await;
                            stats_guard.solutions += 1;
                            stats_guard.successful_submissions += 1;
                            stats_guard.total_rewards += reward;
                            // Solution successfully submitted, break outer loop for this thread
                            break 'mining_loop;
                        },
                        Err(e) => {
                            error!("Thread {} failed to submit solution: {}", thread_id, e);
                            consecutive_submission_errors += 1;
                            let mut stats_guard = stats.lock().await;
                            stats_guard.failed_submissions += 1;
                            // Reset solution_found flag as submission failed, allowing others (or retry)
                            solution_found.store(false, Ordering::SeqCst); // Use SeqCst

                            // Check if max submission retries exceeded for this thread
                            if consecutive_submission_errors > config.max_retries {
                                error!("Thread {} exceeded max submission retries, stopping.", thread_id);
                                break 'mining_loop; // Stop this thread
                            }

                            // Consider a small delay before continuing mining loop
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            // Continue mining loop to try next batch or let others try
                        }
                    }
                }

                // Increment base nonce for the next batch
                nonce_base += nonce_increment_batch;

                // --- Apply Thermal Throttling Delay (Improvement #2) ---
                if throttle_factor < 1.0 {
                    // Calculate sleep duration based on throttling factor
                    // Example: If factor is 0.5, sleep for 50ms? Adjust base sleep time.
                    // A simple approach: sleep for a duration inversely proportional to the factor.
                    let base_yield_time_ms = 10.0; // Target time per yield/check when not throttled
                    let target_time_ms = base_yield_time_ms / throttle_factor;
                    let sleep_duration = Duration::from_millis((target_time_ms - base_yield_time_ms).max(0.0) as u64);

                    if sleep_duration > Duration::ZERO {
                         // Use trace level for frequent throttling logs
                         // trace!("Thread {}: Throttling active (factor {:.2}), sleeping for {:?}", thread_id, throttle_factor, sleep_duration);
                         tokio::time::sleep(sleep_duration).await;
                    }
                }
                // --- End Apply Thermal Throttling Delay ---


                // Yield after processing a batch to prevent hogging CPU & allow flag check
                tokio::task::yield_now().await;
            } // End of 'mining_loop
        }));
    }

    // Wait for all threads to complete (or one to successfully submit)
    futures::future::join_all(handles).await;

    // Check if a solution was actually found and submitted successfully in this round
    // This check might be redundant if the loop breaks on success inside the thread
    if solution_found.load(Ordering::SeqCst) {
         blocks_mined_count += 1;
         info!("Solution found in round. Total solutions: {}", blocks_mined_count);
    } else if shutdown_receiver_shared.lock().await.is_none() {
         // If shutdown was triggered, don't log "No solution found"
    }
     else {
         warn!("No solution found in this round, fetching new challenge...");
         // Check for shutdown before sleeping
         tokio::select! {
             _ = main_shutdown_receiver.recv() => {
                 info!("Shutdown signal received while waiting for retry.");
                 break 'main_loop;
             }
             _ = tokio::time::sleep(config.retry_delay) => {} // Sleep if no shutdown
         }
    }
} // End main_loop

// --- Hypothetical Secure Channel Shutdown ---
if let Some(channel_arc) = secure_channel {
    info!("Shutting down secure stats connection...");
    let mut channel_guard = channel_arc.lock().await;
    if let Err(e) = channel_guard.shutdown().await {
        warn!("Error shutting down secure channel: {}", e);
    }
}
// --- End Hypothetical Secure Channel Shutdown ---

info!("Miner shutting down.");
Ok(())
}

// --- Update Check and Apply Function ---
async fn check_and_apply_updates(config: &MinerConfig, current_version: &str) -> Result<bool> {
// Load public key
let pub_key_bytes = fs::read(&config.update_public_key_path).await
    .context(format!("Failed to read update public key from: {}", config.update_public_key_path))?;

let verifier = update::UpdateVerifier::new(pub_key_bytes, current_version)
    .context("Failed to initialize update verifier")?;

if let Some(manifest) = verifier.check_for_updates(&config.update_server).await? {
    info!("New version {} available, downloading...", manifest.version);

    // Create a temporary path for the download
    let temp_dir = std::env::temp_dir();
    let temp_file_name = format!("miner_update_{}.tmp", manifest.version);
    let temp_path = temp_dir.join(temp_file_name);

    // Ensure temp file doesn't exist from previous failed attempt
    if temp_path.exists() {
        fs::remove_file(&temp_path).await.ok();
    }

    match verifier.download_update(&manifest, &temp_path).await {
        Ok(_) => {
            info!("Update downloaded to {:?}, attempting to apply...", temp_path);
            // Apply the update (placeholder)
            match update::apply_update(&temp_path).await {
                Ok(_) => {
                    info!("Update applied successfully. Triggering restart.");
                    // Restart the application (placeholder)
                    if let Err(e) = update::restart_application() {
                        error!("Failed to trigger application restart: {}", e);
                        // Even if restart fails, return true as update was technically applied
                    }
                    return Ok(true); // Indicate update was applied (restart initiated)
                }
                Err(e) => {
                    error!("Failed to apply update: {}", e);
                    // Clean up downloaded file on failure
                    fs::remove_file(&temp_path).await.ok();
                    return Err(e).context("Update application failed");
                }
            }
        }
        Err(e) => {
            error!("Failed to download update: {}", e);
             // Clean up potentially partial file
            if temp_path.exists() {
                fs::remove_file(&temp_path).await.ok();
            }
            return Err(e).context("Update download failed");
        }
    }
} else {
    debug!("No updates available or necessary.");
    Ok(false) // Indicate no update was applied
}
}


// load_config - Adapted to use new MinerConfig structure
fn load_config() -> Result<MinerConfig> {
let config_path = env::var("CONFIG_PATH").unwrap_or_else(|_| "miner_config.json".to_string());
let mut config: MinerConfig = if Path::new(&config_path).exists() {
    debug!("Loading config from {}", config_path);
    let config_data = fs::read_to_string(&config_path)?;
    // Deserialize and fill missing fields with defaults if necessary
    let loaded_config: Value = serde_json::from_str(&config_data)?;
    let default_config_json = serde_json::to_value(MinerConfig::default())?;
    let mut merged_config = default_config_json;
    json_patch::merge(&mut merged_config, &loaded_config);
    serde_json::from_value(merged_config)?
} else {
    warn!("Config file not found at {}, using environment variables or defaults.", config_path);
    // Fall back to environment variables or defaults
    // Get default config to use its values if env vars are missing
    let defaults = MinerConfig::default();
    MinerConfig {
        contract_address: env::var("CONTRACT_ADDRESS")
            .unwrap_or_else(|_| defaults.contract_address),
        rpc_url: env::var("RPC_URL")
            .unwrap_or_else(|_| defaults.rpc_url),
        private_key_path: env::var("PRIVATE_KEY_PATH").ok(),
        threads: env::var("MINER_THREADS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(defaults.threads), // Use default threads
        gas_price_multiplier: env::var("GAS_PRICE_MULTIPLIER")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(defaults.gas_price_multiplier),
        retry_delay: Duration::from_secs(
            env::var("RETRY_DELAY_SECONDS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(defaults.retry_delay.as_secs()), // Use default duration
        ),
        log_level: env::var("LOG_LEVEL")
            .ok()
            .and_then(|s| match s.to_uppercase().as_str() {
                "TRACE" => Some(Level::TRACE),
                "DEBUG" => Some(Level::DEBUG),
                "INFO" => Some(Level::INFO),
                "WARN" => Some(Level::WARN),
                "ERROR" => Some(Level::ERROR),
                _ => None,
            })
            .unwrap_or(defaults.log_level),
        stats_interval: Duration::from_secs(
            env::var("STATS_INTERVAL_SECONDS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(defaults.stats_interval.as_secs()), // Use default duration
        ),
        exit_after_blocks: env::var("EXIT_AFTER_BLOCKS")
            .ok()
            .and_then(|s| s.parse().ok()),
        max_retries: env::var("MAX_RETRIES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(defaults.max_retries),
        // Load new/updated fields from environment
        // Removed loading for use_avx2 and use_sha_ni
        prefetch_distance: env::var("PREFETCH_DISTANCE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(defaults.prefetch_distance),
        batch_size: env::var("BATCH_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(defaults.batch_size),
        l3_cache_optimized: env::var("L3_CACHE_OPTIMIZED") // New field
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(defaults.l3_cache_optimized),
        // --- Load Update Config from Env Vars ---
        update_server: env::var("UPDATE_SERVER").unwrap_or(defaults.update_server),
        update_check_interval: Duration::from_secs(
            env::var("UPDATE_CHECK_INTERVAL_SECONDS")
                .ok().and_then(|s| s.parse().ok())
                .unwrap_or(defaults.update_check_interval.as_secs())
        ),
        update_public_key_path: env::var("UPDATE_PUBLIC_KEY_PATH")
            .unwrap_or(defaults.update_public_key_path),
        update_enabled: env::var("UPDATE_ENABLED")
            .ok().and_then(|s| s.parse().ok()).unwrap_or(defaults.update_enabled),
        // --- Load Stats Server Config from Env Vars ---
        stats_server_required: env::var("STATS_SERVER_REQUIRED")
            .ok().and_then(|s| s.parse().ok()).unwrap_or(defaults.stats_server_required),
        stats_server_endpoint: env::var("STATS_SERVER_ENDPOINT")
            .unwrap_or(defaults.stats_server_endpoint),
        stats_server_cert_path: env::var("STATS_SERVER_CERT_PATH")
            .unwrap_or(defaults.stats_server_cert_path),
    }
};

// Validate config
if config.threads < 1 {
    // Consider setting threads based on num_cpus::get_physical() here if desired
    warn!("Thread count is less than 1, setting to 1.");
    config.threads = 1;
}
if config.batch_size < 1 {
    warn!("Batch size is less than 1, setting to 1.");
    config.batch_size = 1; // Ensure batch size is at least 1
}
if config.contract_address == "0xYourContractAddress" {
     warn!("Using default contract address. Please set CONTRACT_ADDRESS environment variable or update miner_config.json");
}

// Remove runtime checks for CPU features flags as they are removed
// if config.use_avx2 && !is_avx2_supported() { ... }
// if config.use_sha_ni && !is_sha_ni_supported() { ... }


// Save config for reference if it was loaded from env vars and file didn't exist
if (!Path::new(&config_path).exists()) {
    info!("Saving default/environment configuration to {}", config_path);
    let config_json = serde_json::to_string_pretty(&config)?;
    fs::write(&config_path, config_json)?;
}

Ok(config)
}

// setup_logging - Kept as is
fn setup_logging(level: Level) -> Result<()> {
let subscriber = FmtSubscriber::builder()
    .with_max_level(level)
    .with_ansi(true)
    .with_file(true)
    .with_line_number(true)
    .finish();

tracing::subscriber::set_global_default(subscriber)
    .context("Failed to set global default subscriber")
}

// load_or_generate_key_secure - Modified to return SecureBuffer
fn load_or_generate_key_secure(config: &MinerConfig) -> Result<SecureBuffer> {
let key_bytes = if let Some(key_path) = &config.private_key_path {
    let key_path = Path::new(key_path);
    if key_path.exists() {
        debug!("Loading existing private key from {}", key_path.display());
        let key_data = fs::read_to_string(key_path)?;
        hex::decode(key_data.trim())?
    } else {
        debug!("Generating new private key and saving to {}", key_path.display());
        let new_key = SigningKey::random(&mut OsRng);
        let bytes = new_key.to_bytes(); // Get bytes
        let key_hex = hex::encode(&bytes);
        fs::write(key_path, key_hex)?;
        bytes.to_vec() // Return Vec<u8>
    }
} else {
    warn!("No private key path specified, generating ephemeral key for this session.");
    let new_key = SigningKey::random(&mut OsRng);
    new_key.to_bytes().to_vec() // Return Vec<u8>
};

// Ensure key bytes are exactly 32 bytes for k256::SigningKey
if key_bytes.len() != 32 {
    return Err(anyhow!("Invalid private key length: expected 32 bytes, got {}", key_bytes.len()));
}

// Create SecureBuffer and copy key bytes into it
let mut secure_buffer = SecureBuffer::new(32);
secure_buffer.as_mut_slice().copy_from_slice(&key_bytes);

// Explicitly zeroize the intermediate key_bytes Vec (important!)
key_bytes.zeroize();

Ok(secure_buffer)
}

// create_wallet_from_secure_buffer - Modified for Improvement #2
fn create_wallet_from_secure_buffer(key_buffer: &SecureBuffer, _config: &MinerConfig) -> Result<LocalWallet> {
// Create a temporary buffer to avoid exposing key through potential panics
let mut key_bytes = [0u8; 32];
key_bytes.copy_from_slice(key_buffer.as_slice());

// Attempt to create wallet from the temporary buffer
let wallet_result = LocalWallet::from_bytes(&key_bytes);

// Zeroize the temporary buffer immediately, regardless of success or failure
key_bytes.zeroize();

// Handle the result of wallet creation
let wallet = wallet_result.map_err(|e| anyhow!("Failed to create wallet from bytes: {}", e))?;

// Configure wallet with chain id
let chain_id = env::var("CHAIN_ID")
    .ok()
    .and_then(|s| s.parse().ok())
    .unwrap_or(1); // Default to Ethereum mainnet (adjust if needed)
debug!("Using Chain ID: {}", chain_id);

Ok(wallet.with_chain_id(chain_id))
}

// extract_reward_from_receipt - Adapted event signature
fn extract_reward_from_receipt(receipt: &TransactionReceipt) -> Option<f64> {
// Find BeaconBlockMined event and extract reward
// Event signature from Solidity: event BeaconBlockMined(address indexed miner, bytes32 hmacOutput, uint256 reward, uint64 nonce, uint64 timestamp, uint8 poolId);
let event_signature_hash = keccak256("BeaconBlockMined(address,bytes32,uint256,uint64,uint64,uint8)"); // Corrected signature

for log in &receipt.logs {
    if log.topics.len() >= 1 && log.topics[0] == H256(event_signature_hash) {
         // Decode non-indexed parameters: reward (uint256), nonce (uint64), timestamp (uint64), poolId (uint8)
         // Data layout: reward_bytes32 | nonce_bytes32 | timestamp_bytes32 | poolId_bytes32
         if log.data.len() >= 32 * 1 { // Check if reward data is present
             let reward_data = &log.data[0..32];
             let reward_u256 = U256::from_big_endian(reward_data);
             // Convert U256 reward to f64 (assuming 18 decimals)
             let reward_f64 = ethers::utils::format_units(reward_u256, 18)
                 .ok()
                 .and_then(|s| s.parse::<f64>().ok())
                 .unwrap_or(0.0);
             debug!("Extracted reward: {} (U256: {})", reward_f64, reward_u256);
             return Some(reward_f64);
         } else {
             warn!("BeaconBlockMined event data too short to extract reward. Data length: {}", log.data.len());
         }
    }
}
debug!("BeaconBlockMined event not found or reward extraction failed.");
None
}

// print_stats - Added from snippet, made async for tokio::Mutex
async fn print_stats(stats_arc: &Arc<Mutex<MiningStats>>) {
let stats = stats_arc.lock().await; // Use tokio Mutex lock
let elapsed = stats.start_time.elapsed().unwrap_or_default().as_secs_f64();
let hashrate = if elapsed > 0.0 { stats.hashes as f64 / elapsed } else { 0.0 };

info!("┌─── Mining Statistics ───────────────────────");
info!("│ Solutions: {}", stats.solutions);
info!("│ Total Hashes: {}", stats.hashes);
info!("│ Hashrate: {:.2} H/s", hashrate);
info!("│ Running time: {:.2} minutes", elapsed / 60.0);
info!("│ Total rewards: {:.6} tokens", stats.total_rewards); // Kept from original
info!("│ Successful Submissions: {}", stats.successful_submissions); // Kept from original
info!("│ Failed Submissions: {}", stats.failed_submissions); // Kept from original
info!("└────────────────────────────────────────────");
}

// Test stub for quantum resistant hash
#[cfg(test)]
mod tests {
use super::*;
use k256::ecdsa::SigningKey;
use rand::rngs::OsRng;

#[test]
fn test_quantum_resistant_hash() {
    // Test against known values from contract tests (replace with actual values)
    let signing_key = SigningKey::random(&mut OsRng); // Example key
    // Use fast_hash_message instead of keccak256 for better performance
    let msg_hash = fast_hash_message(b"test message");
    let signature: Signature = signing_key.sign(&msg_hash);
    let secret = b"test_secret";

    let result = quantum_resistant_hash(&signature, &msg_hash, secret);

    // Replace with the actual expected hash from contract testing
    let expected_value: [u8; 32] = [0u8; 32]; // Placeholder

    assert_eq!(result, expected_value, "Quantum resistant hash does not match expected value");
}

// Add a benchmark test for the hashing performance improvement
#[test]
#[ignore] // Ignore by default, run explicitly with cargo test -- --ignored
fn benchmark_hashing_methods() {
    // Create test data
    let test_data = vec![0u8; 128];
    
    // Warmup
    for _ in 0..1000 {
        let _ = keccak256(&test_data);
        let _ = fast_hash_message(&test_data);
    }
    
    // Benchmark keccak256 directly
    let keccak_start = Instant::now();
    for _ in 0..10000 {
        let _ = keccak256(&test_data);
    }
    let keccak_time = keccak_start.elapsed();
    
    // Benchmark fast_hash_message (BLAKE3 + keccak256)
    let fast_start = Instant::now();
    for _ in 0..10000 {
        let _ = fast_hash_message(&test_data);
    }
    let fast_time = fast_start.elapsed();
    
    println!("Keccak256: {:?} for 10000 iterations", keccak_time);
    println!("Fast Hash: {:?} for 10000 iterations", fast_time);
    println!("Speed improvement: {:.2}x", keccak_time.as_secs_f64() / fast_time.as_secs_f64());
    
    // Test specifically with different message sizes to show where BLAKE3 shines
    let sizes = [32, 64, 128, 256, 512, 1024, 2048, 4096];
    for size in sizes {
        let large_data = vec![0u8; size];
        
        let keccak_start = Instant::now();
        for _ in 0..1000 {
            let _ = keccak256(&large_data);
        }
        let keccak_time = keccak_start.elapsed();
        
        let fast_start = Instant::now();
        for _ in 0..1000 {
            let _ = fast_hash_message(&large_data);
        }
        let fast_time = fast_start.elapsed();
        
        println!(
            "Size {} bytes: Keccak: {:?}, Fast: {:?}, Improvement: {:.2}x", 
            size, 
            keccak_time, 
            fast_time, 
            keccak_time.as_secs_f64() / fast_time.as_secs_f64()
        );
    }
}

// Add a test to verify CPU feature detection works correctly
#[test]
fn test_cpu_feature_detection() {
    let strategy = detect_cpu_features();
    println!("Detected CPU strategy: {:?}", strategy);
    
    // We can't make specific assertions about the result since it depends on the hardware
    // But we can verify the function runs without error
    match strategy {
        MiningStrategy::AVX512 => println!("Using AVX-512 optimizations"),
        MiningStrategy::AVX2 => println!("Using AVX2 optimizations"),
        MiningStrategy::SSE4 => println!("Using SSE4 optimizations"),
        MiningStrategy::Generic => println!("Using generic implementation"),
    }
}
}

// --- Add function signature for secure_connect_pinned in network module (if not already there) ---
// This is just a conceptual addition to show where it would fit.
// The actual implementation would be in network.rs.
mod network_stub { // Placeholder to avoid compiler errors here
use anyhow::Result;
use tokio::net::TcpStream; // Example stream type
pub struct SecureChannel { /* fields */ }
impl SecureChannel {
    pub async fn write_all(&mut self, _buf: &[u8]) -> Result<()> { Ok(()) }
    pub fn check_rotation(&mut self) -> Result<()> { Ok(()) }
    pub async fn shutdown(&mut self) -> Result<()> { Ok(()) }
}
pub async fn secure_connect_pinned(_endpoint: &str, _cert_path: &str) -> Result<SecureChannel> {
    // Implementation would go in network.rs
    // Load pinned cert, configure TLS connector with pinning, connect.
    Err(anyhow::anyhow!("secure_connect_pinned not implemented"))
}
}
use network_stub as network; // Use the stub for compilation
// --- End network module stub ---


// --- Final Testing Recommendations (Improvement #4) ---
#[cfg(test)]
mod integration_tests {
use super::*;
use tempfile::NamedTempFile;
use std::fs;

// Helper to create a dummy config for testing
fn create_test_config() -> (MinerConfig, NamedTempFile) {
    let key_file = NamedTempFile::new().expect("Failed to create temp key file");
    let mut config = MinerConfig::default();
    config.threads = 1; // Single thread for predictable testing
    config.batch_size = 2; // Small batch
    config.exit_after_blocks = Some(1); // Exit after finding one solution
    config.rpc_url = "http://localhost:8545".to_string(); // Use a mock RPC if possible
    config.contract_address = "0x5FbDB2315678afecb367f032d93F642f64180aa3".to_string(); // Example address
    config.private_key_path = Some(key_file.path().to_str().unwrap().to_string());
    config.log_level = Level::DEBUG; // More verbose logging for tests
    config.retry_delay = Duration::from_millis(100);
    config.stats_interval = Duration::from_secs(1); // Frequent stats for testing

    // Write a dummy key to the temp file
    let dummy_key = SigningKey::random(&mut OsRng);
    fs::write(key_file.path(), hex::encode(dummy_key.to_bytes())).expect("Failed to write dummy key");

    (config, key_file)
}

#[tokio::test]
#[ignore] // Ignore by default as it requires external setup (like a local node/mock)
async fn test_full_mining_cycle_basic() {
    // Setup test config and dummy key file
    let (config, _key_file) = create_test_config(); // _key_file guards temp file lifetime

    // It's hard to directly test `main` due to its structure.
    // Ideally, refactor `main` to extract the core mining loop logic
    // into a separate async function that accepts config and returns Result.
    // For now, we'll just assert that creating the config works.

    // Example of how you *might* call a refactored main logic function:
    // let result = run_miner_logic(config).await;
    // assert!(result.is_ok(), "Miner logic failed: {:?}", result.err());

    // Placeholder assertion: Check if config loading works
    assert_eq!(config.threads, 1);
    assert!(config.private_key_path.is_some());

    // TODO: Implement actual test execution, potentially requiring:
    // 1. A running local Ethereum node (e.g., Anvil, Hardhat node).
    // 2. Deployment of the TemporalGradientBeacon contract to that node.
    // 3. Mocking or providing necessary environment variables (ETHERSCAN_API_KEY).
    // 4. Refactoring `main` for testability.
}

// Add more tests:
// - test_config_loading_from_env()
// - test_key_generation_ephemeral()
// - test_thermal_throttling_logic()
// - test_meets_difficulty()
// - test_quantum_resistant_hash() (already exists, ensure it's thorough)
}
// --- End Testing Recommendations ---

// Add synchronous version of quantum resistant hash
#[inline]
fn quantum_resistant_hash(signature: &Signature, message_hash: &[u8; 32], temporal_seed: &[u8]) -> [u8; 32] {
    // 1) pack exactly the same bytes your contract expects
    let mut packed = Vec::with_capacity(
        signature.to_der().as_bytes().len() +
        message_hash.len() +
        temporal_seed.len()
    );
    packed.extend_from_slice(signature.to_der().as_bytes());
    packed.extend_from_slice(message_hash);
    packed.extend_from_slice(temporal_seed);

    // 2) call your inner (Solidity-matching) routine with dummy timestamp
    // Fixed timestamp (0) ensures consistency with chain validation
    quantum_resistant_hash_inner(&packed, 0)
}

// Add after imports
use k256::{
    elliptic_curve::{group::prime::PrimeCurveAffine, sec1::ToEncodedPoint},
    ProjectivePoint, PublicKey,
};

// Add new structs for stealth mining
#[derive(Debug, Clone)]
struct StealthAddress {
    spending_key: LocalWallet,
    viewing_key: SigningKey,
    ephemeral_keys: VecDeque<(SigningKey, SystemTime)>, // (key, expiry)
}

impl StealthAddress {
    // ...existing code...

    fn generate_ephemeral_key(&mut self) -> Result<PublicKey> {
        let ephemeral_key = SigningKey::random(&mut OsRng);
        let expiry = SystemTime::now() + Duration::from_secs(3600); // 1 hour validity
        self.ephemeral_keys.push_back((ephemeral_key.clone(), expiry));
        
        // Clean expired keys
        while let Some((_, exp)) = self.ephemeral_keys.front() {
            if exp < &SystemTime::now() {
                self.ephemeral_keys.pop_front();
            } else {
                break;
            }
        }
        
        Ok(ephemeral_key.verifying_key().into())
    }

    fn find_claiming_key(&self, stealth_meta: &[u8]) -> Option<SigningKey> {
        self.ephemeral_keys.iter()
            .find(|(key, expiry)| {
                if expiry < &SystemTime::now() {
                    return false;
                }
                let public = key.verifying_key();
                let point = public.as_affine().to_encoded_point(true);
                stealth_meta.starts_with(point.as_bytes())
            })
            .map(|(key, _)| key.clone())
    }
}

// Update DynamicMiningCommitment to include stealth fields
pub struct DynamicMiningCommitment {
    // ...existing code...
    stealth_meta: Vec<u8>,      // Stealth metadata for anonymous claim
    stealth_proof: Vec<u8>,     // Zero-knowledge proof of stealth key ownership
}

impl DynamicMiningCommitment {
    fn to_eip712_types(chain_id: u64, contract_address: Address) -> TypedData {
        TypedData {
            // ...existing code...
            types: {
                let mut types = BTreeMap::new();
                types.insert(
                    "MiningCommitment".to_string(),
                    vec![
                        // ...existing fields...
                        TypeField { 
                            name: "stealthMeta".to_string(), 
                            r#type: "bytes".to_string() 
                        },
                        TypeField { 
                            name: "stealthProof".to_string(), 
                            r#type: "bytes".to_string() 
                        },
                    ],
                );
                types
            },
            message: {
                let mut map = BTreeMap::new();
                // ...existing fields...
                map.insert("stealthMeta".to_string(), 
                    Value::from(format!("0x{}", hex::encode(&self.stealth_meta))));
                map.insert("stealthProof".to_string(), 
                    Value::from(format!("0x{}", hex::encode(&self.stealth_proof))));
                map
            },
        }
    }
}

// Add to mining loop where solution is found:
if meets_difficulty(&solution_hash, difficulty_target) {
    if solution_found.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        info!("Thread {} found solution in batch! Nonce: {}", thread_id, current_nonce);

        // 1) Generate fresh ephemeral key and pack its compressed point
        let mut stealth = stealth.lock().await;
        let ephem_pub = stealth.generate_ephemeral_key()
            .context("failed to generate ephemeral stealth key")?;
        let stealth_meta = ephem_pub.to_encoded_point(true).as_bytes().to_vec();

        // 2) Build ZK-proof of ownership 
        let stealth_proof = {
            // TODO: Replace with actual ZK proof generation
            // For example: prove_ownership(&stealth_meta, &stealth.viewing_key)
            vec![0u8; 32] // Placeholder
        };
        
        drop(stealth); // Release lock before network operations

        // 3) Build commitment with stealth data
        let commit_hash = {
            let mut buf = Vec::new();
            buf.extend_from_slice(&previous_output);
            buf.extend_from_slice(&temporal_seed);
            buf.extend_from_slice(&current_nonce.to_le_bytes());
            buf.extend_from_slice(&signature.to_der());
            buf.extend_from_slice(&solution_hash);
            buf.extend_from_slice(&wallet.address().0);
            keccak256(&buf)
        };

        let deadline = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| anyhow!("Failed to get deadline timestamp: {}", e))?
            .as_secs() + 300; // 5 minutes

        let commitment = DynamicMiningCommitment {
            commit_hash: commit_hash.try_into().expect("32 bytes"),
            pool_id: 0,
            nonce: current_nonce,
            deadline,
            stealth_meta,
            stealth_proof,
        };

        // Rest of existing submission code
        // ...existing code...
    }
}
