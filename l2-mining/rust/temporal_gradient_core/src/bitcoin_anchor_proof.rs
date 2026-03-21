use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub const TRUC_VERSION: i32 = 3;
pub const MAX_CHILD_WEIGHT: u64 = 40_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BitcoinTxInputRef {
    pub prev_txid: String,
    pub prev_vout: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BitcoinTxOutputRef {
    pub value_sats: u64,
    pub script_pubkey: String,
}

impl BitcoinTxOutputRef {
    pub fn ephemeral_anchor() -> Self {
        Self {
            value_sats: 0,
            script_pubkey: "OP_TRUE".to_string(),
        }
    }

    pub fn is_ephemeral_anchor(&self) -> bool {
        self.value_sats == 0 && self.script_pubkey.trim().eq_ignore_ascii_case("OP_TRUE")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrucTransactionProof {
    pub txid: String,
    pub version: i32,
    pub inputs: Vec<BitcoinTxInputRef>,
    pub outputs: Vec<BitcoinTxOutputRef>,
    pub weight: Option<u64>,
}

impl TrucTransactionProof {
    pub fn is_truc(&self) -> bool {
        self.version == TRUC_VERSION
    }

    pub fn find_anchor_output(&self) -> Result<Option<u32>, String> {
        let mut found: Option<u32> = None;
        for (index, output) in self.outputs.iter().enumerate() {
            if output.is_ephemeral_anchor() {
                let index = index as u32;
                if let Some(existing) = found {
                    return Err(format!(
                        "multiple ephemeral anchors detected at vout {} and {}",
                        existing, index
                    ));
                }
                found = Some(index);
            }
        }
        Ok(found)
    }

    pub fn spends_outpoint(&self, txid: &str, vout: u32) -> bool {
        self.inputs
            .iter()
            .any(|input| input.prev_txid == txid && input.prev_vout == vout)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BitcoinAnchorProof {
    pub proof_id: String,
    pub parent_txid: String,
    pub child_txid: String,
    pub anchor_vout: u32,
    pub block_height: u64,
    pub creator: String,
    pub fee_rate: u64,
    pub created_at: u64,
    pub creator_commitment: Option<String>,
}

impl BitcoinAnchorProof {
    pub fn create(
        parent_txid: impl Into<String>,
        child_txid: impl Into<String>,
        anchor_vout: u32,
        block_height: u64,
        creator: impl Into<String>,
        fee_rate: u64,
        creator_commitment: Option<String>,
    ) -> Self {
        let parent_txid = parent_txid.into();
        let child_txid = child_txid.into();
        let creator = creator.into();
        let created_at = now_secs();
        let proof_id = compute_proof_id(
            &parent_txid,
            &child_txid,
            anchor_vout,
            block_height,
            &creator,
            fee_rate,
            creator_commitment.as_deref(),
        );

        Self {
            proof_id,
            parent_txid,
            child_txid,
            anchor_vout,
            block_height,
            creator,
            fee_rate,
            created_at,
            creator_commitment,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnchorClaimRegistry {
    claims: HashMap<String, BitcoinAnchorProof>,
    anchor_outpoints: HashMap<String, String>,
    child_txids: HashMap<String, String>,
    creator_claim_counts: HashMap<String, u64>,
    max_claims_per_creator: u64,
}

impl AnchorClaimRegistry {
    pub fn new() -> Self {
        Self {
            max_claims_per_creator: 100_000,
            ..Self::default()
        }
    }

    pub fn with_max_claims_per_creator(max_claims_per_creator: u64) -> Self {
        Self {
            max_claims_per_creator,
            ..Self::default()
        }
    }

    pub fn is_claimed(&self, proof_id: &str) -> bool {
        self.claims.contains_key(proof_id)
    }

    pub fn is_outpoint_claimed(&self, parent_txid: &str, anchor_vout: u32) -> bool {
        self.anchor_outpoints
            .contains_key(&format!("{parent_txid}:{anchor_vout}"))
    }

    pub fn is_child_claimed(&self, child_txid: &str) -> bool {
        self.child_txids.contains_key(child_txid)
    }

    pub fn creator_claim_count(&self, creator: &str) -> u64 {
        self.creator_claim_counts.get(creator).copied().unwrap_or(0)
    }

    pub fn register_claim(&mut self, proof: BitcoinAnchorProof) -> Result<(), String> {
        if self.is_claimed(&proof.proof_id) {
            return Err(format!("duplicate proof_id {}", proof.proof_id));
        }

        let outpoint = format!("{}:{}", proof.parent_txid, proof.anchor_vout);
        if let Some(existing) = self.anchor_outpoints.get(&outpoint) {
            return Err(format!("anchor outpoint already claimed by {}", existing));
        }
        if let Some(existing) = self.child_txids.get(&proof.child_txid) {
            return Err(format!("child transaction already claimed by {}", existing));
        }

        let claim_count = self.creator_claim_count(&proof.creator);
        if claim_count >= self.max_claims_per_creator {
            return Err(format!(
                "creator {} reached claim limit ({})",
                proof.creator, self.max_claims_per_creator
            ));
        }

        self.anchor_outpoints.insert(outpoint, proof.proof_id.clone());
        self.child_txids
            .insert(proof.child_txid.clone(), proof.proof_id.clone());
        self.creator_claim_counts
            .insert(proof.creator.clone(), claim_count + 1);
        self.claims.insert(proof.proof_id.clone(), proof);
        Ok(())
    }

    pub fn total_claims(&self) -> usize {
        self.claims.len()
    }
}

pub struct AnchorVerifier;

impl AnchorVerifier {
    pub fn verify(
        proof: &BitcoinAnchorProof,
        parent_tx: &TrucTransactionProof,
        child_tx: &TrucTransactionProof,
        registry: Option<&AnchorClaimRegistry>,
        min_fee_rate: u64,
    ) -> Result<(), String> {
        if !parent_tx.is_truc() {
            return Err(format!(
                "parent nVersion={}, expected {}",
                parent_tx.version, TRUC_VERSION
            ));
        }
        if !child_tx.is_truc() {
            return Err(format!(
                "child nVersion={}, expected {}",
                child_tx.version, TRUC_VERSION
            ));
        }

        let anchor_vout = parent_tx
            .find_anchor_output()?
            .ok_or_else(|| "no OP_TRUE 0-sat anchor output found in parent".to_string())?;
        if anchor_vout != proof.anchor_vout {
            return Err(format!(
                "anchor at vout={}, proof claims vout={}",
                anchor_vout, proof.anchor_vout
            ));
        }
        if !child_tx.spends_outpoint(&parent_tx.txid, anchor_vout) {
            return Err("child does not spend parent anchor output".to_string());
        }
        if proof.parent_txid != parent_tx.txid {
            return Err("proof parent_txid does not match provided parent transaction".to_string());
        }
        if proof.child_txid != child_tx.txid {
            return Err("proof child_txid does not match provided child transaction".to_string());
        }
        if let Some(weight) = child_tx.weight {
            if weight > MAX_CHILD_WEIGHT {
                return Err(format!(
                    "child weight {} exceeds TRUC policy limit {}",
                    weight, MAX_CHILD_WEIGHT
                ));
            }
        }
        if proof.fee_rate < min_fee_rate {
            return Err(format!(
                "fee_rate {} below minimum {}",
                proof.fee_rate, min_fee_rate
            ));
        }

        if let Some(registry) = registry {
            if registry.is_claimed(&proof.proof_id) {
                return Err(format!("anchor already claimed: {}", proof.proof_id));
            }
            if registry.is_outpoint_claimed(&proof.parent_txid, proof.anchor_vout) {
                return Err(format!(
                    "outpoint already claimed: {}:{}",
                    proof.parent_txid, proof.anchor_vout
                ));
            }
            if registry.is_child_claimed(&proof.child_txid) {
                return Err(format!("child transaction already claimed: {}", proof.child_txid));
            }
        }

        Ok(())
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn compute_proof_id(
    parent_txid: &str,
    child_txid: &str,
    anchor_vout: u32,
    block_height: u64,
    creator: &str,
    fee_rate: u64,
    creator_commitment: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(parent_txid.as_bytes());
    hasher.update(child_txid.as_bytes());
    hasher.update(anchor_vout.to_le_bytes());
    hasher.update(block_height.to_le_bytes());
    hasher.update(creator.as_bytes());
    hasher.update(fee_rate.to_le_bytes());
    if let Some(creator_commitment) = creator_commitment {
        hasher.update(creator_commitment.as_bytes());
    }
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parent_with_anchor() -> TrucTransactionProof {
        TrucTransactionProof {
            txid: "parent-tx".to_string(),
            version: TRUC_VERSION,
            inputs: vec![],
            outputs: vec![
                BitcoinTxOutputRef {
                    value_sats: 1_000,
                    script_pubkey: "0014abcd".to_string(),
                },
                BitcoinTxOutputRef::ephemeral_anchor(),
            ],
            weight: Some(800),
        }
    }

    fn child_spending_anchor() -> TrucTransactionProof {
        TrucTransactionProof {
            txid: "child-tx".to_string(),
            version: TRUC_VERSION,
            inputs: vec![BitcoinTxInputRef {
                prev_txid: "parent-tx".to_string(),
                prev_vout: 1,
            }],
            outputs: vec![BitcoinTxOutputRef {
                value_sats: 900,
                script_pubkey: "0014dcba".to_string(),
            }],
            weight: Some(1_200),
        }
    }

    #[test]
    fn verifies_valid_anchor_proof() {
        let parent = parent_with_anchor();
        let child = child_spending_anchor();
        let proof = BitcoinAnchorProof::create(
            "parent-tx",
            "child-tx",
            1,
            900_000,
            "creator-1",
            12,
            Some("creator-commitment".to_string()),
        );

        assert!(AnchorVerifier::verify(&proof, &parent, &child, None, 5).is_ok());
    }

    #[test]
    fn rejects_wrong_anchor_vout() {
        let parent = parent_with_anchor();
        let child = child_spending_anchor();
        let proof = BitcoinAnchorProof::create("parent-tx", "child-tx", 0, 1, "creator", 1, None);

        let err = AnchorVerifier::verify(&proof, &parent, &child, None, 0).unwrap_err();
        assert!(err.contains("proof claims vout=0"));
    }

    #[test]
    fn rejects_duplicate_claims_in_registry() {
        let parent = parent_with_anchor();
        let child = child_spending_anchor();
        let proof = BitcoinAnchorProof::create("parent-tx", "child-tx", 1, 1, "creator", 1, None);
        let mut registry = AnchorClaimRegistry::new();
        registry.register_claim(proof.clone()).unwrap();

        let err = AnchorVerifier::verify(&proof, &parent, &child, Some(&registry), 0).unwrap_err();
        assert!(err.contains("already claimed"));
    }

    #[test]
    fn registry_blocks_outpoint_reuse() {
        let mut registry = AnchorClaimRegistry::new();
        let first = BitcoinAnchorProof::create("parent-tx", "child-a", 1, 1, "creator", 1, None);
        let second = BitcoinAnchorProof::create("parent-tx", "child-b", 1, 2, "creator", 1, None);
        registry.register_claim(first).unwrap();
        let err = registry.register_claim(second).unwrap_err();
        assert!(err.contains("anchor outpoint already claimed"));
    }
}