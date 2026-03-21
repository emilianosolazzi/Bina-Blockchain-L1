//! # CPU Identity, Features, and Thermal Monitoring
//!
//! Cross-platform CPU detection for the Temporal Gradient miner.
//!
//! Provides:
//! - CPU vendor, brand, family/model/stepping
//! - Instruction set feature detection (SSE, AVX, AES, etc.)
//! - Physical core and logical thread counts
//! - L3 cache size
//! - Real CPU temperature on Linux, Windows, and macOS
//! - Privacy-preserving identity masking for peer relay profiles
//! - SHA-256 telemetry fingerprint
//!
//! ## Platform temperature sources
//!
//! | Platform | Primary                        | Fallback                                   |
//! |----------|--------------------------------|--------------------------------------------|
//! | Linux    | `/sys/class/thermal/…`         | `lm-sensors` CLI                           |
//! | Windows  | `MSAcpi_ThermalZoneTemperature`| OpenHardwareMonitor / LibreHardwareMonitor |
//! | macOS    | SMC key `TC0P` via IOKit       | `TC0D`, `TC0E` keys                        |
//!
//! ## Usage
//!
//! ```rust
//! use temporal_gradient_core::cpu::{
//!     detect_cpu_safely,
//!     get_cpu_temperature,
//!     has_cpu_feature,
//!     CpuFeature,
//! };
//!
//! let cpu = detect_cpu_safely();
//! println!("{}", cpu);                            // full info
//! println!("{}", cpu.telemetry_fingerprint());    // entropy-safe hash
//!
//! if let Some(t) = get_cpu_temperature() {
//!     println!("CPU temp: {:.1}°C", t);
//! }
//!
//! if has_cpu_feature(CpuFeature::AVX2) {
//!     // use AVX2-accelerated path
//! }
//! ```

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::time::Instant;

use once_cell::sync::OnceCell;
use raw_cpuid::{CpuId, CpuIdReader};
use sha2::{Digest, Sha256};
use log::{debug, warn};

// ─────────────────────────────────────────────────────────────────
// Cache
// ─────────────────────────────────────────────────────────────────

static CPU_IDENTITY: OnceCell<CpuIdentity> = OnceCell::new();

/// Temperature cache — avoids hammering sensors on every telemetry tick.
static LAST_TEMP_TIME_MS: AtomicU64 = AtomicU64::new(0);
/// Stores `f32::to_bits()` as `i32` (same bit width) for atomic storage.
static LAST_TEMP_BITS: AtomicI32 = AtomicI32::new(0);
const TEMP_CACHE_TTL_MS: u64 = 1_000;

/// Per-platform warning gate — each bit guards one platform warning.
static WARN_FLAGS: AtomicU64 = AtomicU64::new(0);
const WARN_WIN: u64 = 1;
#[cfg(target_os = "macos")]
const WARN_MAC: u64 = 2;
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
const WARN_OTHER: u64 = 4;
#[cfg(target_os = "linux")]
const WARN_SENSORS: u64 = 8;

// ─────────────────────────────────────────────────────────────────
// CpuIdentity
// ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CpuIdentity {
    pub vendor: String,
    pub brand: String,
    pub family: u32,
    pub model: u32,
    pub stepping: u32,
    pub signature: u32,
    pub features: Vec<String>,
    pub cores: u32,
    pub threads: u32,
    pub cache_l3_kb: u32,
    pub detection_us: u64,
}

impl CpuIdentity {
    pub fn display_name(&self) -> String {
        format!("{} {}", self.vendor, self.brand)
    }

    /// Deterministic SHA-256 fingerprint of stable CPU attributes.
    /// Safe to include in telemetry — does not contain timing side channels.
    pub fn telemetry_fingerprint(&self) -> String {
        let mut h = Sha256::new();
        h.update(self.vendor.as_bytes());
        h.update(self.brand.as_bytes());
        h.update(self.signature.to_le_bytes());
        h.update(self.cores.to_le_bytes());
        h.update(self.threads.to_le_bytes());
        for f in &self.features {
            h.update(f.as_bytes());
        }
        format!("{:x}", h.finalize())
    }

    /// Recommended number of mining worker threads for this CPU.
    /// Uses physical cores to avoid hyperthreading contention on the
    /// memory-intensive QR hash computation.
    pub fn recommended_workers(&self) -> usize {
        self.cores.max(1) as usize
    }
}

impl fmt::Display for CpuIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} (fam={} mod={} step={}) cores={} threads={} L3={}KB",
            self.vendor,
            self.brand,
            self.family,
            self.model,
            self.stepping,
            self.cores,
            self.threads,
            self.cache_l3_kb,
        )
    }
}

// ─────────────────────────────────────────────────────────────────
// Detection
// ─────────────────────────────────────────────────────────────────

/// Detect CPU identity once and cache the result for the process lifetime.
pub fn detect_cpu_safely() -> &'static CpuIdentity {
    CPU_IDENTITY.get_or_init(|| {
        let start = Instant::now();
        let cpuid = CpuId::new();
        let mut id = CpuIdentity {
            vendor: String::new(),
            brand: String::new(),
            family: 0,
            model: 0,
            stepping: 0,
            signature: 0,
            features: Vec::new(),
            cores: 1,
            threads: 1,
            cache_l3_kb: 0,
            detection_us: 0,
        };

        // Vendor string
        if let Some(v) = cpuid.get_vendor_info() {
            id.vendor = v.as_str().to_string();
        }

        // Brand string
        if let Some(b) = cpuid.get_processor_brand_string() {
            id.brand = b.as_str().trim().to_string();
        }

        // Family / model / stepping / signature
        if let Some(fi) = cpuid.get_feature_info() {
            id.family = fi.family_id() as u32;
            id.model = fi.model_id() as u32;
            id.stepping = fi.stepping_id() as u32;
            id.signature = (id.family << 8) | (id.model << 4) | id.stepping;
        }

        // Feature flags — collect all known ones
        id.features = collect_features(&cpuid);

        // Core / thread topology
        let (cores, threads) = detect_topology(&cpuid);
        id.cores   = cores;
        id.threads = threads;

        // L3 cache
        if let Some(cache_iter) = cpuid.get_cache_parameters() {
            for c in cache_iter {
                if c.level() == 3 {
                    let bytes = c.associativity()
                        * c.physical_line_partitions()
                        * c.coherency_line_size()
                        * c.sets();
                    id.cache_l3_kb += (bytes / 1024) as u32;
                }
            }
        }

        id.detection_us = start.elapsed().as_micros() as u64;
        debug!("CPU detected in {} µs: {}", id.detection_us, id);
        id
    })
}

fn collect_features<R: CpuIdReader>(cpuid: &CpuId<R>) -> Vec<String> {
    let mut f = Vec::with_capacity(64);

    if let Some(fi) = cpuid.get_feature_info() {
        let checks: &[(&str, bool)] = &[
            ("fpu",   fi.has_fpu()),
            ("tsc",   fi.has_tsc()),
            ("msr",   fi.has_msr()),
            ("apic",  fi.has_apic()),
            ("cmov",  fi.has_cmov()),
            ("mmx",   fi.has_mmx()),
            ("sse",   fi.has_sse()),
            ("sse2",  fi.has_sse2()),
            ("sse3",  fi.has_sse3()),
            ("pclmulqdq", fi.has_pclmulqdq()),
            ("ssse3", fi.has_ssse3()),
            ("fma", fi.has_fma()),
            ("sse4.1", fi.has_sse41()),
            ("sse4.2", fi.has_sse42()),
            ("aesni", fi.has_aesni()),
            ("avx", fi.has_avx()),
            ("rdrand", fi.has_rdrand()),
            ("htt",   fi.has_htt()),
        ];
        for (name, present) in checks {
            if *present { f.push(name.to_string()); }
        }
    }

    if let Some(ef) = cpuid.get_extended_feature_info() {
        let checks: &[(&str, bool)] = &[
            ("avx2",      ef.has_avx2()),
            ("sgx",       ef.has_sgx()),
            ("avx512f",   ef.has_avx512f()),
            ("avx512dq",  ef.has_avx512dq()),
            ("avx512bw",  ef.has_avx512bw()),
            ("avx512vl",  ef.has_avx512vl()),
            ("rdpid",     ef.has_rdpid()),
        ];
        for (name, present) in checks {
            if *present { f.push(name.to_string()); }
        }
    }

    f
}

fn detect_topology<R: CpuIdReader>(cpuid: &CpuId<R>) -> (u32, u32) {
    let logical = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1);

    // Try extended topology leaf first (most accurate)
    if let Some(topo) = cpuid.get_extended_topology_info() {
        let mut logical_per_core = 0u32;
        let mut logical_per_package = 0u32;
        for level in topo {
            match level.level_type() {
                raw_cpuid::TopologyType::Core => logical_per_package = level.processors() as u32,
                raw_cpuid::TopologyType::SMT  => logical_per_core = level.processors() as u32,
                _ => {}
            }
        }

        let threads = logical.max(logical_per_package).max(1);
        if logical_per_package > 0 && logical_per_core > 0 {
            let cores = (logical_per_package / logical_per_core).max(1).min(threads);
            return (cores, threads);
        }

        if logical_per_package > 0 {
            return (logical_per_package.min(threads), threads);
        }
    }

    // Assume SMT-2 if HTT bit is set, otherwise cores == threads
    let has_htt = cpuid
        .get_feature_info()
        .map(|fi| fi.has_htt())
        .unwrap_or(false);

    if has_htt && logical > 1 {
        (logical / 2, logical)
    } else {
        (logical, logical)
    }
}

// ─────────────────────────────────────────────────────────────────
// Feature query
// ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuFeature {
    SSE, SSE2, SSE3, SSSE3, SSE41, SSE42,
    AVX, AVX2, AVX512F, AVX512BW,
    FMA, AES, RDRAND, PCLMULQDQ, SGX,
}

pub fn has_cpu_feature(feature: CpuFeature) -> bool {
    let tag = match feature {
        CpuFeature::SSE      => "sse",
        CpuFeature::SSE2     => "sse2",
        CpuFeature::SSE3     => "sse3",
        CpuFeature::SSSE3    => "ssse3",
        CpuFeature::SSE41    => "sse4.1",
        CpuFeature::SSE42    => "sse4.2",
        CpuFeature::AVX      => "avx",
        CpuFeature::AVX2     => "avx2",
        CpuFeature::AVX512F  => "avx512f",
        CpuFeature::AVX512BW => "avx512bw",
        CpuFeature::FMA      => "fma",
        CpuFeature::AES      => "aesni",
        CpuFeature::RDRAND   => "rdrand",
        CpuFeature::PCLMULQDQ=> "pclmulqdq",
        CpuFeature::SGX      => "sgx",
    };
    detect_cpu_safely().features.iter().any(|f| f == tag)
}

// ─────────────────────────────────────────────────────────────────
// Temperature — public entry point
// ─────────────────────────────────────────────────────────────────

/// Read the current CPU package temperature in °C.
/// Returns `None` if the platform does not expose a sensor or the
/// reading is outside the plausible range (1 – 150 °C).
///
/// Results are cached for 1 second to avoid hammering the sensor
/// on every telemetry tick.
pub fn get_cpu_temperature() -> Option<f32> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let last = LAST_TEMP_TIME_MS.load(Ordering::Relaxed);
    if now_ms.saturating_sub(last) < TEMP_CACHE_TTL_MS {
        let bits = LAST_TEMP_BITS.load(Ordering::Relaxed) as u32;
        return Some(f32::from_bits(bits));
    }

    let temp = platform_temperature();

    if let Some(t) = temp {
        LAST_TEMP_TIME_MS.store(now_ms, Ordering::Relaxed);
        LAST_TEMP_BITS.store(t.to_bits() as i32, Ordering::Relaxed);
    }

    temp
}

#[inline]
fn plausible(t: f32) -> bool {
    t > 1.0 && t < 150.0
}

// ─────────────────────────────────────────────────────────────────
// Linux temperature
// ─────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn platform_temperature() -> Option<f32> {
    use std::fs;
    use std::io::Read;

    // 1. Preferred: hwmon coretemp package reading
    if let Some(t) = linux_hwmon_package() {
        return Some(t);
    }

    // 2. thermal_zone files
    let thermal_paths = [
        "/sys/class/thermal/thermal_zone0/temp",
        "/sys/class/thermal/thermal_zone1/temp",
        "/sys/class/thermal/thermal_zone2/temp",
    ];
    for path in &thermal_paths {
        if let Ok(raw) = fs::read_to_string(path) {
            if let Ok(v) = raw.trim().parse::<u32>() {
                let t = v as f32 / 1000.0;
                if plausible(t) {
                    return Some(t);
                }
            }
        }
    }

    // 3. lm-sensors fallback
    linux_sensors_cli()
}

#[cfg(target_os = "linux")]
fn linux_hwmon_package() -> Option<f32> {
    use std::fs;

    // Walk /sys/class/hwmon/hwmon* looking for coretemp
    let hwmon_base = "/sys/class/hwmon";
    let entries = fs::read_dir(hwmon_base).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();
        // Check name file to find coretemp or k10temp (AMD)
        let name_path = path.join("name");
        if let Ok(name) = fs::read_to_string(&name_path) {
            let name = name.trim();
            if name == "coretemp" || name == "k10temp" || name == "zenpower" {
                // Look for temp*_label containing "Package" or "Tdie"
                if let Some(t) = read_hwmon_package_temp(&path) {
                    return Some(t);
                }
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn read_hwmon_package_temp(hwmon_path: &std::path::Path) -> Option<f32> {
    use std::fs;

    for i in 1u32..=32 {
        let label_path = hwmon_path.join(format!("temp{}_label", i));
        if let Ok(label) = fs::read_to_string(&label_path) {
            let label = label.trim().to_lowercase();
            if label.contains("package") || label.contains("tdie") || label.contains("tccd") {
                let input_path = hwmon_path.join(format!("temp{}_input", i));
                if let Ok(raw) = fs::read_to_string(&input_path) {
                    if let Ok(v) = raw.trim().parse::<u32>() {
                        let t = v as f32 / 1000.0;
                        if plausible(t) {
                            return Some(t);
                        }
                    }
                }
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn linux_sensors_cli() -> Option<f32> {
    use std::process::{Command, Stdio};

    let out = Command::new("sensors")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    let out = match out {
        Ok(o) if o.status.success() => o,
        Ok(_) => return None,
        Err(e) => {
            let flag = WARN_FLAGS.fetch_or(WARN_SENSORS, Ordering::Relaxed);
            if flag & WARN_SENSORS == 0 {
                warn!("lm-sensors not available ({}). Install for better temp support.", e);
            }
            return None;
        }
    };

    let text = String::from_utf8_lossy(&out.stdout);
    let mut max_temp: Option<f32> = None;

    for line in text.lines() {
        // Match lines like "Core 0:  +52.0°C" or "Package id 0: +61.0°C"
        let lower = line.to_lowercase();
        if (lower.contains("core") || lower.contains("package"))
            && line.contains('°')
        {
            if let Some(part) = line.split(':').nth(1) {
                if let Some(temp_part) = part.split('°').next() {
                    if let Ok(t) = temp_part.trim().trim_start_matches('+').parse::<f32>() {
                        if plausible(t) {
                            max_temp = Some(match max_temp {
                                Some(prev) => prev.max(t),
                                None => t,
                            });
                        }
                    }
                }
            }
        }
    }

    max_temp
}

// ─────────────────────────────────────────────────────────────────
// Windows temperature
// ─────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn platform_temperature() -> Option<f32> {
    // Try WMI MSAcpi first, fall back to PDH perf counters
    windows_wmi_temperature()
        .or_else(windows_pdh_temperature)
}

#[cfg(target_os = "windows")]
#[derive(serde::Deserialize, Debug)]
struct OpenHardwareSensor {
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "SensorType")]
    sensor_type: Option<String>,
    #[serde(rename = "Value")]
    value: Option<f32>,
}

#[cfg(target_os = "windows")]
fn select_openhardware_temperature(sensors: Vec<OpenHardwareSensor>) -> Option<f32> {
    let mut preferred = Vec::new();
    let mut fallback = Vec::new();

    for sensor in sensors {
        let value = sensor.value?;
        if !plausible(value) {
            continue;
        }

        let sensor_type = sensor.sensor_type.unwrap_or_default();
        if !sensor_type.eq_ignore_ascii_case("temperature") {
            continue;
        }

        let lower_name = sensor.name.unwrap_or_default().to_ascii_lowercase();
        if lower_name.contains("cpu")
            || lower_name.contains("package")
            || lower_name.contains("tdie")
            || lower_name.contains("tctl")
            || lower_name.contains("core")
        {
            preferred.push(value);
        } else {
            fallback.push(value);
        }
    }

    preferred
        .into_iter()
        .reduce(f32::max)
        .or_else(|| fallback.into_iter().reduce(f32::max))
}

#[cfg(target_os = "windows")]
fn openhardwaremonitor_temperature(namespace: &str) -> Option<f32> {
    use wmi::{COMLibrary, WMIConnection};

    let com = COMLibrary::new().ok()?;
    let wmi = WMIConnection::with_namespace_path(namespace, com).ok()?;
    let query = "SELECT Name, SensorType, Value FROM Sensor WHERE SensorType = 'Temperature'";
    let sensors = wmi.raw_query::<OpenHardwareSensor>(query).ok()?;
    let temperature = select_openhardware_temperature(sensors)?;
    debug!("Windows {} temperature sensor reported {:.1}°C", namespace, temperature);
    Some(temperature)
}

#[cfg(target_os = "windows")]
fn windows_wmi_temperature() -> Option<f32> {
    // WMI via the `wmi` crate (pure Rust, no raw unsafe COM needed).
    // The `wmi` crate wraps COM initialization and query execution safely.
    //
    // Cargo.toml:
    //   wmi = "0.13"
    //   serde = { version = "1", features = ["derive"] }

    use wmi::{COMLibrary, WMIConnection};
    use serde::Deserialize;

    #[derive(Deserialize, Debug)]
    #[serde(rename = "MSAcpi_ThermalZoneTemperature")]
    struct ThermalZone {
        #[serde(rename = "CurrentTemperature")]
        current_temperature: u32,
    }

    #[derive(Deserialize, Debug)]
    #[serde(rename = "Win32_TemperatureProbe")]
    struct TempProbe {
        #[serde(rename = "CurrentReading")]
        current_reading: Option<i32>,
    }

    // Initialise COM — safe to call multiple times (COM ref-counts).
    let com = match COMLibrary::new() {
        Ok(c) => c,
        Err(e) => {
            debug!("WMI COM init failed: {e}");
            return None;
        }
    };

    let wmi = match WMIConnection::with_namespace_path("ROOT\\WMI", com) {
        Ok(w) => w,
        Err(e) => {
            debug!("WMI connect failed: {e}");
            return None;
        }
    };

    // Primary: MSAcpi_ThermalZoneTemperature (tenths of Kelvin)
    if let Ok(zones) = wmi.query::<ThermalZone>() {
        let temps: Vec<f32> = zones
            .iter()
            .map(|z| z.current_temperature as f32 / 10.0 - 273.15)
            .filter(|&t| plausible(t))
            .collect();
        if !temps.is_empty() {
            // Return the maximum (hottest zone = CPU package)
            return temps.iter().cloned().reduce(f32::max);
        }
    }

    if let Some(temp) = openhardwaremonitor_temperature("ROOT\\OpenHardwareMonitor") {
        return Some(temp);
    }

    if let Some(temp) = openhardwaremonitor_temperature("ROOT\\LibreHardwareMonitor") {
        return Some(temp);
    }

    // Fallback: Win32_TemperatureProbe (tenths of degrees Celsius directly)
    let com2 = COMLibrary::new().ok()?;
    let wmi2 = WMIConnection::new(com2).ok()?;
    if let Ok(probes) = wmi2.query::<TempProbe>() {
        let temps: Vec<f32> = probes
            .iter()
            .filter_map(|p| p.current_reading)
            .map(|v| v as f32 / 10.0)
            .filter(|&t| plausible(t))
            .collect();
        if !temps.is_empty() {
            return temps.iter().cloned().reduce(f32::max);
        }
    }

    let flag = WARN_FLAGS.fetch_or(WARN_WIN, Ordering::Relaxed);
    if flag & WARN_WIN == 0 {
        warn!("Windows: no temperature sensor found via WMI/OpenHardwareMonitor. Install and run OpenHardwareMonitor or LibreHardwareMonitor for better CPU temperature support.");
    }
    None
}

#[cfg(target_os = "windows")]
fn windows_pdh_temperature() -> Option<f32> {
    debug!("Windows PDH fallback not enabled in this build; using WMI-only temperature detection");
    None
}

// ─────────────────────────────────────────────────────────────────
// macOS temperature — SMC via IOKit
// ─────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn platform_temperature() -> Option<f32> {
    macos_smc_temperature()
}

#[cfg(target_os = "macos")]
fn macos_smc_temperature() -> Option<f32> {
    // Read Apple SMC (System Management Controller) temperature sensors.
    //
    // SMC keys for CPU temperature:
    //   TC0P  — CPU proximity (most common on Intel)
    //   TC0D  — CPU die
    //   TC0E  — CPU PECI
    //   Tp09  — Apple Silicon efficiency cluster
    //   Tp0P  — Apple Silicon performance cluster
    //
    // We use the `smc` crate which provides safe bindings to IOKit SMC.
    //
    // Cargo.toml:
    //   smc = "0.2"

    use smc::{SMCConnection, Key};

    let conn = match SMCConnection::new("AppleSMC") {
        Ok(c) => c,
        Err(e) => {
            let flag = WARN_FLAGS.fetch_or(WARN_MAC, Ordering::Relaxed);
            if flag & WARN_MAC == 0 {
                warn!("macOS SMC connection failed: {e}. Temperature unavailable.");
            }
            return None;
        }
    };

    // Try CPU keys in priority order
    // Intel Macs: TC0P is package proximity (best single value)
    // Apple Silicon: Tp09 / Tp0P for cluster temps
    let keys = ["TC0P", "TC0D", "TC0E", "Tp09", "Tp0P", "TCXC"];

    for key_str in &keys {
        if let Ok(key) = Key::new(key_str) {
            if let Ok(val) = conn.read_key(&key) {
                if let Ok(t) = val.as_f32() {
                    if plausible(t) {
                        debug!("SMC key {} = {:.1}°C", key_str, t);
                        return Some(t);
                    }
                }
            }
        }
    }

    // Apple Silicon fallback: read all Tp* keys and average
    let mut sum = 0.0f32;
    let mut count = 0u32;
    for suffix in 0u8..16 {
        let key_str = format!("Tp{:02X}", suffix);
        if let Ok(key) = Key::new(&key_str) {
            if let Ok(val) = conn.read_key(&key) {
                if let Ok(t) = val.as_f32() {
                    if plausible(t) {
                        sum += t;
                        count += 1;
                    }
                }
            }
        }
    }

    if count > 0 {
        return Some(sum / count as f32);
    }

    let flag = WARN_FLAGS.fetch_or(WARN_MAC, Ordering::Relaxed);
    if flag & WARN_MAC == 0 {
        warn!("macOS: no valid SMC temperature key found. Is this Apple Silicon?");
    }
    None
}

// ─────────────────────────────────────────────────────────────────
// Other platforms
// ─────────────────────────────────────────────────────────────────

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn platform_temperature() -> Option<f32> {
    let flag = WARN_FLAGS.fetch_or(WARN_OTHER, Ordering::Relaxed);
    if flag & WARN_OTHER == 0 {
        warn!("Temperature monitoring not implemented for this platform.");
    }
    None
}

// ─────────────────────────────────────────────────────────────────
// Masking — privacy-preserving peer relay identity
// ─────────────────────────────────────────────────────────────────

/// Controls how much CPU detail is exposed in relay/peer profiles.
#[derive(Debug, Clone)]
pub struct MaskingConfig {
    /// Percentage of brand string to keep (0–100).
    pub brand_retention_pct: f32,
    /// Round core counts to nearest power of two.
    pub round_cores_power_of_two: bool,
    /// Round L3 cache to nearest MB.
    pub round_cache_mb: bool,
    /// Strip uncommon features, keep only SSE/AVX/AES.
    pub strip_advanced_features: bool,
}

impl Default for MaskingConfig {
    fn default() -> Self {
        Self {
            brand_retention_pct: 50.0,
            round_cores_power_of_two: true,
            round_cache_mb: true,
            strip_advanced_features: false,
        }
    }
}

/// Return a privacy-masked clone of the CPU identity.
/// Used when building the relay profile JSON for peer discovery.
pub fn mask_cpu_identity(config: Option<&MaskingConfig>) -> CpuIdentity {
    let default = MaskingConfig::default();
    let cfg = config.unwrap_or(&default);
    let cpu = detect_cpu_safely();
    let mut m = cpu.clone();

    // Brand string — keep first N chars, pad rest with '*'
    let keep = ((cpu.brand.len() as f32 * cfg.brand_retention_pct / 100.0) as usize)
        .min(cpu.brand.len());
    if keep < cpu.brand.len() {
        m.brand = format!("{}{}", &cpu.brand[..keep], "*".repeat(cpu.brand.len() - keep));
    }

    // Core count — round to power of two
    if cfg.round_cores_power_of_two {
        m.cores   = prev_power_of_two(cpu.cores).max(1);
        m.threads = prev_power_of_two(cpu.threads).max(1);
    }

    // L3 cache — round to MB
    if cfg.round_cache_mb {
        m.cache_l3_kb = (cpu.cache_l3_kb / 1024) * 1024;
    }

    // Features — strip to a small well-known set
    if cfg.strip_advanced_features {
        let keep_set = ["fpu", "sse", "sse2", "avx", "avx2", "aesni", "rdrand"];
        m.features.retain(|f| keep_set.contains(&f.as_str()));
    }

    m
}

fn prev_power_of_two(n: u32) -> u32 {
    if n == 0 { return 0; }
    1u32 << (31 - n.leading_zeros())
}

// ─────────────────────────────────────────────────────────────────
// Performance metrics
// ─────────────────────────────────────────────────────────────────

pub fn detection_metrics() -> HashMap<String, u64> {
    let cpu = detect_cpu_safely();
    let mut m = HashMap::new();
    m.insert("detection_us".into(),      cpu.detection_us);
    m.insert("features_count".into(),    cpu.features.len() as u64);
    m.insert("cores".into(),             cpu.cores as u64);
    m.insert("threads".into(),           cpu.threads as u64);
    m.insert("cache_l3_kb".into(),       cpu.cache_l3_kb as u64);
    m
}

// ─────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_non_empty_vendor() {
        let cpu = detect_cpu_safely();
        assert!(!cpu.vendor.is_empty(), "vendor must not be empty");
    }

    #[test]
    fn detect_returns_non_empty_brand() {
        let cpu = detect_cpu_safely();
        assert!(!cpu.brand.is_empty(), "brand must not be empty");
    }

    #[test]
    fn cores_at_least_one() {
        let cpu = detect_cpu_safely();
        assert!(cpu.cores >= 1);
    }

    #[test]
    fn threads_gte_cores() {
        let cpu = detect_cpu_safely();
        assert!(cpu.threads >= cpu.cores);
    }

    #[test]
    fn recommended_workers_gte_one() {
        assert!(detect_cpu_safely().recommended_workers() >= 1);
    }

    #[test]
    fn fingerprint_is_deterministic() {
        let a = detect_cpu_safely().telemetry_fingerprint();
        let b = detect_cpu_safely().telemetry_fingerprint();
        assert_eq!(a, b);
    }

    #[test]
    fn fingerprint_is_hex_64_chars() {
        let fp = detect_cpu_safely().telemetry_fingerprint();
        assert_eq!(fp.len(), 64);
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn temperature_in_range_if_present() {
        if let Some(t) = get_cpu_temperature() {
            assert!(t > 1.0 && t < 150.0, "temp={} out of range", t);
        }
        // Returning None on CI/VMs is acceptable
    }

    #[test]
    fn temperature_cached_on_second_call() {
        // Two rapid calls should return identical values (cached)
        let t1 = get_cpu_temperature();
        let t2 = get_cpu_temperature();
        assert_eq!(t1, t2, "second call should hit cache");
    }

    #[test]
    fn has_feature_sse2_on_x86() {
        // Every x86_64 CPU manufactured since 2003 has SSE2
        #[cfg(target_arch = "x86_64")]
        assert!(has_cpu_feature(CpuFeature::SSE2), "SSE2 expected on x86_64");
    }

    #[test]
    fn masking_shortens_brand() {
        let cfg = MaskingConfig {
            brand_retention_pct: 0.0,
            ..Default::default()
        };
        let masked = mask_cpu_identity(Some(&cfg));
        let cpu = detect_cpu_safely();
        // All chars should be '*'
        assert!(masked.brand.chars().all(|c| c == '*'),
            "fully masked brand should be all stars, got: {}", masked.brand);
        assert_eq!(masked.brand.len(), cpu.brand.len());
    }

    #[test]
    fn masking_100pct_preserves_brand() {
        let cfg = MaskingConfig {
            brand_retention_pct: 100.0,
            round_cores_power_of_two: false,
            round_cache_mb: false,
            strip_advanced_features: false,
        };
        let masked = mask_cpu_identity(Some(&cfg));
        let cpu = detect_cpu_safely();
        assert_eq!(masked.brand, cpu.brand);
    }

    #[test]
    fn prev_power_of_two_correct() {
        assert_eq!(prev_power_of_two(0), 0);
        assert_eq!(prev_power_of_two(1), 1);
        assert_eq!(prev_power_of_two(3), 2);
        assert_eq!(prev_power_of_two(4), 4);
        assert_eq!(prev_power_of_two(6), 4);
        assert_eq!(prev_power_of_two(8), 8);
        assert_eq!(prev_power_of_two(12), 8);
        assert_eq!(prev_power_of_two(16), 16);
    }

    #[test]
    fn detection_metrics_returns_expected_keys() {
        let m = detection_metrics();
        assert!(m.contains_key("detection_us"));
        assert!(m.contains_key("cores"));
        assert!(m.contains_key("threads"));
        assert!(m.contains_key("cache_l3_kb"));
    }

    #[test]
    fn display_trait_non_empty() {
        let s = format!("{}", detect_cpu_safely());
        assert!(!s.is_empty());
    }
}