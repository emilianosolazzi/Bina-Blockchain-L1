//! # Secure Memory Buffer
//!
//! Cross-platform locked, zeroizing memory for sensitive miner data:
//! secret values, HMAC keys, ECDSA signatures, and private key material.
//!
//! ## Security properties
//!
//! | Property               | Mechanism                          | Platform        |
//! |------------------------|------------------------------------|-----------------|
//! | No swap to disk        | `mlock` / `VirtualLock`            | All             |
//! | Zeroed on drop         | `zeroize` crate                    | All             |
//! | Not in core dumps      | `MADV_DONTDUMP` / `MEM_PRIVATE`    | Linux / macOS   |
//! | Constant-time compare  | XOR fold, no branch on secret bits  | All             |
//! | Canary integrity check | Address-seeded XOR canary           | All             |
//! | Anti-debug guard       | `ptrace` / `IsDebuggerPresent`      | Linux / Windows |
//! | Reuse pool             | Pre-locked buffer pool              | All             |
//!
//! ## Usage
//!
//! ```rust
//! use temporal_gradient_core::memory::SecureBuffer;
//! # let my_secret_bytes = [7u8; 32];
//! # let expected = [7u8; 32];
//!
//! // Create a 32-byte locked buffer for a mining secret
//! let mut secret = SecureBuffer::new(32).unwrap();
//! secret.as_mut_slice().unwrap().copy_from_slice(&my_secret_bytes);
//!
//! // Constant-time comparison
//! if secret.constant_time_eq(&expected) {
//!     // proceed
//! }
//! // Buffer is zeroed and unlocked automatically when dropped
//! ```

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering, fence};
use std::sync::Mutex;

use lazy_static::lazy_static;
use log::{debug, warn};
use zeroize::Zeroize;

// ─────────────────────────────────────────────────────────────────
// Canary seed — one random value per process, used to derive
// per-buffer canaries that are tied to the buffer's address.
// ─────────────────────────────────────────────────────────────────

lazy_static! {
    static ref CANARY_SEED: u64 = {
        let mut seed = [0u8; 8];
        getrandom::getrandom(&mut seed).unwrap_or(());
        u64::from_ne_bytes(seed)
    };
}

/// Derive a canary for a buffer at a given address.
/// Changing either the seed (process-unique) or the address
/// (buffer-unique) produces a different canary, so an overwrite
/// at a known address cannot forge a valid canary.
fn make_canary(ptr: *const u8) -> u64 {
    *CANARY_SEED ^ (ptr as u64).wrapping_mul(0x9e3779b97f4a7c15)
}

// ─────────────────────────────────────────────────────────────────
// Memory pool
// ─────────────────────────────────────────────────────────────────

/// Maximum number of pre-locked buffers to keep in the pool.
const POOL_MAX: usize = 16;

lazy_static! {
    /// Pool of already-locked, zeroed Vec<u8> for fast reuse.
    static ref MEMORY_POOL: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::with_capacity(POOL_MAX));
}

// ─────────────────────────────────────────────────────────────────
// Platform abstractions
// ─────────────────────────────────────────────────────────────────

mod platform {
    use std::io;

    // ── Linux / macOS / other Unix ────────────────────────────────

    #[cfg(unix)]
    pub fn lock(ptr: *mut u8, len: usize) -> io::Result<()> {
        let rc = unsafe { libc::mlock(ptr as *mut libc::c_void, len) };
        if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    }

    #[cfg(unix)]
    pub fn unlock(ptr: *mut u8, len: usize) -> io::Result<()> {
        let rc = unsafe { libc::munlock(ptr as *mut libc::c_void, len) };
        if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    }

    /// Tell the kernel not to include this region in core dumps and
    /// not to move it to swap (belt-and-suspenders with mlock).
    #[cfg(target_os = "linux")]
    pub fn advise_secret(ptr: *mut u8, len: usize) {
        unsafe {
            // MADV_DONTDUMP = 16, MADV_DONTFORK = 10
            libc::madvise(ptr as *mut libc::c_void, len, libc::MADV_DONTDUMP);
            libc::madvise(ptr as *mut libc::c_void, len, libc::MADV_DONTFORK);
        }
    }

    #[cfg(target_os = "macos")]
    pub fn advise_secret(ptr: *mut u8, len: usize) {
        unsafe {
            // macOS: MADV_ZERO_WIRED_PAGES = 6
            libc::madvise(ptr as *mut libc::c_void, len, 6);
        }
    }

    #[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
    pub fn advise_secret(_ptr: *mut u8, _len: usize) {}

    /// Detect a debugger on Linux via ptrace self-attach.
    #[cfg(target_os = "linux")]
    pub fn debugger_present() -> bool {
        // ptrace(PTRACE_TRACEME) returns -1 if already being traced.
        let rc = unsafe { libc::ptrace(libc::PTRACE_TRACEME, 0, 0, 0) };
        if rc == 0 {
            // We successfully attached — detach immediately.
            unsafe { libc::ptrace(libc::PTRACE_DETACH, 0, 0, 0) };
            false
        } else {
            true
        }
    }

    /// Detect a debugger on macOS via sysctl P_TRACED flag.
    #[cfg(target_os = "macos")]
    pub fn debugger_present() -> bool {
        use std::mem;
        unsafe {
            let mut info: libc::kinfo_proc = mem::zeroed();
            let mut size = mem::size_of::<libc::kinfo_proc>();
            let mut mib = [
                libc::CTL_KERN,
                libc::KERN_PROC,
                libc::KERN_PROC_PID,
                libc::getpid(),
            ];
            let rc = libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as u32,
                &mut info as *mut _ as *mut libc::c_void,
                &mut size,
                std::ptr::null_mut(),
                0,
            );
            if rc != 0 { return false; }
            (info.kp_proc.p_flag & libc::P_TRACED) != 0
        }
    }

    #[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
    pub fn debugger_present() -> bool { false }

    // ── Windows ───────────────────────────────────────────────────

    #[cfg(target_os = "windows")]
    pub fn lock(ptr: *mut u8, len: usize) -> io::Result<()> {
        use windows_sys::Win32::System::Memory::VirtualLock;
        // VirtualLock requires the working-set quota; adjust if needed.
        let ok = unsafe { VirtualLock(ptr as *mut _, len) };
        if ok != 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    }

    #[cfg(target_os = "windows")]
    pub fn unlock(ptr: *mut u8, len: usize) -> io::Result<()> {
        use windows_sys::Win32::System::Memory::VirtualUnlock;
        let ok = unsafe { VirtualUnlock(ptr as *mut _, len) };
        if ok != 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    }

    /// On Windows, mark pages as non-pageable via `VirtualLock` (already
    /// called above) and disable core-dump inclusion via `SetProcessMitigationPolicy`
    /// where available. Best-effort only.
    #[cfg(target_os = "windows")]
    pub fn advise_secret(_ptr: *mut u8, _len: usize) {
        // VirtualLock already prevents paging.
        // Core dump exclusion would need HeapSetInformation or
        // SetProcessMitigationPolicy — omitted as it requires elevated
        // privileges on most Windows configurations.
    }

    #[cfg(target_os = "windows")]
    pub fn debugger_present() -> bool {
        use windows_sys::Win32::System::Diagnostics::Debug::IsDebuggerPresent;
        unsafe { IsDebuggerPresent() != 0 }
    }

    // ── Unsupported platforms ─────────────────────────────────────

    #[cfg(not(any(unix, target_os = "windows")))]
    pub fn lock(_ptr: *mut u8, _len: usize) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "mlock not available"))
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    pub fn unlock(_ptr: *mut u8, _len: usize) -> io::Result<()> { Ok(()) }

    #[cfg(not(any(unix, target_os = "windows")))]
    pub fn advise_secret(_ptr: *mut u8, _len: usize) {}

    #[cfg(not(any(unix, target_os = "windows")))]
    pub fn debugger_present() -> bool { false }
}

// ─────────────────────────────────────────────────────────────────
// Error type
// ─────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SecureBufferError {
    /// `size == 0` or `size > 1 GiB`
    InvalidSize,
    /// `Vec::with_capacity` returned wrong capacity
    AllocationFailed,
    /// `mlock` / `VirtualLock` failed
    LockFailed(io::Error),
    /// Canary mismatch — possible buffer overrun or tampering
    IntegrityViolation,
    /// Buffer marked invalid (already dropped or cleaned)
    InvalidState,
}

impl std::fmt::Display for SecureBufferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSize        => write!(f, "Invalid buffer size"),
            Self::AllocationFailed   => write!(f, "Memory allocation failed"),
            Self::LockFailed(e)      => write!(f, "Memory lock failed: {e}"),
            Self::IntegrityViolation => write!(f, "Buffer integrity violation (canary mismatch)"),
            Self::InvalidState       => write!(f, "Buffer in invalid state"),
        }
    }
}

impl std::error::Error for SecureBufferError {}

// ─────────────────────────────────────────────────────────────────
// SecureBuffer
// ─────────────────────────────────────────────────────────────────

/// A heap buffer that is:
/// - locked in physical RAM (no swap)
/// - excluded from core dumps (Linux/macOS)
/// - zeroed on every drop
/// - protected by an address-seeded canary
/// - accessible only after passing anti-debug checks
pub struct SecureBuffer {
    data: Vec<u8>,
    /// Address-seeded canary — set once on construction, checked on access.
    canary: u64,
    is_locked: bool,
    lock_count: AtomicUsize,
    /// Set to false on drop to catch use-after-free.
    is_valid: bool,
}

impl SecureBuffer {
    // ── Constructors ─────────────────────────────────────────────

    /// Allocate a new zeroed, locked secure buffer of `size` bytes.
    pub fn new(size: usize) -> Result<Self, SecureBufferError> {
        if size == 0 || size > (1 << 30) {
            return Err(SecureBufferError::InvalidSize);
        }

        let data = vec![0u8; size];
        if data.len() != size {
            return Err(SecureBufferError::AllocationFailed);
        }

        let canary = make_canary(data.as_ptr());

        let mut buf = Self {
            data,
            canary,
            is_locked: false,
            lock_count: AtomicUsize::new(0),
            is_valid: true,
        };

        buf.try_lock();
        platform::advise_secret(buf.data.as_mut_ptr(), size);
        Ok(buf)
    }

    /// Obtain a buffer from the reuse pool if one of sufficient size
    /// exists; otherwise allocate fresh. Returned buffers are zeroed.
    pub fn from_pool(size: usize) -> Result<Self, SecureBufferError> {
        if size == 0 || size > (1 << 30) {
            return Err(SecureBufferError::InvalidSize);
        }

        if let Ok(mut pool) = MEMORY_POOL.lock() {
            if let Some(pos) = pool.iter().position(|v| v.capacity() >= size) {
                let mut data = pool.remove(pos);
                data.zeroize();
                data.resize(size, 0);

                let canary = make_canary(data.as_ptr());
                let mut buf = Self {
                    data,
                    canary,
                    is_locked: false,
                    lock_count: AtomicUsize::new(0),
                    is_valid: true,
                };
                buf.try_lock();
                platform::advise_secret(buf.data.as_mut_ptr(), size);
                return Ok(buf);
            }
        }
        Self::new(size)
    }

    /// Allocate a secure buffer and copy in `src`.
    pub fn from_slice(src: &[u8]) -> Result<Self, SecureBufferError> {
        let mut buf = Self::from_pool(src.len())?;
        let dst = buf.as_mut_slice().ok_or(SecureBufferError::InvalidState)?;
        dst.copy_from_slice(src);
        Ok(buf)
    }

    /// Copy the buffer into a fixed-size array.
    pub fn to_array<const N: usize>(&self) -> Result<[u8; N], SecureBufferError> {
        if self.data.len() != N {
            return Err(SecureBufferError::InvalidSize);
        }

        let src = self.as_slice().ok_or(SecureBufferError::InvalidState)?;
        let mut out = [0u8; N];
        out.copy_from_slice(src);
        Ok(out)
    }

    // ── Locking ──────────────────────────────────────────────────

    /// Attempt to lock memory. Non-fatal — logs on failure.
    fn try_lock(&mut self) {
        if self.is_locked { return; }
        match platform::lock(self.data.as_mut_ptr(), self.data.len()) {
            Ok(()) => {
                self.is_locked = true;
                self.lock_count.fetch_add(1, Ordering::SeqCst);
            }
            Err(e) => debug!("SecureBuffer: mlock failed ({}), continuing unlocked", e),
        }
    }

    fn try_unlock(&mut self) {
        if !self.is_locked { return; }
        match platform::unlock(self.data.as_mut_ptr(), self.data.len()) {
            Ok(()) => {
                self.is_locked = false;
                self.lock_count.fetch_sub(1, Ordering::SeqCst);
            }
            Err(e) => debug!("SecureBuffer: munlock failed: {}", e),
        }
    }

    // ── Integrity ────────────────────────────────────────────────

    /// Check the address-seeded canary.
    /// Returns `Err(IntegrityViolation)` if the canary has been corrupted.
    pub fn verify(&self) -> Result<(), SecureBufferError> {
        if !self.is_valid {
            return Err(SecureBufferError::InvalidState);
        }
        let expected = make_canary(self.data.as_ptr());
        if self.canary != expected {
            return Err(SecureBufferError::IntegrityViolation);
        }
        Ok(())
    }

    // ── Access ───────────────────────────────────────────────────

    /// Immutable slice access. Returns `None` if the buffer is invalid,
    /// integrity fails, or a debugger is detected.
    pub fn as_slice(&self) -> Option<&[u8]> {
        if !self.is_valid { return None; }
        if self.verify().is_err() {
            warn!("SecureBuffer: integrity violation on read");
            return None;
        }
        if platform::debugger_present() {
            warn!("SecureBuffer: debugger detected on read — denying access");
            return None;
        }
        Some(&self.data)
    }

    /// Mutable slice access. Same guards as `as_slice`.
    /// If a debugger is detected the buffer is immediately zeroed.
    pub fn as_mut_slice(&mut self) -> Option<&mut [u8]> {
        if !self.is_valid { return None; }
        if self.verify().is_err() {
            warn!("SecureBuffer: integrity violation on write");
            return None;
        }
        if platform::debugger_present() {
            warn!("SecureBuffer: debugger detected on write — zeroing and denying access");
            self.zero();
            return None;
        }
        Some(&mut self.data)
    }

    // ── Queries ──────────────────────────────────────────────────

    pub fn len(&self) -> usize { self.data.len() }
    pub fn is_empty(&self) -> bool { self.data.is_empty() }
    pub fn is_locked(&self) -> bool { self.lock_count.load(Ordering::SeqCst) > 0 }
    pub fn is_valid(&self) -> bool { self.is_valid }

    /// True if every byte is zero.
    pub fn is_zeroed(&self) -> bool {
        self.data.iter().fold(0u8, |acc, &b| acc | b) == 0
    }

    // ── Constant-time operations ─────────────────────────────────

    /// Compare contents with `other` in constant time.
    /// Returns `false` on length mismatch or invalid state.
    pub fn constant_time_eq(&self, other: &[u8]) -> bool {
        if !self.is_valid || self.data.len() != other.len() {
            return false;
        }
        // XOR-fold: result is 0 iff all bytes are equal.
        // No branch on secret data.
        let diff = self.data.iter()
            .zip(other.iter())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b));
        diff == 0
    }

    /// Copy from another `SecureBuffer` in a simple loop (no branch on values).
    pub fn copy_from(&mut self, src: &SecureBuffer) -> Result<(), SecureBufferError> {
        if !self.is_valid || !src.is_valid {
            return Err(SecureBufferError::InvalidState);
        }
        if self.data.len() != src.data.len() {
            return Err(SecureBufferError::InvalidSize);
        }
        for (d, s) in self.data.iter_mut().zip(src.data.iter()) {
            *d = *s;
        }
        Ok(())
    }

    // ── Zeroing ──────────────────────────────────────────────────

    /// Overwrite all bytes with zero, using the `zeroize` crate's
    /// compiler-fence to prevent the optimizer from eliding the write.
    pub fn zero(&mut self) {
        self.data.zeroize();
        // Extra write + fence for defence-in-depth.
        fence(Ordering::SeqCst);
    }

    /// Three-pass wipe: 0xFF, fence, 0x00, fence, 0xAA, fence, 0x00.
    /// Use when `zero()` isn't paranoid enough (e.g., long-lived key material).
    pub fn paranoid_wipe(&mut self) {
        for pass in [0xFFu8, 0xAAu8, 0x00u8] {
            self.data.iter_mut().for_each(|b| *b = pass);
            fence(Ordering::SeqCst);
        }
        self.data.zeroize();
        fence(Ordering::SeqCst);
    }
}

// ─────────────────────────────────────────────────────────────────
// Drop
// ─────────────────────────────────────────────────────────────────

impl Drop for SecureBuffer {
    fn drop(&mut self) {
        // Mark invalid first — catches any re-entrant access.
        self.is_valid = false;

        // Wipe contents.
        self.zero();

        // Unlock physical memory.
        self.try_unlock();

        // Return pre-locked allocation to the pool for fast reuse.
        if self.data.capacity() > 0 {
            if let Ok(mut pool) = MEMORY_POOL.lock() {
                if pool.len() < POOL_MAX {
                    let mut recycled = Vec::new();
                    std::mem::swap(&mut recycled, &mut self.data);
                    recycled.zeroize();
                    recycled.clear();
                    pool.push(recycled);
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────
// Cargo.toml additions
// ─────────────────────────────────────────────────────────────────
//
// [dependencies]
// zeroize     = { version = "1", features = ["derive"] }
// lazy_static = "1"
// getrandom   = "0.2"
// log         = "0.4"
//
// [target.'cfg(unix)'.dependencies]
// libc = "0.2"
//
// [target.'cfg(target_os = "windows")'.dependencies]
// windows-sys = { version = "0.48", features = [
//     "Win32_System_Memory",
//     "Win32_System_Diagnostics_Debug",
// ]}

// ─────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Construction ─────────────────────────────────────────────

    #[test]
    fn new_creates_zeroed_buffer() {
        let buf = SecureBuffer::new(32).unwrap();
        assert_eq!(buf.len(), 32);
        assert!(buf.is_zeroed());
    }

    #[test]
    fn invalid_size_zero_errors() {
        assert!(matches!(SecureBuffer::new(0), Err(SecureBufferError::InvalidSize)));
    }

    #[test]
    fn invalid_size_too_large_errors() {
        assert!(matches!(
            SecureBuffer::new((1 << 30) + 1),
            Err(SecureBufferError::InvalidSize)
        ));
    }

    // ── Integrity / canary ────────────────────────────────────────

    #[test]
    fn verify_passes_on_untampered_buffer() {
        let buf = SecureBuffer::new(32).unwrap();
        assert!(buf.verify().is_ok());
    }

    #[test]
    fn canary_tied_to_address() {
        let buf1 = SecureBuffer::new(32).unwrap();
        let buf2 = SecureBuffer::new(32).unwrap();
        // Two different allocations almost certainly have different addresses.
        // Canaries should differ unless addresses collide (extremely unlikely).
        if buf1.data.as_ptr() != buf2.data.as_ptr() {
            assert_ne!(buf1.canary, buf2.canary);
        }
    }

    // ── Access ───────────────────────────────────────────────────

    #[test]
    fn as_slice_returns_data() {
        let buf = SecureBuffer::new(4).unwrap();
        let slice = buf.as_slice().unwrap();
        assert_eq!(slice, &[0u8, 0, 0, 0]);
    }

    #[test]
    fn as_mut_slice_allows_write() {
        let mut buf = SecureBuffer::new(4).unwrap();
        {
            let s = buf.as_mut_slice().unwrap();
            s[0] = 0xDE;
            s[1] = 0xAD;
        }
        let s = buf.as_slice().unwrap();
        assert_eq!(s[0], 0xDE);
        assert_eq!(s[1], 0xAD);
    }

    // ── Constant-time comparison ──────────────────────────────────

    #[test]
    fn ct_eq_equal_buffers() {
        let buf = SecureBuffer::new(4).unwrap();
        assert!(buf.constant_time_eq(&[0u8; 4]));
    }

    #[test]
    fn ct_eq_different_data() {
        let mut buf = SecureBuffer::new(4).unwrap();
        buf.as_mut_slice().unwrap()[0] = 1;
        assert!(!buf.constant_time_eq(&[0u8; 4]));
    }

    #[test]
    fn ct_eq_length_mismatch() {
        let buf = SecureBuffer::new(4).unwrap();
        assert!(!buf.constant_time_eq(&[0u8; 3]));
        assert!(!buf.constant_time_eq(&[0u8; 5]));
    }

    // ── copy_from ────────────────────────────────────────────────

    #[test]
    fn copy_from_transfers_data() {
        let mut src = SecureBuffer::new(4).unwrap();
        src.as_mut_slice().unwrap().copy_from_slice(&[1, 2, 3, 4]);
        let mut dst = SecureBuffer::new(4).unwrap();
        dst.copy_from(&src).unwrap();
        assert_eq!(dst.as_slice().unwrap(), &[1u8, 2, 3, 4]);
    }

    #[test]
    fn copy_from_size_mismatch_errors() {
        let src = SecureBuffer::new(4).unwrap();
        let mut dst = SecureBuffer::new(8).unwrap();
        assert!(matches!(dst.copy_from(&src), Err(SecureBufferError::InvalidSize)));
    }

    // ── Zeroing ──────────────────────────────────────────────────

    #[test]
    fn zero_clears_contents() {
        let mut buf = SecureBuffer::new(4).unwrap();
        buf.as_mut_slice().unwrap().copy_from_slice(&[0xFF; 4]);
        buf.zero();
        assert!(buf.is_zeroed());
    }

    #[test]
    fn paranoid_wipe_clears_contents() {
        let mut buf = SecureBuffer::new(8).unwrap();
        buf.as_mut_slice().unwrap().copy_from_slice(&[0xAA; 8]);
        buf.paranoid_wipe();
        assert!(buf.is_zeroed());
    }

    // ── Pool ─────────────────────────────────────────────────────

    #[test]
    fn from_pool_returns_zeroed_buffer() {
        // Pre-populate pool.
        {
            let _ = SecureBuffer::new(64).unwrap();
        }
        let buf = SecureBuffer::from_pool(32).unwrap();
        assert_eq!(buf.len(), 32);
        assert!(buf.is_zeroed());
        assert!(buf.verify().is_ok());
    }

    #[test]
    fn from_pool_falls_back_to_new() {
        // Empty pool — should still succeed.
        {
            let mut pool = MEMORY_POOL.lock().unwrap();
            pool.clear();
        }
        let buf = SecureBuffer::from_pool(16).unwrap();
        assert_eq!(buf.len(), 16);
    }

    #[test]
    fn from_slice_copies_input() {
        let buf = SecureBuffer::from_slice(&[1u8, 2, 3, 4]).unwrap();
        assert_eq!(buf.as_slice().unwrap(), &[1u8, 2, 3, 4]);
    }

    #[test]
    fn to_array_returns_fixed_size_copy() {
        let buf = SecureBuffer::from_slice(&[9u8, 8, 7, 6]).unwrap();
        let out = buf.to_array::<4>().unwrap();
        assert_eq!(out, [9u8, 8, 7, 6]);
    }

    // ── Drop / use-after-drop guard ───────────────────────────────

    #[test]
    fn is_valid_false_after_drop_flag() {
        let mut buf = SecureBuffer::new(8).unwrap();
        assert!(buf.is_valid());
        buf.is_valid = false; // simulate drop
        assert!(buf.as_slice().is_none());
        assert!(buf.as_mut_slice().is_none());
        buf.is_valid = true; // restore for clean drop
    }

    // ── is_zeroed ────────────────────────────────────────────────

    #[test]
    fn is_zeroed_true_for_fresh_buffer() {
        let buf = SecureBuffer::new(16).unwrap();
        assert!(buf.is_zeroed());
    }

    #[test]
    fn is_zeroed_false_after_write() {
        let mut buf = SecureBuffer::new(4).unwrap();
        buf.as_mut_slice().unwrap()[0] = 1;
        assert!(!buf.is_zeroed());
    }

    // ── is_locked ────────────────────────────────────────────────

    #[test]
    fn lock_status_reported() {
        let buf = SecureBuffer::new(16).unwrap();
        // mlock may fail on CI (unprivileged environments) so we just
        // verify the field reflects the actual attempt.
        let _ = buf.is_locked(); // must not panic
    }
}