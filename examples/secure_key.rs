use super::memory::SecureBuffer;
use std::panic::{self, catch_unwind};
use zeroize::Zeroize;
use std::path::Path;
use anyhow::{Result, anyhow};
use log::{info, warn, error};

#[derive(Debug)]
enum KeyError {
    InvalidSize,
    LowEntropy,
    MemoryError,
    ProcessError,
    CleanupError,
    InvalidKeyData, // Add this new variant
}

fn validate_entropy(data: &[u8]) -> bool {
    let mut zeros = 0;
    let mut ones = 0;
    for byte in data {
        zeros += byte.count_zeros();
        ones += byte.count_ones();
    }
    // Require roughly balanced 0s and 1s
    (zeros as f64 / ones as f64).abs_diff(1.0) < 0.3
}

fn handle_private_key(key_data: &[u8]) -> Result<(), KeyError> {
    if key_data.len() != 32 {
        return Err(KeyError::InvalidSize);
    }

    if !validate_entropy(key_data) {
        return Err(KeyError::LowEntropy);
    }

    // Ensure cleanup even on panic
    let result = catch_unwind(|| {
        let mut key_buffer = SecureBuffer::new(32)
            .map_err(|_| KeyError::MemoryError)?;
        
        // Time-constant copy
        for (i, &byte) in key_data.iter().enumerate() {
            key_buffer.as_mut_slice()[i] = byte;
        }
        
        // Verify copy was successful (constant-time comparison)
        if !constant_time_eq(key_buffer.as_slice(), key_data) {
            return Err(KeyError::MemoryError);
        }

        // Process with panic safety
        process_key(key_buffer.as_slice())
            .map_err(|_| KeyError::ProcessError)?;

        // Explicit cleanup with verification
        key_buffer.clean();
        if !key_buffer.as_slice().iter().all(|&x| x == 0) {
            return Err(KeyError::CleanupError);
        }

        Ok(())
    });

    // Handle any panics
    match result {
        Ok(inner_result) => inner_result,
        Err(_) => {
            // Emergency cleanup on panic
            unsafe { libc::explicit_bzero(key_data.as_ptr() as *mut u8, key_data.len()) };
            Err(KeyError::ProcessError)
        }
    }
}

#[inline(never)]
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

/// Processes the key material
fn process_key(key: &[u8]) -> Result<(), KeyError> {
    // Actual key processing would go here
    // For now just validate it's not all zeros
    if key.iter().all(|&x| x == 0) {
        return Err(KeyError::InvalidKeyData);
    }
    Ok(())
}

/// Loads an existing key or generates a new one with security validation
pub fn load_or_generate_key_secure(key_path: &Path) -> Result<SecureBuffer> {
    let key_buffer = if key_path.exists() {
        info!("Loading existing mining key from: {}", key_path.display());
        // Load key from file
        let key_data = std::fs::read(key_path)
            .map_err(|e| anyhow!("Failed to read key file: {}", e))?;
        
        // Validate the loaded key with security checks
        let result = handle_private_key(&key_data);
        if result.is_err() {
            error!("Key failed integrity or security checks");
            return Err(anyhow!("Key handling failed"));
        }
        
        // Create secure buffer from validated key data
        let mut buffer = SecureBuffer::new(key_data.len())
            .map_err(|_| anyhow!("Failed to allocate secure memory for key"))?;
        buffer.as_mut_slice().copy_from_slice(&key_data);
        
        // Clean up original data
        let mut key_data = key_data;
        key_data.zeroize();
        
        buffer
    } else {
        info!("Generating new mining key at: {}", key_path.display());
        // Generate new key with entropy from system
        let mut buffer = generate_secure_key()?;
        
        // Validate the new key with security checks
        let result = handle_private_key(buffer.as_slice());
        if result.is_err() {
            error!("Generated key failed integrity or security checks");
            return Err(anyhow!("Key generation failed security validation"));
        }
        
        // Save key to file with restricted permissions
        std::fs::write(key_path, buffer.as_slice())
            .map_err(|e| anyhow!("Failed to write key file: {}", e))?;
        
        #[cfg(unix)]
        set_restrictive_permissions(key_path)?;
        
        buffer
    };
    
    Ok(key_buffer)
}

/// Generate a cryptographically secure key
fn generate_secure_key() -> Result<SecureBuffer> {
    let mut buffer = SecureBuffer::new(32)
        .map_err(|_| anyhow!("Failed to allocate secure memory"))?;
    
    // Fill with cryptographic randomness
    getrandom::getrandom(buffer.as_mut_slice())
        .map_err(|_| anyhow!("Failed to generate random bytes"))?;
    
    Ok(buffer)
}

/// Perform a temporary signing operation with security checks
pub fn temporary_signing_operation(key_buffer: &SecureBuffer, message: &[u8]) -> Result<Vec<u8>> {
    // Validate the key before use
    let result = handle_private_key(key_buffer.as_slice());
    if result.is_err() {
        error!("Key failed integrity or security checks during signing");
        return Err(anyhow!("Key handling failed during signing operation"));
    }
    
    // Perform the actual signing (example implementation)
    let signature = sign_message(key_buffer.as_slice(), message)?;
    
    Ok(signature)
}

/// Example signing function - replace with actual implementation
fn sign_message(key: &[u8], message: &[u8]) -> Result<Vec<u8>> {
    // This would be replaced with your actual signing implementation
    // For example, using libsecp256k1 or ed25519-dalek
    
    // Placeholder implementation
    let mut signature = Vec::with_capacity(64);
    for i in 0..32 {
        signature.push(key[i] ^ message[i % message.len()]);
        signature.push(key[(i+16) % 32] ^ message[(i+8) % message.len()]);
    }
    
    Ok(signature)
}

#[cfg(unix)]
fn set_restrictive_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(path)?;
    let mut perms = metadata.permissions();
    
    // Set permissions to 0600 (read/write for owner only)
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms)?;
    
    Ok(())
}
