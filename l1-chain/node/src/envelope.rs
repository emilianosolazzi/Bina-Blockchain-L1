use anyhow::{bail, Result};
use l1_core::claims::SignedBlockClaim;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum BinaMessage {
    #[serde(rename = "bina.block_claim.v1")]
    BlockClaim(BlockClaimEnvelope),

    #[serde(rename = "bina.peer_hello.v1")]
    PeerHello(PeerHelloEnvelope),

    #[serde(rename = "bina.peer_list.v1")]
    PeerList(PeerListEnvelope),

    #[serde(rename = "bina.ping.v1")]
    Ping(PingEnvelope),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BlockClaimEnvelope {
    pub network: String,
    pub message_id: String,
    pub sent_at_unix: u64,
    pub ttl: u8,
    pub claim: SignedBlockClaim,
    pub derived: DerivedFields,
}

impl BlockClaimEnvelope {
    pub fn from_claim(network: impl Into<String>, ttl: u8, claim: SignedBlockClaim) -> Self {
        let network = network.into();
        let sent_at_unix = unix_secs();
        let derived = DerivedFields::from_claim(&claim);
        let message_id = message_id(&network, sent_at_unix, &claim, &derived);
        Self {
            network,
            message_id,
            sent_at_unix,
            ttl,
            claim,
            derived,
        }
    }

    pub fn verify(&self) -> Result<()> {
        self.claim.verify()?;
        let expected = DerivedFields::from_claim(&self.claim);
        if self.derived != expected {
            bail!("block claim derived fields do not match claim");
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct DerivedFields {
    pub header_hash: String,
    pub claim_digest: String,
    pub work_bits: u32,
    pub election_score: String,
}

impl DerivedFields {
    pub fn from_claim(claim: &SignedBlockClaim) -> Self {
        Self {
            header_hash: claim.block_hash_hex(),
            claim_digest: claim.claim_digest_hex(),
            work_bits: claim.work_bits(),
            election_score: claim.election_score_hex(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PeerHelloEnvelope {
    pub network: String,
    pub version: u32,
    pub best_height: u64,
    pub best_hash: String,
    pub listen_addr: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PeerListEnvelope {
    pub peers: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PingEnvelope {
    pub nonce: u64,
    pub height: u64,
}

pub fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn message_id(
    network: &str,
    sent_at_unix: u64,
    claim: &SignedBlockClaim,
    derived: &DerivedFields,
) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"BINA-P2P-MSG-v1");
    h.update(network.as_bytes());
    h.update(&sent_at_unix.to_le_bytes());
    h.update(derived.header_hash.as_bytes());
    h.update(derived.claim_digest.as_bytes());
    h.update(derived.election_score.as_bytes());
    h.update(&claim.signature);
    hex::encode(h.finalize().as_bytes())
}