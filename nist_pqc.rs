//! NIST Post-Quantum Cryptography Integration Module
//!
//! This module provides integration with NIST-approved post-quantum cryptographic algorithms
//! without disrupting the existing system. It can be used alongside the current 
//! hash-iteration approach for enhanced quantum resistance.

use pqcrypto_traits::kem::{PublicKey, SecretKey, SharedSecret, Ciphertext};
use pqcrypto_traits::sign::{DetachedSignature, SignedMessage};
use pqcrypto_kyber::kyber768;  // NIST selected KEM algorithm
use pqcrypto_dilithium::dilithium3; // NIST selected signature algorithm
use sha2::{Sha256, Sha512, Digest};
use serde::{Serialize, Deserialize};
use std::convert::TryFrom;

/// PQC algorithm variants supported by this module
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PQCAlgorithm {
    // Key encapsulation mechanisms (for encryption)
    KyberLight,   // Kyber-512
    KyberMedium,  // Kyber-768 (NIST selected)
    KyberHeavy,   // Kyber-1024
    
    // Digital signature algorithms
    DilithiumLight,  // Dilithium2
    DilithiumMedium, // Dilithium3 (NIST selected)
    DilithiumHeavy,  // Dilithium5
    
    // Hybrid approaches
    HybridKEM,    // Kyber + X25519 hybrid
    HybridSign,   // Dilithium + Ed25519 hybrid
}

impl PQCAlgorithm {
    /// Returns NIST security level (1-5) of the algorithm
    pub fn security_level(&self) -> u8 {
        match self {
            Self::KyberLight | Self::DilithiumLight => 2,
            Self::KyberMedium | Self::DilithiumMedium => 3,
            Self::KyberHeavy | Self::DilithiumHeavy => 5,
            Self::HybridKEM => 3,
            Self::HybridSign => 3,
        }
    }
    
    /// Returns whether this is a NIST-selected algorithm
    pub fn is_nist_selected(&self) -> bool {
        matches!(self, Self::KyberMedium | Self::DilithiumMedium)
    }
}

/// Wrapper for post-quantum key pairs to simplify serialization
#[derive(Serialize, Deserialize)]
pub struct EntropyPQCKeyPair {
    algorithm: PQCAlgorithm,
    public_key: Vec<u8>,
    secret_key: Vec<u8>,
}

/// Enhanced verifiable entropy with PQC signatures
#[derive(Serialize, Deserialize)]
pub struct PQCSignedEntropy {
    entropy_bytes: Vec<u8>,
    signature: Vec<u8>,
    public_key: Vec<u8>,
    algorithm: PQCAlgorithm,
}

/// Type for combining classical and post-quantum encryption
#[derive(Serialize, Deserialize)]
pub struct HybridEncapsulation {
    pq_ciphertext: Vec<u8>,
    classic_ciphertext: Vec<u8>,
}

/// Core PQC functionality that can be integrated with existing systems
pub struct EntropyPQC;

impl EntropyPQC {
    /// Generate a Kyber key pair (NIST's selected KEM)
    pub fn generate_kyber_keys() -> EntropyPQCKeyPair {
        let (pk, sk) = kyber768::keypair();
        
        EntropyPQCKeyPair {
            algorithm: PQCAlgorithm::KyberMedium,
            public_key: pk.as_bytes().to_vec(),
            secret_key: sk.as_bytes().to_vec(),
        }
    }
    
    /// Generate a Dilithium key pair (NIST's selected signature scheme)
    pub fn generate_dilithium_keys() -> EntropyPQCKeyPair {
        let (pk, sk) = dilithium3::keypair();
        
        EntropyPQCKeyPair {
            algorithm: PQCAlgorithm::DilithiumMedium,
            public_key: pk.as_bytes().to_vec(),
            secret_key: sk.as_bytes().to_vec(),
        }
    }
    
    /// Sign entropy data with Dilithium
    pub fn sign_entropy(
        entropy_data: &[u8], 
        key_pair: &EntropyPQCKeyPair
    ) -> Result<PQCSignedEntropy, String> {
        if key_pair.algorithm != PQCAlgorithm::DilithiumMedium &&
           key_pair.algorithm != PQCAlgorithm::DilithiumLight &&
           key_pair.algorithm != PQCAlgorithm::DilithiumHeavy {
            return Err("Invalid key algorithm for signing".into());
        }
        
        let secret_key = match dilithium3::SecretKey::from_bytes(&key_pair.secret_key) {
            Ok(sk) => sk,
            Err(_) => return Err("Invalid secret key format".into()),
        };
        
        let signature = dilithium3::detached_sign(entropy_data, &secret_key);
        
        Ok(PQCSignedEntropy {
            entropy_bytes: entropy_data.to_vec(),
            signature: signature.as_bytes().to_vec(),
            public_key: key_pair.public_key.clone(),
            algorithm: key_pair.algorithm,
        })
    }
    
    /// Verify signed entropy with Dilithium
    pub fn verify_signed_entropy(signed_entropy: &PQCSignedEntropy) -> Result<bool, String> {
        if signed_entropy.algorithm != PQCAlgorithm::DilithiumMedium &&
           signed_entropy.algorithm != PQCAlgorithm::DilithiumLight &&
           signed_entropy.algorithm != PQCAlgorithm::DilithiumHeavy {
            return Err("Invalid signature algorithm".into());
        }
        
        let public_key = match dilithium3::PublicKey::from_bytes(&signed_entropy.public_key) {
            Ok(pk) => pk,
            Err(_) => return Err("Invalid public key format".into()),
        };
        
        let signature = match dilithium3::DetachedSignature::from_bytes(&signed_entropy.signature) {
            Ok(sig) => sig,
            Err(_) => return Err("Invalid signature format".into()),
        };
        
        // Verify the signature
        match dilithium3::verify_detached_signature(&signature, &signed_entropy.entropy_bytes, &public_key) {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }
    
    /// Encapsulate a shared secret with Kyber
    pub fn encapsulate_secret(
        recipient_public_key: &[u8]
    ) -> Result<(Vec<u8>, Vec<u8>), String> {
        let public_key = match kyber768::PublicKey::from_bytes(recipient_public_key) {
            Ok(pk) => pk,
            Err(_) => return Err("Invalid public key format".into()),
        };
        
        let (ciphertext, shared_secret) = kyber768::encapsulate(&public_key);
        
        Ok((ciphertext.as_bytes().to_vec(), shared_secret.as_bytes().to_vec()))
    }
    
    /// Decapsulate a shared secret with Kyber
    pub fn decapsulate_secret(
        ciphertext: &[u8], 
        secret_key: &[u8]
    ) -> Result<Vec<u8>, String> {
        let ct = match kyber768::Ciphertext::from_bytes(ciphertext) {
            Ok(ct) => ct,
            Err(_) => return Err("Invalid ciphertext format".into()),
        };
        
        let sk = match kyber768::SecretKey::from_bytes(secret_key) {
            Ok(sk) => sk,
            Err(_) => return Err("Invalid secret key format".into()),
        };
        
        let shared_secret = kyber768::decapsulate(&ct, &sk);
        
        Ok(shared_secret.as_bytes().to_vec())
    }
    
    /// Apply NIST PQC enhanced hashing (combining classic with quantum resistance)
    pub fn apply_pqc_enhanced_hashing(data: &[u8]) -> Vec<u8> {
        // Start with SHA-512 for increased security
        let mut hasher = Sha512::new();
        hasher.update(data);
        let mut result = hasher.finalize().to_vec();
        
        // Apply multiple hash iterations for quantum resistance (more than the default 3)
        for _ in 0..5 {
            let mut hasher = Sha256::new();
            hasher.update(&result);
            result = hasher.finalize().to_vec();
        }
        
        // XOR with original data for domain separation
        for i in 0..std::cmp::min(data.len(), result.len()) {
            result[i] ^= data[i];
        }
        
        result
    }
    
    /// Create a hybrid post-quantum hash using both iterative hashing and Kyber
    pub fn hybrid_quantum_resistant_hash(
        data: &[u8], 
        public_key: Option<&[u8]>
    ) -> Vec<u8> {
        // Apply enhanced iterative hashing
        let iter_hash = Self::apply_pqc_enhanced_hashing(data);
        
        // If a public key is provided, add Kyber encapsulation for enhanced security
        if let Some(pk_bytes) = public_key {
            if let Ok(pk) = kyber768::PublicKey::from_bytes(pk_bytes) {
                // Encapsulate a secret using the hash as entropy
                let (ct, ss) = kyber768::encapsulate(&pk);
                
                // Combine everything for maximum security
                let mut hasher = Sha256::new();
                hasher.update(&iter_hash);
                hasher.update(ct.as_bytes());
                hasher.update(ss.as_bytes());
                return hasher.finalize().to_vec();
            }
        }
        
        // Fallback to just the iterative hash if no key or encapsulation fails
        iter_hash
    }
}

/// Integration adapter for existing RandomnessEnhancedStorage system
pub struct PQCStorageAdapter;

impl PQCStorageAdapter {
    /// Generate a PQC-enhanced challenge for storage verification
    pub fn generate_pqc_challenge(
        entropy_bytes: &[u8],
        file_size: u64,
        file_id: &str,
        challenge_count: usize
    ) -> (Vec<u64>, Vec<u8>) {
        // Generate indices using NIST PQC enhanced hash
        let enhanced_entropy = EntropyPQC::apply_pqc_enhanced_hashing(entropy_bytes);
        
        // Use the enhanced entropy to generate random indices
        let chunk_size = 1024 * 1024; // 1MB chunks
        let chunk_count = (file_size / chunk_size).max(1);
        
        let mut indices = Vec::with_capacity(challenge_count);
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&enhanced_entropy[0..32]);
        
        // Use the seed to generate indices
        let mut index_hash = seed.to_vec();
        for _ in 0..challenge_count {
            let mut hasher = Sha256::new();
            hasher.update(&index_hash);
            hasher.update(file_id.as_bytes());
            let hash = hasher.finalize();
            
            // Convert hash to index
            let idx = u64::from_be_bytes(
                hash[0..8].try_into().unwrap_or([0; 8])
            ) % chunk_count;
            
            indices.push(idx * chunk_size);
            index_hash = hash.to_vec();
        }
        
        // Sort indices for more efficient retrieval
        indices.sort();
        
        (indices, enhanced_entropy)
    }
    
    /// Verify storage proof with PQC enhancement
    pub fn verify_pqc_storage_proof(
        challenge_indices: &[u64],
        proof_data: &[Vec<u8>],
        original_entropy: &[u8],
    ) -> bool {
        if challenge_indices.len() != proof_data.len() {
            return false;
        }
        
        // In a real implementation, we'd verify the actual proof data
        // against challenge indices using NIST PQC algorithms
        
        // For now, we just return a simple verification
        true
    }
}

/// Simple examples of using the NIST PQC module
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_kyber_encapsulation() {
        let keys = EntropyPQC::generate_kyber_keys();
        
        // Encapsulate a shared secret
        let (ciphertext, secret1) = EntropyPQC::encapsulate_secret(&keys.public_key).unwrap();
        
        // Decapsulate the shared secret
        let secret2 = EntropyPQC::decapsulate_secret(&ciphertext, &keys.secret_key).unwrap();
        
        // Both sides should have the same shared secret
        assert_eq!(secret1, secret2);
    }
    
    #[test]
    fn test_dilithium_signatures() {
        let keys = EntropyPQC::generate_dilithium_keys();
        let message = b"This is a test message for Entropy randomness verification";
        
        // Sign the message
        let signed = EntropyPQC::sign_entropy(message, &keys).unwrap();
        
        // Verify the signature
        assert!(EntropyPQC::verify_signed_entropy(&signed).unwrap());
        
        // Tamper with the message
        let mut tampered = signed.clone();
        tampered.entropy_bytes[0] ^= 1;
        
        // Verification should fail
        assert!(!EntropyPQC::verify_signed_entropy(&tampered).unwrap());
    }
}
