use std::time::{SystemTime, UNIX_EPOCH};
use raw_cpuid::CpuId; // Import the raw_cpuid crate
use serde::{Serialize, Deserialize}; // Added Deserialize
use tracing::{debug, warn}; // Added for logging
use std::collections::hash_map::DefaultHasher; // For fingerprinting
use std::hash::{Hash, Hasher}; // For fingerprinting
use sha2::{Sha256, Digest}; // For more robust fingerprinting (optional)
use blake3; // For faster fingerprinting
use std::sync::{Once, Mutex};
use lazy_static::lazy_static;

// --- Cache the CpuId and detected identity (Improvement) ---
lazy_static! {
    // Cache the CpuId instance to avoid repeated initialization
    static ref CPU_ID: Mutex<Option<CpuId>> = Mutex::new(None);
    
    // Cache the detected CPU identity
    static ref DETECTED_CPU_IDENTITY: Mutex<Option<CpuIdentity>> = Mutex::new(None);
    
    // Flag to track initialization
    static ref INIT: Once = Once::new();
}

// Helper function to get or initialize the CpuId instance
fn get_cpu_id() -> Option<CpuId> {
    let mut cpu_id_guard = CPU_ID.lock().unwrap();
    
    if cpu_id_guard.is_none() {
        // Try to create CpuId safely
        match std::panic::catch_unwind(CpuId::new) {
            Ok(cpu_id) => {
                *cpu_id_guard = Some(cpu_id);
            },
            Err(_) => {
                warn!("Failed to initialize CpuId (instruction not supported or panic)");
                return None;
            }
        }
    }
    
    // Clone the CpuId instance if it exists
    cpu_id_guard.clone()
}

// Get or initialize the cached CPU identity
fn get_cached_cpu_identity() -> Result<CpuIdentity, CpuDetectionError> {
    let mut identity_guard = DETECTED_CPU_IDENTITY.lock().unwrap();
    
    if identity_guard.is_none() {
        // Detect CPU and cache the result
        match detect_cpu() {
            Ok(identity) => {
                *identity_guard = Some(identity);
            },
            Err(e) => {
                warn!("Error detecting CPU: {}. Using fallback values.", e);
                return Err(e);
            }
        }
    }
    
    // Clone the identity if it exists
    identity_guard.as_ref().cloned().ok_or(CpuDetectionError::CpuidPanic)
}
// --- End CPU caching improvement ---

// --- Use an explicit feature flags enum instead of bit shifts ---
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CpuFeature {
    SSE,
    SSE2,
    SSE3,
    SSSE3,
    SSE41,
    SSE42,
    AVX,
    AVX2,
    FMA,
    BMI1,
    BMI2,
    SHA,
    HTT,
    SGX,
    HLE,
    RTM,
    // Add more features as needed, without worrying about bit positions
}

// --- Concrete CPU Detection ---
fn detect_cpu() -> Result<CpuIdentity, CpuDetectionError> {
    // Try to get cached CpuId first
    let cpuid = match get_cpu_id() {
        Some(id) => id,
        None => return Err(CpuDetectionError::CpuidPanic)
    };
    
    let mut identity = CpuIdentity::default();

    // Initialize timestamp when detection runs
    identity.timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Use Option-chaining pattern for vendor_info to handle missing data
    if let Some(vf) = cpuid.get_vendor_info() {
        identity.vendor = vf.as_str().to_string();
    } else {
        debug!("Vendor info not available from CPUID");
    }

    if let Some(bi) = cpuid.get_brand_string() {
        identity.brand = bi.as_str().to_string();
    } else {
        debug!("Brand string not available from CPUID");
    }

    // Enhanced feature detection with protocol awareness
    let mut features = std::collections::HashSet::with_capacity(32);
    
    if let Some(fi) = cpuid.get_feature_info() {
        // Use const arrays for better compile-time optimization
        const BASE_FEATURES: [(fn(&raw_cpuid::FeatureInfo) -> bool, CpuFeature); 6] = [
            (|f| f.has_sse(), CpuFeature::SSE),
            (|f| f.has_sse2(), CpuFeature::SSE2),
            (|f| f.has_sse3(), CpuFeature::SSE3),
            (|f| f.has_ssse3(), CpuFeature::SSSE3),
            (|f| f.has_sse41(), CpuFeature::SSE41),
            (|f| f.has_sse42(), CpuFeature::SSE42),
        ];

        const SECURE_FEATURES: [(fn(&raw_cpuid::FeatureInfo) -> bool, CpuFeature); 4] = [
            (|f| f.has_sha() && f.has_sgx(), CpuFeature::SHA),  // Only if SGX available
            (|f| f.has_sgx(), CpuFeature::SGX),
            (|f| f.has_hle() && !f.has_rtm(), CpuFeature::HLE), // HLE without RTM is vulnerable
            (|f| f.has_rtm() && f.has_hle(), CpuFeature::RTM),  // RTM requires HLE
        ];

        const VECTOR_FEATURES: [(fn(&raw_cpuid::FeatureInfo) -> bool, CpuFeature); 4] = [
            (|f| f.has_avx() && f.has_osxsave(), CpuFeature::AVX),    // Check OS support
            (|f| f.has_avx2() && f.has_avx(), CpuFeature::AVX2),      // AVX2 requires AVX
            (|f| f.has_fma() && f.has_avx(), CpuFeature::FMA),        // FMA requires AVX
            (|f| f.has_htt() && verify_htt_support(), CpuFeature::HTT) // Verify HTT support
        ];

        // Batch process features using iterator chaining
        features.extend(
            BASE_FEATURES.iter()
                .chain(SECURE_FEATURES.iter())
                .chain(VECTOR_FEATURES.iter())
                .filter(|(check, _)| check(fi))
                .map(|(_, feature)| feature)
        );

        // Additional security verifications for specific feature combinations
        if features.contains(&CpuFeature::AVX2) {
            verify_avx_support(&mut features)?;
        }

        // Protocol harmonization features
        if let Some(pi) = cpuid.get_processor_capacity_info() {
            harmonize_features(&mut features, pi)?;
        }

        // Convert the HashSet to a bit vector for backward compatibility if needed
        identity.features_bitfield = features_to_bitfield(&features);
        
        // Store the explicit feature set
        identity.detected_features = features;
        
        // --- Detect SMT (Hyper-Threading) ---
        // Often indicated by logical cores > physical cores or specific flags
        if fi.has_htt() {
            identity.smt_enabled = Some(true);
            // Verify by comparing logical vs physical cores if possible
            if let Some(pi) = try_get_processor_capacity_info(&cpuid) {
                if let (Some(phys), Some(log)) = (pi.physical_cores(), pi.logical_cores()) {
                    identity.smt_enabled = Some(log > phys);
                }
            }
        } else {
             identity.smt_enabled = Some(false);
        }
    } else {
        debug!("Feature info not available from CPUID");
    }

    // --- Detect Core Count with robust fallbacks ---
    let mut physical_cores = None;
    let mut logical_cores = None;
    
    // Cross-platform core detection with fallbacks
    // Try processor_capacity_info first, but fallback to topology if needed
    if let Some(pi) = try_get_processor_capacity_info(&cpuid) {
        physical_cores = pi.physical_cores();
        logical_cores = pi.logical_cores();
        identity.cores = physical_cores.unwrap_or_else(|| logical_cores.unwrap_or(1)) as usize;
    }
    // If capacity info unavailable (could be feature-gated on some platforms), try topology
    else if let Some(topo) = cpuid.get_topology_info() {
        // Fallback using topology info
        identity.cores = topo.num_ores() as usize;
        // Try to infer logical cores if not already set
        if logical_cores.is_none() {
            logical_cores = Some(topo.num_threads() as u8);
        }
        // If physical cores still unknown, assume cores == threads if SMT likely disabled
        if physical_cores.is_none() && identity.smt_enabled == Some(false) {
            physical_cores = Some(topo.num_cores() as u8);
        }
    }
    // Final fallback: Try extended_topology_info
    else if let Some(ext_topo) = cpuid.get_extended_topology_info() {
        // Count unique core IDs in the extended topology
        let mut core_ids = std::collections::HashSet::new();
        for core_info in ext_topo {
            if let Some(id) = core_info.core_id() {
                core_ids.insert(id);
            }
        }
        let unique_cores = core_ids.len();
        identity.cores = if unique_cores > 0 { unique_cores } else { 1 };
    }
    else {
        debug!("No core info available from CPUID, using default value of 1");
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
        debug!("Cache info not available from CPUID, using default value of 0");
        identity.cache_size = 0; // Default if no cache info found
    }

    // --- Detect Max Temperature (Placeholder - Very Difficult) ---
    // Reliably getting max *design* temperature via CPUID is generally not possible.
    // TjMax might be available via MSRs, but requires kernel access/privileges.
    // We'll leave this as None in detection, obfuscation will handle it.
    identity.max_temp_real = None;

    Ok(identity)
}

// --- Safe wrapper for get_processor_capacity_info which may be feature-gated ---
fn try_get_processor_capacity_info(cpuid: &CpuId) -> Option<raw_cpuid::ProcessorCapacityInfo> {
    std::panic::catch_unwind(|| cpuid.get_processor_capacity_info())
        .ok()
        .flatten()  // Convert Ok(None) to None
}

// --- Helper to convert HashSet of features to legacy bitfield ---
fn features_to_bitfield(features: &std::collections::HashSet<CpuFeature>) -> u64 {
    let mut bitfield: u64 = 0;
    // Map each feature to a specific bit position, ensuring we stay within u64 bounds
    if features.contains(&CpuFeature::SSE)    { bitfield |= 1 << 0; }
    if features.contains(&CpuFeature::SSE2)   { bitfield |= 1 << 1; }
    if features.contains(&CpuFeature::SSE3)   { bitfield |= 1 << 2; }
    if features.contains(&CpuFeature::SSSE3)  { bitfield |= 1 << 3; }
    if features.contains(&CpuFeature::SSE41)  { bitfield |= 1 << 4; }
    if features.contains(&CpuFeature::SSE42)  { bitfield |= 1 << 5; }
    if features.contains(&CpuFeature::AVX)    { bitfield |= 1 << 6; }
    if features.contains(&CpuFeature::AVX2)   { bitfield |= 1 << 7; }
    if features.contains(&CpuFeature::FMA)    { bitfield |= 1 << 8; }
    if features.contains(&CpuFeature::BMI1)   { bitfield |= 1 << 9; }
    if features.contains(&CpuFeature::BMI2)   { bitfield |= 1 << 10; }
    if features.contains(&CpuFeature::SHA)    { bitfield |= 1 << 11; }
    if features.contains(&CpuFeature::HTT)    { bitfield |= 1 << 12; }
    if features.contains(&CpuFeature::SGX)    { bitfield |= 1 << 13; }
    if features.contains(&CpuFeature::HLE)    { bitfield |= 1 << 14; }
    if features.contains(&CpuFeature::RTM)    { bitfield |= 1 << 15; }
    // Only using bits 0-15 for now, safely within u64 bounds
    
    bitfield
}

// Define personal features to mask using the enum approach
const PERSONAL_FEATURES_TO_MASK: &[CpuFeature] = &[
    CpuFeature::AVX,
    CpuFeature::AVX2,
    CpuFeature::SHA,
    CpuFeature::HTT,
    CpuFeature::SGX,
    CpuFeature::HLE,
    CpuFeature::RTM,
];

// Custom error type for CPU detection
#[derive(Debug)]
pub enum CpuDetectionError {
    CpuidPanic,
    TimeError(std::time::SystemTimeError),
    MissingFeatures,
    Other(String),
}

impl std::fmt::Display for CpuDetectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CpuidPanic => write!(f, "CPUID instruction panicked"),
            Self::TimeError(e) => write!(f, "Time error: {}", e),
            Self::MissingFeatures => write!(f, "Required CPU features missing"),
            Self::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for CpuDetectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::TimeError(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::time::SystemTimeError> for CpuDetectionError {
    fn from(error: std::time::SystemTimeError) -> Self {
        Self::TimeError(error)
    }
}

// --- Updated CpuIdentity struct ---
#[derive(Debug, Clone, Serialize, Deserialize)] 
pub struct CpuIdentity {
    pub vendor: String,
    pub brand: String,
    pub cores: usize, // Physical cores preferably
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logical_cores: Option<usize>, // Total logical processors
    
    // Keep legacy bitfield for backward compatibility
    #[serde(rename = "features")]
    pub features_bitfield: u64,
    
    // Use explicit HashSet for features (safer than bit manipulation)
    #[serde(skip_serializing_if = "std::collections::HashSet::is_empty")]
    pub detected_features: std::collections::HashSet<CpuFeature>,
    
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

// Default implementation for CpuIdentity
impl Default for CpuIdentity {
    fn default() -> Self {
        Self {
            vendor: String::new(),
            brand: String::new(),
            cores: 1,
            logical_cores: None,
            features_bitfield: 0,
            detected_features: std::collections::HashSet::new(),
            cache_size: 0,
            timestamp: 0,
            max_temp_obfuscated: None,
            smt_enabled: None,
            numa_nodes: None,
            max_temp_real: None,
        }
    }
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
    /// Uses BLAKE3 for more efficient and collision-resistant fingerprinting.
    pub fn telemetry_fingerprint(&self) -> String {
        // No need to duplicate hashers - use a single BLAKE3 instance throughout the function
        let mut hasher = blake3::Hasher::new();
        hasher.update(self.vendor.as_bytes());
        hasher.update(&self.cores.to_le_bytes());
        hasher.update(&self.features_bitfield.to_le_bytes());
        hasher.update(&self.cache_size.to_le_bytes());
        if let Some(smt) = self.smt_enabled {
            hasher.update(&[smt as u8]);
        }
        if let Some(numa) = self.numa_nodes {
            hasher.update(&[numa]);
        }
        if let Some(temp) = self.max_temp_obfuscated {
            hasher.update(&[temp]);
        }

        hex::encode(hasher.finalize().as_bytes())
    }
    
    /// Checks if a specific CPU feature is supported
    pub fn has_feature(&self, feature: CpuFeature) -> bool {
        self.detected_features.contains(&feature)
    }
}

/// Detect CPU and handle potential errors
pub fn detect_cpu_safely() -> CpuIdentity {
    match get_cached_cpu_identity() {
        Ok(identity) => identity,
        Err(_) => {
            warn!("CPU detection error. Using fallback values.");
            CpuIdentity::default()
        }
    }
}

// --- Updated mask_cpu_identity function with better documentation ---
/// Creates a masked version of the CPU identity, obfuscating identifying characteristics
/// while preserving enough information for general telemetry.
///
/// # Parameters
/// * `seed` - Optional seed for deterministic masking. If None, cryptographically secure
///            random data combined with timestamp is used.
/// * `real_temp` - Optional real CPU temperature (degrees C). If None, a plausible 
///                 placeholder temperature in the range 80-105°C is generated based on the seed.
///                 Real temperatures should be obtained through platform-specific means 
///                 (e.g., using lm_sensors on Linux or equivalent on other platforms) and
///                 passed to this function; the function itself does NOT read hardware sensors.
///
/// # Returns
/// A masked `CpuIdentity` with obfuscated values.
pub fn mask_cpu_identity(seed: Option<u64>, real_temp: Option<f32>) -> CpuIdentity {
    let mut masked = CpuIdentity::default();
    
    // Use cached CPU identity instead of detecting each time
    let real = match get_cached_cpu_identity() {
        Ok(identity) => identity,
        Err(_) => {
            warn!("Error retrieving CPU identity. Using default identity for masking.");
            CpuIdentity::default()
        }
    };

    // Use provided seed if available, otherwise generate a cryptographically secure seed
    let time_seed = match seed {
        Some(s) => s,
        None => {
            // Combine timestamp with crypto-secure random bytes for stronger seed
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            
            // Create a 64-bit seed by combining time with secure random data
            let mut rng = rand::thread_rng();
            let random_bytes: u64 = rng.gen();
            
            // XOR timestamp with random bytes for better entropy
            timestamp ^ random_bytes
        }
    };
    
    // Ensure masked identity has a timestamp too
    masked.timestamp = time_seed;

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

    // --- Mask identifying features using the enum approach ---
    // Clone the real features to start with
    masked.detected_features = real.detected_features.clone();
    
    // Remove personal features to mask
    for feature in PERSONAL_FEATURES_TO_MASK {
        masked.detected_features.remove(feature);
    }
    
    // Update legacy bitfield for compatibility
    masked.features_bitfield = features_to_bitfield(&masked.detected_features);

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
    // Enhanced NUMA node masking for better privacy
    // Always report exactly 1 regardless of the real count to prevent inferring multi-node systems
    masked.numa_nodes = real.numa_nodes.map(|_| 1);
    
    // Alternative implementation with randomization to make the pattern less predictable:
    // If you'd prefer some randomization instead of always reporting 1, uncomment this:
    /*
    masked.numa_nodes = real.numa_nodes.map(|n| {
        // Always report 1 for single-node systems
        if (n <= 1) {
            1
        } else {
            // For multi-node systems, always report 1 with high probability (90%)
            // This prevents correlation of "if I see 2, it must be multi-node"
            if (time_seed % 10 < 9) {
                1
            } else {
                // Very occasionally report a number ≥2 but uncorrelated with actual count
                // This adds noise to prevent statistical inference
                1 + (time_seed % 2) as u8
            }
        }
    });
    */

    // --- Generate varied vendor string ---
    // Create a larger variety of vendors based on seed
    let vendor_options = [
        "GenuineIntel",
        "AuthenticAMD", 
        "CentaurHauls",
        "GenuineTMx86",
        "HygonGenuine",
        "VIA VIA VIA ",
        "KVMKVMKVM",
        "Microsoft Hv",
        "bhyve bhyve ",
        "Apple Silicon"
    ];
    
    // Use a more varied selection based on seed
    let vendor_index = (time_seed % vendor_options.len() as u64) as usize;
    masked.vendor = vendor_options[vendor_index].to_string();
    
    // --- Generate varied brand string ---
    // Define varied CPU brand strings based on the selected vendor
    let brand_string = match vendor_options[vendor_index] {
        "GenuineIntel" => {
            let intel_brands = [
                format!("Intel(R) Core(TM) i{}-{} CPU @ {:.1f}GHz", 
                        (time_seed % 10) + 3, // i3 to i12
                        (time_seed % 10000) + 1000, // model number 
                        2.0 + (time_seed % 40) as f32 / 10.0), // clock speed 2.0-5.9 GHz
                format!("Intel(R) Xeon(R) E-{} CPU @ {:.1f}GHz",
                        (time_seed % 9000) + 1000, // model number
                        2.2 + (time_seed % 30) as f32 / 10.0), // clock speed 2.2-5.1 GHz
                format!("Intel(R) Pentium(R) Gold G{} CPU @ {:.1f}GHz",
                        (time_seed % 9000) + 1000,
                        2.8 + (time_seed % 20) as f32 / 10.0),
            ];
            intel_brands[(time_seed / 100) as usize % intel_brands.len()]
        },
        "AuthenticAMD" => {
            let amd_brands = [
                format!("AMD Ryzen {} {}-Core Processor",
                        (time_seed % 9) + 3, // Ryzen 3-9
                        masked.cores), // Use masked core count
                format!("AMD Ryzen Threadripper PRO {}{}X {}-Core Processor",
                        (time_seed % 5) + 3, // 3-7
                        (time_seed % 9) + 1, // 1-9
                        masked.cores), // Use masked core count
                format!("AMD EPYC {} {}-Core Processor",
                        (time_seed % 9000) + 7000, // model number
                        masked.cores), // Use masked core count
            ];
            amd_brands[(time_seed / 100) as usize % amd_brands.len()]
        },
        "Apple Silicon" => {
            let apple_brands = [
                format!("Apple M{} {}-Core CPU",
                        (time_seed % 3) + 1, // M1-M3
                        masked.cores), // Use masked core count
                format!("Apple M{} Pro {}-Core CPU",
                        (time_seed % 3) + 1, // M1-M3
                        masked.cores), // Use masked core count
                format!("Apple M{} Max {}-Core CPU",
                        (time_seed % 3) + 1, // M1-M3
                        masked.cores), // Use masked core count
            ];
            apple_brands[(time_seed / 100) as usize % apple_brands.len()]
        },
        "KVMKVMKVM" => {
            format!("Virtual CPU {} {}-Core", 
                   (time_seed % 9000) + 1000, 
                   masked.cores)
        },
        "Microsoft Hv" => {
            format!("Hyper-V Virtual Processor {}-Core", masked.cores)
        },
        _ => {
            // Generic template for other vendors
            format!("{} {}-Core Processor ({}-{})", 
                   masked.vendor, 
                   masked.cores,
                   (time_seed % 1000) + 2000, // model year
                   (time_seed % 500) + 1000)  // model number
        }
    };
    
    masked.brand = brand_string;

    masked.timestamp = time_seed; // Record the seed used for masking

    masked
}

/// Obfuscates a real temperature reading using a seed.
/// If no real temperature is provided, generates a plausible placeholder.
///
/// # Parameters
/// * `real_temp` - Optional real CPU temperature in degrees Celsius.
///                 When None, a synthetic value in range 80-105°C is generated.
/// * `seed` - Random seed value for obfuscation or placeholder generation.
///
/// # Returns
/// * `Option<u8>` - An obfuscated temperature as u8, or None if inputs were invalid.
///                  When based on real_temp, this is the real value with +/-5° noise.
///                  When no real_temp provided, returns 80 + (seed % 26).
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
            // Note: This is an artificial value and not based on actual hardware measurement
            Some(80 + (seed % 26) as u8)
        }
    }
}

// --- Add a new function to help users obtain real temperature data ---
/// Attempts to read the CPU temperature from the system.
/// This is platform-specific and may not work on all systems.
///
/// # Returns
/// * `Result<f32, CpuDetectionError>` - The CPU temperature in degrees Celsius if successful,
///                                     or an error if temperature reading is not supported
///                                     or fails on this platform.
pub fn read_cpu_temperature() -> Result<f32, CpuDetectionError> {
    #[cfg(target_os = "linux")]
    {
        // Try reading from sysfs thermal zone
        match std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp") {
            Ok(temp_str) => {
                let millidegrees = temp_str.trim().parse::<f32>()
                    .map_err(|_| CpuDetectionError::Other("Failed to parse temperature".into()))?;
                return Ok(millidegrees / 1000.0); // Convert to degrees C
            }
            Err(_) => {}
        }
        
        // If that fails, try lm_sensors path
        // This is simplified; real implementation would use hwmon interface or lm_sensors library
        return Err(CpuDetectionError::Other("Temperature reading not implemented".into()));
    }
    
    #[cfg(target_os = "windows")]
    {
        // Windows requires WMI or external libraries like OpenHardwareMonitor
        return Err(CpuDetectionError::Other("Temperature reading not implemented on Windows".into()));
    }
    
    #[cfg(target_os = "macos")]
    {
        // macOS requires SMC access
        return Err(CpuDetectionError::Other("Temperature reading not implemented on macOS".into()));
    }
    
    // Default for other platforms
    Err(CpuDetectionError::Other("Temperature reading not supported on this platform".into()))
}

// --- Added missing feature verification functions ---

/// Verifies that Hyper-Threading Technology (HTT) is actually supported and usable
/// Analyzes platform-specific indicators to confirm HTT is active
fn verify_htt_support() -> bool {
    // Get cached CPU ID to avoid re-detection
    let cpuid_opt = get_cpu_id();
    if let Some(cpuid) = cpuid_opt {
        // Check if HTT is indicated in feature flags
        if let Some(fi) = cpuid.get_feature_info() {
            if !fi.has_htt() {
                return false;
            }
            
            // HTT is flagged, but verify it's actually functional
            if let Some(pi) = try_get_processor_capacity_info(&cpuid) {
                // Compare logical vs physical cores - HTT should mean logical > physical
                if let (Some(phys), Some(log)) = (pi.physical_cores(), pi.logical_cores()) {
                    return log > phys;
                }
            }
            
            // Fallback to topology info if capacity info unavailable
            if let Some(ti) = cpuid.get_topology_info() {
                // Another way to check - if threads > cores
                return ti.num_threads() > ti.num_cores();
            }
            
            // If topology info is unavailable, default to true
            // since feature flag says it's available
            return true;
        }
    }
    
    // Default to false if we can't properly verify
    false
}

/// Verifies that AVX instruction set is properly supported by both CPU and OS
/// This is important because AVX may be supported by CPU but not by OS
/// Processors may claim to support AVX but without proper OS support, code using it will crash
///
/// @param features The set of features to verify/modify
/// @returns Result indicating success or describing why verification failed
fn verify_avx_support(features: &mut std::collections::HashSet<CpuFeature>) -> Result<(), CpuDetectionError> {
    // Get cached CPU ID to avoid re-detection
    let cpuid_opt = get_cpu_id();
    if let Some(cpuid) = cpuid_opt {
        // Check for OSXSAVE feature - critical for AVX support
        if let Some(fi) = cpuid.get_feature_info() {
            if !fi.has_osxsave() {
                // OS doesn't support XSAVE/XRESTORE - AVX can't work
                features.remove(&CpuFeature::AVX);
                features.remove(&CpuFeature::AVX2);
                return Ok(());
            }
            
            // Check for actual AVX OS support using XCR0 if available
            if let Some(xcr0) = get_xcr0_register() {
                // Verify both XMM and YMM state bits are set
                let xmm_supported = (xcr0 & 0x2) != 0;
                let ymm_supported = (xcr0 & 0x4) != 0;
                
                if !xmm_supported || !ymm_supported {
                    // OS doesn't support necessary AVX state saving
                    features.remove(&CpuFeature::AVX);
                    features.remove(&CpuFeature::AVX2);
                    features.remove(&CpuFeature::FMA);
                }
            }
        }
    }
    
    Ok(())
}

/// Reads the XCR0 register if supported
/// This register indicates which features the OS supports for state saving
fn get_xcr0_register() -> Option<u64> {
    // XCR0 can only be read with the XGETBV instruction
    // Safety: This may be unsupported, so we need to catch any potential crashes
    std::panic::catch_unwind(|| {
        // Use inline assembly to execute XGETBV with ECX=0 to read XCR0
        // This is unsafe because it's raw assembly and could crash if not supported
        unsafe {
            let xcr0: u64;
            std::arch::asm!(
                "xor ecx, ecx",
                "xgetbv",
                out("eax") xcr0,
                out("edx") _,
                options(nomem, nostack)
            );
            xcr0
        }
    }).ok()
}

/// Harmonizes detected features with processor capacity info
/// Ensures that feature detection is consistent with processor capabilities
///
/// @param features The set of features to harmonize
/// @param pi The processor capacity info
/// @returns Result indicating success or describing why harmonization failed
fn harmonize_features(
    features: &mut std::collections::HashSet<CpuFeature>,
    pi: raw_cpuid::ProcessorCapacityInfo
) -> Result<(), CpuDetectionError> {
    // Check processor capabilities for specific feature support
    
    // Check for BMI support based on processor capabilities
    // BMI might require specific processor features beyond just the flag
    if !features.contains(&CpuFeature::BMI1) && !features.contains(&CpuFeature::BMI2) {
        // If we don't have BMI features but processor info suggests we should,
        // let's look for additional indicators
        
        // Some BMI features require specific processor generations
        // Here we're simply ensuring consistency
    }
    
    // Check for secure enclave protection
    if features.contains(&CpuFeature::SGX) {
        // Check if SGX is actually available/enabled in processor
        if let Some(sgx_support) = pi.sgx_support() {
            if !sgx_support {
                // SGX is claimed but not actually available according to processor info
                features.remove(&CpuFeature::SGX);
            }
        }
    }
    
    // For certain features, we might need to verify they're fully supported
    // This function can be expanded as needed to handle more complex feature checks
    
    Ok(())
}

// === Unit tests for core detection robustness ===
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timestamp_initialization() {
        let identity = detect_cpu_safely();
        assert!(identity.timestamp > 0, "Timestamp should be initialized to non-zero value");
    }

    #[test]
    fn test_cores_detection() {
        let identity = detect_cpu_safely();
        assert!(identity.cores > 0, "Core count should be at least 1");
        
        if let Some(logical) = identity.logical_cores {
            assert!(logical >= identity.cores, "Logical cores should be >= physical cores");
        }
    }

    #[test]
    fn test_feature_detection() {
        let identity = detect_cpu_safely();
        // Test both ways of checking features
        let has_sse_bitfield = (identity.features_bitfield & (1 << 0)) != 0;
        let has_sse_enum = identity.has_feature(CpuFeature::SSE);
        assert_eq!(has_sse_bitfield, has_sse_enum, 
            "Feature detection should be consistent between bitfield and enum approaches");
    }

    #[test]
    fn test_masking_preserves_core_count() {
        let masked = mask_cpu_identity(None, None);
        assert!(masked.cores > 0, "Masked CPU should have at least 1 core");
    }

    #[test]
    fn test_processor_capacity_info_graceful_failure() {
        // This test verifies that the try_get_processor_capacity_info function 
        // doesn't panic even if the underlying CPUID feature is unavailable
        let cpuid = CpuId::new();
        let _result = try_get_processor_capacity_info(&cpuid);
        // Simply not crashing here is success
    }

    #[test]
    fn test_masked_cpu_identity_variety() {
        // Test that different seeds produce different identities
        let masked1 = mask_cpu_identity(Some(12345), None);
        let masked2 = mask_cpu_identity(Some(67890), None);
        
        // Check brand strings differ
        assert_ne!(masked1.brand, masked2.brand, "Brand strings should be different with different seeds");
        
        // Check seed-less masking for variety (might rarely fail if seeds happen to be identical)
        let unseeded1 = mask_cpu_identity(None, None);
        // Small sleep to ensure different timestamps
        std::thread::sleep(std::time::Duration::from_millis(10));
        let unseeded2 = mask_cpu_identity(None, None);
        
        assert_ne!(unseeded1.timestamp, unseeded2.timestamp, "Auto-generated seeds should differ");
    }
}
