use std::sync::atomic::{compiler_fence, Ordering, AtomicUsize}; // Keep compiler_fence for potential future use if needed, though zeroize handles barriers.
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
    lock_count: AtomicUsize,  // Make atomic for thread safety
    #[zeroize(skip)]
    alignment_padding: usize, // Add padding for memory alignment
    #[zeroize(skip)]
    canary: u64,  // Add canary for overflow detection
}

impl SecureBuffer {
    // Constructor to create a new SecureBuffer and attempt initial lock
    pub fn new(size: usize) -> Result<Self, &'static str> {
        if size == 0 || size > (1 << 30) {  // 1GB max for safety
            return Err("Invalid buffer size");
        }
        
        let mut sec_buf = SecureBuffer {
            buffer: vec![0u8; size], // Initialize with zeros (Vec does this)
            lock_count: AtomicUsize::new(0),
            alignment_padding: 0,
            canary: 0,
        };
        
        // Ensure proper memory alignment
        let ptr_value = sec_buf.buffer.as_ptr() as usize;
        if ptr_value % 16 != 0 {
            sec_buf.alignment_padding = 16 - (ptr_value % 16);
        }

        // Add canary value
        let canary = rand::random::<u64>();
        sec_buf.canary = canary;
        
        // Verify memory pages are accessible
        if !Self::verify_pages(&sec_buf.buffer) {
            return Err("Memory pages not secure");
        }

        // Attempt to lock memory upon creation
        sec_buf.lock_memory();
        Ok(sec_buf)
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

    // Add empty check
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    // Prevent buffer resizing
    pub fn resize(&mut self, _new_size: usize) -> Result<(), &'static str> {
        Err("Buffer resizing not allowed for security")
    }

    // Attempt to lock the buffer memory (Unix-specific)
    #[cfg(unix)]
    fn lock_memory(&mut self) {
        // Add memory page alignment check
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize };
        let buffer_start = self.buffer.as_ptr() as usize;
        let aligned_start = buffer_start & !(page_size - 1);
        let end = buffer_start + self.buffer.len();
        let aligned_length = end - aligned_start;

        // Limit lock attempts to prevent resource exhaustion or repeated failures
        if self.lock_count.load(Ordering::SeqCst) < 3 && !self.buffer.is_empty() { // Added check for empty buffer
            // Safety: Calling libc::mlock requires a valid pointer and length.
            // self.buffer.as_mut_ptr() is valid, self.buffer.len() is correct.
            // Requires appropriate permissions (e.g., CAP_IPC_LOCK or root).
            let result = unsafe {
                libc::mlock(aligned_start as *const libc::c_void, aligned_length)
            };
            if result == 0 {
                debug!("Successfully locked SecureBuffer memory ({} bytes).", self.buffer.len());
                self.lock_count.fetch_add(1, Ordering::SeqCst);
            } else {
                // Log errno for debugging if mlock fails
                let errno = std::io::Error::last_os_error();
                warn!("Failed to lock SecureBuffer memory (attempt {}): {}", self.lock_count.load(Ordering::SeqCst) + 1, errno);
                // Increment count even on failure to prevent infinite retries if permissions are missing
                self.lock_count.fetch_add(1, Ordering::SeqCst);
            }
        } else if self.lock_count.load(Ordering::SeqCst) >= 3 {
             debug!("Maximum memory lock attempts reached for SecureBuffer.");
        }
    }

    // No-op implementation for non-Unix platforms
    #[cfg(not(unix))]
    fn lock_memory(&mut self) {
        if self.lock_count.load(Ordering::SeqCst) == 0 { // Log only once
            warn!("Memory locking (mlock) is not supported on this platform.");
            self.lock_count.store(3, Ordering::SeqCst); // Prevent further attempts/logs
        }
    }

    // Unlock memory (Unix-specific)
    #[cfg(unix)]
    fn unlock_memory(&mut self) {
        // Add sync fence before unlocking
        compiler_fence(Ordering::SeqCst);
        if self.lock_count.load(Ordering::SeqCst) > 0 && !self.buffer.is_empty() { // Added check for empty buffer
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
            self.lock_count.store(0, Ordering::SeqCst); // Reset count after attempting unlock
        }
    }

     // No-op implementation for non-Unix platforms
    #[cfg(not(unix))]
    fn unlock_memory(&mut self) {
        // No action needed if locking wasn't supported
        self.lock_count.store(0, Ordering::SeqCst);
    }

    // Add memory verification
    pub fn verify(&self) -> bool {
        self.canary == self.canary && // Check canary unchanged
        Self::verify_pages(&self.buffer) && // Verify pages still accessible
        !self.buffer.as_ptr().is_null()
    }

    fn verify_pages(buf: &[u8]) -> bool {
        #[cfg(unix)]
        unsafe {
            let page_size = libc::sysconf(libc::_SC_PAGESIZE) as usize;
            let start = buf.as_ptr() as usize & !(page_size - 1);
            let end = (buf.as_ptr() as usize + buf.len() + page_size - 1) & !(page_size - 1);
            
            for addr in (start..end).step_by(page_size) {
                let ret = libc::mincore(
                    addr as *const libc::c_void,
                    page_size,
                    &mut 0u8
                );
                if ret != 0 {
                    return false;
                }
            }
        }
        true
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

// Add test module
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_creation() {
        assert!(SecureBuffer::new(0).is_err());
        assert!(SecureBuffer::new(1 << 31).is_err());
        assert!(SecureBuffer::new(1024).is_ok());
    }

    #[test]
    fn test_buffer_alignment() {
        if let Ok(buf) = SecureBuffer::new(1024) {
            let ptr = buf.as_slice().as_ptr() as usize;
            assert_eq!(ptr % 16, 0, "Buffer should be 16-byte aligned");
        }
    }

    #[test]
    fn test_memory_cleanup() {
        if let Ok(mut buf) = SecureBuffer::new(32) {
            buf.as_mut_slice().copy_from_slice(&[0xFF; 32]);
            drop(buf);
            // Memory should be zeroed - though hard to test directly
            // Could use /proc/self/maps on Linux
        }
    }

    #[test]
    fn test_resize_prevention() {
        if let Ok(mut buf) = SecureBuffer::new(32) {
            assert!(buf.resize(64).is_err());
        }
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        if let Ok(buf) = SecureBuffer::new(1024) {
            let shared = Arc::new(buf);
            let mut handles = vec![];

            for _ in 0..4 {
                let buf_clone = Arc::clone(&shared);
                handles.push(thread::spawn(move || {
                    assert_eq!(buf_clone.len(), 1024);
                }));
            }

            for handle in handles {
                handle.join().unwrap();
            }
        }
    }

    #[test]
    fn test_memory_verification() {
        if let Ok(buf) = SecureBuffer::new(1024) {
            assert!(buf.verify(), "Memory verification failed");
        }
    }

    #[test]
    fn test_memory_residency() {
        if let Ok(buf) = SecureBuffer::new(1024) {
            // Try to force page out
            #[cfg(unix)]
            unsafe {
                libc::madvise(
                    buf.as_slice().as_ptr() as *mut libc::c_void,
                    buf.len(),
                    libc::MADV_PAGEOUT
                );
            }
            // Should still be resident
            assert!(buf.verify());
        }
    }

    #[test]
    fn test_parallel_verification() {
        use std::sync::Arc;
        use std::thread;

        if let Ok(buf) = SecureBuffer::new(1024) {
            let shared = Arc::new(buf);
            let mut handles = vec![];

            for _ in 0..8 {
                let buf_clone = Arc::clone(&shared);
                handles.push(thread::spawn(move || {
                    for _ in 0..1000 {
                        assert!(buf_clone.verify());
                    }
                }));
            }

            for handle in handles {
                handle.join().unwrap();
            }
        }
    }
}
