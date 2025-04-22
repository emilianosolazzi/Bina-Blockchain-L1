use k256::ecdsa::{SigningKey, Signature, signature::Signer};
use sha2::{Sha256, Digest};
use hmac::{Hmac, Mac};
use rand::rngs::OsRng;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use std::env;
use std::fs;
use std::path::Path;
use std::thread;
use std::sync::{Arc, Mutex};
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

type HmacSha256 = Hmac<Sha256>;

// Configuration struct with defaults
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MinerConfig {
    contract_address: String,
    rpc_url: String,
    private_key_path: Option<String>,
    difficulty_target: [u8; 32],
    threads: usize,
    gas_price_multiplier: f64,
    retry_delay: Duration,
    log_level: Level,
    stats_interval: Duration,
    exit_after_blocks: Option<usize>,
    max_retries: usize,
}

impl Default for MinerConfig {
    fn default() -> Self {
        Self {
            contract_address: "0xYourContractAddress".to_string(),
            rpc_url: "http://localhost:8545".to_string(),
            private_key_path: None,
            difficulty_target: [0, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 
                               255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            threads: 4,
            gas_price_multiplier: 1.1,
            retry_delay: Duration::from_secs(5),
            log_level: Level::INFO,
            stats_interval: Duration::from_secs(60),
            exit_after_blocks: None,
            max_retries: 5,
        }
    }
}

// Mining statistics with additional metrics
#[derive(Debug, Clone, Default)]
struct MiningStats {
    blocks_mined: usize,
    total_rewards: f64,
    start_time: SystemTime,
    hashes_computed: u64,
    failed_submissions: usize,
    successful_submissions: usize,
    highest_difficulty: u128,
    mining_efficiency: f64, // Solutions per 1M hashes
}

// Memory-efficient buffer for mining operations
struct MiningBuffer {
    data: Vec<u8>,
    sig_buffer: Vec<u8>,
    result_buffer: [0u8; 32],
}

impl MiningBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
            sig_buffer: Vec::with_capacity(128), // Typical signature size
            result_buffer: [0u8; 32],
        }
    }

    fn reset(&mut self) {
        self.data.clear();
    }
}

fn generate_temporal_seed() -> Vec<u8> {
    // Create high-quality entropy from timing variations
    let start = Instant::now();
    
    // Perform computationally intensive operation with random inputs
    // Pre-allocate buffer for better memory efficiency
    let mut buffer = [0u8; 32];
    for _ in 0..10000 {
        rand::Rng::fill(&mut OsRng, &mut buffer);
        let _ = Sha256::digest(&buffer);
    }
    
    // Capture nanosecond-level timing differences
    let duration = start.elapsed();
    let nanos = duration.as_nanos().to_le_bytes();
    
    // Mix with additional system entropy and hardware-specific timing
    let mut seed_data = Vec::with_capacity(nanos.len() + 64);
    seed_data.extend_from_slice(&nanos);
    rand::Rng::fill(&mut OsRng, &mut buffer);
    seed_data.extend_from_slice(&buffer);
    
    // Add process-specific entropy
    let pid = std::process::id().to_le_bytes();
    seed_data.extend_from_slice(&pid);
    
    // Add system timestamp in nanoseconds for additional entropy
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_le_bytes();
    seed_data.extend_from_slice(&now);
    
    Sha256::digest(&seed_data).to_vec()
}

#[inline]
fn meets_difficulty(hash: &[u8], target: &[u8; 32]) -> bool {
    // Compare hash against target difficulty (numerical comparison)
    // Using inline for performance in tight loops
    for i in 0..32 {
        if hash[i] < target[i] {
            return true;
        } else if hash[i] > target[i] {
            return false;
        }
    }
    false
}

#[inline]
fn calculate_actual_difficulty(hash: &[u8]) -> u128 {
    // Calculate actual difficulty as inverse of hash value
    // (lower hash = higher difficulty)
    let mut value: u128 = 0;
    for i in 0..16 {
        value = (value << 8) | hash[i] as u128;
    }
    u128::MAX - value
}

async fn get_current_challenge(
    provider: &Provider<Http>, 
    contract_address: &str
) -> Result<(Vec<u8>, [u8; 32])> {
    // Use ethers-rs for proper ABI encoding/decoding
    let function = "getMiningChallenge()";
    let function_selector = &keccak256(function.as_bytes())[0..4];
    
    // Call the function
    let call_request = TransactionRequest::new()
        .to(contract_address.parse::<Address>()?)
        .data(function_selector);
    
    let result = provider
        .call(&call_request, None)
        .await
        .context("Failed to call getMiningChallenge")?;
    
    // Parse the result (bytes32 output, uint256 difficulty)
    if result.len() < 64 {
        return Err(anyhow!("Invalid response length: {}", result.len()));
    }
    
    let last_output = result[0..32].to_vec();
    
    // Convert difficulty to target bytes
    let mut target = [0u8; 32];
    target.copy_from_slice(&result[32..64]);
    
    Ok((last_output, target))
}

async fn submit_block(
    wallet: &SignerMiddleware<GasOracleMiddleware<Provider<Http>, EthGasStation>, LocalWallet>,
    contract_address: Address,
    previous_output: &[u8],
    temporal_seed: &[u8],
    nonce: u64,
    signature: &Signature,
    hmac_output: &[u8],
) -> Result<TransactionReceipt> {
    // Prepare function parameters
    let function_signature = "submitBeaconBlock(bytes32,bytes,uint64,bytes,bytes32)";
    let function_selector = &keccak256(function_signature.as_bytes())[0..4];
    
    // Create ABI encoder
    let mut call_data = Vec::with_capacity(256);
    call_data.extend_from_slice(function_selector);
    
    // Encode previousOutput (bytes32)
    let mut padded_output = [0u8; 32];
    padded_output[..previous_output.len().min(32)].copy_from_slice(&previous_output[..previous_output.len().min(32)]);
    call_data.extend_from_slice(&padded_output);
    
    // Encode dynamic data offsets
    // Offset to temporalSeed
    call_data.extend_from_slice(&(5 * 32u32).to_be_bytes());
    
    // Encode nonce (uint64)
    let mut nonce_bytes = [0u8; 32];
    nonce_bytes[24..].copy_from_slice(&nonce.to_be_bytes());
    call_data.extend_from_slice(&nonce_bytes);
    
    // Offset to signature
    let sig_offset = 5 * 32 + 32 + temporal_seed.len() + (32 - temporal_seed.len() % 32) % 32;
    call_data.extend_from_slice(&(sig_offset as u32).to_be_bytes());
    
    // Encode hmacOutput (bytes32)
    call_data.extend_from_slice(hmac_output);
    
    // Encode temporal_seed length and data
    call_data.extend_from_slice(&(temporal_seed.len() as u32).to_be_bytes());
    call_data.extend_from_slice(temporal_seed);
    
    // Pad to 32-byte boundary
    let padding = (32 - temporal_seed.len() % 32) % 32;
    call_data.extend_from_slice(&vec![0u8; padding]);
    
    // Encode signature length and data
    let sig_der = signature.to_der();
    call_data.extend_from_slice(&(sig_der.len() as u32).to_be_bytes());
    call_data.extend_from_slice(&sig_der);
    
    // Pad signature data
    let sig_padding = (32 - sig_der.len() % 32) % 32;
    call_data.extend_from_slice(&vec![0u8; sig_padding]);
    
    // Create and send transaction
    let tx = TransactionRequest::new()
        .to(contract_address)
        .data(call_data)
        .gas(1_000_000); // Use high gas limit for safety
    
    // Send transaction and wait for receipt
    let pending_tx = wallet
        .send_transaction(tx, None)
        .await
        .context("Failed to send transaction")?;
    
    debug!("Transaction sent: {:?}", pending_tx);
    
    let receipt = pending_tx
        .await
        .context("Failed to confirm transaction")?
        .ok_or_else(|| anyhow!("No transaction receipt"))?;
    
    Ok(receipt)
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
    let wallet = create_wallet(&signing_key, &config)?;
    
    let public_key = signing_key.verifying_key();
    info!("Mining with public key: {}", hex::encode(public_key.to_encoded_point(false).as_bytes()));
    
    // Initialize provider with gas station
    let provider = Provider::<Http>::try_from(&config.rpc_url)?;
    let gas_oracle = EthGasStation::new(None);
    let gas_provider = GasOracleMiddleware::new(provider.clone(), gas_oracle, config.gas_price_multiplier);
    let wallet = SignerMiddleware::new(gas_provider, wallet);
    
    // Initialize mining statistics
    let stats = Arc::new(Mutex::new(MiningStats {
        start_time: SystemTime::now(),
        ..Default::default()
    }));
    
    // Start statistics reporting thread
    let stats_clone = Arc::clone(&stats);
    let stats_interval = config.stats_interval;
    thread::spawn(move || {
        loop {
            thread::sleep(stats_interval);
            let stats = stats_clone.lock().unwrap();
            let elapsed = stats.start_time.elapsed().unwrap_or(Duration::from_secs(1));
            let hash_rate = stats.hashes_computed as f64 / elapsed.as_secs_f64();
            let efficiency = if stats.hashes_computed > 0 {
                (stats.successful_submissions as f64 * 1_000_000.0) / stats.hashes_computed as f64
            } else {
                0.0
            };
            
            info!("┌─── Mining Statistics ───────────────────────");
            info!("│ Blocks mined: {}", stats.blocks_mined);
            info!("│ Total rewards: {:.6} tokens", stats.total_rewards);
            info!("│ Hash rate: {:.2} H/s", hash_rate);
            info!("│ Running time: {:.2} minutes", elapsed.as_secs_f64() / 60.0);
            info!("│ Success rate: {:.6} solutions per 1M hashes", efficiency);
            info!("│ Highest difficulty: {}", stats.highest_difficulty);
            info!("└────────────────────────────────────────────");
        }
    });
    
    // Contract address
    let contract_address = config.contract_address.parse::<Address>()?;
    
    // Main mining loop
    let mut consecutive_errors = 0;
    loop {
        // Check if we've mined enough blocks (for testing)
        if let Some(exit_after) = config.exit_after_blocks {
            let blocks_mined = stats.lock().unwrap().blocks_mined;
            if blocks_mined >= exit_after {
                info!("Reached target of {} blocks mined, exiting", exit_after);
                break;
            }
        }
        
        // Get current challenge from contract
        let (previous_output, difficulty_target) = match get_current_challenge(&provider, &config.contract_address).await {
            Ok(challenge) => {
                consecutive_errors = 0;
                challenge
            },
            Err(e) => {
                error!("Failed to get current challenge: {}", e);
                consecutive_errors += 1;
                
                if consecutive_errors > config.max_retries {
                    return Err(anyhow!("Too many consecutive failures, exiting"));
                }
                
                thread::sleep(config.retry_delay);
                continue;
            }
        };
        
        info!("Mining new block with previous output: {}", hex::encode(&previous_output));
        
        // Generate temporal seed
        let temporal_seed = generate_temporal_seed();
        debug!("Generated temporal seed: {}", hex::encode(&temporal_seed));
        
        // Create thread-safe atomic flag for signaling successful mining
        let solution_found = Arc::new(std::sync::atomic::AtomicBool::new(false));
        
        // Start mining with multiple threads
        let thread_handles = (0..config.threads).map(|thread_id| {
            let wallet = wallet.clone();
            let config = config.clone();
            let signing_key = signing_key.clone();
            let previous_output = previous_output.clone();
            let temporal_seed = temporal_seed.clone();
            let difficulty_target = difficulty_target.clone();
            let stats = Arc::clone(&stats);
            let contract_address = contract_address.clone();
            let solution_found = Arc::clone(&solution_found);
            
            tokio::spawn(async move {
                let mut nonce = thread_id as u64;
                let nonce_increment = config.threads as u64;
                let mut buffer = MiningBuffer::new(previous_output.len() + temporal_seed.len() + 8 + 32 + 16);
                
                while !solution_found.load(std::sync::atomic::Ordering::Relaxed) {
                    // Prepare data for signing with additional entropy
                    buffer.reset();
                    
                    // Generate system entropy and capture precise timestamp
                    let system_entropy: [u8; 32] = rand::thread_rng().gen(); // adds OS noise
                    let timestamp = match SystemTime::now().duration_since(UNIX_EPOCH) {
                        Ok(d) => d.as_nanos(),
                        Err(_) => 0,
                    };
                    
                    buffer.data.extend_from_slice(&previous_output);
                    buffer.data.extend_from_slice(&temporal_seed);
                    buffer.data.extend_from_slice(&system_entropy);
                    buffer.data.extend_from_slice(&timestamp.to_le_bytes());
                    buffer.data.extend_from_slice(&nonce.to_le_bytes());
                    
                    // Sign the data
                    let signature: Signature = signing_key.sign(&buffer.data);
                    
                    // Calculate HMAC
                    let mut mac = HmacSha256::new_from_slice(&buffer.data).unwrap();
                    mac.update(&signature.to_der());
                    let result = mac.finalize();
                    buffer.result_buffer.copy_from_slice(&result.into_bytes());
                    
                    // Update hash counter
                    {
                        let mut stats = stats.lock().unwrap();
                        stats.hashes_computed += 1;
                    }
                    
                    // Check if meets difficulty
                    if meets_difficulty(&buffer.result_buffer, &difficulty_target) {
                        let actual_difficulty = calculate_actual_difficulty(&buffer.result_buffer);
                        
                        // Update highest difficulty if this solution is better
                        {
                            let mut stats = stats.lock().unwrap();
                            if actual_difficulty > stats.highest_difficulty {
                                stats.highest_difficulty = actual_difficulty;
                            }
                        }
                        
                        info!("Found solution! Thread: {}, Nonce: {}, Difficulty: {}", 
                              thread_id, nonce, actual_difficulty);
                        
                        // Signal other threads to stop mining
                        solution_found.store(true, std::sync::atomic::Ordering::Relaxed);
                        
                        // Submit to blockchain
                        match submit_block(
                            &wallet,
                            contract_address,
                            &previous_output,
                            &temporal_seed,
                            nonce,
                            &signature,
                            &buffer.result_buffer,
                        ).await {
                            Ok(receipt) => {
                                info!("Block submitted successfully!");
                                debug!("Transaction receipt: {:?}", receipt);
                                
                                // Calculate actual reward from logs (if available)
                                let reward = extract_reward_from_receipt(&receipt).unwrap_or(1.0);
                                
                                // Update statistics
                                let mut stats = stats.lock().unwrap();
                                stats.blocks_mined += 1;
                                stats.successful_submissions += 1;
                                stats.total_rewards += reward;
                                stats.mining_efficiency = 
                                    (stats.successful_submissions as f64 * 1_000_000.0) / stats.hashes_computed as f64;
                                
                                return Some((buffer.result_buffer.to_vec(), nonce));
                            },
                            Err(e) => {
                                error!("Failed to submit block: {}", e);
                                
                                // Update statistics
                                let mut stats = stats.lock().unwrap();
                                stats.failed_submissions += 1;
                                
                                // Allow other threads to continue
                                solution_found.store(false, std::sync::atomic::Ordering::Relaxed);
                                thread::sleep(config.retry_delay);
                            }
                        }
                    }
                    
                    nonce += nonce_increment;
                }
                
                None
            })
        }).collect::<Vec<_>>();
        
        // Wait for any thread to find a solution
        let results = futures::future::join_all(thread_handles).await;
        let successful_result = results.into_iter()
            .filter_map(|r| r.ok().flatten())
            .next();
        
        if let Some((result, nonce)) = successful_result {
            info!("Successfully mined block with nonce {}", nonce);
            info!("Result hash: {}", hex::encode(&result));
            // Continue with next block
        } else {
            warn!("All mining threads failed, retrying...");
            thread::sleep(config.retry_delay);
        }
    }
    
    Ok(())
}

fn load_config() -> Result<MinerConfig> {
    // Try to load from file first
    let config_path = env::var("CONFIG_PATH").unwrap_or_else(|_| "miner_config.json".to_string());
    let config = if Path::new(&config_path).exists() {
        let config_data = fs::read_to_string(&config_path)?;
        serde_json::from_str(&config_data)?
    } else {
        // Fall back to environment variables
        MinerConfig {
            contract_address: env::var("CONTRACT_ADDRESS")
                .unwrap_or_else(|_| MinerConfig::default().contract_address),
            rpc_url: env::var("RPC_URL")
                .unwrap_or_else(|_| MinerConfig::default().rpc_url),
            private_key_path: env::var("PRIVATE_KEY_PATH").ok(),
            threads: env::var("MINER_THREADS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(MinerConfig::default().threads),
            gas_price_multiplier: env::var("GAS_PRICE_MULTIPLIER")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(MinerConfig::default().gas_price_multiplier),
            retry_delay: Duration::from_secs(
                env::var("RETRY_DELAY_SECONDS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(5),
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
                .unwrap_or(MinerConfig::default().log_level),
            stats_interval: Duration::from_secs(
                env::var("STATS_INTERVAL_SECONDS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(60),
            ),
            exit_after_blocks: env::var("EXIT_AFTER_BLOCKS")
                .ok()
                .and_then(|s| s.parse().ok()),
            max_retries: env::var("MAX_RETRIES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(MinerConfig::default().max_retries),
            ..MinerConfig::default()
        }
    };
    
    // Validate config
    if config.threads < 1 {
        return Err(anyhow!("Thread count must be at least 1"));
    }
    
    // Save config for reference if it was loaded from env vars
    if !Path::new(&config_path).exists() {
        let config_json = serde_json::to_string_pretty(&config)?;
        fs::write(&config_path, config_json)?;
    }
    
    Ok(config)
}

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
        debug!("No key path specified, using ephemeral key");
        Ok(SigningKey::random(&mut OsRng))
    }
}

fn create_wallet(signing_key: &SigningKey, config: &MinerConfig) -> Result<LocalWallet> {
    // Convert k256 signing key to ethers-rs wallet
    let bytes = signing_key.to_bytes();
    let wallet = LocalWallet::from_bytes(&bytes)?;
    
    // Configure wallet with chain id
    let chain_id = env::var("CHAIN_ID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1); // Default to Ethereum mainnet
    
    Ok(wallet.with_chain_id(chain_id))
}

fn extract_reward_from_receipt(receipt: &TransactionReceipt) -> Option<f64> {
    // Find BeaconBlockMined event and extract reward
    for log in &receipt.logs {
        // Topic 0 should be the event signature for BeaconBlockMined
        if log.topics.len() >= 1 && log.topics[0] == keccak256("BeaconBlockMined(address,bytes32,uint256,uint64,uint256)") {
            if log.data.len() >= 32 * 3 {
                // Reward amount is the third parameter (uint256)
                let reward_data = &log.data[32*2..32*3];
                if let Ok(reward) = U256::from_big_endian(reward_data).try_into() {
                    return Some(reward);
                }
            }
        }
    }
    None
}
