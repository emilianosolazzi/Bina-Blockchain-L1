//! Hybrid wallet cryptography: Ed25519 (classical) + Falcon-512 (post-quantum)
//!
//! Both signatures MUST verify for a transaction to be accepted.  Compromise of
//! one algorithm still leaves the wallet protected by the other.
//!
//! Key / signature sizes
//! ──────────────────────────────────────────────────────────
//!   Ed25519    pubkey   32 B  │  seckey   32 B  │  sig  64 B
//!   Falcon-512 pubkey  897 B  │  seckey 1281 B  │  sig ≤666 B (variable)
//!
//! Address (20 bytes)
//!   blake3("BINA-ADDR-v1" ‖ ed25519_pk_bytes ‖ falcon_pk_bytes)[..20]
//!
//! Serialisation layout
//!   WalletPublicKey  │ 32 (ed) ‖ 897 (fal_pk) = 929 B
//!   WalletKeypair    │ 32 (ed_sk) ‖ 897 (fal_pk) ‖ 1281 (fal_sk) = 2210 B
//!   HybridSignature  │ u16_le (fal_len) ‖ 64 (ed_sig) ‖ fal_bytes

use anyhow::{anyhow, bail, Result};
use blake3::Hasher;
use ed25519_dalek::{Signature as Ed25519Sig, Signer, SigningKey, Verifier, VerifyingKey};
use pqcrypto_falcon::falcon512;
use pqcrypto_traits::sign::{
    DetachedSignature as PqDetachedSig,
    PublicKey        as PqPk,
    SecretKey        as PqSk,
};
use rand::rngs::OsRng;

// Key-size constants (from Falcon-512 spec)
const FALCON_PK_BYTES: usize = 897;
const FALCON_SK_BYTES: usize = 1281;

const ADDR_TAG: &[u8] = b"BINA-ADDR-v1";

// ─── Address ──────────────────────────────────────────────────────────────

/// 20-byte on-chain wallet address.  Compatible with the `miner_address`
/// field in `L1BlockHeader`.
pub type WalletAddress = [u8; 20];

// ─── Public key ───────────────────────────────────────────────────────────

/// Hybrid public key: Ed25519 (classical, 32 B) + Falcon-512 (PQ, 897 B)
#[derive(Clone)]
pub struct WalletPublicKey {
    /// Ed25519 verifying key
    pub ed25519: VerifyingKey,
    /// Falcon-512 public key
    pub falcon:  falcon512::PublicKey,
}

impl WalletPublicKey {
    // ── Address ────────────────────────────────────────────────────────────

    /// Derive the 20-byte wallet address (first 20 bytes of BLAKE3).
    pub fn address(&self) -> WalletAddress {
        let mut h = Hasher::new();
        h.update(ADDR_TAG);
        h.update(self.ed25519.as_bytes());
        h.update(self.falcon.as_bytes());
        let hash = h.finalize();
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&hash.as_bytes()[..20]);
        addr
    }

    // ── Serialisation ──────────────────────────────────────────────────────

    /// Serialize to 929 bytes: 32 (ed25519) ‖ 897 (falcon)
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + FALCON_PK_BYTES);
        out.extend_from_slice(self.ed25519.as_bytes());
        out.extend_from_slice(self.falcon.as_bytes());
        out
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        let expected = 32 + FALCON_PK_BYTES;
        if b.len() != expected {
            bail!("WalletPublicKey: expected {expected} bytes, got {}", b.len());
        }
        let ed25519 = VerifyingKey::from_bytes(b[..32].try_into()?)
            .map_err(|e| anyhow!("Ed25519 public key invalid: {e}"))?;
        let falcon = falcon512::PublicKey::from_bytes(&b[32..])
            .map_err(|_| anyhow!("Falcon-512 public key invalid"))?;
        Ok(Self { ed25519, falcon })
    }

    /// Hex-encoded address string for display
    pub fn address_hex(&self) -> String {
        hex::encode(self.address())
    }

    // ── Verification ──────────────────────────────────────────────────────

    /// Verify a hybrid signature.  BOTH classical and PQ must pass.
    pub fn verify(&self, msg: &[u8], sig: &HybridSignature) -> Result<()> {
        // 1. Ed25519 (classical)
        self.ed25519
            .verify(msg, &sig.ed25519)
            .map_err(|e| anyhow!("Ed25519 verification failed: {e}"))?;

        // 2. Falcon-512 (post-quantum)
        let falcon_sig = falcon512::DetachedSignature::from_bytes(&sig.falcon)
            .map_err(|_| anyhow!("Falcon-512 signature bytes invalid"))?;
        falcon512::verify_detached_signature(&falcon_sig, msg, &self.falcon)
            .map_err(|_| anyhow!("Falcon-512 verification failed"))?;

        Ok(())
    }
}

// ─── Keypair ──────────────────────────────────────────────────────────────

/// Hybrid signing keypair: Ed25519 + Falcon-512
pub struct WalletKeypair {
    /// Public component (address, verification)
    pub public:   WalletPublicKey,
    ed_sk:        SigningKey,
    falcon_sk:    falcon512::SecretKey,
}

impl WalletKeypair {
    // ── Generation ─────────────────────────────────────────────────────────

    /// Generate a fresh hybrid keypair from OS randomness.
    pub fn generate() -> Self {
        let ed_sk = SigningKey::generate(&mut OsRng);
        let (falcon_pk, falcon_sk) = falcon512::keypair();
        let public = WalletPublicKey {
            ed25519: ed_sk.verifying_key(),
            falcon:  falcon_pk,
        };
        Self { public, ed_sk, falcon_sk }
    }

    // ── Convenience accessors ──────────────────────────────────────────────

    pub fn address(&self) -> WalletAddress     { self.public.address() }
    pub fn address_hex(&self) -> String        { self.public.address_hex() }
    pub fn public_key(&self) -> &WalletPublicKey { &self.public }

    // ── Signing ────────────────────────────────────────────────────────────

    /// Sign `msg` with both keys.  Returns a `HybridSignature`.
    pub fn sign(&self, msg: &[u8]) -> HybridSignature {
        let ed_sig     = self.ed_sk.sign(msg);
        let falcon_sig = falcon512::detached_sign(msg, &self.falcon_sk);
        HybridSignature {
            ed25519: ed_sig,
            falcon:  falcon_sig.as_bytes().to_vec(),
        }
    }

    // ── Serialisation (secret bytes — store securely!) ─────────────────────

    /// Serialize secret key material to 2210 bytes:
    ///   32 (ed25519 sk) ‖ 897 (falcon pk) ‖ 1281 (falcon sk)
    ///
    /// The ed25519 public key is re-derived from the signing key on load.
    pub fn to_secret_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + FALCON_PK_BYTES + FALCON_SK_BYTES);
        out.extend_from_slice(&self.ed_sk.to_bytes());
        out.extend_from_slice(self.public.falcon.as_bytes());
        out.extend_from_slice(self.falcon_sk.as_bytes());
        out
    }

    pub fn from_secret_bytes(b: &[u8]) -> Result<Self> {
        const TOTAL: usize = 32 + FALCON_PK_BYTES + FALCON_SK_BYTES;
        if b.len() != TOTAL {
            bail!("WalletKeypair: expected {TOTAL} bytes, got {}", b.len());
        }
        let ed_sk_bytes: [u8; 32] = b[..32].try_into()?;
        let ed_sk     = SigningKey::from_bytes(&ed_sk_bytes);
        let falcon_pk = falcon512::PublicKey::from_bytes(&b[32..32 + FALCON_PK_BYTES])
            .map_err(|_| anyhow!("Falcon-512 public key invalid in secret bytes"))?;
        let falcon_sk = falcon512::SecretKey::from_bytes(&b[32 + FALCON_PK_BYTES..])
            .map_err(|_| anyhow!("Falcon-512 secret key invalid in secret bytes"))?;
        let public = WalletPublicKey { ed25519: ed_sk.verifying_key(), falcon: falcon_pk };
        Ok(Self { public, ed_sk, falcon_sk })
    }
}

// ─── Hybrid signature ─────────────────────────────────────────────────────

/// A signature valid only when BOTH Ed25519 and Falcon-512 verify.
#[derive(Clone)]
pub struct HybridSignature {
    /// Ed25519 component (64 bytes, deterministic)
    pub ed25519: Ed25519Sig,
    /// Falcon-512 component (≤ 666 bytes, probabilistic)
    pub falcon:  Vec<u8>,
}

impl HybridSignature {
    // ── Serialisation ──────────────────────────────────────────────────────

    /// Layout: u16_le(falcon_len) ‖ 64 (ed25519) ‖ falcon_bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let flen = self.falcon.len() as u16;
        let mut out = Vec::with_capacity(2 + 64 + self.falcon.len());
        out.extend_from_slice(&flen.to_le_bytes());
        out.extend_from_slice(&self.ed25519.to_bytes());
        out.extend_from_slice(&self.falcon);
        out
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        if b.len() < 2 + 64 {
            bail!("HybridSignature too short ({} bytes)", b.len());
        }
        let flen = u16::from_le_bytes([b[0], b[1]]) as usize;
        if b.len() < 2 + 64 + flen {
            bail!("HybridSignature truncated (declared falcon len = {flen})");
        }
        let ed_bytes: [u8; 64] = b[2..2 + 64].try_into()?;
        let ed25519 = Ed25519Sig::from_bytes(&ed_bytes);
        let falcon  = b[2 + 64..2 + 64 + flen].to_vec();
        Ok(Self { ed25519, falcon })
    }

    // ── Introspection ──────────────────────────────────────────────────────

    /// Ed25519 component as hex (128 chars)
    pub fn ed_hex(&self) -> String    { hex::encode(self.ed25519.to_bytes()) }
    /// Falcon-512 component as hex
    pub fn falcon_hex(&self) -> String { hex::encode(&self.falcon) }
    /// Total serialized size in bytes
    pub fn byte_len(&self) -> usize    { 2 + 64 + self.falcon.len() }
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn gen() -> WalletKeypair { WalletKeypair::generate() }

    #[test]
    fn generate_and_sign_verify() {
        let kp  = gen();
        let msg = b"hello TGBT-L1";
        let sig = kp.sign(msg);
        kp.public.verify(msg, &sig).expect("hybrid verify must pass");
    }

    #[test]
    fn tampered_message_fails() {
        let kp   = gen();
        let sig  = kp.sign(b"original");
        let err  = kp.public.verify(b"tampered", &sig);
        assert!(err.is_err(), "tampered message must not verify");
    }

    #[test]
    fn address_is_deterministic() {
        let kp = gen();
        assert_eq!(kp.address(), kp.address(), "address must be stable");
        assert_eq!(kp.address(), kp.public.address());
    }

    #[test]
    fn different_keypairs_different_addresses() {
        let a = gen();
        let b = gen();
        assert_ne!(a.address(), b.address());
    }

    #[test]
    fn pubkey_serialisation_roundtrip() {
        let kp     = gen();
        let bytes  = kp.public.to_bytes();
        assert_eq!(bytes.len(), 32 + FALCON_PK_BYTES);
        let pk2    = WalletPublicKey::from_bytes(&bytes).unwrap();
        assert_eq!(kp.public.address(), pk2.address());
    }

    #[test]
    fn keypair_secret_serialisation_roundtrip() {
        let kp    = gen();
        let msg   = b"roundtrip test";
        let sig   = kp.sign(msg);
        let bytes = kp.to_secret_bytes();
        assert_eq!(bytes.len(), 32 + FALCON_PK_BYTES + FALCON_SK_BYTES);
        let kp2   = WalletKeypair::from_secret_bytes(&bytes).unwrap();
        // Same address
        assert_eq!(kp.address(), kp2.address());
        // Signature from restored key also verifies
        let sig2 = kp2.sign(msg);
        kp.public.verify(msg, &sig2).unwrap();
        kp2.public.verify(msg, &sig).unwrap();
    }

    #[test]
    fn hybrid_sig_serialisation_roundtrip() {
        let kp    = gen();
        let msg   = b"sig roundtrip";
        let sig   = kp.sign(msg);
        let bytes = sig.to_bytes();
        let sig2  = HybridSignature::from_bytes(&bytes).unwrap();
        kp.public.verify(msg, &sig2).unwrap();
    }

    #[test]
    fn falcon_is_probabilistic_ed25519_is_deterministic() {
        // Ed25519 is deterministic: same key + msg → same sig every time
        // Falcon-512 is randomized: same key + msg → different sig each time (both valid)
        let kp  = gen();
        let msg = b"same message";
        let s1  = kp.sign(msg);
        let s2  = kp.sign(msg);
        assert_eq!(s1.ed25519, s2.ed25519, "Ed25519 must be deterministic");
        // Falcon sigs will almost certainly differ (probabilistic)
        // Both must still verify
        kp.public.verify(msg, &s1).unwrap();
        kp.public.verify(msg, &s2).unwrap();
    }

    #[test]
    fn wrong_key_fails_verification() {
        let kp1 = gen();
        let kp2 = gen();
        let sig = kp1.sign(b"message");
        let err = kp2.public.verify(b"message", &sig);
        assert!(err.is_err(), "signature from kp1 must not verify under kp2");
    }
}
