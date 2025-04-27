use super::memory::SecureBuffer;
use std::panic::{self, catch_unwind};
use zeroize::Zeroize;

#[derive(Debug)]
enum KeyError {
    InvalidSize,
    LowEntropy,
    MemoryError,
    ProcessError,
    CleanupError,
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
