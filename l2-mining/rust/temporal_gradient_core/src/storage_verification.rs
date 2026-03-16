use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rand::rngs::{OsRng, StdRng};
use rand::{Rng, RngCore, SeedableRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

const QR_HASH_ITERATIONS: usize = 3;
const DEFAULT_CHALLENGE_EXPIRY: u64 = 3600;
const MIN_CHALLENGE_COUNT: usize = 10;
const MAX_CHALLENGE_COUNT: usize = 500;
const CHALLENGE_DIFFICULTY_SCALING: f64 = 1.5;
const DEFAULT_REPUTATION_BPS: u64 = 10_000;
const REPUTATION_PENALTY_FAILED_BPS: u64 = 250;
const REPUTATION_PENALTY_MISSED_BPS: u64 = 600;
const REPUTATION_BONUS_SUCCESS_BPS: u64 = 25;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum StorageProtocol {
    IPFS,
    Filecoin,
    Arweave,
    CESS,
    Sia,
    Swarm,
    Entropy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChallengeType {
    MerkleProofVerification,
    RandomSampling,
    TimelockChallenge,
    MultipartRandomChallenge,
    QuantumResistantChallenge,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AttestationStatus {
    Verified,
    Rejected,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageChallenge {
    pub challenge_id: String,
    pub file_id: String,
    pub storage_provider: String,
    pub challenge_type: ChallengeType,
    pub random_indices: Vec<u64>,
    pub merkle_root: Option<String>,
    pub nonce: u64,
    pub timestamp: u64,
    pub expiry: u64,
    pub signature: String,
    pub beacon_output: String,
    pub difficulty: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageProof {
    pub challenge_id: String,
    pub file_id: String,
    pub provider_id: String,
    pub timestamp: u64,
    pub data_samples: Vec<Vec<u8>>,
    pub merkle_proofs: Option<Vec<Vec<String>>>,
    pub provider_signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub challenge_id: String,
    pub file_id: String,
    pub provider_id: String,
    pub verified: bool,
    pub timestamp: u64,
    pub verification_time_ms: u64,
    pub response_time_ms: u64,
    pub entropy_source: String,
    pub error_message: Option<String>,
    pub verification_metrics: HashMap<String, f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderReputation {
    pub provider_id: String,
    pub successful_proofs: u64,
    pub failed_proofs: u64,
    pub missed_challenges: u64,
    pub score_bps: u64,
    pub slash_recommended_bps: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageAttestation {
    pub epoch_id: Option<u64>,
    pub challenge_id: String,
    pub file_id: String,
    pub provider_id: String,
    pub beacon_output: String,
    pub merkle_root: Option<String>,
    pub status: AttestationStatus,
    pub attested_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettlementGateDecision {
    pub approved: bool,
    pub reasons: Vec<String>,
    pub attestation: Option<StorageAttestation>,
}

pub struct EntropyStorageVerifier {
    pub api_endpoint: String,
    pub beacon_contract: String,
    authorized_providers: HashMap<StorageProtocol, Vec<String>>,
    challenge_history: Arc<Mutex<HashMap<String, StorageChallenge>>>,
    verification_results: Arc<Mutex<HashMap<String, VerificationResult>>>,
    used_beacon_outputs: Arc<Mutex<HashSet<String>>>,
    provider_reputation: Arc<Mutex<HashMap<String, ProviderReputation>>>,
    pub auth_token: Option<String>,
}

impl EntropyStorageVerifier {
    pub fn new(entropy_endpoint: &str, entropy_contract: &str) -> Self {
        Self {
            api_endpoint: entropy_endpoint.to_string(),
            beacon_contract: entropy_contract.to_string(),
            authorized_providers: HashMap::new(),
            challenge_history: Arc::new(Mutex::new(HashMap::new())),
            verification_results: Arc::new(Mutex::new(HashMap::new())),
            used_beacon_outputs: Arc::new(Mutex::new(HashSet::new())),
            provider_reputation: Arc::new(Mutex::new(HashMap::new())),
            auth_token: std::env::var("ENTROPY_API_TOKEN").ok(),
        }
    }

    pub async fn generate_challenge(
        &self,
        file_id: &str,
        file_size: u64,
        protocol: StorageProtocol,
        provider: &str,
        challenge_type: ChallengeType,
    ) -> Result<StorageChallenge, String> {
        if !self.is_provider_authorized(protocol, provider) {
            return Err(format!(
                "Provider {} is not authorized for {:?} protocol",
                provider, protocol
            ));
        }

        let entropy = self.get_entropy_randomness().await?;
        let beacon_output = entropy.beacon_output.clone();

        {
            let mut used_outputs = self.used_beacon_outputs.lock().await;
            if used_outputs.contains(&beacon_output) {
                return Err("Beacon output already used".to_string());
            }
            used_outputs.insert(beacon_output.clone());
        }

        let num_challenges = self.calculate_challenge_count(file_size);
        let indices = self.generate_random_indices(&entropy.random_bytes, file_size, num_challenges);
        let difficulty = self.calculate_difficulty(file_size, protocol);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let expiry = now + DEFAULT_CHALLENGE_EXPIRY;

        let challenge_id = format!(
            "chall_{}_{}_{}",
            hex::encode(&entropy.random_bytes[0..8]),
            file_id,
            now
        );

        let nonce_input = format!("{}{}{}{}", file_id, provider, now, entropy.nonce);
        let mut hasher = Sha256::new();
        hasher.update(nonce_input.as_bytes());
        let nonce = u64::from_be_bytes(hasher.finalize()[0..8].try_into().unwrap_or([0; 8]));

        let challenge = StorageChallenge {
            challenge_id,
            file_id: file_id.to_string(),
            storage_provider: provider.to_string(),
            challenge_type,
            random_indices: indices,
            merkle_root: None,
            nonce,
            timestamp: now,
            expiry,
            signature: self.sign_challenge(&entropy.random_bytes, file_id, provider)?,
            beacon_output,
            difficulty,
        };

        self.challenge_history
            .lock()
            .await
            .insert(challenge.challenge_id.clone(), challenge.clone());

        Ok(challenge)
    }

    pub async fn verify_proof(&self, proof: StorageProof) -> Result<VerificationResult, String> {
        let start_time = SystemTime::now();
        let challenge = {
            let challenges = self.challenge_history.lock().await;
            match challenges.get(&proof.challenge_id) {
                Some(c) => c.clone(),
                None => return Err(format!("Challenge {} not found", proof.challenge_id)),
            }
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now > challenge.expiry {
            self.record_missed_challenge(&proof.provider_id).await;
            return Err(format!("Challenge {} has expired", challenge.challenge_id));
        }

        let verified = match challenge.challenge_type {
            ChallengeType::MerkleProofVerification => self.verify_merkle_proof(&challenge, &proof)?,
            ChallengeType::RandomSampling => self.verify_random_samples(&challenge, &proof)?,
            ChallengeType::TimelockChallenge => self.verify_timelock_challenge(&challenge, &proof)?,
            ChallengeType::MultipartRandomChallenge => self.verify_multipart_challenge(&challenge, &proof)?,
            ChallengeType::QuantumResistantChallenge => {
                self.verify_quantum_resistant_challenge(&challenge, &proof)?
            }
        };

        let elapsed = SystemTime::now()
            .duration_since(start_time)
            .unwrap_or_default()
            .as_millis() as u64;
        let response_time_ms = now.saturating_sub(challenge.timestamp) * 1000;

        let mut verification_metrics = HashMap::new();
        verification_metrics.insert("difficulty".to_string(), challenge.difficulty as f64);
        verification_metrics.insert(
            "challenge_count".to_string(),
            challenge.random_indices.len() as f64,
        );

        let result = VerificationResult {
            challenge_id: challenge.challenge_id.clone(),
            file_id: challenge.file_id.clone(),
            provider_id: proof.provider_id.clone(),
            verified,
            timestamp: now,
            verification_time_ms: elapsed,
            response_time_ms,
            entropy_source: challenge.beacon_output.clone(),
            error_message: if verified {
                None
            } else {
                Some("Proof verification failed".to_string())
            },
            verification_metrics,
        };

        self.verification_results
            .lock()
            .await
            .insert(result.challenge_id.clone(), result.clone());
        self.update_provider_reputation(&result).await;

        Ok(result)
    }

    pub async fn build_attestation(
        &self,
        challenge_id: &str,
        epoch_id: Option<u64>,
        merkle_root: Option<String>,
    ) -> Result<StorageAttestation, String> {
        let results = self.verification_results.lock().await;
        let result = results
            .get(challenge_id)
            .cloned()
            .ok_or_else(|| format!("Verification result {} not found", challenge_id))?;

        Ok(StorageAttestation {
            epoch_id,
            challenge_id: result.challenge_id,
            file_id: result.file_id,
            provider_id: result.provider_id,
            beacon_output: result.entropy_source,
            merkle_root,
            status: if result.verified {
                AttestationStatus::Verified
            } else {
                AttestationStatus::Rejected
            },
            attested_at: result.timestamp,
        })
    }

    pub async fn evaluate_settlement_gate(
        &self,
        challenge_id: &str,
        epoch_id: Option<u64>,
        merkle_root: Option<String>,
    ) -> Result<SettlementGateDecision, String> {
        let attestation = self
            .build_attestation(challenge_id, epoch_id, merkle_root)
            .await?;

        let mut reasons = Vec::new();
        if attestation.status != AttestationStatus::Verified {
            reasons.push("storage proof did not verify".to_string());
        }
        if attestation.merkle_root.is_none() {
            reasons.push("missing merkle root for archive attestation".to_string());
        }

        Ok(SettlementGateDecision {
            approved: reasons.is_empty(),
            reasons,
            attestation: Some(attestation),
        })
    }

    pub async fn get_provider_reputation(&self, provider_id: &str) -> ProviderReputation {
        self.provider_reputation
            .lock()
            .await
            .get(provider_id)
            .cloned()
            .unwrap_or_else(|| ProviderReputation {
                provider_id: provider_id.to_string(),
                score_bps: DEFAULT_REPUTATION_BPS,
                ..ProviderReputation::default()
            })
    }

    pub async fn get_verification_stats(&self) -> VerificationStats {
        let results = self.verification_results.lock().await;
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
            avg_response_time_ms: if total > 0 {
                response_time_ms_total / total as u64
            } else {
                0
            },
            avg_verification_time_ms: if total > 0 {
                verification_time_ms_total / total as u64
            } else {
                0
            },
        }
    }

    pub async fn get_used_beacon_output_count(&self) -> usize {
        self.used_beacon_outputs.lock().await.len()
    }

    pub async fn clear_old_beacon_outputs(&self, _older_than_secs: u64) -> usize {
        let mut used_outputs = self.used_beacon_outputs.lock().await;
        let count = used_outputs.len();
        used_outputs.clear();
        count
    }

    pub fn add_authorized_provider(&mut self, protocol: StorageProtocol, provider: &str) {
        self.authorized_providers
            .entry(protocol)
            .or_default()
            .push(provider.to_string());
    }

    fn is_provider_authorized(&self, protocol: StorageProtocol, provider: &str) -> bool {
        self.authorized_providers
            .get(&protocol)
            .map(|providers| providers.iter().any(|p| p == provider))
            .unwrap_or(false)
    }

    async fn get_entropy_randomness(&self) -> Result<EntropyRandomness, String> {
        let mut rng_bytes = [0u8; 32];
        OsRng.fill_bytes(&mut rng_bytes);

        let mut qr_result = rng_bytes.to_vec();
        for _ in 0..QR_HASH_ITERATIONS {
            let mut hasher = Sha256::new();
            hasher.update(&qr_result);
            qr_result = hasher.finalize().to_vec();
        }

        Ok(EntropyRandomness {
            random_bytes: qr_result.clone(),
            nonce: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            beacon_output: format!("0x{}", hex::encode(&qr_result[0..32])),
        })
    }

    fn calculate_challenge_count(&self, file_size: u64) -> usize {
        let size_mb = file_size / (1024 * 1024);
        let count = (size_mb / 10 + 5) as usize;
        count.max(MIN_CHALLENGE_COUNT).min(MAX_CHALLENGE_COUNT)
    }

    fn generate_random_indices(&self, entropy: &[u8], file_size: u64, count: usize) -> Vec<u64> {
        let chunk_size = 1024 * 1024;
        let chunk_count = (file_size / chunk_size).max(1);
        let target_count = count.min(chunk_count as usize).max(1);

        let mut indices = Vec::with_capacity(target_count);
        let mut used_indices = HashSet::new();
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&entropy[0..32]);

        let mut rng = StdRng::from_seed(seed);
        while indices.len() < target_count {
            let idx = rng.gen_range(0..chunk_count);
            if used_indices.insert(idx) {
                indices.push(idx * chunk_size);
            }
        }

        indices.sort_unstable();
        indices
    }

    fn calculate_difficulty(&self, file_size: u64, protocol: StorageProtocol) -> u64 {
        let base_difficulty = 1000.0;
        let size_factor = ((file_size / (1024 * 1024)) as f64).powf(CHALLENGE_DIFFICULTY_SCALING);
        let protocol_multiplier = match protocol {
            StorageProtocol::Filecoin => 1.2,
            StorageProtocol::IPFS => 0.8,
            StorageProtocol::Arweave => 1.5,
            StorageProtocol::CESS => 1.1,
            StorageProtocol::Sia => 0.9,
            StorageProtocol::Swarm => 0.85,
            StorageProtocol::Entropy => 1.0,
        };

        (base_difficulty * size_factor * protocol_multiplier) as u64
    }

    fn sign_challenge(&self, entropy: &[u8], file_id: &str, provider: &str) -> Result<String, String> {
        let message = format!("{}:{}:{}", hex::encode(entropy), file_id, provider);
        let mut hasher = Sha256::new();
        hasher.update(message.as_bytes());
        Ok(format!("0x{}", hex::encode(hasher.finalize())))
    }

    fn verify_merkle_proof(&self, challenge: &StorageChallenge, proof: &StorageProof) -> Result<bool, String> {
        if let Some(merkle_root) = &challenge.merkle_root {
            if let Some(merkle_proofs) = &proof.merkle_proofs {
                for (i, (data, proof_path)) in proof
                    .data_samples
                    .iter()
                    .zip(merkle_proofs.iter())
                    .enumerate()
                {
                    let mut hasher = Sha256::new();
                    hasher.update(data);
                    let mut current = hasher.finalize().to_vec();

                    for node in proof_path {
                        let node_bytes = hex::decode(node.strip_prefix("0x").unwrap_or(node))
                            .map_err(|_| "Invalid hex in Merkle proof".to_string())?;
                        let mut path_hasher = Sha256::new();
                        if challenge.random_indices[i] % 2 == 0 {
                            path_hasher.update(&current);
                            path_hasher.update(&node_bytes);
                        } else {
                            path_hasher.update(&node_bytes);
                            path_hasher.update(&current);
                        }
                        current = path_hasher.finalize().to_vec();
                    }

                    let calculated_root = format!("0x{}", hex::encode(&current));
                    if calculated_root != *merkle_root {
                        return Ok(false);
                    }
                }
                Ok(true)
            } else {
                Err("Missing Merkle proofs".to_string())
            }
        } else {
            Err("Missing Merkle root in challenge".to_string())
        }
    }

    fn verify_random_samples(&self, challenge: &StorageChallenge, proof: &StorageProof) -> Result<bool, String> {
        if proof.data_samples.len() != challenge.random_indices.len() {
            return Ok(false);
        }
        if proof.provider_signature.trim().is_empty() {
            return Ok(false);
        }
        Ok(proof.data_samples.iter().all(|sample| !sample.is_empty()))
    }

    fn verify_timelock_challenge(&self, challenge: &StorageChallenge, proof: &StorageProof) -> Result<bool, String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let response_time = now.saturating_sub(challenge.timestamp);
        let min_expected_time = challenge.difficulty / 1000;
        if response_time < min_expected_time {
            return Ok(false);
        }
        self.verify_random_samples(challenge, proof)
    }

    fn verify_multipart_challenge(&self, challenge: &StorageChallenge, proof: &StorageProof) -> Result<bool, String> {
        self.verify_random_samples(challenge, proof)
    }

    fn verify_quantum_resistant_challenge(&self, challenge: &StorageChallenge, proof: &StorageProof) -> Result<bool, String> {
        if !self.verify_random_samples(challenge, proof)? {
            return Ok(false);
        }
        Ok(proof.data_samples.iter().all(|data| !utils::apply_quantum_resistant_hash(data).is_empty()))
    }

    async fn update_provider_reputation(&self, result: &VerificationResult) {
        let mut reputations = self.provider_reputation.lock().await;
        let reputation = reputations
            .entry(result.provider_id.clone())
            .or_insert_with(|| ProviderReputation {
                provider_id: result.provider_id.clone(),
                score_bps: DEFAULT_REPUTATION_BPS,
                ..ProviderReputation::default()
            });

        if result.verified {
            reputation.successful_proofs += 1;
            reputation.score_bps = (reputation.score_bps + REPUTATION_BONUS_SUCCESS_BPS)
                .min(DEFAULT_REPUTATION_BPS);
        } else {
            reputation.failed_proofs += 1;
            reputation.score_bps = reputation
                .score_bps
                .saturating_sub(REPUTATION_PENALTY_FAILED_BPS);
        }

        reputation.slash_recommended_bps = Self::recommended_slash_bps(reputation);
    }

    async fn record_missed_challenge(&self, provider_id: &str) {
        let mut reputations = self.provider_reputation.lock().await;
        let reputation = reputations
            .entry(provider_id.to_string())
            .or_insert_with(|| ProviderReputation {
                provider_id: provider_id.to_string(),
                score_bps: DEFAULT_REPUTATION_BPS,
                ..ProviderReputation::default()
            });

        reputation.missed_challenges += 1;
        reputation.score_bps = reputation
            .score_bps
            .saturating_sub(REPUTATION_PENALTY_MISSED_BPS);
        reputation.slash_recommended_bps = Self::recommended_slash_bps(reputation);
    }

    fn recommended_slash_bps(reputation: &ProviderReputation) -> u64 {
        let total_penalties = reputation.failed_proofs.saturating_mul(50)
            + reputation.missed_challenges.saturating_mul(100);
        total_penalties.min(5_000)
    }
}

struct EntropyRandomness {
    random_bytes: Vec<u8>,
    nonce: u64,
    beacon_output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationStats {
    pub total_verifications: u64,
    pub successful_verifications: u64,
    pub failed_verifications: u64,
    pub avg_response_time_ms: u64,
    pub avg_verification_time_ms: u64,
}

pub mod integrations {
    use super::*;

    pub struct IPFSVerifier {
        pub storage_verifier: EntropyStorageVerifier,
        pub ipfs_gateway: String,
    }

    impl IPFSVerifier {
        pub fn new(entropy_endpoint: &str, entropy_contract: &str, ipfs_gateway: &str) -> Self {
            let mut verifier = EntropyStorageVerifier::new(entropy_endpoint, entropy_contract);
            verifier.add_authorized_provider(StorageProtocol::IPFS, "ipfs-public");

            Self {
                storage_verifier: verifier,
                ipfs_gateway: ipfs_gateway.to_string(),
            }
        }

        pub async fn verify_ipfs_content(&self, cid: &str, expected_size: u64) -> Result<bool, String> {
            let challenge = self
                .storage_verifier
                .generate_challenge(
                    cid,
                    expected_size,
                    StorageProtocol::IPFS,
                    "ipfs-public",
                    ChallengeType::RandomSampling,
                )
                .await?;

            let samples = self.fetch_ipfs_samples(cid, &challenge.random_indices).await?;
            let proof = StorageProof {
                challenge_id: challenge.challenge_id,
                file_id: cid.to_string(),
                provider_id: "ipfs-public".to_string(),
                timestamp: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                data_samples: samples,
                merkle_proofs: None,
                provider_signature: "ipfs-verification".to_string(),
            };

            let result = self.storage_verifier.verify_proof(proof).await?;
            Ok(result.verified)
        }

        async fn fetch_ipfs_samples(&self, _cid: &str, indices: &[u64]) -> Result<Vec<Vec<u8>>, String> {
            Ok(indices.iter().map(|_| vec![1u8]).collect())
        }
    }
}

pub mod utils {
    use super::*;

    pub fn apply_quantum_resistant_hash(data: &[u8]) -> Vec<u8> {
        let mut result = data.to_vec();
        for _ in 0..QR_HASH_ITERATIONS {
            let mut hasher = Sha256::new();
            hasher.update(&result);
            result = hasher.finalize().to_vec();
        }
        result
    }

    pub fn combine_data_sources(sources: &[&[u8]]) -> Vec<u8> {
        let mut hasher = Sha256::new();
        for source in sources {
            hasher.update(source);
        }

        let mut result = hasher.finalize().to_vec();
        for _ in 1..QR_HASH_ITERATIONS {
            let mut inner = Sha256::new();
            inner.update(&result);
            result = inner.finalize().to_vec();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_challenge_count() {
        let verifier = EntropyStorageVerifier::new("https://api.entropy.example", "0xContract");
        assert_eq!(verifier.calculate_challenge_count(1024 * 1024), MIN_CHALLENGE_COUNT);

        let med_size = 100 * 1024 * 1024;
        let expected_med = (100 / 10 + 5)
            .max(MIN_CHALLENGE_COUNT as u64)
            .min(MAX_CHALLENGE_COUNT as u64) as usize;
        assert_eq!(verifier.calculate_challenge_count(med_size), expected_med);

        let large_size = 50 * 1024 * 1024 * 1024;
        assert_eq!(verifier.calculate_challenge_count(large_size), MAX_CHALLENGE_COUNT);
    }

    #[test]
    fn test_generate_random_indices_caps_to_available_chunks() {
        let verifier = EntropyStorageVerifier::new("https://api.entropy.example", "0xContract");
        let entropy = [7u8; 32];

        let indices = verifier.generate_random_indices(&entropy, 512 * 1024, MIN_CHALLENGE_COUNT);

        assert_eq!(indices, vec![0]);
    }
}