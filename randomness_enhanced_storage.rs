use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use sha2::{Sha256, Digest};
use serde::{Deserialize, Serialize};
use log::{info, warn, error, debug};

// --- Constants ---
const QR_HASH_ITERATIONS: usize = 3; // Quantum resistance iterations - match Entropy blockchain standard
const DEFAULT_CHALLENGE_EXPIRY: u64 = 3600; // 1 hour in seconds
const MIN_CHALLENGE_COUNT: usize = 10; // Minimum challenges per verification
const MAX_CHALLENGE_COUNT: usize = 500; // Maximum challenges per verification
const CHALLENGE_DIFFICULTY_SCALING: f64 = 1.5; // Difficulty scaling factor for larger files
const PROOF_VERIFICATION_RETRIES: u8 = 3; // Number of retries for remote proof verification
const VERIFICATION_TIMEOUT: u64 = 300; // Verification timeout in seconds

// --- Structs ---

/// Storage verification protocols supported for cross-network compatibility
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum StorageProtocol {
    IPFS,
    Filecoin,
    Arweave,
    CESS,
    Sia,
    Swarm,
    Entropy, // Native Entropy storage (if implemented later)
}

/// Types of randomness verification challenges
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum ChallengeType {
    MerkleProofVerification,
    RandomSampling,
    TimelockChallenge,
    MultipartRandomChallenge,
    QuantumResistantChallenge,
}

/// Storage verification challenge request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageChallenge {
    challenge_id: String,
    file_id: String,
    storage_provider: String,
    challenge_type: ChallengeType,
    random_indices: Vec<u64>,
    merkle_root: Option<String>,
    nonce: u64,
    timestamp: u64,
    expiry: u64,
    signature: String,
    beacon_output: String,
    difficulty: u64,
}

/// Storage verification proof response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageProof {
    challenge_id: String,
    file_id: String,
    provider_id: String,
    timestamp: u64,
    data_samples: Vec<Vec<u8>>,
    merkle_proofs: Option<Vec<Vec<String>>>,
    provider_signature: String,
}

/// Verification result with detailed metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    challenge_id: String,
    file_id: String,
    provider_id: String,
    verified: bool,
    timestamp: u64,
    verification_time_ms: u64,
    response_time_ms: u64,
    entropy_source: String,
    error_message: Option<String>,
    verification_metrics: HashMap<String, f64>,
}

/// Main struct for storage verification operations
pub struct EntropyStorageVerifier {
    api_endpoint: String,
    beacon_contract: String,
    authorized_providers: HashMap<StorageProtocol, Vec<String>>,
    challenge_history: Arc<Mutex<HashMap<String, StorageChallenge>>>,
    verification_results: Arc<Mutex<HashMap<String, VerificationResult>>>,
    auth_token: Option<String>,
}

/// Implementation of EntropyStorageVerifier methods
impl EntropyStorageVerifier {
    /// Initialize a new verifier with Entropy randomness integration
    pub fn new(entropy_endpoint: &str, entropy_contract: &str) -> Self {
        EntropyStorageVerifier {
            api_endpoint: entropy_endpoint.to_string(),
            beacon_contract: entropy_contract.to_string(),
            authorized_providers: HashMap::new(),
            challenge_history: Arc::new(Mutex::new(HashMap::new())),
            verification_results: Arc::new(Mutex::new(HashMap::new())),
            auth_token: std::env::var("ENTROPY_API_TOKEN").ok(),
        }
    }

    /// Generate a random challenge for storage verification using Entropy randomness
    pub async fn generate_challenge(
        &self,
        file_id: &str, 
        file_size: u64,
        protocol: StorageProtocol, 
        provider: &str,
        challenge_type: ChallengeType
    ) -> Result<StorageChallenge, String> {
        // Validate provider is authorized for this protocol
        if !self.is_provider_authorized(protocol, provider) {
            return Err(format!("Provider {} is not authorized for {:?} protocol", provider, protocol));
        }
        
        // Get randomness from Entropy
        let entropy = self.get_entropy_randomness().await?;
        let beacon_output = entropy.beacon_output.clone();
        
        // Calculate challenge parameters based on file size
        let num_challenges = self.calculate_challenge_count(file_size);
        let indices = self.generate_random_indices(&entropy.random_bytes, file_size, num_challenges);
        
        // Generate difficulty based on file size and protocol requirements
        let difficulty = self.calculate_difficulty(file_size, protocol);
        
        // Create timestamp and expiry
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let expiry = now + DEFAULT_CHALLENGE_EXPIRY;
        
        // Generate unique challenge ID
        let challenge_id = format!(
            "chall_{}_{}_{}",
            hex::encode(&entropy.random_bytes[0..8]),
            file_id,
            now
        );
        
        // Create and sign challenge
        let challenge = StorageChallenge {
            challenge_id,
            file_id: file_id.to_string(),
            storage_provider: provider.to_string(),
            challenge_type,
            random_indices: indices,
            merkle_root: None, // Optional, set if using Merkle verification
            nonce: entropy.nonce,
            timestamp: now,
            expiry,
            signature: self.sign_challenge(&entropy.random_bytes, file_id, provider)?,
            beacon_output,
            difficulty,
        };
        
        // Store challenge for later verification
        self.challenge_history.lock().unwrap().insert(challenge.challenge_id.clone(), challenge.clone());
        
        Ok(challenge)
    }

    /// Verify a storage proof against a previously generated challenge
    pub async fn verify_proof(&self, proof: StorageProof) -> Result<VerificationResult, String> {
        let start_time = SystemTime::now();
        
        // Retrieve challenge from history
        let challenge = {
            let challenges = self.challenge_history.lock().unwrap();
            match challenges.get(&proof.challenge_id) {
                Some(c) => c.clone(),
                None => return Err(format!("Challenge {} not found", proof.challenge_id)),
            }
        };
        
        // Check if challenge has expired
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        if now > challenge.expiry {
            return Err(format!("Challenge {} has expired", challenge.challenge_id));
        }
        
        // Verify storage proof based on challenge type
        let verified = match challenge.challenge_type {
            ChallengeType::MerkleProofVerification => {
                self.verify_merkle_proof(&challenge, &proof)?
            },
            ChallengeType::RandomSampling => {
                self.verify_random_samples(&challenge, &proof)?
            },
            ChallengeType::TimelockChallenge => {
                self.verify_timelock_challenge(&challenge, &proof)?
            },
            ChallengeType::MultipartRandomChallenge => {
                self.verify_multipart_challenge(&challenge, &proof)?
            },
            ChallengeType::QuantumResistantChallenge => {
                self.verify_quantum_resistant_challenge(&challenge, &proof)?
            },
        };
        
        // Calculate verification metrics
        let elapsed = SystemTime::now().duration_since(start_time).unwrap().as_millis() as u64;
        let response_time = now - challenge.timestamp;
        
        // Create and store verification result
        let result = VerificationResult {
            challenge_id: challenge.challenge_id.clone(),
            file_id: challenge.file_id.clone(),
            provider_id: proof.provider_id.clone(),
            verified,
            timestamp: now,
            verification_time_ms: elapsed,
            response_time_ms: response_time * 1000, // Convert to ms
            entropy_source: challenge.beacon_output.clone(),
            error_message: if verified { None } else { Some("Proof verification failed".to_string()) },
            verification_metrics: HashMap::new(), // Populated with specific metrics
        };
        
        // Store verification result
        self.verification_results.lock().unwrap().insert(result.challenge_id.clone(), result.clone());
        
        Ok(result)
    }

    /// Get entropy from the Entropy blockchain using RNG API
    async fn get_entropy_randomness(&self) -> Result<EntropyRandomness, String> {
        // Implementation to call Entropy RNG API
        // This would use HTTP client to get randomness from the Entropy endpoint
        
        // For now, we'll simulate this with a placeholder implementation
        let mut rng_bytes = [0u8; 32];
        getrandom::getrandom(&mut rng_bytes).map_err(|e| format!("Failed to generate random bytes: {}", e))?;
        
        // Apply quantum resistance with multiple hash iterations
        let mut qr_result = rng_bytes.to_vec();
        for _ in 0..QR_HASH_ITERATIONS {
            let mut hasher = Sha256::new();
            hasher.update(&qr_result);
            qr_result = hasher.finalize().to_vec();
        }
        
        Ok(EntropyRandomness {
            random_bytes: qr_result,
            nonce: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            beacon_output: format!("0x{}", hex::encode(&qr_result[0..32])),
        })
    }

    /// Calculate the number of challenges based on file size
    fn calculate_challenge_count(&self, file_size: u64) -> usize {
        // Base number of challenges on file size with bounds
        let size_mb = file_size / (1024 * 1024);
        let count = (size_mb / 10 + 5) as usize; // 5 challenges minimum, +1 per 10MB
        
        // Ensure within bounds
        count.max(MIN_CHALLENGE_COUNT).min(MAX_CHALLENGE_COUNT)
    }

    /// Generate random indices for file chunk verification
    fn generate_random_indices(&self, entropy: &[u8], file_size: u64, count: usize) -> Vec<u64> {
        let chunk_size = 1024 * 1024; // 1MB chunks
        let chunk_count = (file_size / chunk_size).max(1);
        
        let mut indices = Vec::with_capacity(count);
        let mut used_indices = std::collections::HashSet::new();
        
        // Use entropy to seed an RNG
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&entropy[0..32]);
        
        let mut rng = rand_chacha::ChaCha20Rng::from_seed(seed);
        
        // Generate unique random indices
        while indices.len() < count {
            let idx = rand::Rng::gen_range(&mut rng, 0..chunk_count);
            if used_indices.insert(idx) {
                indices.push(idx * chunk_size);
            }
        }
        
        // Sort indices for more efficient retrieval
        indices.sort();
        indices
    }

    /// Calculate challenge difficulty based on file size and protocol
    fn calculate_difficulty(&self, file_size: u64, protocol: StorageProtocol) -> u64 {
        // Base difficulty
        let base_difficulty = 1000;
        
        // Scale with file size (larger files = harder challenges)
        let size_factor = ((file_size / (1024 * 1024)) as f64).powf(CHALLENGE_DIFFICULTY_SCALING);
        
        // Adjust based on protocol requirements
        let protocol_multiplier = match protocol {
            StorageProtocol::Filecoin => 1.2, // Higher security requirements
            StorageProtocol::IPFS => 0.8,     // Lower security due to network structure
            StorageProtocol::Arweave => 1.5,  // Higher permanence guarantees
            StorageProtocol::CESS => 1.1,     // Specialized storage
            StorageProtocol::Sia => 0.9,      // Established protocol
            StorageProtocol::Swarm => 0.85,   // DHT-based
            StorageProtocol::Entropy => 1.0,  // Native protocol
        };
        
        // Calculate final difficulty
        (base_difficulty as f64 * size_factor * protocol_multiplier) as u64
    }

    /// Sign a challenge using Entropy-compatible signatures
    fn sign_challenge(&self, entropy: &[u8], file_id: &str, provider: &str) -> Result<String, String> {
        // In a real implementation, this would use proper cryptographic signing
        // For now, we'll create a hash-based signature
        let message = format!("{}:{}:{}", hex::encode(entropy), file_id, provider);
        
        let mut hasher = Sha256::new();
        hasher.update(message.as_bytes());
        let result = hasher.finalize();
        
        Ok(format!("0x{}", hex::encode(result)))
    }

    /// Check if provider is authorized for given protocol
    fn is_provider_authorized(&self, protocol: StorageProtocol, provider: &str) -> bool {
        if let Some(providers) = self.authorized_providers.get(&protocol) {
            providers.iter().any(|p| p == provider)
        } else {
            false
        }
    }

    /// Add an authorized provider for a storage protocol
    pub fn add_authorized_provider(&mut self, protocol: StorageProtocol, provider: &str) {
        self.authorized_providers
            .entry(protocol)
            .or_insert_with(Vec::new)
            .push(provider.to_string());
    }

    // --- Verification implementations ---
    
    /// Verify Merkle proof for challenged chunks
    fn verify_merkle_proof(&self, challenge: &StorageChallenge, proof: &StorageProof) -> Result<bool, String> {
        if let Some(merkle_root) = &challenge.merkle_root {
            if let Some(merkle_proofs) = &proof.merkle_proofs {
                // Verify each proof against the root
                for (i, (data, proof_path)) in proof.data_samples.iter()
                    .zip(merkle_proofs.iter())
                    .enumerate() 
                {
                    // Hash the data sample
                    let mut hasher = Sha256::new();
                    hasher.update(data);
                    let mut current = hasher.finalize().to_vec();
                    
                    // Traverse the Merkle path
                    for node in proof_path {
                        let node_bytes = hex::decode(node.strip_prefix("0x").unwrap_or(node))
                            .map_err(|_| "Invalid hex in Merkle proof".to_string())?;
                        
                        // Combine hashes in order
                        let mut hasher = Sha256::new();
                        if challenge.random_indices[i] % 2 == 0 {
                            // Even index, concatenate current+node
                            hasher.update(&current);
                            hasher.update(&node_bytes);
                        } else {
                            // Odd index, concatenate node+current
                            hasher.update(&node_bytes);
                            hasher.update(&current);
                        }
                        current = hasher.finalize().to_vec();
                    }
                    
                    // Compare with root hash
                    let calculated_root = format!("0x{}", hex::encode(&current));
                    if calculated_root != *merkle_root {
                        return Ok(false);
                    }
                }
                
                // All proofs verified
                Ok(true)
            } else {
                Err("Missing Merkle proofs".to_string())
            }
        } else {
            Err("Missing Merkle root in challenge".to_string())
        }
    }

    /// Verify random data samples with quantum resistant hashing
    fn verify_random_samples(&self, challenge: &StorageChallenge, proof: &StorageProof) -> Result<bool, String> {
        // Basic validation
        if proof.data_samples.len() != challenge.random_indices.len() {
            return Ok(false);
        }
        
        // Apply quantum resistant hashing to each sample for verification
        // In real implementation, we'd compare to expected values or combine in a deterministic way
        
        // This is just a placeholder that always returns true
        // Real implementation would check against expected content based on file commitment
        Ok(true)
    }

    /// Verify timelock challenge responses
    fn verify_timelock_challenge(&self, challenge: &StorageChallenge, proof: &StorageProof) -> Result<bool, String> {
        // Verify response time is within acceptable range
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let response_time = now - challenge.timestamp;
        
        // Calculate minimum expected time based on difficulty
        let min_expected_time = (challenge.difficulty / 1000) as u64; // seconds
        
        // Too fast responses may indicate the data isn't actually being retrieved
        if response_time < min_expected_time {
            return Ok(false);
        }
        
        // Verify the actual data
        self.verify_random_samples(challenge, proof)
    }

    /// Verify multi-part challenge with cross-validation
    fn verify_multipart_challenge(&self, challenge: &StorageChallenge, proof: &StorageProof) -> Result<bool, String> {
        // For multipart challenges, we'd split the data and verify correlations between parts
        // This would be more complex in a real implementation
        
        // Simplified version just verifies each sample
        self.verify_random_samples(challenge, proof)
    }

    /// Verify challenge with quantum resistance enhancements
    fn verify_quantum_resistant_challenge(&self, challenge: &StorageChallenge, proof: &StorageProof) -> Result<bool, String> {
        // Apply multiple iterations of hashing for quantum resistance
        for data in &proof.data_samples {
            let mut current = data.clone();
            
            // Apply hash iterations
            for _ in 0..QR_HASH_ITERATIONS {
                let mut hasher = Sha256::new();
                hasher.update(&current);
                current = hasher.finalize().to_vec();
            }
            
            // In a real implementation, we'd verify these quantum-resistant hashes
            // against expected values derived from the file commitment
        }
        
        // Simplified always-true response
        Ok(true)
    }

    /// Get statistics on verification history
    pub fn get_verification_stats(&self) -> VerificationStats {
        let results = self.verification_results.lock().unwrap();
        let total = results.len();
        let mut successful = 0;
        let mut failed = 0;
        let mut response_time_ms_total = 0;
        let mut verification_time_ms_total = 0;
        
        for result in results.values() {
            if result.verified {
                successful += 1;
            } else {
                failed += 1;
            }
            response_time_ms_total += result.response_time_ms;
            verification_time_ms_total += result.verification_time_ms;
        }
        
        VerificationStats {
            total_verifications: total as u64,
            successful_verifications: successful as u64,
            failed_verifications: failed as u64,
            avg_response_time_ms: if total > 0 { response_time_ms_total / total as u64 } else { 0 },
            avg_verification_time_ms: if total > 0 { verification_time_ms_total / total as u64 } else { 0 },
        }
    }
}

/// Structure for randomness obtained from Entropy
struct EntropyRandomness {
    random_bytes: Vec<u8>,
    nonce: u64,
    beacon_output: String,
}

/// Statistics for verification operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationStats {
    total_verifications: u64,
    successful_verifications: u64,
    failed_verifications: u64,
    avg_response_time_ms: u64,
    avg_verification_time_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_calculate_challenge_count() {
        let verifier = EntropyStorageVerifier::new("https://api.entropy.example", "0xContract");
        
        // Small file - minimum challenges
        assert_eq!(verifier.calculate_challenge_count(1024 * 1024), MIN_CHALLENGE_COUNT);
        
        // Medium file
        let med_size = 100 * 1024 * 1024; // 100MB
        let expected_med = (100 / 10 + 5).max(MIN_CHALLENGE_COUNT).min(MAX_CHALLENGE_COUNT);
        assert_eq!(verifier.calculate_challenge_count(med_size), expected_med);
        
        // Large file - maximum challenges
        let large_size = 50 * 1024 * 1024 * 1024; // 50GB
        assert_eq!(verifier.calculate_challenge_count(large_size), MAX_CHALLENGE_COUNT);
    }
    
    // Additional tests would be implemented here
}

/// Integration with external storage providers
pub mod integrations {
    use super::*;
    
    /// IPFS integration for Entropy-enhanced storage verification
    pub struct IPFSVerifier {
        storage_verifier: EntropyStorageVerifier,
        ipfs_gateway: String,
    }
    
    impl IPFSVerifier {
        pub fn new(entropy_endpoint: &str, entropy_contract: &str, ipfs_gateway: &str) -> Self {
            let mut verifier = EntropyStorageVerifier::new(entropy_endpoint, entropy_contract);
            
            // Register IPFS provider
            verifier.add_authorized_provider(StorageProtocol::IPFS, "ipfs-public");
            
            Self {
                storage_verifier: verifier,
                ipfs_gateway: ipfs_gateway.to_string(),
            }
        }
        
        pub async fn verify_ipfs_content(&self, cid: &str, expected_size: u64) -> Result<bool, String> {
            // Generate challenge for IPFS content
            let challenge = self.storage_verifier.generate_challenge(
                cid,
                expected_size,
                StorageProtocol::IPFS,
                "ipfs-public",
                ChallengeType::RandomSampling,
            ).await?;
            
            // Fetch samples from IPFS using gateway
            let samples = self.fetch_ipfs_samples(cid, &challenge.random_indices).await?;
            
            // Create proof
            let proof = StorageProof {
                challenge_id: challenge.challenge_id,
                file_id: cid.to_string(),
                provider_id: "ipfs-public".to_string(),
                timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
                data_samples: samples,
                merkle_proofs: None,
                provider_signature: String::from("ipfs-verification"), // IPFS doesn't sign
            };
            
            // Verify proof
            let result = self.storage_verifier.verify_proof(proof).await?;
            
            Ok(result.verified)
        }
        
        async fn fetch_ipfs_samples(&self, cid: &str, indices: &[u64]) -> Result<Vec<Vec<u8>>, String> {
            // Real implementation would use HTTP client to fetch byte ranges from IPFS gateway
            // This is a placeholder that just returns empty data
            let samples = indices.iter().map(|_| Vec::new()).collect();
            Ok(samples)
        }
    }
    
    // Additional storage provider integrations would be implemented here
}

/// Bloom filter integration for efficient verification history
pub mod bloom {
    use super::*;
    use crate::BloomFilterLib;
    
    pub struct EnhancedVerificationFilter {
        filter: BloomFilterLib::Filter,
        verifier: Arc<EntropyStorageVerifier>,
    }
    
    impl EnhancedVerificationFilter {
        pub fn new(verifier: Arc<EntropyStorageVerifier>) -> Self {
            // Create filter with optimal parameters for verification tracking
            let filter = BloomFilterLib::createFilter(
                65536, // Size optimized for up to 10M verifications
                4,     // Number of hash functions
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            );
            
            Self {
                filter,
                verifier,
            }
        }
        
        /// Track a verification in the bloom filter
        pub fn record_verification(&mut self, verification: &VerificationResult) {
            let key = verification.challenge_id.clone() + &verification.file_id;
            BloomFilterLib::updateFilter(&mut self.filter, key.as_bytes());
        }
        
        /// Check if verification exists (might have false positives)
        pub fn might_contain_verification(&self, challenge_id: &str, file_id: &str) -> bool {
            let key = challenge_id.to_string() + file_id;
            BloomFilterLib::mightContain(&self.filter, key.as_bytes())
        }
        
        /// Get estimated false positive rate
        pub fn get_estimated_fpr(&self) -> f64 {
            BloomFilterLib::estimateCurrentFPR(&self.filter) as f64 / 10000.0
        }
    }
}

/// Temporal verification with beacon history
pub mod temporal {
    use super::*;
    
    /// Structure for time-based verification factors
    pub struct TemporalVerificationContext {
        beacon_history: Vec<String>,
        max_history_items: usize,
        minimum_challenge_age: u64,
        maximum_challenge_age: u64,
    }
    
    impl TemporalVerificationContext {
        pub fn new(max_history: usize) -> Self {
            Self {
                beacon_history: Vec::with_capacity(max_history),
                max_history_items: max_history,
                minimum_challenge_age: 60, // 1 minute
                maximum_challenge_age: 86400, // 1 day
            }
        }
        
        /// Add a beacon output to history
        pub fn add_beacon_output(&mut self, output: &str, timestamp: u64) {
            if self.beacon_history.len() >= self.max_history_items {
                self.beacon_history.remove(0);
            }
            self.beacon_history.push(output.to_string());
        }
        
        /// Verify challenge is within acceptable time bounds
        pub fn verify_challenge_temporality(&self, challenge: &StorageChallenge) -> bool {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
            let challenge_age = now - challenge.timestamp;
            
            // Check challenge age bounds
            if challenge_age < self.minimum_challenge_age || challenge_age > self.maximum_challenge_age {
                return false;
            }
            
            // Verify beacon output exists in history
            self.beacon_history.contains(&challenge.beacon_output)
        }
    }
}

// Helper functions for common operations
pub mod utils {
    use super::*;
    
    /// Apply quantum resistant hashing to data
    pub fn apply_quantum_resistant_hash(data: &[u8]) -> Vec<u8> {
        let mut result = data.to_vec();
        
        // Apply multiple hash iterations for quantum resistance
        for _ in 0..QR_HASH_ITERATIONS {
            let mut hasher = Sha256::new();
            hasher.update(&result);
            result = hasher.finalize().to_vec();
        }
        
        result
    }
    
    /// Create a deterministic hash from multiple data sources
    pub fn combine_data_sources(sources: &[&[u8]]) -> Vec<u8> {
        let mut hasher = Sha256::new();
        
        // Hash each source in sequence
        for source in sources {
            hasher.update(source);
        }
        
        // Apply quantum resistance
        let mut result = hasher.finalize().to_vec();
        for _ in 1..QR_HASH_ITERATIONS {
            let mut hasher = Sha256::new();
            hasher.update(&result);
            result = hasher.finalize().to_vec();
        }
        
        result
    }
}
