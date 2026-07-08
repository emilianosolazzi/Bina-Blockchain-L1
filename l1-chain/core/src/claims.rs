//! Signed PoW block claims and deterministic candidate election.
//!
//! A miner does not earn a reward for merely placing an address in a header.
//! The miner must sign the exact header with the wallet whose public key derives
//! that address. When several valid claims target the same height, nodes rank
//! them by objective work first and a deterministic score second, so arrival
//! order is not consensus.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::block::{leading_zero_bits, meets_difficulty, L1BlockHeader};
use crate::crypto::{HybridSignature, WalletKeypair, WalletPublicKey};

const CLAIM_TAG: &[u8] = b"BINA-BLOCK-CLAIM-v1";
const ELECTION_TAG: &[u8] = b"BINA-ELECTION-v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedBlockClaim {
    pub header: L1BlockHeader,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
}

impl SignedBlockClaim {
    pub fn sign(header: L1BlockHeader, keypair: &WalletKeypair) -> Self {
        let msg = claim_message(&header);
        let sig = keypair.sign(&msg);
        Self {
            header,
            public_key: keypair.public_key().to_bytes(),
            signature: sig.to_bytes(),
        }
    }

    pub fn verify(&self) -> Result<()> {
        let public_key = WalletPublicKey::from_bytes(&self.public_key)?;
        if public_key.address() != self.header.miner_address {
            bail!("claim public key does not derive header miner_address");
        }

        let block_hash = self.block_hash();
        if !meets_difficulty(&block_hash, self.header.difficulty_bits) {
            bail!("claim block hash does not meet declared difficulty");
        }

        let signature = HybridSignature::from_bytes(&self.signature)?;
        public_key.verify(&claim_message(&self.header), &signature)
    }

    pub fn block_hash(&self) -> [u8; 32] {
        self.header.hash()
    }

    pub fn block_hash_hex(&self) -> String {
        hex::encode(self.block_hash())
    }

    pub fn miner_address_hex(&self) -> String {
        hex::encode(self.header.miner_address)
    }

    pub fn work_bits(&self) -> u32 {
        leading_zero_bits(&self.block_hash())
    }

    pub fn claim_digest_hex(&self) -> String {
        hex::encode(claim_message(&self.header))
    }

    pub fn election_score(&self) -> [u8; 32] {
        election_score(&self.header)
    }

    pub fn election_score_hex(&self) -> String {
        hex::encode(self.election_score())
    }
}

pub fn claim_message(header: &L1BlockHeader) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(CLAIM_TAG);
    h.update(&header.hash());
    *h.finalize().as_bytes()
}

pub fn election_score(header: &L1BlockHeader) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(ELECTION_TAG);
    h.update(&header.height.to_le_bytes());
    h.update(&header.prev_hash);
    h.update(&header.hash());
    h.update(&header.miner_address);
    *h.finalize().as_bytes()
}

pub fn claim_is_better(candidate: &SignedBlockClaim, incumbent: &SignedBlockClaim) -> bool {
    let candidate_work = candidate.work_bits();
    let incumbent_work = incumbent.work_bits();
    if candidate_work != incumbent_work {
        return candidate_work > incumbent_work;
    }

    let candidate_score = candidate.election_score();
    let incumbent_score = incumbent.election_score();
    if candidate_score != incumbent_score {
        return candidate_score < incumbent_score;
    }

    candidate.block_hash() < incumbent.block_hash()
}

pub fn select_winning_claim<I>(claims: I) -> Option<SignedBlockClaim>
where
    I: IntoIterator<Item = SignedBlockClaim>,
{
    claims.into_iter().reduce(|winner, claim| {
        if claim_is_better(&claim, &winner) {
            claim
        } else {
            winner
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_for(kp: &WalletKeypair, nonce: u64) -> L1BlockHeader {
        L1BlockHeader {
            version: 1,
            height: 1,
            prev_hash: [1u8; 32],
            merkle_root: [0u8; 32],
            state_root: [3u8; 32],
            timestamp: 1_800_000_000,
            nonce,
            miner_address: kp.address(),
            difficulty_bits: 0,
            bitcoin_seed_hash: [2u8; 32],
        }
    }

    #[test]
    fn signed_claim_verifies_and_binds_address() {
        let kp = WalletKeypair::generate();
        let claim = SignedBlockClaim::sign(header_for(&kp, 7), &kp);
        claim.verify().unwrap();

        let mut tampered = claim.clone();
        tampered.header.miner_address = [9u8; 20];
        assert!(tampered.verify().is_err());
    }

    #[test]
    fn winner_is_independent_of_arrival_order() {
        let kp_a = WalletKeypair::generate();
        let kp_b = WalletKeypair::generate();
        let claim_a = SignedBlockClaim::sign(header_for(&kp_a, 11), &kp_a);
        let claim_b = SignedBlockClaim::sign(header_for(&kp_b, 12), &kp_b);

        let first = select_winning_claim(vec![claim_a.clone(), claim_b.clone()]).unwrap();
        let second = select_winning_claim(vec![claim_b, claim_a]).unwrap();

        assert_eq!(first.block_hash(), second.block_hash());
        assert_eq!(first.election_score(), second.election_score());
    }
}
