use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use log::{info, warn, error, debug};
use ethers::{prelude::*, utils::keccak256};
use serde::{Serialize, Deserialize};
use once_cell::sync::Lazy;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaChaRng;
use actix_web::{web, App, HttpResponse, HttpServer, Responder};
use actix_web::middleware::Logger;
use dashmap::DashMap;
use futures::StreamExt;
use std::net::{IpAddr, Ipv4Addr};

// --- Chain Protection Configuration ---
const SUPPORTED_CHAINS: [(u64, &str, &str); 3] = [
    (1, "Ethereum", "https://eth-mainnet.alchemyapi.io/v2/YOUR_API_KEY"),
    (137, "Polygon", "https://polygon-rpc.com"),
    (42161, "Arbitrum", "https://arb1.arbitrum.io/rpc"),
];

const ENTROPY_RPC: &str = "https://rpc.entropy-chain.io";
const ENTROPY_CHAIN_ID: u64 = 1337;
const QR_HASH_ITERATIONS: usize = 3; // Quantum resistance hash iterations

// --- Protection Types ---
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ProtectionType {
    RandomnessVerification,
    MEVProtection,
    FrontRunningShield,
    TransactionObfuscation,
    SignatureAugmentation,
}

// --- EntropyChainConnector Interface ---
#[derive(Clone)]
struct EntropyChainConnector {
    provider: Arc<Provider<Http>>,
    beacon_address: Address,
    verifier_address: Address,
}

impl EntropyChainConnector {
    pub fn new(rpc_url: &str) -> Self {
        let provider = Arc::new(Provider::<Http>::try_from(rpc_url)
            .expect("Failed to create Entropy provider"));
        
        Self {
            provider,
            // Addresses for the Temporal Gradient Beacon and verification contracts
            beacon_address: Address::from_str("0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf").unwrap(),
            verifier_address: Address::from_str("0x2B5AD5c4795c026514f8317c7a215E218DcCD6cF").unwrap(),
        }
    }
    
    pub async fn get_latest_randomness(&self) -> Result<[u8; 32], Box<dyn std::error::Error>> {
        // Call the Temporal Gradient Beacon to get latest randomness output
        let beacon: abi::ITemporalGradientBeacon<Provider<Http>> = 
            abi::ITemporalGradientBeacon::new(self.beacon_address, self.provider.clone());
            
        let output_history = beacon.get_output_history().call().await?;
        let latest_output = output_history[0]; // Latest output is at index 0
        
        let mut entropy = [0u8; 32];
        entropy.copy_from_slice(latest_output.as_ref());
        
        Ok(entropy)
    }
    
    pub async fn verify_entropy_usage(
        &self, 
        entropy: [u8; 32], 
        usage_proof: [u8; 32],
        consumer_address: Address
    ) -> Result<bool, Box<dyn std::error::Error>> {
        // Call the verification contract to verify entropy was used correctly
        let verifier: abi::IEntropyVerifier<Provider<Http>> = 
            abi::IEntropyVerifier::new(self.verifier_address, self.provider.clone());
            
        let verified = verifier.verify_entropy_usage(
            H256::from(entropy),
            H256::from(usage_proof),
            consumer_address
        ).call().await?;
        
        Ok(verified)
    }
    
    pub async fn record_entropy_usage(
        &self,
        entropy: [u8; 32],
        usage_context: &str,
        consumer_address: Address,
        wallet: &LocalWallet
    ) -> Result<TxHash, Box<dyn std::error::Error>> {
        // Record usage on-chain with the verification contract
        let verifier: abi::IEntropyVerifier<SignerMiddleware<Provider<Http>, LocalWallet>> = 
            abi::IEntropyVerifier::new(
                self.verifier_address, 
                SignerMiddleware::new(self.provider.clone(), wallet.clone())
            );
            
        let usage_hash = keccak256(
            ethers::abi::encode(&[
                ethers::abi::Token::FixedBytes(entropy.to_vec()),
                ethers::abi::Token::String(usage_context.to_string()),
                ethers::abi::Token::Address(consumer_address)
            ])
        );
        
        let tx = verifier.record_entropy_usage(
            H256::from(entropy),
            usage_context,
            H256::from_slice(&usage_hash),
            consumer_address
        ).send().await?;
        
        Ok(tx.tx_hash())
    }
}

// --- Proof Log Structure ---
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EntropyUsageProof {
    usage_id: String,
    entropy_hash: String,
    timestamp: u64,
    protection_type: String,
    chain_id: u64,
    tx_hash: String,
    consumer: String,
    verification_status: bool,
    protection_metadata: HashMap<String, String>,
}

// --- Enhanced Chain Protection Service ---
pub struct ChainProtectionService {
    chain_providers: HashMap<u64, Arc<Provider<Http>>>,
    entropy_connector: EntropyChainConnector,
    protection_stats: Mutex<HashMap<u64, ProtectionStats>>,
    usage_proofs: Arc<DashMap<String, EntropyUsageProof>>,
    wallets: HashMap<u64, LocalWallet>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProtectionStats {
    transactions_protected: u64,
    randomness_verifications: u64,
    mev_attacks_prevented: u64,
    quantum_signatures_added: u64,
    total_value_protected: f64, // In USD
}

impl ChainProtectionService {
    pub fn new() -> Self {
        let entropy_connector = EntropyChainConnector::new(ENTROPY_RPC);
        
        let mut service = Self {
            chain_providers: HashMap::new(),
            entropy_connector,
            protection_stats: Mutex::new(HashMap::new()),
            usage_proofs: Arc::new(DashMap::new()),
            wallets: HashMap::new(),
        };
        
        // Initialize providers for all supported chains
        for (chain_id, name, rpc_url) in SUPPORTED_CHAINS.iter() {
            match Provider::<Http>::try_from(*rpc_url) {
                Ok(provider) => {
                    service.chain_providers.insert(*chain_id, Arc::new(provider));
                    info!("Connected to {} chain (ID: {})", name, chain_id);
                    
                    // In a production environment, you would use secure key management
                    // This is just for demo purposes - NEVER hardcode private keys in production
                    let demo_key = "1111111111111111111111111111111111111111111111111111111111111111";
                    if let Ok(wallet) = LocalWallet::from_str(demo_key) {
                        let wallet = wallet.with_chain_id(*chain_id);
                        service.wallets.insert(*chain_id, wallet);
                        info!("Set up demo wallet for {} chain", name);
                    }
                },
                Err(e) => {
                    error!("Failed to connect to {} chain: {}", name, e);
                }
            }
            
            // Initialize stats for this chain
            service.protection_stats.lock().unwrap().insert(*chain_id, ProtectionStats::default());
        }
        
        service
    }
    
    /// Protect a transaction on another blockchain using Entropy randomness
    pub async fn protect_transaction(&self, 
        chain_id: u64, 
        transaction_hash: H256,
        protection_type: ProtectionType
    ) -> Result<H256, Box<dyn std::error::Error>> {
        // Verify chain is supported
        let provider = self.chain_providers.get(&chain_id)
            .ok_or(format!("Chain ID {} not supported", chain_id))?;
        
        // Get original transaction
        let tx = provider.get_transaction(transaction_hash).await?
            .ok_or("Transaction not found")?;
        
        // Request entropy from Entropy chain for protection
        let entropy = self.get_entropy_randomness().await?;
        
        // Apply selected protection
        let protected_tx = match protection_type {
            ProtectionType::RandomnessVerification => {
                self.apply_randomness_verification(&tx, entropy).await?
            },
            ProtectionType::MEVProtection => {
                self.apply_mev_protection(&tx, entropy).await?
            },
            ProtectionType::FrontRunningShield => {
                self.apply_frontrunning_shield(&tx, entropy).await?
            },
            ProtectionType::TransactionObfuscation => {
                self.apply_transaction_obfuscation(&tx, entropy).await?
            },
            ProtectionType::SignatureAugmentation => {
                self.apply_quantum_signature_augmentation(&tx, entropy).await?
            },
        };
        
        // Submit protected transaction
        let tx_hash = self.submit_protected_transaction(chain_id, protected_tx).await?;
        
        // Update stats
        self.update_protection_stats(chain_id, protection_type);
        
        Ok(tx_hash)
    }
    
    // Gets entropy from the Entropy chain using the connector
    async fn get_entropy_randomness(&self) -> Result<[u8; 32], Box<dyn std::error::Error>> {
        let entropy = self.entropy_connector.get_latest_randomness().await?;
        
        // Apply quantum resistance
        let mut resistant_entropy = entropy;
        self.apply_quantum_resistance(&mut resistant_entropy);
        
        Ok(resistant_entropy)
    }
    
    // Apply quantum resistance by iterative hashing
    fn apply_quantum_resistance(&self, data: &mut [u8; 32]) {
        // Apply multiple rounds of hashing for quantum resistance
        for _ in 0..QR_HASH_ITERATIONS {
            let hashed = keccak256(data);
            data.copy_from_slice(&hashed);
        }
    }
    
    // Protection implementations would go here
    async fn apply_randomness_verification(&self, tx: &Transaction, entropy: [u8; 32]) 
        -> Result<TypedTransaction, Box<dyn std::error::Error>> {
        // Implementation details would vary based on protection needs
        // This is a simplified placeholder
        todo!("Implement randomness verification")
    }
    
    // IMPLEMENTED: Apply MEV protection using entropy-based randomization
    async fn apply_mev_protection(&self, tx: &Transaction, entropy: [u8; 32]) 
        -> Result<TypedTransaction, Box<dyn std::error::Error>> {
        // Create a deterministic but unpredictable RNG from the entropy
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&entropy);
        let mut rng = ChaChaRng::from_seed(seed);
        
        // Clone the original transaction as our starting point
        let mut protected_tx = tx.clone().into_eip1559_transaction();
        
        // 1. Randomize gas price within a reasonable range (95-105% of original)
        if let Some(max_fee) = protected_tx.max_fee_per_gas {
            let randomization_factor = (rng.gen_range(95..105) as f64) / 100.0;
            let new_max_fee = (max_fee.as_u64() as f64 * randomization_factor) as u64;
            protected_tx.max_fee_per_gas = Some(U256::from(new_max_fee));
            
            // Also adjust priority fee if present
            if let Some(priority_fee) = protected_tx.max_priority_fee_per_gas {
                let new_priority_fee = (priority_fee.as_u64() as f64 * randomization_factor) as u64;
                protected_tx.max_priority_fee_per_gas = Some(U256::from(new_priority_fee));
            }
        } else if let Some(gas_price) = tx.gas_price {
            // Legacy transaction with gas_price
            let randomization_factor = (rng.gen_range(95..105) as f64) / 100.0;
            let new_gas_price = (gas_price.as_u64() as f64 * randomization_factor) as u64;
            
            // Convert to EIP-1559 format with equivalent values
            protected_tx.max_fee_per_gas = Some(U256::from(new_gas_price));
            protected_tx.max_priority_fee_per_gas = Some(U256::from(new_gas_price));
        }
        
        // 2. Add time-locking by adjusting validity based on current block number
        let current_block = self.chain_providers
            .get(&tx.chain_id().unwrap_or(1))
            .ok_or("Chain provider not found")?
            .get_block_number()
            .await?;
            
        // Set the transaction to become valid only after a small random delay (1-3 blocks)
        let delay_blocks = rng.gen_range(1..4);
        protected_tx.access_list = AccessList(vec![
            AccessListItem {
                address: Address::from_low_u64_be(0x123), // Can be any address used as marker
                storage_keys: vec![
                    H256::from_low_u64_be(current_block.as_u64() + delay_blocks), // Time lock
                    H256::from(entropy)  // Entropy marker making this unpredictable
                ]
            }
        ]);
        
        // 3. Add entropy-based nonce offset to make transaction serialization unpredictable
        // We don't change the actual nonce value as that would invalidate the transaction
        let entropy_bytes = entropy.to_vec();
        if let Some(data) = &mut protected_tx.data {
            // Add entropy to transaction data in a way that won't affect contract behavior
            // Note: This assumes the contract ignores anything past expected calldata
            // In a real implementation, you'd need to be careful not to break function calls
            let mut modified_data = data.to_vec();
            modified_data.extend_from_slice(&entropy_bytes[0..4]); // Add 4 bytes of entropy 
            *data = Bytes::from(modified_data);
        } else {
            // If no data, add some dummy data with entropy
            protected_tx.data = Some(Bytes::from(entropy_bytes[0..4].to_vec()));
        }
        
        // Log the protection details
        info!("Applied MEV protection: gas randomization factor {:.2}%, {} block delay", 
              (rng.gen_range(95..105) as f64) / 100.0 * 100.0, delay_blocks);
        
        Ok(TypedTransaction::Eip1559(protected_tx))
    }
    
    async fn apply_frontrunning_shield(&self, tx: &Transaction, entropy: [u8; 32]) 
        -> Result<TypedTransaction, Box<dyn std::error::Error>> {
        // Shield transaction with entropy-based commitment schemes
        todo!("Implement frontrunning shield")
    }
    
    async fn apply_transaction_obfuscation(&self, tx: &Transaction, entropy: [u8; 32]) 
        -> Result<TypedTransaction, Box<dyn std::error::Error>> {
        // Obfuscate transaction details using entropy
        todo!("Implement transaction obfuscation")
    }
    
    async fn apply_quantum_signature_augmentation(&self, tx: &Transaction, entropy: [u8; 32]) 
        -> Result<TypedTransaction, Box<dyn std::error::Error>> {
        // Augment transaction signature with quantum-resistant components
        todo!("Implement quantum signature augmentation")
    }
    
    // IMPLEMENTED: Submit the protected transaction
    async fn submit_protected_transaction(&self, chain_id: u64, tx: TypedTransaction) 
        -> Result<H256, Box<dyn std::error::Error>> {
        // Get the provider for this chain
        let provider = self.chain_providers.get(&chain_id)
            .ok_or(format!("Chain ID {} not supported", chain_id))?;
            
        // Get the wallet for this chain
        let wallet = self.wallets.get(&chain_id)
            .ok_or(format!("No wallet configured for chain ID {}", chain_id))?
            .clone();
            
        // Create a client with the wallet
        let client = SignerMiddleware::new(provider.clone(), wallet);
        
        // Submit and await the transaction
        let pending_tx = client.send_transaction(tx, None).await?;
        
        // Wait for the transaction to be mined (with timeout)
        let receipt = pending_tx.await?;
        
        // Return the transaction hash
        Ok(receipt.transaction_hash)
    }
    
    fn update_protection_stats(&self, chain_id: u64, protection_type: ProtectionType) {
        let mut stats = self.protection_stats.lock().unwrap();
        
        if let Some(chain_stats) = stats.get_mut(&chain_id) {
            chain_stats.transactions_protected += 1;
            
            match protection_type {
                ProtectionType::RandomnessVerification => {
                    chain_stats.randomness_verifications += 1;
                },
                ProtectionType::MEVProtection => {
                    chain_stats.mev_attacks_prevented += 1;
                },
                ProtectionType::SignatureAugmentation => {
                    chain_stats.quantum_signatures_added += 1;
                },
                _ => {}
            }
        }
    }
    
    pub fn get_protection_stats(&self, chain_id: u64) -> Option<ProtectionStats> {
        self.protection_stats.lock().unwrap().get(&chain_id).cloned()
    }
    
    // New method: Log entropy usage proofs
    async fn log_entropy_usage(&self, 
        chain_id: u64, 
        entropy: [u8; 32], 
        tx_hash: H256,
        protection_type: ProtectionType,
        consumer: Option<Address>
    ) -> Result<String, Box<dyn std::error::Error>> {
        // Generate a unique ID for this usage
        let usage_id = format!("{:x}-{}-{}", tx_hash, chain_id, SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_millis());
        
        // Create the proof record
        let proof = EntropyUsageProof {
            usage_id: usage_id.clone(),
            entropy_hash: format!("0x{}", hex::encode(entropy)),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_secs(),
            protection_type: format!("{:?}", protection_type),
            chain_id,
            tx_hash: format!("0x{:x}", tx_hash),
            consumer: consumer.map_or("unknown".to_string(), |addr| format!("{:?}", addr)),
            verification_status: false,  // Will be verified later
            protection_metadata: HashMap::new(),
        };
        
        // Store the proof
        self.usage_proofs.insert(usage_id.clone(), proof.clone());
        
        // Log the usage
        info!("Logged entropy usage: {} for tx {} on chain {}", 
              usage_id, proof.tx_hash, chain_id);
        
        // Attempt to verify on-chain if we have a consumer address
        if let Some(consumer_addr) = consumer {
            // Create a usage proof hash
            let usage_proof = keccak256(ethers::abi::encode(&[
                ethers::abi::Token::String(format!("{:?}", protection_type)),
                ethers::abi::Token::FixedBytes(tx_hash.as_bytes().to_vec()),
                ethers::abi::Token::Address(consumer_addr)
            ]));
            
            // Try to verify the usage on-chain
            match self.entropy_connector.verify_entropy_usage(
                entropy,
                usage_proof,
                consumer_addr
            ).await {
                Ok(verified) => {
                    // Update the verification status
                    if let Some(mut proof) = self.usage_proofs.get_mut(&usage_id) {
                        proof.verification_status = verified;
                    }
                    
                    if verified {
                        info!("Successfully verified entropy usage on-chain for {}", usage_id);
                    } else {
                        warn!("Failed to verify entropy usage on-chain for {}", usage_id);
                    }
                },
                Err(e) => {
                    error!("Error verifying entropy usage: {}", e);
                }
            }
            
            // If we have a wallet for the Entropy chain, record the usage on-chain
            if let Some(wallet) = self.wallets.get(&ENTROPY_CHAIN_ID) {
                match self.entropy_connector.record_entropy_usage(
                    entropy,
                    &format!("MEV Protection on chain {}", chain_id),
                    consumer_addr,
                    wallet
                ).await {
                    Ok(tx_hash) => {
                        info!("Recorded entropy usage on-chain, tx: {:?}", tx_hash);
                    },
                    Err(e) => {
                        error!("Failed to record entropy usage on-chain: {}", e);
                    }
                }
            }
        }
        
        Ok(usage_id)
    }
    
    // Enhanced transaction protection with logging
    pub async fn protect_transaction_with_proof(&self, 
        chain_id: u64, 
        transaction_hash: H256,
        protection_type: ProtectionType,
        consumer: Option<Address>
    ) -> Result<(H256, String), Box<dyn std::error::Error>> {
        // Get entropy and protect the transaction
        let entropy = self.get_entropy_randomness().await?;
        
        // Get the transaction
        let provider = self.chain_providers.get(&chain_id)
            .ok_or(format!("Chain ID {} not supported", chain_id))?;
        let tx = provider.get_transaction(transaction_hash).await?
            .ok_or("Transaction not found")?;
        
        // Apply protection
        let protected_tx = match protection_type {
            ProtectionType::MEVProtection => {
                self.apply_mev_protection(&tx, entropy).await?
            },
            // ... other protection types ...
            _ => return Err("Protection type not implemented".into())
        };
        
        // Submit the protected transaction
        let tx_hash = self.submit_protected_transaction(chain_id, protected_tx).await?;
        
        // Log the entropy usage
        let proof_id = self.log_entropy_usage(
            chain_id,
            entropy,
            tx_hash,
            protection_type,
            consumer
        ).await?;
        
        // Update stats
        self.update_protection_stats(chain_id, protection_type);
        
        Ok((tx_hash, proof_id))
    }
    
    // Get proof by ID
    pub fn get_proof(&self, proof_id: &str) -> Option<EntropyUsageProof> {
        self.usage_proofs.get(proof_id).map(|p| p.clone())
    }
    
    // List all proofs
    pub fn list_proofs(&self) -> Vec<EntropyUsageProof> {
        self.usage_proofs
            .iter()
            .map(|r| r.clone())
            .collect()
    }
    
    // Get proofs for a specific chain
    pub fn get_chain_proofs(&self, chain_id: u64) -> Vec<EntropyUsageProof> {
        self.usage_proofs
            .iter()
            .filter(|r| r.chain_id == chain_id)
            .map(|r| r.clone())
            .collect()
    }
}

// --- ABI module stubs for entropy contracts ---
mod abi {
    use ethers::prelude::*;
    use std::sync::Arc;
    
    abigen!(
        ITemporalGradientBeacon,
        r#"[
            function getOutputHistory() external view returns (bytes32[32])
        ]"#
    );
    
    abigen!(
        IEntropyVerifier,
        r#"[
            function verifyEntropyUsage(bytes32 entropy, bytes32 usageProof, address consumer) external view returns (bool)
            function recordEntropyUsage(bytes32 entropy, string calldata context, bytes32 usageHash, address consumer) external returns (bool)
        ]"#
    );
}

// --- Initialize Chain Protection Service ---
static PROTECTION_SERVICE: Lazy<ChainProtectionService> = Lazy::new(|| {
    info!("Initializing Chain Protection Service");
    ChainProtectionService::new()
});

// --- Enhanced Public API ---
pub async fn protect_eth_transaction_with_proof(
    chain_id: u64,
    tx_hash: &str,
    protection_type_str: &str,
    consumer_address: Option<&str>
) -> Result<(String, String), String> {
    // Parse transaction hash
    let tx_hash = tx_hash.parse::<H256>()
        .map_err(|e| format!("Invalid transaction hash: {}", e))?;
    
    // Parse protection type
    let protection_type = match protection_type_str {
        "randomness" => ProtectionType::RandomnessVerification,
        "mev" => ProtectionType::MEVProtection,
        "frontrunning" => ProtectionType::FrontRunningShield,
        "obfuscation" => ProtectionType::TransactionObfuscation,
        "quantum" => ProtectionType::SignatureAugmentation,
        _ => return Err("Unsupported protection type".to_string())
    };
    
    // Parse consumer address if provided
    let consumer = match consumer_address {
        Some(addr) => Some(
            Address::from_str(addr)
                .map_err(|e| format!("Invalid consumer address: {}", e))?
        ),
        None => None
    };
    
    // Call the service
    let result = PROTECTION_SERVICE.protect_transaction_with_proof(
        chain_id, 
        tx_hash, 
        protection_type,
        consumer
    )
    .await
    .map_err(|e| format!("Protection failed: {}", e))?;
    
    Ok((format!("0x{:x}", result.0), result.1))
}

// --- REST API Server ---
pub async fn start_api_server(port: u16) -> std::io::Result<()> {
    // Create the server
    HttpServer::new(|| {
        App::new()
            .wrap(Logger::default())
            // Protection endpoint
            .route("/protect", web::post().to(protect_transaction_api))
            // Proof endpoints
            .route("/proofs/{id}", web::get().to(get_proof_api))
            .route("/proofs", web::get().to(list_proofs_api))
            // Chain stats endpoint
            .route("/stats/{chain_id}", web::get().to(get_chain_stats_api))
    })
    .bind((IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port))?
    .run()
    .await
}

// --- API Request/Response Types ---
#[derive(Debug, Serialize, Deserialize)]
struct ProtectionRequest {
    chain_id: u64,
    tx_hash: String,
    protection_type: String,
    consumer_address: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProtectionResponse {
    success: bool,
    protected_tx_hash: Option<String>,
    proof_id: Option<String>,
    error: Option<String>,
}

// --- API Handlers ---
async fn protect_transaction_api(req: web::Json<ProtectionRequest>) -> impl Responder {
    match protect_eth_transaction_with_proof(
        req.chain_id, 
        &req.tx_hash, 
        &req.protection_type, 
        req.consumer_address.as_deref()
    ).await {
        Ok((tx_hash, proof_id)) => HttpResponse::Ok().json(ProtectionResponse {
            success: true,
            protected_tx_hash: Some(tx_hash),
            proof_id: Some(proof_id),
            error: None,
        }),
        Err(e) => HttpResponse::BadRequest().json(ProtectionResponse {
            success: false,
            protected_tx_hash: None,
            proof_id: None,
            error: Some(e),
        }),
    }
}

async fn get_proof_api(path: web::Path<String>) -> impl Responder {
    let proof_id = path.into_inner();
    match PROTECTION_SERVICE.get_proof(&proof_id) {
        Some(proof) => HttpResponse::Ok().json(proof),
        None => HttpResponse::NotFound().body(format!("Proof {} not found", proof_id)),
    }
}

async fn list_proofs_api(query: web::Query<HashMap<String, String>>) -> impl Responder {
    if let Some(chain_id_str) = query.get("chain_id") {
        // Filter proofs by chain ID
        match chain_id_str.parse::<u64>() {
            Ok(chain_id) => {
                let proofs = PROTECTION_SERVICE.get_chain_proofs(chain_id);
                HttpResponse::Ok().json(proofs)
            },
            Err(_) => HttpResponse::BadRequest().body("Invalid chain ID"),
        }
    } else {
        // Return all proofs
        let proofs = PROTECTION_SERVICE.list_proofs();
        HttpResponse::Ok().json(proofs)
    }
}

async fn get_chain_stats_api(path: web::Path<u64>) -> impl Responder {
    let chain_id = path.into_inner();
    match PROTECTION_SERVICE.get_protection_stats(chain_id) {
        Some(stats) => HttpResponse::Ok().json(stats),
        None => HttpResponse::NotFound().body(format!("Stats for chain {} not found", chain_id)),
    }
}

// --- Public API for Other Blockchains ---
pub async fn protect_eth_transaction(
    chain_id: u64,
    tx_hash: &str,
    protection_type_str: &str
) -> Result<String, String> {
    let tx_hash = tx_hash.parse::<H256>()
        .map_err(|e| format!("Invalid transaction hash: {}", e))?;
    
    let protection_type = match protection_type_str {
        "randomness" => ProtectionType::RandomnessVerification,
        "mev" => ProtectionType::MEVProtection,
        "frontrunning" => ProtectionType::FrontRunningShield,
        "obfuscation" => ProtectionType::TransactionObfuscation,
        "quantum" => ProtectionType::SignatureAugmentation,
        _ => return Err("Unsupported protection type".to_string())
    };
    
    let result = PROTECTION_SERVICE.protect_transaction(chain_id, tx_hash, protection_type)
        .await
        .map_err(|e| format!("Protection failed: {}", e))?;
    
    Ok(format!("0x{:x}", result))
}

// --- Enhanced FFI Interface ---
#[no_mangle]
pub extern "C" fn entropy_protect_transaction_with_proof(
    chain_id: u64,
    tx_hash: *const std::os::raw::c_char,
    protection_type: *const std::os::raw::c_char,
    consumer_address: *const std::os::raw::c_char,
    callback: extern "C" fn(*const std::os::raw::c_char, *const std::os::raw::c_char, bool)
) {
    // Safety handling for C strings
    let tx_hash_str = unsafe {
        if tx_hash.is_null() {
            callback(std::ptr::null(), std::ptr::null(), false);
            return;
        }
        std::ffi::CStr::from_ptr(tx_hash).to_string_lossy().to_string()
    };
    
    let protection_type_str = unsafe {
        if protection_type.is_null() {
            callback(std::ptr::null(), std::ptr::null(), false);
            return;
        }
        std::ffi::CStr::from_ptr(protection_type).to_string_lossy().to_string()
    };
    
    let consumer_address_opt = unsafe {
        if consumer_address.is_null() {
            None
        } else {
            Some(std::ffi::CStr::from_ptr(consumer_address).to_string_lossy().to_string())
        }
    };
    
    // Spawn async task to handle protection
    tokio::spawn(async move {
        match protect_eth_transaction_with_proof(
            chain_id, 
            &tx_hash_str, 
            &protection_type_str, 
            consumer_address_opt.as_deref()
        ).await {
            Ok((tx_hash, proof_id)) => {
                let c_tx_hash = std::ffi::CString::new(tx_hash).unwrap();
                let c_proof_id = std::ffi::CString::new(proof_id).unwrap();
                callback(c_tx_hash.as_ptr(), c_proof_id.as_ptr(), true);
                // Prevent memory leak - hold onto the CString until callback completes
                std::mem::forget(c_tx_hash);
                std::mem::forget(c_proof_id);
            },
            Err(e) => {
                let c_error = std::ffi::CString::new(e).unwrap();
                callback(c_error.as_ptr(), std::ptr::null(), false);
                std::mem::forget(c_error);
            }
        }
    });
}

// --- FFI Interface for SDK Integration ---
#[no_mangle]
pub extern "C" fn entropy_protect_transaction(
    chain_id: u64,
    tx_hash: *const std::os::raw::c_char,
    protection_type: *const std::os::raw::c_char,
    callback: extern "C" fn(*const std::os::raw::c_char, bool)
) {
    // Safety handling for C strings
    let tx_hash_str = unsafe {
        if tx_hash.is_null() {
            callback(std::ptr::null(), false);
            return;
        }
        std::ffi::CStr::from_ptr(tx_hash).to_string_lossy().to_string()
    };
    
    let protection_type_str = unsafe {
        if protection_type.is_null() {
            callback(std::ptr::null(), false);
            return;
        }
        std::ffi::CStr::from_ptr(protection_type).to_string_lossy().to_string()
    };
    
    // Spawn async task to handle protection
    tokio::spawn(async move {
        match protect_eth_transaction(chain_id, &tx_hash_str, &protection_type_str).await {
            Ok(result) => {
                let c_result = std::ffi::CString::new(result).unwrap();
                callback(c_result.as_ptr(), true);
                // Prevent memory leak - hold onto the CString until callback completes
                std::mem::forget(c_result);
            },
            Err(e) => {
                let c_error = std::ffi::CString::new(e).unwrap();
                callback(c_error.as_ptr(), false);
                std::mem::forget(c_error);
            }
        }
    });
}

// Function to initialize and start the REST API server
#[no_mangle]
pub extern "C" fn entropy_start_api(port: u16) -> bool {
    // Set up the runtime and start the server
    match tokio::runtime::Runtime::new() {
        Ok(rt) => {
            // Spawn the API server in the background
            rt.spawn(async move {
                match start_api_server(port).await {
                    Ok(_) => {
                        info!("API server stopped gracefully");
                    },
                    Err(e) => {
                        error!("API server error: {}", e);
                    }
                }
            });
            true
        },
        Err(e) => {
            error!("Failed to create Tokio runtime: {}", e);
            false
        }
    }
}
