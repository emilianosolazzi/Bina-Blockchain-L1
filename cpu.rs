use std::time::{SystemTime, UNIX_EPOCH};
use raw_cpuid::CpuId; // Import the raw_cpuid crate
use serde::{Serialize, Deserialize}; // Added Deserialize
use tracing::debug; // Added for logging
use std::collections::hash_map::DefaultHasher; // For fingerprinting
use std::hash::{Hash, Hasher}; // For fingerprinting
use sha2::{Sha256, Digest}; // For more robust fingerprinting (optional)

// --- Concrete CPU Detection ---
fn detect_cpu() -> CpuIdentity {
    let cpuid = CpuId::new();
    let mut identity = CpuIdentity::default();

    if let Some(vf) = cpuid.get_vendor_info() {
        identity.vendor = vf.as_str().to_string();
    }

    if let Some(bi) = cpuid.get_brand_string() {
        identity.brand = bi.as_str().to_string();
    }

    if let Some(fi) = cpuid.get_feature_info() {
        // Combine various feature flags into a single u64 for simplicity in masking
        // This is a simplified representation; a real implementation might need more detail
        let mut features_val: u64 = 0;
        // Assign bits for common features
        if fi.has_sse()    { features_val |= 1 << 0; } // Bit 0: SSE
        if fi.has_sse2()   { features_val |= 1 << 1; } // Bit 1: SSE2
        if fi.has_sse3()   { features_val |= 1 << 2; } // Bit 2: SSE3
        if fi.has_ssse3()  { features_val |= 1 << 3; } // Bit 3: SSSE3
        if fi.has_sse41()  { features_val |= 1 << 4; } // Bit 4: SSE4.1
        if fi.has_sse42()  { features_val |= 1 << 5; } // Bit 5: SSE4.2
        if fi.has_avx()    { features_val |= 1 << 6; } // Bit 6: AVX
        if fi.has_avx2()   { features_val |= 1 << 7; } // Bit 7: AVX2
        if fi.has_fma()    { features_val |= 1 << 8; } // Bit 8: FMA
        if fi.has_bmi1()   { features_val |= 1 << 9; } // Bit 9: BMI1
        if fi.has_bmi2()   { features_val |= 1 << 10; } // Bit 10: BMI2
        if fi.has_sha()    { features_val |= 1 << 11; } // Bit 11: SHA
        // --- Add more potentially identifying features ---
        if fi.has_htt()    { features_val |= 1 << 12; } // Bit 12: Hyper-Threading Technology
        if fi.has_sgx()    { features_val |= 1 << 13; } // Bit 13: Software Guard Extensions
        if fi.has_hle()    { features_val |= 1 << 14; } // Bit 14: Hardware Lock Elision (Part of TSX)
        if fi.has_rtm()    { features_val |= 1 << 15; } // Bit 15: Restricted Transactional Memory (Part of TSX)
        // Add other relevant features as needed, assigning unique bits
        identity.features = features_val;
        // --- Detect SMT (Hyper-Threading) ---
        // Often indicated by logical cores > physical cores or specific flags
        if fi.has_htt() {
            identity.smt_enabled = Some(true);
            // Verify by comparing logical vs physical cores if possible
            if let Some(pi) = cpuid.get_processor_capacity_info() {
                if let (Some(phys), Some(log)) = (pi.physical_cores(), pi.logical_cores()) {
                    identity.smt_enabled = Some(log > phys);
                }
            }
        } else {
             identity.smt_enabled = Some(false);
        }
    }

    // --- Detect Core Count ---
    let mut physical_cores = None;
    let mut logical_cores = None;
    if let Some(pi) = cpuid.get_processor_capacity_info() {
         physical_cores = pi.physical_cores();
         logical_cores = pi.logical_cores();
         identity.cores = physical_cores.unwrap_or_else(|| logical_cores.unwrap_or(1)) as usize;
    } else if let Some(topo) = cpuid.get_topology_info() {
         // Fallback using topology info if capacity info is not available
         identity.cores = topo.num_cores() as usize;
         // Try to infer logical cores from topology if not already set
         if logical_cores.is_none() {
             logical_cores = Some(topo.num_threads() as u8);
         }
         // If physical cores still unknown, assume cores == threads if SMT likely disabled
         if physical_cores.is_none() && identity.smt_enabled == Some(false) {
             physical_cores = Some(topo.num_cores() as u8);
         }
    } else {
         identity.cores = 1; // Default if no core info found
         physical_cores = Some(1);
         logical_cores = Some(1);
    }
    // Store logical cores if detected
    identity.logical_cores = logical_cores.map(|lc| lc as usize);


    // --- Detect NUMA Nodes (Simplified Placeholder) ---
    // Accurate NUMA detection is complex and platform-specific.
    // raw_cpuid doesn't directly expose NUMA node count easily.
    // We'll use a placeholder based on cache info or assume 1.
    if let Some(ext_topo) = cpuid.get_extended_topology_info() {
         // Example: Check if multiple L3 caches exist, might indicate nodes
         // This is a very rough heuristic.
         let mut nodes = 1;
         let mut last_l3_id = None;
         for core_topo in ext_topo {
             if let Some(l3_id) = core_topo.l3_cache_id() {
                 if last_l3_id.is_none() {
                     last_l3_id = Some(l3_id);
                 } else if last_l3_id != Some(l3_id) {
                     nodes += 1; // Found a different L3 ID, assume new node
                     last_l3_id = Some(l3_id);
                 }
             }
         }
         identity.numa_nodes = Some(nodes.min(u8::MAX as u32) as u8); // Cap at u8::MAX
    } else {
         identity.numa_nodes = Some(1); // Default assumption
    }


    // Get L3 cache size if available
    if let Some(l3) = cpuid.get_l3_cache_info() {
        identity.cache_size = l3.size() as usize;
    } else if let Some(l2) = cpuid.get_l2_cache_info() {
        // Fallback to L2 if L3 is not available
        identity.cache_size = l2.size() as usize;
    } else {
        identity.cache_size = 0; // Default if no cache info found
    }

    // --- Detect Max Temperature (Placeholder - Very Difficult) ---
    // Reliably getting max *design* temperature via CPUID is generally not possible.
    // TjMax might be available via MSRs, but requires kernel access/privileges.
    // We'll leave this as None in detection, obfuscation will handle it.
    identity.max_temp_real = None;

    identity
}

// Define potentially identifying feature bits to mask
// Example: Masking AVX/AVX2/SHA might obscure performance characteristics
// Corresponds to bits 6, 7, 11 defined above
// --- Updated feature bits to mask ---
const PERSONAL_FEATURE_BITS: u64 = (1 << 6)  // AVX
                                 | (1 << 7)  // AVX2
                                 | (1 << 11) // SHA
                                 | (1 << 12) // HTT
                                 | (1 << 13) // SGX
                                 | (1 << 14) // HLE (TSX)
                                 | (1 << 15); // RTM (TSX)

// --- End Concrete CPU Detection ---


// --- Updated CpuIdentity struct ---
#[derive(Default, Debug, Clone, Serialize, Deserialize)] // Added Deserialize
pub struct CpuIdentity {
    pub vendor: String,
    pub brand: String,
    pub cores: usize, // Physical cores preferably
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logical_cores: Option<usize>, // Total logical processors
    pub features: u64, // Represents a bitmask of detected features
    pub cache_size: usize, // L3 cache size preferably, or L2 as fallback
    pub timestamp: u64, // Timestamp or seed used for masking
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_temp_obfuscated: Option<u8>, // Optional obfuscated operating temp range indicator (degrees C)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smt_enabled: Option<bool>, // Simultaneous Multithreading (Hyper-Threading)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numa_nodes: Option<u8>, // Number of NUMA nodes detected/inferred

    // Internal field, not serialized
    #[serde(skip)]
    max_temp_real: Option<f32>, // Actual max temp if detectable (rarely available)
}

// --- Implementation for CpuIdentity ---
impl CpuIdentity {
    /// Serializes the CpuIdentity struct to a JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserializes a CpuIdentity struct from a JSON string.
    pub fn from_json(json_str: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json_str)
    }

    /// Generates a telemetry fingerprint hash based on key masked characteristics.
    /// Uses Sha256 for a more stable and collision-resistant fingerprint.
    pub fn telemetry_fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.vendor.as_bytes());
        hasher.update(&self.cores.to_le_bytes());
        hasher.update(&self.features.to_le_bytes()); // Use masked features
        hasher.update(&self.cache_size.to_le_bytes());
        if let Some(smt) = self.smt_enabled {
            hasher.update([smt as u8]);
        }
        if let Some(numa) = self.numa_nodes {
            hasher.update([numa]);
        }
        // Consider adding obfuscated temp range if needed for fingerprinting
        // if let Some(temp) = self.max_temp_obfuscated {
        //     hasher.update([temp]);
        // }

        hex::encode(hasher.finalize())
    }
}


// --- Updated mask_cpu_identity function ---
pub fn mask_cpu_identity(seed: Option<u64>, real_temp: Option<f32>) -> CpuIdentity {
    let mut masked = CpuIdentity::default();
    let real = detect_cpu(); // Call the concrete detection function

    // Use provided seed if available, otherwise use time-based noise
    let time_seed = seed.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    });

    // --- Add Logging ---
    debug!("Applying CPU identity mask using seed: {}", time_seed);

    // --- Mask Core Counts ---
    let base_cores = (real.cores / 2).max(1);
    masked.cores = base_cores + (time_seed % (base_cores as u64 + 1)) as usize;
    masked.cores = masked.cores.max(1); // Ensure at least 1 core
    // Obfuscate logical cores based on physical cores and SMT status
    if let Some(smt) = real.smt_enabled {
         masked.smt_enabled = Some(smt); // Pass through SMT status for now
         if smt {
             // If SMT enabled, logical cores should be >= 2 * physical cores (roughly)
             let base_logical = masked.cores * 2;
             masked.logical_cores = Some(base_logical + (time_seed % (masked.cores as u64 + 1)) as usize);
         } else {
             // If SMT disabled, logical cores == physical cores
             masked.logical_cores = Some(masked.cores);
         }
    } else {
         // If SMT status unknown, make a guess or leave None
         masked.smt_enabled = None;
         masked.logical_cores = Some(masked.cores + (time_seed % (masked.cores as u64 + 1)) as usize); // Guess logical cores
    }


    // Mask identifying features using the defined constant
    masked.features = real.features & !PERSONAL_FEATURE_BITS;

    // Cache size within 50-75% of real (ensure it doesn't go to 0 if real.cache_size is small)
    if real.cache_size > 0 {
        masked.cache_size = (real.cache_size as u64 * (50 + (time_seed % 26))) / 100;
        masked.cache_size = masked.cache_size.max(1024 * 1024); // Ensure a minimum plausible cache size (e.g., 1MB)
    } else {
        masked.cache_size = 1024 * 1024; // Default if real cache size was 0
    }

    // --- Obfuscate Thermal Info ---
    masked.max_temp_obfuscated = obfuscate_temperature(real_temp, time_seed);

    // --- Obfuscate NUMA Nodes ---
    // Simple obfuscation: report 1 if real > 1, otherwise report real (usually 1)
    masked.numa_nodes = real.numa_nodes.map(|n| if n > 1 { 1 + (time_seed % 2) as u8 } else { n }); // Report 1 or 2 if multi-node


    // Add random vendor string based on time seed
    masked.vendor = match time_seed % 3 {
        0 => "GenuineIntel".to_string(),
        1 => "AuthenticAMD".to_string(),
        _ => "GenericCPU".to_string(), // Use a more generic name
    };

    // Brand string is often highly specific, set to generic
    masked.brand = format!("Masked {} {} Cores", masked.vendor, masked.cores); // More descriptive generic brand

    masked.timestamp = time_seed; // Record the seed used for masking

    masked
}

/// Obfuscates a real temperature reading using a seed.
/// If no real temperature is provided, generates a plausible placeholder.
fn obfuscate_temperature(real_temp: Option<f32>, seed: u64) -> Option<u8> {
    match real_temp {
        Some(temp) => {
            // Apply noise based on seed, e.g., +/- 5 degrees
            let noise = (seed % 11) as i8 - 5; // Range -5 to +5
            let obfuscated = temp + noise as f32;
            // Clamp to a reasonable range (e.g., 60-110 C) and convert to u8
            Some(obfuscated.max(60.0).min(110.0) as u8)
        }
        None => {
            // Generate a plausible placeholder temp range indicator (e.g., 80-105 C)
            Some(80 + (seed % 26) as u8)
        }
    }
}
