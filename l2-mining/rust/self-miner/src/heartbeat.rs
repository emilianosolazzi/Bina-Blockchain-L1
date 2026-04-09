use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use temporal_gradient_core::TelemetrySnapshot;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::cpu_info;
use crate::entropy_scorer::EntropyQualityScorer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertType {
    TelemetryStale,
    MinerNotRunning,
    HashrateDrop,
    TemperatureWarn,
    TemperatureCritical,
    RansomwareDetected,
    ProtectedAssetMissing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    #[serde(rename = "type")]
    pub alert_type: AlertType,
    pub severity: Severity,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    pub since: u64,
    pub last_seen_at: u64,
    pub active: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RansomwareStatus {
    pub active: bool,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub indicators: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatStatus {
    pub service: String,
    pub status: String,
    pub heartbeat: HeartbeatMetrics,
    pub security: SecurityStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<CpuSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entropy_quality: Option<EntropyQualitySnapshot>,
    pub uptime_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuSnapshot {
    pub vendor: String,
    pub brand: String,
    pub cores: u32,
    pub threads: u32,
    pub features: Vec<String>,
    pub cache_l3_kb: u32,
    pub fingerprint: String,
    pub recommended_workers: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature_c: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntropyQualitySnapshot {
    pub latest_score: u32,
    pub quality_tier: u8,
    pub bit_score: u32,
    pub byte_score: u32,
    pub run_score: u32,
    pub pattern_score: u32,
    pub is_acceptable: bool,
    pub detected_flaws: Vec<String>,
    pub samples_scored: u32,
    pub average_score: f32,
    pub trend: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatMetrics {
    pub online: bool,
    pub telemetry_fresh: bool,
    pub telemetry_age_ms: u64,
    pub hashrate_hs: f64,
    pub baseline_hashrate_hs: f64,
    pub hashrate_ratio: f64,
    pub worker_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature_c: Option<f32>,
    pub phase: String,
    pub paused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityStatus {
    pub intrusion_score: u32,
    pub active_alerts: Vec<Alert>,
    pub ransomware: RansomwareStatus,
}

const TICK_INTERVAL_SECS: u64 = 2;
const TELEMETRY_STALE_MS: u64 = 15_000;
const HASHRATE_DROP_RATIO: f64 = 0.55;
const HASHRATE_WINDOW: usize = 60;
const MAX_SCAN_DEPTH: u32 = 4;
const MAX_SCAN_FILES: usize = 1024;

const PROTECTED_FILES: &[&str] = &[
    "miner-config.json",
    "miner.key",
    "telemetry.jsonl",
    "miner-control.json",
    "miner-trust-seal.json",
];

const RANSOM_PATTERNS: &[&str] = &[
    "readme", "decrypt", "recover", "restore", "ransom", "how_to", "payment",
];

const ENCRYPTED_EXTS: &[&str] = &[
    ".encrypted", ".lockbit", ".conti", ".zepto", ".wnry", ".cerber", ".clop", ".akira",
    ".pay", ".enc", ".locked",
];

pub struct HeartbeatMonitor {
    pub status: Arc<RwLock<HeartbeatStatus>>,
    protected_roots: Vec<PathBuf>,
    latest: Arc<RwLock<Option<TelemetrySnapshot>>>,
    hashrate_samples: Vec<f64>,
    alerts: HashMap<AlertType, Alert>,
    start: Instant,
    last_snapshot_ms: u64,
    entropy_scorer: EntropyQualityScorer,
}

impl HeartbeatMonitor {
    pub fn default_status() -> Arc<RwLock<HeartbeatStatus>> {
        Arc::new(RwLock::new(HeartbeatStatus {
            service: "heartbeat-embedded".into(),
            status: "ok".into(),
            heartbeat: HeartbeatMetrics {
                online: true,
                telemetry_fresh: false,
                telemetry_age_ms: 0,
                hashrate_hs: 0.0,
                baseline_hashrate_hs: 0.0,
                hashrate_ratio: 1.0,
                worker_count: 0,
                temperature_c: None,
                phase: "starting".into(),
                paused: false,
            },
            security: SecurityStatus {
                intrusion_score: 0,
                active_alerts: vec![],
                ransomware: RansomwareStatus {
                    active: false,
                    status: "clean".into(),
                    reason: None,
                    indicators: vec![],
                },
            },
            cpu: None,
            entropy_quality: None,
            uptime_seconds: 0,
        }))
    }

    pub fn new(
        latest: Arc<RwLock<Option<TelemetrySnapshot>>>,
        protected_roots: Vec<PathBuf>,
    ) -> Self {
        Self {
            status: Self::default_status(),
            protected_roots,
            latest,
            hashrate_samples: Vec::with_capacity(HASHRATE_WINDOW),
            alerts: HashMap::new(),
            start: Instant::now(),
            last_snapshot_ms: 0,
            entropy_scorer: EntropyQualityScorer::new(40, 80),
        }
    }

    pub async fn run(mut self, shutdown: CancellationToken) {
        tracing::info!(
            "Heartbeat monitor started — scanning {} protected roots",
            self.protected_roots.len()
        );
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(TICK_INTERVAL_SECS));
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = interval.tick() => self.tick().await,
            }
        }
    }

    async fn tick(&mut self) {
        let now_ms = now_unix_ms();
        let snap_opt = self.latest.read().await.clone();

        let (telemetry_fresh, telemetry_age_ms, paused) = if let Some(ref snap) = snap_opt {
            let ts = snap.timestamp_unix_ms as u64;
            if ts > self.last_snapshot_ms {
                self.last_snapshot_ms = ts;
            }
            let age = now_ms.saturating_sub(self.last_snapshot_ms);
            (age < TELEMETRY_STALE_MS, age, snap.mining_paused.unwrap_or(false))
        } else {
            (false, now_ms, false)
        };

        let phase = snap_opt
            .as_ref()
            .and_then(|s| s.mining_phase.as_ref())
            .map(|p| format!("{:?}", p).to_lowercase())
            .unwrap_or_else(|| "unknown".into());

        let is_waiting_phase = matches!(
            phase.as_str(),
            "waiting_for_clearance" | "committing" | "commitment_locked" | "revealing"
        );

        let hashrate = snap_opt.as_ref().map(|s| s.hashrate_hs).unwrap_or(0.0);
        if hashrate > 0.0 {
            if self.hashrate_samples.len() >= HASHRATE_WINDOW {
                self.hashrate_samples.remove(0);
            }
            self.hashrate_samples.push(hashrate);
        }
        let baseline = median(&self.hashrate_samples);
        let ratio = if baseline > 0.0 { hashrate / baseline } else { 1.0 };

        if !telemetry_fresh && self.last_snapshot_ms > 0 {
            self.raise(
                AlertType::TelemetryStale,
                Severity::Critical,
                "No fresh telemetry for >15s".into(),
                None,
                now_ms,
            );
        } else {
            self.resolve(AlertType::TelemetryStale);
        }

        let state_str = snap_opt
            .as_ref()
            .map(|s| format!("{:?}", s.state).to_lowercase())
            .unwrap_or_default();
        if snap_opt.is_some() && !state_str.contains("running") {
            self.raise(
                AlertType::MinerNotRunning,
                Severity::High,
                format!("Miner state: {state_str}"),
                None,
                now_ms,
            );
        } else {
            self.resolve(AlertType::MinerNotRunning);
        }

        if !paused && !is_waiting_phase && baseline > 10.0 && ratio < HASHRATE_DROP_RATIO {
            self.raise(
                AlertType::HashrateDrop,
                Severity::High,
                format!("Hashrate dropped to {:.0}% of baseline", ratio * 100.0),
                Some(format!("current={hashrate:.0} baseline={baseline:.0}")),
                now_ms,
            );
        } else {
            self.resolve(AlertType::HashrateDrop);
        }

        let temp_c = snap_opt.as_ref().and_then(|s| s.temperature_c);
        if let Some(t) = temp_c {
            if t >= 90.0 {
                self.raise(
                    AlertType::TemperatureCritical,
                    Severity::Critical,
                    format!("CPU temperature critical: {t:.1}C"),
                    None,
                    now_ms,
                );
                self.resolve(AlertType::TemperatureWarn);
            } else if t >= 82.0 {
                self.raise(
                    AlertType::TemperatureWarn,
                    Severity::Medium,
                    format!("CPU temperature elevated: {t:.1}C"),
                    None,
                    now_ms,
                );
                self.resolve(AlertType::TemperatureCritical);
            } else {
                self.resolve(AlertType::TemperatureWarn);
                self.resolve(AlertType::TemperatureCritical);
            }
        }

        let ticks = (self.start.elapsed().as_secs() / TICK_INTERVAL_SECS) as u32;
        let ransomware = if ticks % 10 == 0 {
            let roots = self.protected_roots.clone();
            let scan = tokio::task::spawn_blocking(move || scan_filesystem(&roots))
                .await
                .unwrap_or_default();
            self.process_ransomware_scan(scan, now_ms)
        } else {
            self.status.read().await.security.ransomware.clone()
        };

        let active_alerts: Vec<Alert> = self.alerts.values().filter(|a| a.active).cloned().collect();
        let score = intrusion_score(&active_alerts);
        let overall = if ransomware.active || score >= 70 {
            "error"
        } else if score >= 30 {
            "alert"
        } else if !telemetry_fresh {
            "degraded"
        } else {
            "ok"
        };

        let worker_count = snap_opt.as_ref().map(|s| s.worker_count).unwrap_or(0);

        // ── CPU identity + independent temperature reading ─────
        let cpu_id = cpu_info::detect_cpu();
        let independent_temp = cpu_info::get_cpu_temperature();
        let cpu_snapshot = CpuSnapshot {
            vendor: cpu_id.vendor.clone(),
            brand: cpu_id.brand.clone(),
            cores: cpu_id.cores,
            threads: cpu_id.threads,
            features: cpu_id.features.clone(),
            cache_l3_kb: cpu_id.cache_l3_kb,
            fingerprint: cpu_id.telemetry_fingerprint(),
            recommended_workers: cpu_id.recommended_workers(),
            temperature_c: independent_temp.or(temp_c),
        };

        // ── Entropy quality scoring ───────────────────────────
        let entropy_snapshot = if let Some(ref snap) = snap_opt {
            if let Some(ref hash_hex) = snap.last_solution_hash_hex {
                if let Ok(bytes) = hex::decode(hash_hex) {
                    if bytes.len() == 32 {
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(&bytes);
                        let report = self.entropy_scorer.score_entropy("pow", &arr);
                        let (avg, trend) = self.entropy_scorer.contributor_trend("pow");
                        Some(EntropyQualitySnapshot {
                            latest_score: report.score,
                            quality_tier: report.quality_tier,
                            bit_score: report.bit_score,
                            byte_score: report.byte_distribution_score,
                            run_score: report.run_length_score,
                            pattern_score: report.pattern_score,
                            is_acceptable: report.is_acceptable,
                            detected_flaws: report.detected_flaws,
                            samples_scored: report.contribution_count,
                            average_score: avg,
                            trend,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        let mut st = self.status.write().await;
        st.status = overall.into();
        st.uptime_seconds = self.start.elapsed().as_secs();
        st.heartbeat = HeartbeatMetrics {
            online: true,
            telemetry_fresh,
            telemetry_age_ms,
            hashrate_hs: hashrate,
            baseline_hashrate_hs: baseline,
            hashrate_ratio: ratio,
            worker_count,
            temperature_c: temp_c,
            phase,
            paused,
        };
        st.security = SecurityStatus {
            intrusion_score: score,
            active_alerts,
            ransomware,
        };
        st.cpu = Some(cpu_snapshot);
        st.entropy_quality = entropy_snapshot;
    }

    fn process_ransomware_scan(&mut self, scan: FsScanResult, now_ms: u64) -> RansomwareStatus {
        let FsScanResult { indicators, missing_protected } = scan;

        let has_ransom_note = indicators.iter().any(|i| i.starts_with("ransom_note:"));
        let has_encrypted = indicators.iter().any(|i| {
            i.starts_with("encrypted_protected:") || i.starts_with("encrypted_file:")
        });
        let active = has_ransom_note && (has_encrypted || missing_protected);

        if active {
            self.raise(
                AlertType::RansomwareDetected,
                Severity::Critical,
                "Ransomware indicators detected".into(),
                Some(indicators.iter().take(5).cloned().collect::<Vec<_>>().join("; ")),
                now_ms,
            );
        } else {
            self.resolve(AlertType::RansomwareDetected);
        }

        if missing_protected && !active {
            self.raise(
                AlertType::ProtectedAssetMissing,
                Severity::High,
                "Protected assets missing".into(),
                None,
                now_ms,
            );
        } else {
            self.resolve(AlertType::ProtectedAssetMissing);
        }

        if let Some(log_root) = self.protected_roots.iter().find(|p| p.ends_with("logs")) {
            let status_path = log_root.join("ransomware-status.json");
            let obj = RansomwareStatus {
                active,
                status: if active {
                    "RANSOMWARE_DETECTED".into()
                } else {
                    "clean".into()
                },
                reason: if active {
                    Some("Ransom note + encrypted/missing protected files".into())
                } else {
                    None
                },
                indicators: indicators.clone(),
            };
            if let Ok(json) = serde_json::to_string_pretty(&obj) {
                let _ = std::fs::write(status_path, json);
            }
        }

        RansomwareStatus {
            active,
            status: if active {
                "RANSOMWARE_DETECTED".into()
            } else {
                "clean".into()
            },
            reason: if active {
                Some("Ransom note + encrypted/missing protected files".into())
            } else {
                None
            },
            indicators,
        }
    }

    fn raise(
        &mut self,
        ty: AlertType,
        severity: Severity,
        message: String,
        details: Option<String>,
        now_ms: u64,
    ) {
        let entry = self.alerts.entry(ty).or_insert_with(|| Alert {
            alert_type: ty,
            severity,
            message: message.clone(),
            details: details.clone(),
            since: now_ms,
            last_seen_at: now_ms,
            active: false,
        });
        entry.active = true;
        entry.last_seen_at = now_ms;
        entry.severity = severity;
        entry.message = message;
        entry.details = details;
    }

    fn resolve(&mut self, ty: AlertType) {
        if let Some(a) = self.alerts.get_mut(&ty) {
            a.active = false;
        }
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

fn intrusion_score(alerts: &[Alert]) -> u32 {
    alerts
        .iter()
        .filter(|a| a.active)
        .map(|a| match a.severity {
            Severity::Critical => 40,
            Severity::High => 25,
            Severity::Medium => 10,
            Severity::Low => 5,
        })
        .sum::<u32>()
        .min(100)
}

#[derive(Debug, Clone, Default)]
struct FsScanResult {
    indicators: Vec<String>,
    missing_protected: bool,
}

/// Pure filesystem scan — runs on a blocking thread, no async.
fn scan_filesystem(roots: &[PathBuf]) -> FsScanResult {
    let mut indicators = Vec::new();
    let mut missing_protected = false;

    for root in roots {
        if !root.exists() {
            continue;
        }

        for name in PROTECTED_FILES {
            let path = root.join(name);
            for ext in ENCRYPTED_EXTS {
                let enc = root.join(format!("{name}{ext}"));
                if enc.exists() {
                    indicators.push(format!("encrypted_protected: {}", enc.display()));
                }
            }
            if (name == &"miner.key" && root.ends_with("keys")
                || name == &"miner-config.json" && root.ends_with("config"))
                && !path.exists()
            {
                missing_protected = true;
                indicators.push(format!("missing_protected: {}", path.display()));
            }
        }

        let _ = walk_for_indicators(root, 0, MAX_SCAN_DEPTH, &mut indicators);
    }

    FsScanResult { indicators, missing_protected }
}

fn walk_for_indicators(
    dir: &Path,
    depth: u32,
    max_depth: u32,
    indicators: &mut Vec<String>,
) -> std::io::Result<()> {
    if depth > max_depth || indicators.len() >= MAX_SCAN_FILES {
        return Ok(());
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        if indicators.len() >= MAX_SCAN_FILES {
            break;
        }

        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_lowercase();

        // Ignore our own scanner output files.
        if name == "ransomware-status.json" {
            continue;
        }

        if path.is_dir() {
            if !matches!(
                name.as_str(),
                "node_modules" | ".git" | "target" | "ransomware-evidence"
            ) {
                let _ = walk_for_indicators(&path, depth + 1, max_depth, indicators);
            }
            continue;
        }

        if RANSOM_PATTERNS.iter().any(|p| name.contains(p))
            && !name.ends_with(".rs")
            && !name.ends_with(".js")
            && !name.ends_with(".ts")
            && !name.ends_with(".md")
            && !name.ends_with(".toml")
        {
            indicators.push(format!("ransom_note: {}", path.display()));
        }

        if ENCRYPTED_EXTS.iter().any(|ext| name.ends_with(ext)) {
            indicators.push(format!("encrypted_file: {}", path.display()));
        }
    }

    Ok(())
}