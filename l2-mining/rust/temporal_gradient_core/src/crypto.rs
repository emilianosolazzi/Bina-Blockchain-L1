use crate::pqc::{apply_pqc_enhancement, PqcMode};
use blake3;
use k256::ecdsa::{signature::Signer as _, Signature, SigningKey};
use rand::{rngs::OsRng, RngCore};
use sha2::Digest;
use sha3::Keccak256;

pub const QR_HASH_ITERATIONS: u8 = 3;
pub const QR_HASH_ROTATION: u8 = 7;

#[derive(Debug, Clone)]
pub struct DynamicMiningCommitment {
    pub commit_hash: [u8; 32],
    pub pool_id: u8,
    pub nonce: u64,
    pub deadline: u64,
}

#[derive(Debug, Clone)]
pub struct MiningMaterial {
    pub previous_output: [u8; 32],
    pub temporal_seed: [u8; 8],
    pub nonce: u64,
    pub miner_address: [u8; 20],
    pub time_based_entropy: [u8; 32],
    pub secret_value: [u8; 32],
}

#[derive(Debug, Clone)]
pub struct CommitmentPayload {
    pub commitment: DynamicMiningCommitment,
    pub entropy_hash: [u8; 32],
    pub signature: Vec<u8>,
    pub solution_hash: [u8; 32],
    pub secret_value: [u8; 32],
    pub temporal_seed: [u8; 8],
}

pub fn contract_hash_message(message: &[u8]) -> [u8; 32] {
    Keccak256::digest(message).into()
}

pub fn miner_address_from_signing_key(key: &SigningKey) -> [u8; 20] {
    let point = key.verifying_key().to_encoded_point(false);
    let hash = Keccak256::digest(point.as_bytes());
    let mut out = [0u8; 20];
    out.copy_from_slice(&hash[12..32]);
    out
}

pub fn random_secret() -> [u8; 32] {
    let mut secret = [0u8; 32];
    OsRng.fill_bytes(&mut secret);
    secret
}

pub fn create_entropy_hash(material: &MiningMaterial) -> [u8; 32] {
    contract_hash_message(
        &[
            material.previous_output.as_slice(),
            material.temporal_seed.as_slice(),
            &material.nonce.to_be_bytes(),
            material.miner_address.as_slice(),
            material.secret_value.as_slice(),
        ]
        .concat(),
    )
}

fn quantum_resistant_hash_inner(input: &[u8]) -> [u8; 32] {
    let mut h: [u8; 32] = contract_hash_message(input);
    for i in 0..QR_HASH_ITERATIONS {
        let mut xor_h = h;
        // Solidity: h ^ bytes32(uint256(i+1))  — value lands at byte[31] (LSB)
        xor_h[31] ^= i + 1;
        h = contract_hash_message(&xor_h);
        // 256-bit left rotation by QR_HASH_ROTATION bits to match Solidity:
        //   h = bytes32((uint256(h) << 7) | (uint256(h) >> (256 - 7)));
        h = rotate_256_left(h, QR_HASH_ROTATION as u32);
    }
    h
}

/// Left-rotate a 256-bit big-endian value by `bits` (must be < 8).
/// Matches Solidity: `bytes32((uint256(h) << bits) | (uint256(h) >> (256 - bits)))`.
fn rotate_256_left(input: [u8; 32], bits: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    for i in 0..32 {
        let next = (i + 1) % 32;
        out[i] = (input[i] << bits) | (input[next] >> (8 - bits));
    }
    out
}

pub fn quantum_resistant_hash(
    signature: &Signature,
    entropy_hash: &[u8; 32],
    secret: &[u8; 32],
    pqc_mode: PqcMode,
) -> [u8; 32] {
    let packed = [signature.to_der().as_bytes(), entropy_hash, secret].concat();
    let qr = quantum_resistant_hash_inner(&packed);
    let pqc = apply_pqc_enhancement(&packed, pqc_mode);
    let hybrid = [qr.as_slice(), pqc.as_slice()].concat();
    contract_hash_message(&hybrid)
}

pub fn build_commitment_payload(
    signing_key: &SigningKey,
    material: &MiningMaterial,
    pool_id: u8,
    deadline: u64,
    pqc_mode: PqcMode,
) -> CommitmentPayload {
    let entropy_hash = create_entropy_hash(material);
    let signature: Signature = signing_key.sign(&entropy_hash);
    let solution_hash = quantum_resistant_hash(&signature, &entropy_hash, &material.secret_value, pqc_mode);

    let commit_hash = contract_hash_message(
        &[
            material.previous_output.as_slice(),
            material.temporal_seed.as_slice(),
            &material.nonce.to_be_bytes(),
            signature.to_der().as_bytes(),
            material.secret_value.as_slice(),
            material.miner_address.as_slice(),
        ]
        .concat(),
    );

    CommitmentPayload {
        commitment: DynamicMiningCommitment {
            commit_hash,
            pool_id,
            nonce: material.nonce,
            deadline,
        },
        entropy_hash,
        signature: signature.to_der().as_bytes().to_vec(),
        solution_hash,
        secret_value: material.secret_value,
        temporal_seed: material.temporal_seed,
    }
}

pub fn pre_filter_nonce(nonce: u64, input: &[u8], target_divisor: u64) -> bool {
    let hash = blake3::hash(&[input, &nonce.to_be_bytes()].concat());
    let mut prefix = [0u8; 8];
    prefix.copy_from_slice(&hash.as_bytes()[..8]);
    let value = u64::from_be_bytes(prefix);
    value % target_divisor.max(1) == 0
}

pub fn has_leading_zero_bits(hash: &[u8; 32], zero_bits: u8) -> bool {
    let full_bytes = (zero_bits / 8) as usize;
    let extra_bits = zero_bits % 8;

    if hash.iter().take(full_bytes).any(|byte| *byte != 0) {
        return false;
    }

    if extra_bits == 0 {
        return true;
    }

    let mask = 0xFFu8 << (8 - extra_bits);
    hash[full_bytes] & mask == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_leading_zero_bits() {
        let hash = [0u8; 32];
        assert!(has_leading_zero_bits(&hash, 12));
    }
}
