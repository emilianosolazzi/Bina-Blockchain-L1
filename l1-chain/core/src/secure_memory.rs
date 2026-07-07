//! BINA secure-memory helpers for short-lived wallet secret material.
//!
//! This module is intentionally smaller than the L2 miner's memory hardening:
//! it focuses on BINA wallet boundaries, where secret bytes are decoded from
//! disk before being handed to the Ed25519/Falcon key types.

use std::fmt;
use std::sync::atomic::{fence, Ordering};
use std::sync::OnceLock;

use zeroize::Zeroize;

const GUARD_BYTE: u8 = 0xFD;
const GUARD_SIZE: usize = 16;
const MAX_SECURE_BYTES: usize = 1 << 20;

static CANARY_SEED: OnceLock<u64> = OnceLock::new();

fn canary_seed() -> u64 {
    *CANARY_SEED.get_or_init(|| {
        let mut seed = [0u8; 8];
        let _ = getrandom::getrandom(&mut seed);
        u64::from_ne_bytes(seed)
    })
}

fn make_canary(ptr: *const u8) -> u64 {
    canary_seed() ^ (ptr as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

mod platform {
    use std::io;

    #[cfg(unix)]
    pub fn lock(ptr: *mut u8, len: usize) -> io::Result<()> {
        let rc = unsafe { libc::mlock(ptr as *mut libc::c_void, len) };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(unix)]
    pub fn unlock(ptr: *mut u8, len: usize) -> io::Result<()> {
        let rc = unsafe { libc::munlock(ptr as *mut libc::c_void, len) };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(target_os = "linux")]
    pub fn advise_secret(ptr: *mut u8, len: usize) {
        unsafe {
            let _ = libc::madvise(ptr as *mut libc::c_void, len, libc::MADV_DONTDUMP);
            let _ = libc::madvise(ptr as *mut libc::c_void, len, libc::MADV_DONTFORK);
        }
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    pub fn advise_secret(_ptr: *mut u8, _len: usize) {}

    #[cfg(target_os = "windows")]
    pub fn lock(ptr: *mut u8, len: usize) -> io::Result<()> {
        use std::ffi::c_void;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn VirtualLock(lpAddress: *const c_void, dwSize: usize) -> i32;
        }
        let ok = unsafe { VirtualLock(ptr as *const c_void, len) };
        if ok != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(target_os = "windows")]
    pub fn unlock(ptr: *mut u8, len: usize) -> io::Result<()> {
        use std::ffi::c_void;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn VirtualUnlock(lpAddress: *const c_void, dwSize: usize) -> i32;
        }
        let ok = unsafe { VirtualUnlock(ptr as *const c_void, len) };
        if ok != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(target_os = "windows")]
    pub fn advise_secret(_ptr: *mut u8, _len: usize) {}

    #[cfg(not(any(unix, target_os = "windows")))]
    pub fn lock(_ptr: *mut u8, _len: usize) -> io::Result<()> {
        Ok(())
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    pub fn unlock(_ptr: *mut u8, _len: usize) -> io::Result<()> {
        Ok(())
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    pub fn advise_secret(_ptr: *mut u8, _len: usize) {}
}

#[derive(Debug)]
pub enum SecureMemoryError {
    InvalidSize,
    IntegrityViolation,
    InvalidState,
    HexDecode(hex::FromHexError),
}

impl fmt::Display for SecureMemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSize => write!(f, "invalid secure buffer size"),
            Self::IntegrityViolation => write!(f, "secure buffer integrity violation"),
            Self::InvalidState => write!(f, "secure buffer is invalid"),
            Self::HexDecode(err) => write!(f, "secret hex decode failed: {err}"),
        }
    }
}

impl std::error::Error for SecureMemoryError {}

impl From<hex::FromHexError> for SecureMemoryError {
    fn from(value: hex::FromHexError) -> Self {
        Self::HexDecode(value)
    }
}

/// Locked, guarded, zero-on-drop bytes for temporary BINA wallet secrets.
pub struct SecureBuffer {
    data: Vec<u8>,
    user_len: usize,
    canary: u64,
    locked: bool,
    valid: bool,
}

impl SecureBuffer {
    pub fn new(size: usize) -> Result<Self, SecureMemoryError> {
        if size == 0 || size > MAX_SECURE_BYTES {
            return Err(SecureMemoryError::InvalidSize);
        }

        let total = GUARD_SIZE + size + GUARD_SIZE;
        let mut data = vec![0u8; total];
        data[..GUARD_SIZE].fill(GUARD_BYTE);
        data[GUARD_SIZE + size..].fill(GUARD_BYTE);

        let mut buffer = Self {
            canary: make_canary(data.as_ptr()),
            data,
            user_len: size,
            locked: false,
            valid: true,
        };
        buffer.lock_best_effort();
        platform::advise_secret(buffer.data.as_mut_ptr(), buffer.data.len());
        Ok(buffer)
    }

    pub fn from_slice(secret: &[u8]) -> Result<Self, SecureMemoryError> {
        let mut buffer = Self::new(secret.len())?;
        buffer.as_mut_slice()?.copy_from_slice(secret);
        Ok(buffer)
    }

    pub fn from_vec(mut secret: Vec<u8>) -> Result<Self, SecureMemoryError> {
        let buffer = Self::from_slice(&secret)?;
        secret.zeroize();
        Ok(buffer)
    }

    pub fn from_hex(secret_hex: &str) -> Result<Self, SecureMemoryError> {
        let decoded = hex::decode(secret_hex.trim())?;
        Self::from_vec(decoded)
    }

    pub fn len(&self) -> usize {
        self.user_len
    }
    pub fn is_empty(&self) -> bool {
        self.user_len == 0
    }
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    pub fn verify(&self) -> Result<(), SecureMemoryError> {
        if !self.valid {
            return Err(SecureMemoryError::InvalidState);
        }
        if self.canary != make_canary(self.data.as_ptr()) {
            return Err(SecureMemoryError::IntegrityViolation);
        }
        let high_guard_start = GUARD_SIZE + self.user_len;
        let low_guard_ok = self.data[..GUARD_SIZE]
            .iter()
            .all(|byte| *byte == GUARD_BYTE);
        let high_guard_ok = self.data[high_guard_start..]
            .iter()
            .all(|byte| *byte == GUARD_BYTE);
        if !low_guard_ok || !high_guard_ok {
            return Err(SecureMemoryError::IntegrityViolation);
        }
        Ok(())
    }

    pub fn as_slice(&self) -> Result<&[u8], SecureMemoryError> {
        self.verify()?;
        Ok(&self.data[GUARD_SIZE..GUARD_SIZE + self.user_len])
    }

    pub fn as_mut_slice(&mut self) -> Result<&mut [u8], SecureMemoryError> {
        self.verify()?;
        Ok(&mut self.data[GUARD_SIZE..GUARD_SIZE + self.user_len])
    }

    pub fn constant_time_eq(&self, other: &[u8]) -> bool {
        if self.user_len != other.len() || self.verify().is_err() {
            return false;
        }
        let secret = &self.data[GUARD_SIZE..GUARD_SIZE + self.user_len];
        secret
            .iter()
            .zip(other.iter())
            .fold(0u8, |diff, (left, right)| diff | (left ^ right))
            == 0
    }

    pub fn zero(&mut self) {
        self.data[GUARD_SIZE..GUARD_SIZE + self.user_len].zeroize();
        fence(Ordering::SeqCst);
    }

    fn lock_best_effort(&mut self) {
        self.locked = platform::lock(self.data.as_mut_ptr(), self.data.len()).is_ok();
    }

    fn unlock_best_effort(&mut self) {
        if self.locked {
            let _ = platform::unlock(self.data.as_mut_ptr(), self.data.len());
            self.locked = false;
        }
    }
}

impl Drop for SecureBuffer {
    fn drop(&mut self) {
        self.valid = false;
        self.data.zeroize();
        fence(Ordering::SeqCst);
        self.unlock_best_effort();
    }
}

#[cfg(target_os = "linux")]
pub fn debugger_present() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("TracerPid:")
                    .and_then(|pid| pid.trim().parse::<u32>().ok())
            })
        })
        .map(|pid| pid != 0)
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
pub fn debugger_present() -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn IsDebuggerPresent() -> i32;
    }
    unsafe { IsDebuggerPresent() != 0 }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn debugger_present() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_buffer_is_zeroed() {
        let buffer = SecureBuffer::new(32).unwrap();
        assert_eq!(buffer.len(), 32);
        assert!(buffer.as_slice().unwrap().iter().all(|byte| *byte == 0));
    }

    #[test]
    fn from_hex_decodes_into_buffer() {
        let buffer = SecureBuffer::from_hex("deadbeef").unwrap();
        assert_eq!(buffer.as_slice().unwrap(), &[0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn constant_time_eq_checks_contents() {
        let buffer = SecureBuffer::from_hex("00010203").unwrap();
        assert!(buffer.constant_time_eq(&[0, 1, 2, 3]));
        assert!(!buffer.constant_time_eq(&[0, 1, 2, 4]));
        assert!(!buffer.constant_time_eq(&[0, 1, 2]));
    }

    #[test]
    fn guard_corruption_fails_verification() {
        let mut buffer = SecureBuffer::new(16).unwrap();
        buffer.data[0] = 0;
        assert!(matches!(
            buffer.verify(),
            Err(SecureMemoryError::IntegrityViolation)
        ));
    }
}
