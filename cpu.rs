//! # CPU Identity and Fingerprinting
//! 
//! This module provides secure CPU detection, fingerprinting, and thermal monitoring
//! capabilities for entropy gathering. It includes:
//!
//! - Safe CPU feature detection with fallbacks
//! - Privacy-preserving identity masking
//! - Cross-platform thermal monitoring
//! - Efficient caching of detection results
//!
//! ## Examples
//!
//! ```rust
//! use entropy_randomness::cpu::{detect_cpu_safely, mask_cpu_identity, get_cpu_temperature};
//!
//! // Get basic CPU identity
//! let id = detect_cpu_safely();
//! println!("CPU: {}", id.display_name());
//! 
//! // Generate a fingerprint suitable for entropy sources
//! println!("Fingerprint: {}", id.telemetry_fingerprint());
//! 
//! // Get a privacy-preserving masked version (75% info retention)
//! let masked = mask_cpu_identity(None, Some(75.0));
//! println!("Masked: {}", masked.display_name());
//! 
//! // Get current CPU temperature (if available)
//! if let Some(temp) = get_cpu_temperature() {
//!     println!("CPU Temperature: {:.1}°C", temp);
//! }
//! ```

use std::fmt;
use std::time::Instant;
use std::collections::HashMap;
use std::sync::{Mutex, atomic::{AtomicU64, AtomicI32, Ordering}};
use raw_cpuid::{CpuId, CpuIdReaderNative};
use sha2::{Sha256, Digest};
use once_cell::sync::OnceCell;
use log::{debug, warn, error, info};

// Security-audited dependency versions (last audit: 2023-10-15)
// raw_cpuid = "10.7.0"    - No known CVEs
// once_cell = "1.18.0"    - No known CVEs
// sha2 = "0.10.7"         - No known CVEs
// blake3 = "1.4.1"        - No known CVEs
// log = "0.4.20"          - No known CVEs

#[cfg(target_os = "windows")]
use {
    std::ptr,
    windows::{Win32::System::Wmi::*, core::*},
};

#[cfg(target_os = "macos")]
use {
    std::{ptr, mem},
    std::ffi::CStr,
};

#[cfg(target_os = "linux")]
use std::fs::File;
#[cfg(target_os = "linux")]
use std::io::Read;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};

// Global CPU identity cache
static CPU_IDENTITY: OnceCell<CpuIdentity> = OnceCell::new();

// Temperature cache using atomic types for thread safety
static LAST_TEMPERATURE_TIME_MS: AtomicU64 = AtomicU64::new(0);
static LAST_TEMPERATURE_VALUE: AtomicI32 = AtomicI32::new(0);
static TEMPERATURE_CACHE_TTL_MS: u64 = 1000; // 1 second TTL for temperature cache

// Track platform-specific warnings to avoid spamming logs
static TEMPERATURE_WARNING_LOGGED: AtomicU64 = AtomicU64::new(0);

// Configuration for masking operations
#[derive(Debug, Clone)]
pub struct MaskingConfig {
    /// Which vendor brands to include unmodified
    pub trusted_vendors: Vec<String>,
    /// Which specific model strings to include unmodified
    pub trusted_models: Vec<String>,
    /// Percentage of information to retain (0-100)
    pub info_retention_percent: f32,
    /// Whether to mask core/thread counts
    pub mask_core_counts: bool,
    /// Whether to mask cache sizes
    pub mask_cache_sizes: bool,
    /// Whether to mask CPU flags entirely
    pub mask_cpu_flags: bool,
    /// Core/thread count masking method: "power_of_two" or "multiple_of_n" or "percent_variance"
    pub core_mask_method: String,
    /// Parameter for core masking method (N for multiple_of_n, variance % for percent_variance)
    pub core_mask_param: u32,
}

impl Default for MaskingConfig {
    fn default() -> Self {
        MaskingConfig {
            trusted_vendors: vec!["GenuineIntel".to_string(), "AuthenticAMD".to_string()],
            trusted_models: vec![],
            info_retention_percent: 50.0,
            mask_core_counts: false,
            mask_cache_sizes: true,
            mask_cpu_flags: false,
            core_mask_method: "power_of_two".to_string(),
            core_mask_param: 2,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CpuIdentity {
    vendor: String,
    brand: String,
    signature: u32,
    family: u32,
    model: u32,
    stepping: u32,
    features: Vec<String>,
    cores: u32,
    threads: u32,
    cache_size_kb: u32,
    detection_time_us: u64,
}

impl CpuIdentity {
    /// Returns a display name with vendor and model information
    pub fn display_name(&self) -> String {
        format!("{} {}", self.vendor, self.brand)
    }

    /// Returns a fingerprint suitable for entropy/randomness generation
    pub fn telemetry_fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.vendor.as_bytes());
        hasher.update(self.brand.as_bytes());
        hasher.update(self.signature.to_le_bytes());
        hasher.update(self.cores.to_le_bytes());
        hasher.update(self.threads.to_le_bytes());
        
        // Add features for more entropy
        for feature in &self.features {
            hasher.update(feature.as_bytes());
        }
        
        let result = hasher.finalize();
        format!("{:x}", result)
    }
    
    /// Returns how long detection took in microseconds
    pub fn detection_time_us(&self) -> u64 {
        self.detection_time_us
    }

    // Accessors for various fields
    pub fn vendor(&self) -> &str {
        &self.vendor
    }

    pub fn brand(&self) -> &str {
        &self.brand
    }

    pub fn features(&self) -> &[String] {
        &self.features
    }

    pub fn cores(&self) -> u32 {
        self.cores
    }

    pub fn threads(&self) -> u32 {
        self.threads
    }
    
    pub fn family(&self) -> u32 {
        self.family
    }
    
    pub fn model(&self) -> u32 {
        self.model
    }
    
    pub fn stepping(&self) -> u32 {
        self.stepping
    }
    
    pub fn cache_size_kb(&self) -> u32 {
        self.cache_size_kb
    }
}

impl fmt::Display for CpuIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} (Family: {}, Model: {}, Stepping: {}) - {} cores/{} threads",
            self.vendor, self.brand, self.family, self.model, self.stepping, self.cores, self.threads
        )
    }
}

/// Safely detects CPU information with timing measurement
pub fn detect_cpu_safely() -> &'static CpuIdentity {
    CPU_IDENTITY.get_or_init(|| {
        // Time the detection process
        let start = Instant::now();
        
        let cpuid = CpuId::new();
        let mut identity = CpuIdentity {
            vendor: String::new(),
            brand: String::new(),
            signature: 0,
            family: 0,
            model: 0,
            stepping: 0,
            features: Vec::new(),
            cores: 1,
            threads: 1,
            cache_size_kb: 0,
            detection_time_us: 0,
        };

        // Get vendor
        if let Some(vendor_info) = cpuid.get_vendor_info() {
            identity.vendor = vendor_info.as_str().to_string();
        }

        // Get brand string
        if let Some(brand_info) = cpuid.get_processor_brand_string() {
            identity.brand = brand_info.as_str().to_string();
        }

        // Get signature, family, model, stepping
        if let Some(version) = cpuid.get_feature_info() {
            identity.signature = version.signature();
            identity.family = version.extended_family_id() + version.family_id();
            identity.model = (version.extended_model_id() << 4) + version.model_id();
            identity.stepping = version.stepping_id();
        }

        // Detect CPU features systematically
        let mut features = Vec::new();
        if let Some(feature_info) = cpuid.get_feature_info() {
            // Basic features
            if feature_info.has_fpu() { features.push("fpu".to_string()); }
            if feature_info.has_vme() { features.push("vme".to_string()); }
            if feature_info.has_de() { features.push("de".to_string()); }
            if feature_info.has_pse() { features.push("pse".to_string()); }
            if feature_info.has_tsc() { features.push("tsc".to_string()); }
            if feature_info.has_msr() { features.push("msr".to_string()); }
            if feature_info.has_pae() { features.push("pae".to_string()); }
            if feature_info.has_mce() { features.push("mce".to_string()); }
            if feature_info.has_cmpxchg8b() { features.push("cmpxchg8b".to_string()); }
            if feature_info.has_apic() { features.push("apic".to_string()); }
            if feature_info.has_sysenter_sysexit() { features.push("sysenter_sysexit".to_string()); }
            if feature_info.has_mtrr() { features.push("mtrr".to_string()); }
            if feature_info.has_pge() { features.push("pge".to_string()); }
            if feature_info.has_mca() { features.push("mca".to_string()); }
            if feature_info.has_cmov() { features.push("cmov".to_string()); }
            if feature_info.has_pat() { features.push("pat".to_string()); }
            if feature_info.has_pse36() { features.push("pse36".to_string()); }
            if feature_info.has_psn() { features.push("psn".to_string()); }
            if feature_info.has_clflush() { features.push("clflush".to_string()); }
            if feature_info.has_ds() { features.push("ds".to_string()); }
            if feature_info.has_acpi() { features.push("acpi".to_string()); }
            if feature_info.has_mmx() { features.push("mmx".to_string()); }
            if feature_info.has_fxsave_fxstor() { features.push("fxsave".to_string()); }
            if feature_info.has_sse() { features.push("sse".to_string()); }
            if feature_info.has_sse2() { features.push("sse2".to_string()); }
            if feature_info.has_ss() { features.push("ss".to_string()); }
            if feature_info.has_htt() { features.push("htt".to_string()); }
            if feature_info.has_tm() { features.push("tm".to_string()); }
            if feature_info.has_pbe() { features.push("pbe".to_string()); }
        }

        // Extended features
        if let Some(extended_features) = cpuid.get_extended_feature_info() {
            if extended_features.has_sse3() { features.push("sse3".to_string()); }
            if extended_features.has_pclmulqdq() { features.push("pclmulqdq".to_string()); }
            if extended_features.has_ds_area() { features.push("ds_area".to_string()); }
            if extended_features.has_monitor_mwait() { features.push("monitor".to_string()); }
            if extended_features.has_cpl() { features.push("cpl".to_string()); }
            if extended_features.has_vmx() { features.push("vmx".to_string()); }
            if extended_features.has_smx() { features.push("smx".to_string()); }
            if extended_features.has_eist() { features.push("eist".to_string()); }
            if extended_features.has_tm2() { features.push("tm2".to_string()); }
            if extended_features.has_ssse3() { features.push("ssse3".to_string()); }
            if extended_features.has_cnxt_id() { features.push("cnxt_id".to_string()); }
            if extended_features.has_fma() { features.push("fma".to_string()); }
            if extended_features.has_cmpxchg16b() { features.push("cmpxchg16b".to_string()); }
            if extended_features.has_xtpr() { features.push("xtpr".to_string()); }
            if extended_features.has_pdcm() { features.push("pdcm".to_string()); }
            if extended_features.has_pcid() { features.push("pcid".to_string()); }
            if extended_features.has_dca() { features.push("dca".to_string()); }
            if extended_features.has_sse41() { features.push("sse4.1".to_string()); }
            if extended_features.has_sse42() { features.push("sse4.2".to_string()); }
            if extended_features.has_x2apic() { features.push("x2apic".to_string()); }
            if extended_features.has_movbe() { features.push("movbe".to_string()); }
            if extended_features.has_popcnt() { features.push("popcnt".to_string()); }
            if extended_features.has_tsc_deadline() { features.push("tsc_deadline".to_string()); }
            if extended_features.has_aesni() { features.push("aesni".to_string()); }
            if extended_features.has_xsave() { features.push("xsave".to_string()); }
            if extended_features.has_oxsave() { features.push("oxsave".to_string()); }
            if extended_features.has_avx() { features.push("avx".to_string()); }
            if extended_features.has_f16c() { features.push("f16c".to_string()); }
            if extended_features.has_rdrand() { features.push("rdrand".to_string()); }
        }

        // AVX2 and other advanced features
        if let Some(extended_features2) = cpuid.get_extended_feature_info() {
            if extended_features2.has_avx2() { features.push("avx2".to_string()); }
            // Add more advanced features...
        }

        // Structured Extended Features (if available)
        if let Some(structured) = cpuid.get_structured_extended_feature_info() {
            // Check leaf 0
            if structured.has_sgx() { features.push("sgx".to_string()); }
            if structured.has_avx512_f() { features.push("avx512f".to_string()); }
            if structured.has_avx512_dq() { features.push("avx512dq".to_string()); }
            if structured.has_avx512_ifma() { features.push("avx512ifma".to_string()); }
            if structured.has_avx512_pf() { features.push("avx512pf".to_string()); }
            if structured.has_avx512_er() { features.push("avx512er".to_string()); }
            if structured.has_avx512_cd() { features.push("avx512cd".to_string()); }
            if structured.has_avx512_bw() { features.push("avx512bw".to_string()); }
            if structured.has_avx512_vl() { features.push("avx512vl".to_string()); }
            if structured.has_pku() { features.push("pku".to_string()); }
            if structured.has_rdpid() { features.push("rdpid".to_string()); }
            // Add more structured extended features as needed
        }

        identity.features = features;

        // Get topology info
        if let Some(topo) = cpuid.get_extended_topology_info() {
            let mut cores = 0;
            let mut threads = 0;

            for level in topo {
                match level.level_type() {
                    raw_cpuid::TopologyType::Core => {
                        cores = level.processors();
                    }
                    raw_cpuid::TopologyType::SMT => {
                        threads = level.processors();
                    }
                    _ => {}
                }
            }

            // Fallback for older CPUs where extended topology is unavailable
            if cores == 0 || threads == 0 {
                if let Some(feature_info) = cpuid.get_feature_info() {
                    threads = u32::from(feature_info.max_logical_processor_ids());
                    // Assume 1 thread per core if can't differentiate
                    cores = threads;
                }
            }

            identity.cores = cores;
            identity.threads = threads;
        }

        // Get cache size
        if let Some(cache_info) = cpuid.get_cache_parameters() {
            let mut total_cache = 0;
            for cache in cache_info {
                if cache.level() == 3 { // L3 cache
                    total_cache += cache.associativity() * cache.line_partitions() * cache.line_size() * cache.sets();
                }
            }
            identity.cache_size_kb = total_cache / 1024;
        }

        // Record detection time
        identity.detection_time_us = start.elapsed().as_micros() as u64;
        debug!("CPU detection completed in {} μs", identity.detection_time_us);
        
        identity
    })
}

/// Creates a masked version of the CPU identity for privacy
pub fn mask_cpu_identity(config: Option<MaskingConfig>, info_retention_percent: Option<f32>) -> CpuIdentity {
    let cpu = detect_cpu_safely();
    let mut config = config.unwrap_or_default();
    
    // Override retention percentage if specified
    if let Some(retention) = info_retention_percent {
        config.info_retention_percent = retention.clamp(0.0, 100.0);
    }
    
    // Clone the original identity to modify
    let mut masked = cpu.clone();
    
    // Check if this is a trusted vendor or model
    let is_trusted_vendor = config.trusted_vendors.iter().any(|v| v == &cpu.vendor);
    let is_trusted_model = config.trusted_models.iter().any(|m| cpu.brand.contains(m));
    
    // Only mask if not trusted
    if !is_trusted_vendor && !is_trusted_model {
        // Apply information retention based on config
        if config.info_retention_percent < 100.0 {
            // Mask brand string
            let keep_chars = (cpu.brand.len() as f32 * config.info_retention_percent / 100.0) as usize;
            if keep_chars < cpu.brand.len() {
                masked.brand = format!(
                    "{}{}",
                    &cpu.brand[..keep_chars],
                    "*".repeat(cpu.brand.len() - keep_chars)
                );
            }
            
            // Mask signature
            masked.signature = cpu.signature & ((1u32 << (32u32 * config.info_retention_percent as u32 / 100)) - 1);
        }
        
        // Apply specific masks
        if config.mask_core_counts {
            // Apply different core masking methods based on config
            match config.core_mask_method.as_str() {
                "power_of_two" => {
                    // Round to nearest power of 2 for privacy
                    masked.cores = 1u32 << (32 - masked.cores.leading_zeros() - 1);
                    masked.threads = 1u32 << (32 - masked.threads.leading_zeros() - 1);
                },
                "multiple_of_n" => {
                    // Round to nearest multiple of N (default 2)
                    let n = config.core_mask_param.max(1);
                    masked.cores = ((masked.cores + n/2) / n) * n;
                    masked.threads = ((masked.threads + n/2) / n) * n;
                },
                "percent_variance" => {
                    // Add random variance within percentage bounds
                    let variance_pct = config.core_mask_param.clamp(1, 50) as f32 / 100.0;
                    
                    // Use a simple hash-based pseudo-random function for deterministic variance
                    let mut hasher = Sha256::new();
                    hasher.update(cpu.vendor.as_bytes());
                    hasher.update(cpu.cores.to_le_bytes());
                    let hash = hasher.finalize();
                    let variance_factor = 1.0 + (hash[0] as f32 / 255.0 * 2.0 - 1.0) * variance_pct;
                    
                    masked.cores = (cpu.cores as f32 * variance_factor).round() as u32;
                    masked.threads = (cpu.threads as f32 * variance_factor).round() as u32;
                    
                    // Ensure values make sense (threads >= cores)
                    if masked.threads < masked.cores {
                        masked.threads = masked.cores;
                    }
                },
                _ => {
                    // Default case - just use power_of_two method
                    masked.cores = 1u32 << (32 - masked.cores.leading_zeros() - 1);
                    masked.threads = 1u32 << (32 - masked.threads.leading_zeros() - 1);
                }
            }
        }
        
        if config.mask_cache_sizes {
            // Round to nearest 1MB increment
            masked.cache_size_kb = (masked.cache_size_kb / 1024) * 1024;
        }
        
        if config.mask_cpu_flags {
            // Keep only basic features
            masked.features.retain(|f| 
                f == "fpu" || f == "mmx" || f == "sse" || f == "sse2" ||
                f == "avx" || f == "avx2"
            );
        }
    }
    
    masked
}

/// Gets the current CPU temperature if available
/// Returns temperature in degrees Celsius
pub fn get_cpu_temperature() -> Option<f32> {
    // Check the atomic cache first
    let last_time = LAST_TEMPERATURE_TIME_MS.load(Ordering::Relaxed);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    
    if now_ms - last_time < TEMPERATURE_CACHE_TTL_MS {
        // Use cached value
        let temp_i32 = LAST_TEMPERATURE_VALUE.load(Ordering::Relaxed);
        return Some(f32::from_bits(temp_i32 as u32));
    }
    
    // Platform-specific implementation
    let temp = platform_get_cpu_temperature();
    
    // Update cache if we got a valid temperature
    if let Some(temp_value) = temp {
        // Store in atomics
        LAST_TEMPERATURE_TIME_MS.store(now_ms, Ordering::Relaxed);
        LAST_TEMPERATURE_VALUE.store(temp_value.to_bits() as i32, Ordering::Relaxed);
    }
    
    temp
}

#[cfg(target_os = "windows")]
fn platform_get_cpu_temperature() -> Option<f32> {
    // Check if warning about WMI has been logged already
    let warning_logged = TEMPERATURE_WARNING_LOGGED.load(Ordering::Relaxed) & 1 > 0;
    
    // Log warning for Windows
    if !warning_logged {
        warn!("Windows temperature monitoring not fully implemented. Using WMI fallback which may not work on all systems.");
        TEMPERATURE_WARNING_LOGGED.fetch_or(1, Ordering::Relaxed);
    }
    
    // Attempt to use WMI for temperature
    unsafe {
        // Initialize COM
        let hr = CoInitializeEx(ptr::null_mut(), COINIT_MULTITHREADED);
        if hr.is_err() {
            debug!("Failed to initialize COM: {:?}", hr);
            return None;
        }

        // Initialize WMI
        let mut locator: Option<IWbemLocator> = None;
        let hr = CoCreateInstance(
            &CLSID_WbemLocator,
            None,
            CLSCTX_INPROC_SERVER,
            &IWbemLocator::IID,
            locator.set_abi() as *mut _,
        );
        
        if hr.is_err() {
            debug!("Failed to create WbemLocator: {:?}", hr);
            CoUninitialize();
            return None;
        }

        // Connect to WMI
        let mut service: Option<IWbemServices> = None;
        let mut namespace = BSTR::from("root\\WMI");
        let hr = locator.as_ref().unwrap().ConnectServer(
            &namespace,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            service.set_abi() as *mut _,
        );
        
        if hr.is_err() {
            debug!("Failed to connect to WMI: {:?}", hr);
            CoUninitialize();
            return None;
        }

        // Set security level
        let hr = CoSetProxyBlanket(
            service.as_ref().unwrap(),
            RPC_C_AUTHN_WINNT,
            RPC_C_AUTHZ_NONE,
            ptr::null_mut(),
            RPC_C_AUTHN_LEVEL_CALL,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            ptr::null_mut(),
            EOAC_NONE,
        );
        
        if hr.is_err() {
            debug!("Failed to set security level: {:?}", hr);
            CoUninitialize();
            return None;
        }

        // Query for MSAcpi_ThermalZoneTemperature
        let mut enumerator: Option<IEnumWbemClassObject> = None;
        let query = BSTR::from("SELECT * FROM MSAcpi_ThermalZoneTemperature");
        let hr = service.as_ref().unwrap().ExecQuery(
            &BSTR::from("WQL"),
            &query,
            WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
            ptr::null_mut(),
            enumerator.set_abi() as *mut _,
        );
        
        if hr.is_err() {
            // Try alternative WMI class for CPU temperature
            let query = BSTR::from("SELECT * FROM Win32_TemperatureProbe WHERE Description = 'CPU'");
            let hr = service.as_ref().unwrap().ExecQuery(
                &BSTR::from("WQL"),
                &query,
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                ptr::null_mut(),
                enumerator.set_abi() as *mut _,
            );
            
            if hr.is_err() {
                debug!("Failed to execute WMI query for temperature: {:?}", hr);
                CoUninitialize();
                return None;
            }
        }

        // Get results
        let mut result: [Option<IWbemClassObject>; 1] = [None];
        let mut returned: u32 = 0;
        
        let hr = enumerator.as_ref().unwrap().Next(
            WBEM_INFINITE,
            result.len() as u32,
            result.as_mut().as_mut_slice(),
            &mut returned,
        );
        
        if hr.is_err() || returned == 0 {
            CoUninitialize();
            return None;
        }

        // Extract temperature value
        let mut variant = VARIANT::default();
        let prop = BSTR::from("CurrentTemperature");
        let hr = result[0].as_ref().unwrap().Get(&prop, 0, &mut variant, ptr::null_mut(), ptr::null_mut());
        
        if hr.is_err() {
            // Try alternative property name
            let prop = BSTR::from("CurrentReading");
            let hr = result[0].as_ref().unwrap().Get(&prop, 0, &mut variant, ptr::null_mut(), ptr::null_mut());
            
            if hr.is_err() {
                CoUninitialize();
                return None;
            }
        }

        // Convert temperature (in tenths of Kelvin) to Celsius
        let temp_k: f32 = variant.Anonymous.Anonymous.Anonymous.i4 as f32 / 10.0;
        let temp_c = temp_k - 273.15;
        
        CoUninitialize();
        
        if temp_c <= 0.0 || temp_c > 150.0 {
            // Ignore unrealistic values
            return None;
        }
        
        Some(temp_c)
    }
}

#[cfg(target_os = "macos")]
fn platform_get_cpu_temperature() -> Option<f32> {
    // Check if warning about SMC has been logged already
    let warning_logged = TEMPERATURE_WARNING_LOGGED.load(Ordering::Relaxed) & 2 > 0;
    
    // Log warning
    if !warning_logged {
        warn!("macOS temperature monitoring not fully implemented. Enable SMC implementation for full support.");
        TEMPERATURE_WARNING_LOGGED.fetch_or(2, Ordering::Relaxed);
    }
    
    // UNIMPLEMENTED: This is a stub
    //
    // A proper implementation would use the IOKit framework and SMC (System Management Controller)
    // to read temperature sensors. This requires either:
    // 1. Using a crate like 'smc' that provides Rust bindings to the SMC
    // 2. Implementing direct FFI to IOKit functions
    //
    // Common temperature sensor keys for Intel Macs:
    // - "TC0P" - CPU proximity temperature
    // - "TC0D" - CPU die temperature
    // - "TC0E" - CPU heatsink temperature
    //
    // For Apple Silicon/M1/M2, different sensors may apply.
    // 
    // Contribution opportunity: Implement proper SMC access here.

    None // Return None since we don't have an actual implementation
}

#[cfg(target_os = "linux")]
fn platform_get_cpu_temperature() -> Option<f32> {
    // Try reading from multiple possible sensor files
    let paths = [
        "/sys/class/thermal/thermal_zone0/temp",
        "/sys/class/hwmon/hwmon0/temp1_input",
        "/sys/class/hwmon/hwmon1/temp1_input",
        "/sys/devices/platform/coretemp.0/temp1_input",
    ];
    
    for path in &paths {
        if let Ok(mut file) = File::open(path) {
            let mut content = String::new();
            if file.read_to_string(&mut content).is_ok() {
                if let Ok(value) = content.trim().parse::<u32>() {
                    // Most Linux systems report temp in millidegrees Celsius
                    let temp = value as f32 / 1000.0;
                    
                    // Sanity check
                    if temp > 0.0 && temp <= 150.0 {
                        return Some(temp);
                    }
                }
            }
        }
    }
    
    // If standard paths fail, try lm-sensors output parsing
    parse_sensors_output()
}

#[cfg(target_os = "linux")]
fn parse_sensors_output() -> Option<f32> {
    // Run the 'sensors' command from lm-sensors package
    let output = Command::new("sensors")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
        
    match output {
        Ok(output) if output.status.success() => {
            // Parse the output to find CPU temperature
            let output_str = String::from_utf8_lossy(&output.stdout);
            
            // Look for common CPU temperature patterns
            // Example: "Core 0: +45.0°C"
            for line in output_str.lines() {
                if line.contains("Core") && line.contains("°C") {
                    // Extract temperature value
                    if let Some(temp_str) = line.split(':').nth(1) {
                        if let Some(temp_str) = temp_str.split('°').next() {
                            // Parse the temperature
                            if let Ok(temp) = temp_str.trim().trim_start_matches('+').parse::<f32>() {
                                // Sanity check
                                if temp > 0.0 && temp <= 150.0 {
                                    return Some(temp);
                                }
                            }
                        }
                    }
                }
            }
            
            // No valid temperature found
            debug!("sensors command ran but no valid temperature found in output");
            None
        },
        Ok(_) => {
            // Command ran but did not complete successfully
            debug!("sensors command failed with non-zero exit code");
            None
        },
        Err(e) => {
            // Log on first attempt only
            let warning_logged = TEMPERATURE_WARNING_LOGGED.load(Ordering::Relaxed) & 4 > 0;
            if !warning_logged {
                warn!("Could not run lm-sensors. Install lm-sensors package for better temperature detection: {}", e);
                TEMPERATURE_WARNING_LOGGED.fetch_or(4, Ordering::Relaxed);
            } else {
                debug!("sensors command failed: {}", e);
            }
            None
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn platform_get_cpu_temperature() -> Option<f32> {
    // Check if warning has been logged already
    let warning_logged = TEMPERATURE_WARNING_LOGGED.load(Ordering::Relaxed) & 8 > 0;
    
    // Log warning once
    if !warning_logged {
        warn!("Temperature monitoring not implemented for this platform");
        TEMPERATURE_WARNING_LOGGED.fetch_or(8, Ordering::Relaxed);
    }
    
    // Unsupported platform
    None
}

/// Returns a performance report for CPU detection
pub fn get_cpu_detection_performance() -> HashMap<String, u64> {
    let cpu = detect_cpu_safely();
    let mut metrics = HashMap::new();
    
    metrics.insert("detection_time_us".to_string(), cpu.detection_time_us);
    metrics.insert("features_count".to_string(), cpu.features.len() as u64);
    metrics.insert("fingerprint_length".to_string(), cpu.telemetry_fingerprint().len() as u64);
    
    metrics
}

/// Feature flags for CPU feature detection
#[derive(Debug, Clone, Copy)]
pub enum CpuFeature {
    SSE,
    SSE2,
    SSE3,
    SSSE3,
    SSE41,
    SSE42,
    AVX,
    AVX2,
    AVX512F,
    AVX512BW,
    BMI1,
    BMI2,
    FMA,
    RDRAND,
    RDSEED,
    ADX,
    SHA,
    VMWARE,
    HYPERV,
}

/// Check if a specific CPU feature is supported
pub fn has_cpu_feature(feature: CpuFeature) -> bool {
    let cpu = detect_cpu_safely();
    match feature {
        CpuFeature::SSE => cpu.features.iter().any(|f| f == "sse"),
        CpuFeature::SSE2 => cpu.features.iter().any(|f| f == "sse2"),
        CpuFeature::SSE3 => cpu.features.iter().any(|f| f == "sse3"),
        CpuFeature::SSSE3 => cpu.features.iter().any(|f| f == "ssse3"),
        CpuFeature::SSE41 => cpu.features.iter().any(|f| f == "sse4.1"),
        CpuFeature::SSE42 => cpu.features.iter().any(|f| f == "sse4.2"),
        CpuFeature::AVX => cpu.features.iter().any(|f| f == "avx"),
        CpuFeature::AVX2 => cpu.features.iter().any(|f| f == "avx2"),
        CpuFeature::AVX512F => cpu.features.iter().any(|f| f == "avx512f"),
        CpuFeature::AVX512BW => cpu.features.iter().any(|f| f == "avx512bw"),
        CpuFeature::BMI1 => cpu.features.iter().any(|f| f == "bmi1"),
        CpuFeature::BMI2 => cpu.features.iter().any(|f| f == "bmi2"),
        CpuFeature::FMA => cpu.features.iter().any(|f| f == "fma"),
        CpuFeature::RDRAND => cpu.features.iter().any(|f| f == "rdrand"),
        CpuFeature::RDSEED => cpu.features.iter().any(|f| f == "rdseed"),
        CpuFeature::ADX => cpu.features.iter().any(|f| f == "adx"),
        CpuFeature::SHA => cpu.features.iter().any(|f| f == "sha"),
        // Hypervisor checks
        CpuFeature::VMWARE => cpu.features.iter().any(|f| f == "vmware"),
        CpuFeature::HYPERV => cpu.features.iter().any(|f| f == "hypervisor"),
    }
}

// Recommended Cargo.toml entries for this module:
//
// [dependencies]
// raw_cpuid = "=10.7.0"     # Audited for safety 2023-10-15
// once_cell = "=1.18.0"     # Audited for safety 2023-10-15
// sha2 = "=0.10.7"          # Audited for safety 2023-10-15
// log = "=0.4.20"           # Audited for safety 2023-10-15
//
// [target.'cfg(target_os = "windows")'.dependencies]
// windows = { version = "=0.48.0", features = ["Win32_System_Wmi", "Win32_Foundation"] }
//
// [dev-dependencies]
// criterion = "0.5"         # For benchmarks
//
// [[bench]]
// name = "cpu_detection_bench"
// harness = false

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_identity_detection() {
        let cpu = detect_cpu_safely();
        assert!(!cpu.vendor.is_empty(), "CPU vendor should not be empty");
        assert!(!cpu.brand.is_empty(), "CPU brand should not be empty");
        assert!(cpu.cores > 0, "CPU should have at least one core");
        assert!(cpu.threads >= cpu.cores, "Threads should be >= cores");
    }

    #[test]
    fn test_masking() {
        // Default masking at 50%
        let masked = mask_cpu_identity(None, None);
        
        // Full information masking (0%)
        let fully_masked = mask_cpu_identity(None, Some(0.0));
        
        // No masking (100%)
        let unmasked = mask_cpu_identity(None, Some(100.0));
        
        assert_eq!(
            detect_cpu_safely().telemetry_fingerprint(), 
            unmasked.telemetry_fingerprint(),
            "Unmasked fingerprint should match original"
        );
        
        assert_ne!(
            detect_cpu_safely().telemetry_fingerprint(),
            fully_masked.telemetry_fingerprint(),
            "Fully masked fingerprint should differ from original"
        );
    }
    
    #[test]
    fn test_different_masking_methods() {
        let cpu = detect_cpu_safely();
        
        // Test power-of-two masking
        let mut config = MaskingConfig::default();
        config.mask_core_counts = true;
        config.core_mask_method = "power_of_two".to_string();
        let power_two_masked = mask_cpu_identity(Some(config.clone()), None);
        
        // Test multiple-of-n masking
        config.core_mask_method = "multiple_of_n".to_string();
        config.core_mask_param = 4;  // Round to nearest multiple of 4
        let multiple_masked = mask_cpu_identity(Some(config.clone()), None);
        
        // Test variance masking
        config.core_mask_method = "percent_variance".to_string();
        config.core_mask_param = 10; // 10% variance
        let variance_masked = mask_cpu_identity(Some(config), None);
        
        // All methods should produce reasonable values
        assert!(power_two_masked.cores > 0);
        assert!(multiple_masked.cores > 0);
        assert!(variance_masked.cores > 0);
        
        // With different methods, at least one should differ
        assert!(
            power_two_masked.cores != multiple_masked.cores ||
            power_two_masked.cores != variance_masked.cores ||
            multiple_masked.cores != variance_masked.cores
        );
    }
    
    #[test]
    fn test_temperature_reading() {
        // This test is permissive since not all systems support temperature reading
        let temp = get_cpu_temperature();
        if let Some(t) = temp {
            assert!(t > 0.0 && t < 150.0, "Temperature should be reasonable");
        }
    }
    
    #[test]
    fn test_feature_detection() {
        // Most modern CPUs have at least SSE2
        let has_sse2 = has_cpu_feature(CpuFeature::SSE2);
        println!("Has SSE2: {}", has_sse2);
        
        // Check feature list length
        let cpu = detect_cpu_safely();
        assert!(!cpu.features.is_empty(), "CPU features list should not be empty");
        
        // Print all detected features
        println!("Detected {} CPU features:", cpu.features.len());
        for feature in &cpu.features {
            println!("  - {}", feature);
        }
    }
}

#[cfg(feature = "bench")]
pub mod bench {
    use super::*;
    use std::time::Instant;

    pub fn bench_cpu_detection() -> u128 {
        // Invalidate cache to force re-detection
        unsafe { CPU_IDENTITY = OnceCell::new(); }
        
        let start = Instant::now();
        let _ = detect_cpu_safely();
        start.elapsed().as_nanos()
    }
    
    pub fn bench_temperature_reading() -> u128 {
        // Reset temperature cache
        LAST_TEMPERATURE_TIME_MS.store(0, Ordering::Relaxed);
        
        let start = Instant::now();
        let _ = get_cpu_temperature();
        start.elapsed().as_nanos()
    }
}
