//! Signed BINA value-transfer transactions.
//!
//! Native wallets sign transfers with the full Ed25519 + Falcon hybrid key.
//! FastPath-derived provisional wallets may sign transfers with Ed25519 only so
//! they can sweep received BINA into a sovereign native wallet before Falcon
//! deterministic derivation exists. Block/mining claims remain hybrid-only.

use crate::crypto::{HybridSignature, WalletAddress, WalletKeypair, WalletPublicKey};
use anyhow::{anyhow, bail, Result};
use ed25519_dalek::{Signature as Ed25519Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

pub const TX_DOMAIN_TAG: &[u8] = b"BINA-TX-v1";
pub const ED25519_PUBLIC_KEY_BYTES: usize = 32;
pub const ED25519_SIGNATURE_BYTES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    pub from: WalletAddress,
    pub to: WalletAddress,
    pub amount: u64,
    pub nonce: u64,
    pub fee: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedTransaction {
    pub tx: Transaction,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
}

impl Transaction {
    pub fn new(from: WalletAddress, to: WalletAddress, amount: u64, nonce: u64, fee: u64) -> Self {
        Self { from, to, amount, nonce, fee }
    }

    pub fn digest(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        h.update(TX_DOMAIN_TAG);
        h.update(&self.from);
        h.update(&self.to);
        h.update(&self.amount.to_le_bytes());
        h.update(&self.nonce.to_le_bytes());
        h.update(&self.fee.to_le_bytes());
        *h.finalize().as_bytes()
    }

    pub fn digest_hex(&self) -> String {
        hex::encode(self.digest())
    }
}

impl SignedTransaction {
    pub fn sign(tx: Transaction, keypair: &WalletKeypair) -> Result<Self> {
        if tx.from != keypair.address() {
            bail!("transaction from address does not match signing wallet");
        }
        let digest = tx.digest();
        let signature = keypair.sign(&digest).to_bytes();
        Ok(Self {
            tx,
            public_key: keypair.public_key().to_bytes(),
            signature,
        })
    }

    pub fn sign_ed25519_only(tx: Transaction, signing_key: &SigningKey) -> Result<Self> {
        let verifying_key = signing_key.verifying_key();
        let public_key = verifying_key.as_bytes();
        if tx.from != ed25519_only_address(public_key)? {
            bail!("transaction from address does not match Ed25519 signing key");
        }
        let digest = tx.digest();
        let signature = signing_key.sign(&digest);
        Ok(Self {
            tx,
            public_key: public_key.to_vec(),
            signature: signature.to_bytes().to_vec(),
        })
    }

    pub fn verify(&self) -> Result<()> {
        self.validate_fields()?;
        if self.public_key.len() == ED25519_PUBLIC_KEY_BYTES {
            return self.verify_ed25519_only();
        }

        let public_key = WalletPublicKey::from_bytes(&self.public_key)?;
        if public_key.address() != self.tx.from {
            bail!("transaction public key does not derive the from address");
        }
        let signature = HybridSignature::from_bytes(&self.signature)?;
        public_key
            .verify(&self.tx.digest(), &signature)
            .map_err(|e| anyhow!("transaction signature verification failed: {e}"))
    }

    fn validate_fields(&self) -> Result<()> {
        if self.tx.amount == 0 {
            bail!("transaction amount must be non-zero");
        }
        if self.tx.from == self.tx.to {
            bail!("transaction cannot send to self");
        }
        Ok(())
    }

    fn verify_ed25519_only(&self) -> Result<()> {
        if self.tx.from != ed25519_only_address(&self.public_key)? {
            bail!("transaction Ed25519 public key does not derive the from address");
        }
        if self.signature.len() != ED25519_SIGNATURE_BYTES {
            bail!(
                "Ed25519-only signature must be {ED25519_SIGNATURE_BYTES} bytes, got {}",
                self.signature.len()
            );
        }
        let public_key_bytes: [u8; ED25519_PUBLIC_KEY_BYTES] = self.public_key[..]
            .try_into()
            .map_err(|_| anyhow!("Ed25519 public key length mismatch"))?;
        let signature_bytes: [u8; ED25519_SIGNATURE_BYTES] = self.signature[..]
            .try_into()
            .map_err(|_| anyhow!("Ed25519 signature length mismatch"))?;
        let public_key = VerifyingKey::from_bytes(&public_key_bytes)
            .map_err(|e| anyhow!("Ed25519 public key invalid: {e}"))?;
        let signature = Ed25519Signature::from_bytes(&signature_bytes);
        public_key
            .verify(&self.tx.digest(), &signature)
            .map_err(|e| anyhow!("Ed25519 transaction signature verification failed: {e}"))
    }

    pub fn tx_id(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        h.update(b"BINA-TXID-v1");
        h.update(&self.tx.digest());
        h.update(&self.signature);
        *h.finalize().as_bytes()
    }

    pub fn tx_id_hex(&self) -> String {
        hex::encode(self.tx_id())
    }

    pub fn from_hex(&self) -> String { hex::encode(self.tx.from) }
    pub fn to_hex(&self) -> String { hex::encode(self.tx.to) }
    pub fn public_key_hex(&self) -> String { hex::encode(&self.public_key) }
    pub fn signature_hex(&self) -> String { hex::encode(&self.signature) }
}

pub fn parse_address_hex(address: &str) -> Result<WalletAddress> {
    let trimmed = address.strip_prefix("0x").unwrap_or(address);
    let bytes = hex::decode(trimmed).map_err(|e| anyhow!("address is not valid hex: {e}"))?;
    if bytes.len() != 20 {
        bail!("address must be 20 bytes / 40 hex chars, got {} bytes", bytes.len());
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Ok(out)
}

pub fn ed25519_only_address(public_key: &[u8]) -> Result<WalletAddress> {
    if public_key.len() != ED25519_PUBLIC_KEY_BYTES {
        bail!(
            "Ed25519 public key must be {ED25519_PUBLIC_KEY_BYTES} bytes, got {}",
            public_key.len()
        );
    }
    let digest = Keccak256::digest(public_key);
    let mut out = [0u8; 20];
    out.copy_from_slice(&digest[..20]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_transaction_verifies() {
        let sender = WalletKeypair::generate();
        let recipient = WalletKeypair::generate();
        let tx = Transaction::new(sender.address(), recipient.address(), 42, 7, 1);
        let signed = SignedTransaction::sign(tx, &sender).unwrap();
        signed.verify().unwrap();
        assert_eq!(signed.from_hex(), sender.address_hex());
        assert_eq!(signed.to_hex(), recipient.address_hex());
    }

    #[test]
    fn signature_binds_transaction_fields() {
        let sender = WalletKeypair::generate();
        let recipient = WalletKeypair::generate();
        let tx = Transaction::new(sender.address(), recipient.address(), 42, 7, 1);
        let mut signed = SignedTransaction::sign(tx, &sender).unwrap();
        signed.tx.amount = 43;
        assert!(signed.verify().is_err());
    }

    #[test]
    fn rejects_wrong_from_address() {
        let sender = WalletKeypair::generate();
        let other = WalletKeypair::generate();
        let recipient = WalletKeypair::generate();
        let tx = Transaction::new(other.address(), recipient.address(), 42, 7, 1);
        assert!(SignedTransaction::sign(tx, &sender).is_err());
    }

    #[test]
    fn ed25519_only_transaction_verifies() {
        let sender = SigningKey::from_bytes(&[7u8; 32]);
        let recipient = WalletKeypair::generate();
        let from = ed25519_only_address(sender.verifying_key().as_bytes()).unwrap();
        let tx = Transaction::new(from, recipient.address(), 42, 0, 1);
        let signed = SignedTransaction::sign_ed25519_only(tx, &sender).unwrap();
        signed.verify().unwrap();
        assert_eq!(signed.public_key.len(), ED25519_PUBLIC_KEY_BYTES);
        assert_eq!(signed.signature.len(), ED25519_SIGNATURE_BYTES);
    }

    #[test]
    fn ed25519_only_transaction_binds_fields() {
        let sender = SigningKey::from_bytes(&[9u8; 32]);
        let recipient = WalletKeypair::generate();
        let from = ed25519_only_address(sender.verifying_key().as_bytes()).unwrap();
        let tx = Transaction::new(from, recipient.address(), 42, 0, 1);
        let mut signed = SignedTransaction::sign_ed25519_only(tx, &sender).unwrap();
        signed.tx.to = WalletKeypair::generate().address();
        assert!(signed.verify().is_err());
    }

    #[test]
    fn parses_hex_address() {
        let address = "3054ac8bc5c9b358e270e17183851201d0bc6b69";
        assert_eq!(hex::encode(parse_address_hex(address).unwrap()), address);
        assert_eq!(hex::encode(parse_address_hex(&format!("0x{address}")).unwrap()), address);
    }
}
