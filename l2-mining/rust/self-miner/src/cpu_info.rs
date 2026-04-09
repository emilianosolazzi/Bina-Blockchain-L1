//! # CPU Identity, Features, and Thermal Monitoring
//!
//! Standalone copy adapted from `temporal_gradient_core::cpu` for the self-miner.
//!
//! Changes from the core version:
//! - `std::sync::OnceLock` instead of `once_cell`
//! - `tracing` instead of `log`
//! - `sha3::Keccak256` instead of `sha2::Sha256` for fingerprint
//! - macOS temperature stubbed (no `smc` crate dependency)

use std::fmt;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use raw_cpuid::{CpuId, CpuIdReader};
use sha3::{Digest, Keccak256};
use tracing::{debug, warn};

// ─────────────────────────────────────────────────────────────────
// Cache
// ─────────────────────────────────────────────────────────────────

static CPU_IDENTITY: OnceLock<CpuIdentity> = OnceLock::new();

static TEMP_EPOCH: OnceLock<Instant> = OnceLock::new();
static LAST_TEMP_US: AtomicU64 = AtomicU64::new(0);
static LAST_TEMP_BITS: AtomicI32 = AtomicI32::new(0);
const TEMP_CACHE_TTL_US: u64 = 1_000_000;

static WARN_FLAGS: AtomicU64 = AtomicU64::new(0);
const WARN_WIN: u64 = 1;
#[cfg(target_os = "linux")]
const WARN_SENSORS: u64 = 8;

// ─────────────────────────────────────────────────────────────────
// CpuIdentity
// ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
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

    pub fn telemetry_fingerprint(&self) -> String {
        let mut h = Keccak256::new();
        h.update(self.vendor.as_bytes());
        h.update(self.brand.as_bytes());
        h.update(self.signature.to_le_bytes());
        h.update(self.cores.to_le_bytes());
        h.update(self.threads.to_le_bytes());
        h.update(self.cache_l3_kb.to_le_bytes());
        for f in &self.features {
            h.update(f.as_bytes());
        }
        format!("{:x}", h.finalize())
    }

    pub fn recommended_workers(&self) -> usize {
        self.cores.max(1) as usize
    }
}

impl fmt::Display for CpuIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} (fam={} mod={} step={}) cores={} threads={} L3={}KB",
            self.vendor, self.brand,
            self.family, self.model, self.stepping,
            self.cores, self.threads, self.cache_l3_kb,
        )
    }
}

// ─────────────────────────────────────────────────────────────────
// Detection
// ─────────────────────────────────────────────────────────────────

pub fn detect_cpu() -> &'static CpuIdentity {
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

        if let Some(v) = cpuid.get_vendor_info() {
            id.vendor = v.as_str().to_string();
        }

        if let Some(b) = cpuid.get_processor_brand_string() {
            id.brand = b.as_str().trim().to_string();
        }

        if let Some(fi) = cpuid.get_feature_info() {
            let base_family = fi.family_id() as u32;
            let base_model = fi.model_id() as u32;
            let ext_family = fi.extended_family_id() as u32;
            let ext_model = fi.extended_model_id() as u32;

            id.family = if base_family == 0xF { base_family + ext_family } else { base_family };
            id.model = if base_family == 0x6 || base_family == 0xF {
                (ext_model << 4) | base_model
            } else {
                base_model
            };
            id.stepping = fi.stepping_id() as u32;
            id.signature = (id.family << 12) | (id.model << 4) | id.stepping;
        }

        id.features = collect_features(&cpuid);

        let (cores, threads) = detect_topology(&cpuid);
        id.cores = cores;
        id.threads = threads;

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
        debug!("CPU detected in {} us: {}", id.detection_us, id);
        id
    })
}

fn collect_features<R: CpuIdReader>(cpuid: &CpuId<R>) -> Vec<String> {
    let mut f = Vec::with_capacity(64);

    if let Some(fi) = cpuid.get_feature_info() {
        let checks: &[(&str, bool)] = &[
            ("fpu", fi.has_fpu()),
            ("tsc", fi.has_tsc()),
            ("msr", fi.has_msr()),
            ("apic", fi.has_apic()),
            ("cmov", fi.has_cmov()),
            ("mmx", fi.has_mmx()),
            ("sse", fi.has_sse()),
            ("sse2", fi.has_sse2()),
            ("sse3", fi.has_sse3()),
            ("pclmulqdq", fi.has_pclmulqdq()),
            ("ssse3", fi.has_ssse3()),
            ("fma", fi.has_fma()),
            ("sse4.1", fi.has_sse41()),
            ("sse4.2", fi.has_sse42()),
            ("aesni", fi.has_aesni()),
            ("avx", fi.has_avx()),
            ("rdrand", fi.has_rdrand()),
            ("htt", fi.has_htt()),
        ];
        for (name, present) in checks {
            if *present {
                f.push(name.to_string());
            }
        }
    }

    if let Some(ef) = cpuid.get_extended_feature_info() {
        let checks: &[(&str, bool)] = &[
            ("avx2", ef.has_avx2()),
            ("sgx", ef.has_sgx()),
            ("avx512f", ef.has_avx512f()),
            ("avx512dq", ef.has_avx512dq()),
            ("avx512bw", ef.has_avx512bw()),
            ("avx512vl", ef.has_avx512vl()),
            ("rdpid", ef.has_rdpid()),
        ];
        for (name, present) in checks {
            if *present {
                f.push(name.to_string());
            }
        }
    }

    f
}

fn detect_topology<R: CpuIdReader>(cpuid: &CpuId<R>) -> (u32, u32) {
    let logical = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1);

    if let Some(topo) = cpuid.get_extended_topology_info() {
        let mut logical_per_core = 0u32;
        let mut logical_per_package = 0u32;
        for level in topo {
            match level.level_type() {
                raw_cpuid::TopologyType::Core => logical_per_package = level.processors() as u32,
                raw_cpuid::TopologyType::SMT => logical_per_core = level.processors() as u32,
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
#[allow(dead_code)]
pub enum CpuFeature {
    SSE, SSE2, SSE3, SSSE3, SSE41, SSE42,
    AVX, AVX2, AVX512F, AVX512BW,
    FMA, AES, RDRAND, PCLMULQDQ, SGX,
}

#[allow(dead_code)]
pub fn has_cpu_feature(feature: CpuFeature) -> bool {
    let tag = match feature {
        CpuFeature::SSE => "sse",
        CpuFeature::SSE2 => "sse2",
        CpuFeature::SSE3 => "sse3",
        CpuFeature::SSSE3 => "ssse3",
        CpuFeature::SSE41 => "sse4.1",
        CpuFeature::SSE42 => "sse4.2",
        CpuFeature::AVX => "avx",
        CpuFeature::AVX2 => "avx2",
        CpuFeature::AVX512F => "avx512f",
        CpuFeature::AVX512BW => "avx512bw",
        CpuFeature::FMA => "fma",
        CpuFeature::AES => "aesni",
        CpuFeature::RDRAND => "rdrand",
        CpuFeature::PCLMULQDQ => "pclmulqdq",
        CpuFeature::SGX => "sgx",
    };
    detect_cpu().features.iter().any(|f| f == tag)
}

// ─────────────────────────────────────────────────────────────────
// Temperature — public entry point
// ─────────────────────────────────────────────────────────────────

pub fn get_cpu_temperature() -> Option<f32> {
    let epoch = TEMP_EPOCH.get_or_init(Instant::now);
    let now_us = epoch.elapsed().as_micros() as u64;

    let last = LAST_TEMP_US.load(Ordering::Relaxed);
    if last > 0 && now_us.saturating_sub(last) < TEMP_CACHE_TTL_US {
        let bits = LAST_TEMP_BITS.load(Ordering::Relaxed) as u32;
        return Some(f32::from_bits(bits));
    }

    let temp = platform_temperature();

    if let Some(t) = temp {
        LAST_TEMP_US.store(now_us.max(1), Ordering::Relaxed);
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
    if let Some(t) = linux_hwmon_package() {
        return Some(t);
    }

    let thermal_paths = [
        "/sys/class/thermal/thermal_zone0/temp",
        "/sys/class/thermal/thermal_zone1/temp",
        "/sys/class/thermal/thermal_zone2/temp",
    ];
    for path in &thermal_paths {
        if let Ok(raw) = std::fs::read_to_string(path) {
            if let Ok(v) = raw.trim().parse::<u32>() {
                let t = v as f32 / 1000.0;
                if plausible(t) {
                    return Some(t);
                }
            }
        }
    }

    linux_sensors_cli()
}

#[cfg(target_os = "linux")]
fn linux_hwmon_package() -> Option<f32> {
    let hwmon_base = "/sys/class/hwmon";
    let entries = std::fs::read_dir(hwmon_base).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();
        let name_path = path.join("name");
        if let Ok(name) = std::fs::read_to_string(&name_path) {
            let name = name.trim();
            if name == "coretemp" || name == "k10temp" || name == "zenpower" {
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
    for i in 1u32..=32 {
        let label_path = hwmon_path.join(format!("temp{}_label", i));
        if let Ok(label) = std::fs::read_to_string(&label_path) {
            let label = label.trim().to_lowercase();
            if label.contains("package") || label.contains("tdie") || label.contains("tccd") {
                let input_path = hwmon_path.join(format!("temp{}_input", i));
                if let Ok(raw) = std::fs::read_to_string(&input_path) {
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
                warn!("lm-sensors not available ({e}). Install for better temp support.");
            }
            return None;
        }
    };

    let text = String::from_utf8_lossy(&out.stdout);
    let mut max_temp: Option<f32> = None;

    for line in text.lines() {
        let lower = line.to_lowercase();
        if (lower.contains("core") || lower.contains("package")) && line.contains('°') {
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
    windows_wmi_temperature()
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
    debug!("Windows {} temperature sensor reported {:.1} C", namespace, temperature);
    Some(temperature)
}

#[cfg(target_os = "windows")]
fn windows_wmi_temperature() -> Option<f32> {
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

    if let Ok(zones) = wmi.query::<ThermalZone>() {
        let temps: Vec<f32> = zones
            .iter()
            .map(|z| z.current_temperature as f32 / 10.0 - 273.15)
            .filter(|&t| plausible(t))
            .collect();
        if !temps.is_empty() {
            return temps.iter().cloned().reduce(f32::max);
        }
    }

    if let Some(temp) = openhardwaremonitor_temperature("ROOT\\OpenHardwareMonitor") {
        return Some(temp);
    }

    if let Some(temp) = openhardwaremonitor_temperature("ROOT\\LibreHardwareMonitor") {
        return Some(temp);
    }

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
        warn!(
            "Windows: no temperature sensor found via WMI/OpenHardwareMonitor. \
             Install and run OpenHardwareMonitor or LibreHardwareMonitor for CPU temperature support."
        );
    }
    None
}

// ─────────────────────────────────────────────────────────────────
// macOS temperature (stubbed — no smc crate dependency)
// ─────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn platform_temperature() -> Option<f32> {
    None
}

// ─────────────────────────────────────────────────────────────────
// Other platforms
// ─────────────────────────────────────────────────────────────────

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn platform_temperature() -> Option<f32> {
    None
}
