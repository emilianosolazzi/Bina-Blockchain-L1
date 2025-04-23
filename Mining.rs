use k256::ecdsa::{SigningKey, Signature, signature::Signer};
use sha2::{Sha256, Digest};
use hmac::{Hmac, Mac};
use rand::rngs::OsRng; // Keep OsRng for key generation
use rand::Rng; // Keep Rng for random bytes in temporal seed
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use std::env;
use std::fs;
use std::path::Path;
use std::thread;
use std::sync::{Arc}; // Removed Mutex from here, using tokio::sync::Mutex
use std::time::Duration;
use ethers::{
    prelude::*,
    utils::{hex, keccak256},
    middleware::gas_oracle::{GasOracle, GasOracleMiddleware, EthGasStation}
};
use tracing::{info, error, warn, debug, Level};
use tracing_subscriber::FmtSubscriber;
use anyhow::{Result, Context, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex; // Use tokio's Mutex for async compatibility
use std::arch::x86_64::is_x86_feature_detected; // Import for CPU feature detection

type HmacSha256 = Hmac<Sha256>;

// Enum to represent different mining strategies based on CPU features
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MiningStrategy {
    Generic,
    SSE4,
    AVX2,
    // Potentially add SHA-NI strategy if specific optimizations exist
}

// Function to detect CPU features and select a strategy
fn detect_cpu_features() -> MiningStrategy {
    // Check for AVX2 first as it's generally the most performant
    if is_x86_feature_detected!("avx2") {
        info!("AVX2 detected, using AVX2 optimized strategy.");
        MiningStrategy::AVX2
    } else if is_x86_feature_detected!("sse4.1") { // Check for SSE4.1 as a fallback
        info!("SSE4.1 detected, using SSE4 optimized strategy.");
        MiningStrategy::SSE4
    } else {
        info!("No specific CPU features detected, using generic strategy.");
        MiningStrategy::Generic
    }
    // Note: SHA-NI detection (is_x86_feature_detected!("sha")) could be added here
    // if specific SHA-NI optimizations are implemented in the hashing logic.
}

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
    // Updated performance tuning fields
    use_avx2: bool, // Changed from use_avx
    use_sha_ni: bool, // New field
    prefetch_distance: usize,
    batch_size: usize,
    l3_cache_optimized: bool, // New field
}

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
            use_avx2: true, // Defaulting to true as requested
            use_sha_ni: true, // Defaulting to true as requested
            prefetch_distance: 4, // New default
            batch_size: 16, // New default
            l3_cache_optimized: true, // Defaulting to true as requested
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

// generate_temporal_seed - Adapted from snippet, keeping OsRng usage
fn generate_temporal_seed() -> Vec<u8> {
    let mut seed = Vec::with_capacity(64);
    seed.extend_from_slice(&rand::random::<[u8; 32]>()); // Use rand::random for simplicity
    seed.extend_from_slice(&SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default() // Use unwrap_or_default for robustness
        .as_nanos()
        .to_le_bytes());
    Sha256::digest(&seed).to_vec()
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

    // Assuming poolId 0 for now, adjust if needed
    let pool_id = u8;
    let (_outputs, difficulty) = contract.get_mining_challenge(pool_id).call().await?;

    // Assuming the first output is the one needed as `previous_output`
    // This might need adjustment based on contract logic.
    // For now, let's just return a placeholder or handle the array properly if needed.
    // If the contract returns `bytes32[]`, we need to decide which one to use.
    // Let's assume for now the contract is simplified or we only care about difficulty.
    // Returning a dummy previous_output for compilation.
    // *** THIS NEEDS REVIEW BASED ON ACTUAL CONTRACT `getMiningChallenge` RETURN ***
    let previous_output = vec![0u8; 32]; // Placeholder

    Ok((previous_output, difficulty))
}


// submit_solution - From snippet, adapted for existing wallet type and error handling
async fn submit_solution(
    client: &SignerMiddleware<GasOracleMiddleware<Provider<Http>, EthGasStation>, LocalWallet>, // Use existing wallet type
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
    let prev_output_bytes32: [u8; 32] = previous_output.try_into()
        .map_err(|_| anyhow!("Previous output is not 32 bytes"))?;
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

// quantum_resistant_hash - Implements keccak256(abi.encodePacked(signature, messageHash, secret))
// Matches the pattern described in the Solidity comments.
fn quantum_resistant_hash(signature: &Signature, message_hash: &[u8; 32], secret: &[u8]) -> [u8; 32] {
    // Convert signature to bytes (using DER format as previously)
    let sig_bytes = signature.to_der();

    // Concatenate signature || message_hash || secret
    let mut combined = Vec::with_capacity(sig_bytes.as_bytes().len() + message_hash.len() + secret.len());
    combined.extend_from_slice(sig_bytes.as_bytes());
    combined.extend_from_slice(message_hash);
    combined.extend_from_slice(secret);

    // Compute Keccak256 hash
    keccak256(&combined)
}


#[tokio::main]
async fn main() -> Result<()> {
    // Load configuration from file or environment
    let config = load_config()?;

    // Initialize logging
    setup_logging(config.log_level)?;

    info!("Starting Temporal Gradient Beacon Miner v1.0.0");
    debug!("Configuration: {:?}", config);

    // Load or generate signing key
    let signing_key = load_or_generate_key(&config)?;
    // Create ethers wallet from signing key
    let wallet = create_wallet(&signing_key, &config)?; // Use existing function

    let public_key = signing_key.verifying_key();
    info!("Mining with public key: {}", hex::encode(public_key.to_encoded_point(false).as_bytes()));

    // Initialize provider with gas station
    let provider = Provider::<Http>::try_from(&config.rpc_url)?;
    let gas_oracle = EthGasStation::new(None);
    let gas_provider = GasOracleMiddleware::new(provider.clone(), gas_oracle, config.gas_price_multiplier);
    // Create SignerMiddleware client
    let client = SignerMiddleware::new(gas_provider, wallet.clone()); // Use wallet clone

    // Initialize mining statistics (using tokio Mutex)
    let stats = Arc::new(Mutex::new(MiningStats {
        start_time: SystemTime::now(),
        ..Default::default()
    }));

    // Start statistics reporting thread
    let stats_clone_for_reporter = Arc::clone(&stats);
    let stats_interval = config.stats_interval;
    tokio::spawn(async move { // Use tokio::spawn for async block
        loop {
            tokio::time::sleep(stats_interval).await; // Use tokio sleep
            print_stats(&stats_clone_for_reporter).await; // Call async print_stats
        }
    });

    // Contract address
    let contract_address = config.contract_address.parse::<Address>()?;

    // Main mining loop
    let mut consecutive_errors = 0;
    loop {
        // Check exit condition
        if let Some(exit_after) = config.exit_after_blocks {
             let solutions_count = stats.lock().await.solutions; // Use tokio Mutex lock
             if solutions_count >= exit_after as u32 {
                 info!("Reached target of {} solutions, exiting", exit_after);
                 break;
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
                tokio::time::sleep(config.retry_delay).await; // Use tokio sleep
                continue;
            }
        };

        info!("Mining new block. Difficulty Target: {}", difficulty_target_u256);
        // info!("Previous output: {}", hex::encode(&previous_output)); // Keep if previous_output is valid

        // Create thread-safe atomic flag for signaling successful mining
        let solution_found = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut handles = vec![]; // Use Vec for handles

        // Start mining with multiple threads
        for thread_id in 0..config.threads {
            // Clone necessary variables for the thread
            let client = client.clone();
            let stats = Arc::clone(&stats);
            let solution_found = Arc::clone(&solution_found);
            let previous_output = previous_output.clone(); // Clone previous_output
            let config = config.clone(); // Clone config
            let signing_key = signing_key.clone(); // Clone signing_key

            handles.push(tokio::spawn(async move { // Use tokio::spawn
                let mut nonce = thread_id as u64;
                let nonce_increment = config.threads as u64;

                while !solution_found.load(std::sync::atomic::Ordering::Relaxed) {
                    let temporal_seed = generate_temporal_seed();
                    // Use create_message from snippet
                    let message = create_message(&previous_output, &temporal_seed, nonce);

                    // --- Call CPU optimization ---
                    // Optimize cache for the message data before hashing/signing
                    optimize_for_cpu(message.as_ptr());
                    // --- End CPU optimization ---


                    // Sign the message hash (standard practice)
                    let message_hash = keccak256(&message);
                    let signature: Signature = signing_key.sign(&message_hash);

                    // Calculate the solution hash using the quantum-resistant function
                    // *** Assumption: `temporal_seed` is used as the `secret` input for hashing ***
                    // This needs verification against the actual contract logic.
                    let solution_hash = quantum_resistant_hash(&signature, &message_hash, &temporal_seed);

                    // Update hash counter
                    {
                        let mut stats_guard = stats.lock().await; // Use tokio Mutex lock
                        stats_guard.hashes += 1;
                    }

                    // Use meets_difficulty from snippet, checking the new solution_hash
                    if meets_difficulty(&solution_hash, difficulty_target_u256) {
                        // Attempt to set solution_found flag
                        if solution_found.compare_exchange(false, true, std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed).is_ok() {
                            info!("Thread {} found solution! Nonce: {}", thread_id, nonce);

                            // Submit to blockchain using submit_solution
                            // Pass the calculated solution_hash as the 'secretValue' (hmac parameter)
                            match submit_solution(
                                &client,
                                config.contract_address.parse().unwrap(), // Parse address here
                                &previous_output,
                                &temporal_seed,
                                nonce,
                                &signature,
                                &solution_hash, // Pass the calculated solution hash
                            ).await {
                                Ok(receipt) => {
                                    info!("Solution submitted successfully!");
                                    debug!("Transaction receipt: {:?}", receipt);

                                    let reward = extract_reward_from_receipt(&receipt).unwrap_or(0.0); // Use 0.0 default

                                    // Update statistics
                                    let mut stats_guard = stats.lock().await; // Use tokio Mutex lock
                                    stats_guard.solutions += 1;
                                    stats_guard.successful_submissions += 1; // Keep this if needed
                                    stats_guard.total_rewards += reward;
                                    // stats_guard.mining_efficiency = ... // Keep if needed

                                    // Break the loop for this thread as solution is submitted
                                    break;
                                },
                                Err(e) => {
                                    error!("Failed to submit solution: {}", e);
                                    let mut stats_guard = stats.lock().await; // Use tokio Mutex lock
                                    stats_guard.failed_submissions += 1; // Keep this if needed

                                    // Reset solution_found flag to allow other threads or retries
                                    solution_found.store(false, std::sync::atomic::Ordering::Relaxed);
                                    // Consider adding a small delay before retrying submission or continuing mining
                                    tokio::time::sleep(config.retry_delay).await;
                                }
                            }
                        } else {
                            // Another thread found the solution first, stop this thread
                            break;
                        }
                    }
                    nonce += nonce_increment;
                     // Add a small yield to prevent busy-waiting and allow other tasks to run
                    tokio::task::yield_now().await;
                }
            }));
        }

        // Wait for all threads to complete (or one to successfully submit)
        futures::future::join_all(handles).await;

        // Check if a solution was actually found and submitted successfully in this round
        // This check might be redundant if the loop breaks on success inside the thread
        if !solution_found.load(std::sync::atomic::Ordering::Relaxed) {
             warn!("No solution found in this round, fetching new challenge...");
             tokio::time::sleep(config.retry_delay).await; // Use tokio sleep
        }
        // print_stats is handled by the dedicated stats thread now
    }

    Ok(())
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
            use_avx2: env::var("USE_AVX2") // Updated field name
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(defaults.use_avx2),
            use_sha_ni: env::var("USE_SHA_NI") // New field
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(defaults.use_sha_ni),
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

    // Add runtime checks for CPU features if possible/needed
    // Example (conceptual, requires checking specific CPU features):
    // if config.use_avx2 && !is_avx2_supported() {
    //     warn!("AVX2 specified but not supported by CPU, disabling.");
    //     config.use_avx2 = false;
    // }
    // if config.use_sha_ni && !is_sha_ni_supported() {
    //     warn!("SHA-NI specified but not supported by CPU, disabling.");
    //     config.use_sha_ni = false;
    // }


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

// load_or_generate_key - Kept as is
fn load_or_generate_key(config: &MinerConfig) -> Result<SigningKey> {
    if let Some(key_path) = &config.private_key_path {
        let key_path = Path::new(key_path);
        if key_path.exists() {
            debug!("Loading existing private key from {}", key_path.display());
            let key_data = fs::read_to_string(key_path)?;
            SigningKey::from_bytes(&hex::decode(key_data.trim())?)
                .map_err(|e| anyhow!("Invalid private key: {}", e))
        } else {
            debug!("Generating new private key and saving to {}", key_path.display());
            let new_key = SigningKey::random(&mut OsRng);
            let key_hex = hex::encode(new_key.to_bytes());
            fs::write(key_path, key_hex)?;
            Ok(new_key)
        }
    } else {
        warn!("No private key path specified, generating ephemeral key for this session.");
        Ok(SigningKey::random(&mut OsRng))
    }
}

// create_wallet - Kept as is
fn create_wallet(signing_key: &SigningKey, _config: &MinerConfig) -> Result<LocalWallet> { // config not needed here anymore
    // Convert k256 signing key to ethers-rs wallet
    let bytes = signing_key.to_bytes();
    let wallet = LocalWallet::from_bytes(&bytes)?;

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
