use std::sync::atomic::{compiler_fence, Ordering}; // Keep compiler_fence for potential future use if needed, though zeroize handles barriers.
use tracing::{warn, debug};
use zeroize::Zeroize; // Import the Zeroize trait

// Conditional compilation for libc functions
#[cfg(unix)]
use libc;

#[derive(Debug, Zeroize)] // Add Zeroize derive for the struct itself (optional but good practice)
#[zeroize(drop)] // Ensure fields are zeroized on drop if SecureBuffer itself is dropped unexpectedly
pub struct SecureBuffer {
    // Make buffer private to enforce controlled access via methods
    buffer: Vec<u8>,
    #[zeroize(skip)] // Don't zeroize the lock_count itself
    lock_count: usize,
}

impl SecureBuffer {
    // Constructor to create a new SecureBuffer and attempt initial lock
    pub fn new(size: usize) -> Self {
        let mut sec_buf = SecureBuffer {
            buffer: vec![0u8; size], // Initialize with zeros (Vec does this)
            lock_count: 0,
        };
        // Attempt to lock memory upon creation
        sec_buf.lock_memory();
        sec_buf
    }

    // Get a mutable slice to the buffer data
    // Ensure buffer is unlocked before providing mutable access if locking implies exclusivity (mlock doesn't)
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // If locking mechanism required exclusive access, unlock here. mlock doesn't.
        &mut self.buffer
    }

    // Get an immutable slice to the buffer data
    pub fn as_slice(&self) -> &[u8] {
        &self.buffer
    }

    // Get the length of the buffer
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    // Securely clear the buffer contents using zeroize
    pub fn clean(&mut self) {
        debug!("Cleaning SecureBuffer of size {}", self.buffer.len());
        self.buffer.zeroize();
        // No explicit compiler_fence needed here, zeroize handles necessary barriers.
    }

    // Attempt to lock the buffer memory (Unix-specific)
    #[cfg(unix)]
    fn lock_memory(&mut self) {
        // Limit lock attempts to prevent resource exhaustion or repeated failures
        if self.lock_count < 3 && !self.buffer.is_empty() { // Added check for empty buffer
            // Safety: Calling libc::mlock requires a valid pointer and length.
            // self.buffer.as_mut_ptr() is valid, self.buffer.len() is correct.
            // Requires appropriate permissions (e.g., CAP_IPC_LOCK or root).
            let result = unsafe {
                libc::mlock(self.buffer.as_mut_ptr() as *const libc::c_void, self.buffer.len())
            };
            if result == 0 {
                debug!("Successfully locked SecureBuffer memory ({} bytes).", self.buffer.len());
                self.lock_count += 1;
            } else {
                // Log errno for debugging if mlock fails
                let errno = std::io::Error::last_os_error();
                warn!("Failed to lock SecureBuffer memory (attempt {}): {}", self.lock_count + 1, errno);
                // Increment count even on failure to prevent infinite retries if permissions are missing
                self.lock_count += 1;
            }
        } else if self.lock_count >= 3 {
             debug!("Maximum memory lock attempts reached for SecureBuffer.");
        }
    }

    // No-op implementation for non-Unix platforms
    #[cfg(not(unix))]
    fn lock_memory(&mut self) {
        if self.lock_count == 0 { // Log only once
            warn!("Memory locking (mlock) is not supported on this platform.");
            self.lock_count = 3; // Prevent further attempts/logs
        }
    }

    // Unlock memory (Unix-specific)
    #[cfg(unix)]
    fn unlock_memory(&mut self) {
        if self.lock_count > 0 && !self.buffer.is_empty() { // Added check for empty buffer
            // Safety: Calling libc::munlock requires a valid pointer and length previously locked.
            let result = unsafe {
                libc::munlock(self.buffer.as_mut_ptr() as *const libc::c_void, self.buffer.len())
            };
             if result == 0 {
                debug!("Successfully unlocked SecureBuffer memory ({} bytes).", self.buffer.len());
            } else {
                let errno = std::io::Error::last_os_error();
                warn!("Failed to unlock SecureBuffer memory: {}", errno);
            }
            // Reset lock count regardless of success/failure as the intent was to unlock
            self.lock_count = 0; // Reset count after attempting unlock
        }
    }

     // No-op implementation for non-Unix platforms
    #[cfg(not(unix))]
    fn unlock_memory(&mut self) {
        // No action needed if locking wasn't supported
        self.lock_count = 0;
    }
}

// Implement Drop to ensure memory is unlocked and cleaned on scope exit
impl Drop for SecureBuffer {
    fn drop(&mut self) {
        debug!("Dropping SecureBuffer, unlocking and cleaning memory.");
        // Unlock memory first (if locked and supported)
        self.unlock_memory();
        // Then clean the memory using the zeroize-based method
        self.clean();
        // The #[zeroize(drop)] on the struct handles the fields if needed,
        // but explicit cleaning of the buffer here is still good practice.
    }
}
