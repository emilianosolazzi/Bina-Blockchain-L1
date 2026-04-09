//! Private key generation and wallet address derivation for self-miners.

use anyhow::{Context, Result};
use k256::ecdsa::SigningKey;
use rand::rngs::OsRng;
use std::path::Path;

/// Generate a new secp256k1 private key, save to `path`, return the hex key
/// and derived Ethereum address.
pub fn generate_key(path: &Path) -> Result<(String, String)> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create key directory {}", parent.display()))?;
    }

    let signing_key = SigningKey::random(&mut OsRng);
    let key_bytes = signing_key.to_bytes();
    let key_hex = hex::encode(key_bytes);

    std::fs::write(path, &key_hex)
        .with_context(|| format!("Failed to write key to {}", path.display()))?;

    let address = derive_address(&signing_key);
    Ok((key_hex, address))
}

/// Derive an Ethereum address from a signing key.
fn derive_address(key: &SigningKey) -> String {
    use k256::ecdsa::VerifyingKey;
    let verifying_key = VerifyingKey::from(key);
    let public_key = verifying_key.to_encoded_point(false);
    // Skip the 0x04 prefix byte, hash the 64 raw bytes
    let pubkey_bytes = &public_key.as_bytes()[1..];
    let hash = keccak256(pubkey_bytes);
    // Address is last 20 bytes of the keccak256 hash
    format!("0x{}", hex::encode(&hash[12..]))
}

/// Keccak-256 hash (Ethereum uses keccak, not SHA-3).
fn keccak256(data: &[u8]) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let mut hasher = Keccak256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

/// Read an existing key file and derive the wallet address.
#[allow(dead_code)]
pub fn address_from_key_file(path: &Path) -> Result<String> {
    let key_hex = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read key from {}", path.display()))?;
    let key_hex = key_hex.trim();
    let key_bytes =
        hex::decode(key_hex).with_context(|| "Invalid hex in key file")?;
    // Support both 32-byte raw keys and 80-byte extended keys (first 32 bytes)
    let raw = if key_bytes.len() >= 32 {
        &key_bytes[..32]
    } else {
        anyhow::bail!("Key file too short — expected at least 32 bytes, got {}", key_bytes.len());
    };
    let signing_key =
        SigningKey::from_bytes(raw.into()).with_context(|| "Invalid secp256k1 private key")?;
    Ok(derive_address(&signing_key))
}
