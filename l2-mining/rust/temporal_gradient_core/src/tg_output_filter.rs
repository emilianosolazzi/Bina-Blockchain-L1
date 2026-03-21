// ─────────────────────────────────────────────────────────────────────────────
// tg_output_filter.rs
// Temporal Gradient — Output Bloom Filter
//
// Three jobs:
//   1. Pre-filter mining candidates (fast reject before expensive QR hash)
//   2. Deduplicate output hashes across restarts (persistent off-chain mirror)
//   3. Audit trail for the personal threat dashboard (epoch-tracked history)
//
// No Bitcoin dependencies. Drop-in replacement for the existing BloomFilterLib
// off-chain mirror. Compatible with the existing telemetry and chain modules.
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bitvec::prelude::*;
use blake3;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};

// ─────────────────────────────────────────────────────────────────
// Errors
// ─────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum FilterError {
    #[error("Invalid filter size {0} — must be power of two")]
    InvalidSize(usize),
    #[error("Invalid hash count {0} — must be 2–7")]
    InvalidNumHashes(u8),
    #[error("Invalid false-positive rate {0}")]
    InvalidFPRate(f64),
    #[error("Storage error: {0}")]
    StorageError(String),
    #[error("Serialization error: {0}")]
    SerializationError(String),
    #[error("Metadata validation failed for output {0}")]
    MetadataValidationFailed(String),
    #[error("Rate limit exceeded")]
    RateLimitExceeded,
    #[error("Invalid input: {0}")]
    InvalidInput(String),
}

// ─────────────────────────────────────────────────────────────────
// Config
// ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FilterConfig {
    /// Bit-array size — must be power of two.
    pub size: usize,
    /// Number of hash functions (2–7).
    pub num_hashes: u8,
    /// Random tweak for hash diversification.
    pub tweak: u32,
    /// Maximum acceptable false-positive rate.
    pub max_fp_rate: f64,
    /// Seconds before an output record is eligible for GC.
    /// None = keep until manually removed.
    pub max_output_age_secs: Option<u64>,
    /// Epoch bucket duration in seconds (default 1 hour).
    pub epoch_duration_secs: u64,
}

impl FilterConfig {
    pub fn validate(&self) -> Result<(), FilterError> {
        if !self.size.is_power_of_two() {
            return Err(FilterError::InvalidSize(self.size));
        }
        if !(2..=7).contains(&self.num_hashes) {
            return Err(FilterError::InvalidNumHashes(self.num_hashes));
        }
        if !(0.0..1.0).contains(&self.max_fp_rate) {
            return Err(FilterError::InvalidFPRate(self.max_fp_rate));
        }
        Ok(())
    }

    /// Optimal bit-array size for `expected_items` at `fp_rate`.
    pub fn optimal_size(expected_items: usize, fp_rate: f64) -> usize {
        let ln2 = std::f64::consts::LN_2;
        let size = (-(expected_items as f64) * fp_rate.ln() / (ln2 * ln2)).ceil() as usize;
        size.next_power_of_two()
    }

    /// Optimal hash count for the given size and expected item count.
    pub fn optimal_hashes(size: usize, expected_items: usize) -> u8 {
        let k = (size as f64 / expected_items as f64) * std::f64::consts::LN_2;
        k.round().clamp(2.0, 7.0) as u8
    }
}

impl Default for FilterConfig {
    fn default() -> Self {
        // Sized for ~500k outputs (roughly 10 days of testnet at current rate)
        // with a 0.01% false-positive rate.
        let expected = 500_000;
        let fp = 0.0001;
        let size = Self::optimal_size(expected, fp);
        let num_hashes = Self::optimal_hashes(size, expected);
        Self {
            size,
            num_hashes,
            tweak: rand::random(),
            max_fp_rate: fp,
            max_output_age_secs: Some(7 * 24 * 3600), // 7 days
            epoch_duration_secs: 3600,                 // 1 hour buckets
        }
    }
}

// ─────────────────────────────────────────────────────────────────
// Output metadata
// ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutputRecord {
    /// The 32-byte hmacOutput / solution hash.
    pub output_hash: [u8; 32],
    /// Unix timestamp (seconds) when first seen.
    pub first_seen: u64,
    /// Epoch bucket (first_seen / epoch_duration_secs).
    pub epoch: u64,
    /// Mining nonce that produced this output.
    pub nonce: u64,
    /// Miner wallet address (hex string).
    pub miner: String,
    /// blake3 integrity hash of this record.
    pub record_hash: [u8; 32],
}

impl OutputRecord {
    pub fn new(
        output_hash: [u8; 32],
        nonce: u64,
        miner: &str,
        epoch_duration: u64,
    ) -> Self {
        let first_seen = now_secs();
        let epoch = first_seen / epoch_duration;
        let record_hash = Self::compute_hash(&output_hash, nonce, first_seen);
        Self {
            output_hash,
            first_seen,
            epoch,
            nonce,
            miner: miner.to_string(),
            record_hash,
        }
    }

    fn compute_hash(output_hash: &[u8; 32], nonce: u64, timestamp: u64) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        h.update(output_hash);
        h.update(&nonce.to_le_bytes());
        h.update(&timestamp.to_le_bytes());
        h.finalize().into()
    }

    pub fn validate(&self) -> Result<(), FilterError> {
        let expected = Self::compute_hash(&self.output_hash, self.nonce, self.first_seen);
        if expected != self.record_hash {
            return Err(FilterError::MetadataValidationFailed(
                hex::encode(self.output_hash),
            ));
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────
// Storage backend (swappable)
// ─────────────────────────────────────────────────────────────────

pub trait StorageBackend: Send + Sync {
    fn insert(&self, key: &[u8], value: &[u8]) -> Result<(), String>;
    fn remove(&self, key: &[u8]) -> Result<(), String>;
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, String>;
    fn iter(&self) -> Box<dyn Iterator<Item = Result<(Vec<u8>, Vec<u8>), String>> + '_>;
    fn flush(&self) -> Result<(), String>;
}

/// In-memory backend — suitable for testing and short-lived sessions.
#[derive(Default)]
pub struct MemoryBackend {
    data: DashMap<Vec<u8>, Vec<u8>>,
}

impl StorageBackend for MemoryBackend {
    fn insert(&self, key: &[u8], value: &[u8]) -> Result<(), String> {
        self.data.insert(key.to_vec(), value.to_vec());
        Ok(())
    }
    fn remove(&self, key: &[u8]) -> Result<(), String> {
        self.data.remove(key);
        Ok(())
    }
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
        Ok(self.data.get(key).map(|v| v.clone()))
    }
    fn iter(&self) -> Box<dyn Iterator<Item = Result<(Vec<u8>, Vec<u8>), String>> + '_> {
        Box::new(
            self.data
                .iter()
                .map(|entry| Ok((entry.key().clone(), entry.value().clone()))),
        )
    }
    fn flush(&self) -> Result<(), String> {
        Ok(())
    }
}

/// Sled-backed persistent storage.
pub struct SledBackend {
    db: sled::Db,
}

impl SledBackend {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, FilterError> {
        let db = sled::open(path).map_err(|e| FilterError::StorageError(e.to_string()))?;
        Ok(Self { db })
    }
}

impl StorageBackend for SledBackend {
    fn insert(&self, key: &[u8], value: &[u8]) -> Result<(), String> {
        self.db.insert(key, value).map_err(|e| e.to_string())?;
        Ok(())
    }
    fn remove(&self, key: &[u8]) -> Result<(), String> {
        self.db.remove(key).map_err(|e| e.to_string())?;
        Ok(())
    }
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
        self.db
            .get(key)
            .map_err(|e| e.to_string())
            .map(|opt| opt.map(|iv| iv.to_vec()))
    }
    fn iter(&self) -> Box<dyn Iterator<Item = Result<(Vec<u8>, Vec<u8>), String>> + '_> {
        Box::new(self.db.iter().map(|r| {
            r.map_err(|e| e.to_string())
                .map(|(k, v)| (k.to_vec(), v.to_vec()))
        }))
    }
    fn flush(&self) -> Result<(), String> {
        self.db.flush().map_err(|e| e.to_string())?;
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────
// Rate limiter
// ─────────────────────────────────────────────────────────────────

pub struct RateLimiter {
    last_op: AtomicU64,
    min_interval_ms: u64,
}

impl RateLimiter {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            last_op: AtomicU64::new(0),
            min_interval_ms: min_interval.as_millis() as u64,
        }
    }

    pub fn allow(&self) -> bool {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let last = self.last_op.load(Ordering::Relaxed);
        if now_ms >= last + self.min_interval_ms {
            self.last_op
                .compare_exchange(last, now_ms, Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
        } else {
            false
        }
    }
}

// ─────────────────────────────────────────────────────────────────
// Core filter (bit array + hashing)
// ─────────────────────────────────────────────────────────────────

struct Core {
    bits: BitVec<u64, Lsb0>,
    config: FilterConfig,
    seeds: [u32; 8],
    item_count: AtomicU64,
    rate_limiter: RateLimiter,
}

impl Core {
    fn new(config: FilterConfig) -> Result<Self, FilterError> {
        config.validate()?;
        let cap = config.size;
        let mut bits = BitVec::with_capacity(cap);
        bits.resize(cap, false);

        // Derive per-instance seeds from tweak via blake3
        let mut seeds = [0u32; 8];
        let hash = blake3::hash(&config.tweak.to_le_bytes());
        for i in 0..8 {
            seeds[i] = u32::from_le_bytes(
                hash.as_bytes()[i * 4..i * 4 + 4].try_into().unwrap(),
            );
        }

        Ok(Self {
            bits,
            config,
            seeds,
            item_count: AtomicU64::new(0),
            rate_limiter: RateLimiter::new(Duration::from_micros(100)),
        })
    }

    // Double-blake3 for two independent hash values
    #[inline]
    fn two_hashes(&self, data: &[u8]) -> [u64; 2] {
        let key: [u8; 32] = {
            let mut k = [0u8; 32];
            let tb = self.config.tweak.to_le_bytes();
            for i in 0..8 {
                k[i * 4..(i + 1) * 4].copy_from_slice(&tb);
            }
            k
        };
        let mut h = blake3::Hasher::new_keyed(&key);
        h.update(data);
        let h1 = h.finalize();
        h.reset();
        h.update(h1.as_bytes());
        let h2 = h.finalize();
        [
            u64::from_le_bytes(h1.as_bytes()[..8].try_into().unwrap()),
            u64::from_le_bytes(h2.as_bytes()[..8].try_into().unwrap()),
        ]
    }

    // Enhanced double-hashing — avoids full hash for each k
    #[inline]
    fn bit_positions(&self, data: &[u8]) -> Vec<usize> {
        let [h1, h2] = self.two_hashes(data);
        (0..self.config.num_hashes as u64)
            .map(|i| {
                let pos = h1.wrapping_add(i.wrapping_mul(h2)) % self.config.size as u64;
                pos as usize
            })
            .collect()
    }

    fn set_bits(&mut self, positions: &[usize]) {
        for &pos in positions {
            self.bits.set(pos, true);
        }
    }

    fn check_bits(&self, positions: &[usize]) -> bool {
        positions.iter().all(|&pos| self.bits[pos])
    }

    /// Insert `data` into the filter. Returns true if it was NOT previously present.
    pub fn insert(&mut self, data: &[u8]) -> Result<bool, FilterError> {
        if !self.rate_limiter.allow() {
            return Err(FilterError::RateLimitExceeded);
        }
        let positions = self.bit_positions(data);
        let is_new = !self.check_bits(&positions);
        self.set_bits(&positions);
        if is_new {
            self.item_count.fetch_add(1, Ordering::Relaxed);
        }
        Ok(is_new)
    }

    /// Check if `data` might be present (probabilistic).
    pub fn might_contain(&self, data: &[u8]) -> bool {
        let positions = self.bit_positions(data);
        self.check_bits(&positions)
    }

    pub fn false_positive_rate(&self) -> f64 {
        let n = self.item_count.load(Ordering::Relaxed) as f64;
        let m = self.config.size as f64;
        let k = self.config.num_hashes as f64;
        let exp = (-k * n / m).max(-20.0);
        (1.0 - exp.exp()).powf(k)
    }

    pub fn item_count(&self) -> u64 {
        self.item_count.load(Ordering::Relaxed)
    }

    pub fn memory_bytes(&self) -> usize {
        self.bits.capacity() / 8
    }
}

// ─────────────────────────────────────────────────────────────────
// Record store (metadata + epoch tracking)
// ─────────────────────────────────────────────────────────────────

struct RecordStore {
    records: DashMap<[u8; 32], OutputRecord>, // keyed by output_hash
    epoch_counts: DashMap<u64, u64>,
    storage: Option<Arc<dyn StorageBackend>>,
}

impl RecordStore {
    fn new(storage: Option<Arc<dyn StorageBackend>>) -> Self {
        Self {
            records: DashMap::with_capacity(4096),
            epoch_counts: DashMap::new(),
            storage,
        }
    }

    fn insert(&self, record: OutputRecord) -> Result<(), FilterError> {
        record.validate()?;
        if let Some(backend) = &self.storage {
            let value = bincode::serialize(&record)
                .map_err(|e| FilterError::SerializationError(e.to_string()))?;
            backend
                .insert(&record.output_hash, &value)
                .map_err(|e| FilterError::StorageError(e))?;
        }
        *self.epoch_counts.entry(record.epoch).or_insert(0) += 1;
        self.records.insert(record.output_hash, record);
        Ok(())
    }

    fn get(&self, output_hash: &[u8; 32]) -> Option<OutputRecord> {
        self.records.get(output_hash).map(|r| r.clone())
    }

    fn remove(&self, output_hash: &[u8; 32]) -> Option<OutputRecord> {
        if let Some((_, record)) = self.records.remove(output_hash) {
            if let Some(backend) = &self.storage {
                let _ = backend.remove(output_hash);
            }
            if let Some(mut count) = self.epoch_counts.get_mut(&record.epoch) {
                *count = count.saturating_sub(1);
            }
            Some(record)
        } else {
            None
        }
    }

    fn garbage_collect(&self, config: &FilterConfig) -> u64 {
        let now = now_secs();
        let max_age = match config.max_output_age_secs {
            Some(age) => age,
            None => return 0,
        };

        let expired_hashes: Vec<[u8; 32]> = self
            .records
            .iter()
            .filter(|entry| now.saturating_sub(entry.value().first_seen) > max_age)
            .map(|entry| *entry.key())
            .collect();

        let count = expired_hashes.len() as u64;
        for hash in expired_hashes {
            self.remove(&hash);
        }
        count
    }

    fn epoch_stats(&self) -> HashMap<u64, u64> {
        self.epoch_counts
            .iter()
            .map(|e| (*e.key(), *e.value()))
            .collect()
    }

    fn load_from_storage(&self, config: &FilterConfig) -> Result<Vec<[u8; 32]>, FilterError> {
        let backend = match &self.storage {
            Some(b) => b,
            None => return Ok(vec![]),
        };

        let mut loaded_hashes = vec![];
        for entry in backend.iter() {
            let (_, value) = entry.map_err(|e| FilterError::StorageError(e))?;
            if let Ok(record) = bincode::deserialize::<OutputRecord>(&value) {
                if record.validate().is_ok() {
                    let now = now_secs();
                    let expired = config
                        .max_output_age_secs
                        .map(|age| now.saturating_sub(record.first_seen) > age)
                        .unwrap_or(false);
                    if !expired {
                        let hash = record.output_hash;
                        *self.epoch_counts.entry(record.epoch).or_insert(0) += 1;
                        self.records.insert(hash, record);
                        loaded_hashes.push(hash);
                    }
                }
            }
        }
        Ok(loaded_hashes)
    }
}

// ─────────────────────────────────────────────────────────────────
// Typestate markers
// ─────────────────────────────────────────────────────────────────

pub struct Uninitialized;
pub struct Ready;

// ─────────────────────────────────────────────────────────────────
// Public API — builder
// ─────────────────────────────────────────────────────────────────

pub struct TgOutputFilter<State = Uninitialized> {
    core: Option<Core>,
    store: Option<RecordStore>,
    _state: std::marker::PhantomData<State>,
}

impl TgOutputFilter<Uninitialized> {
    pub fn new() -> Self {
        Self {
            core: None,
            store: None,
            _state: std::marker::PhantomData,
        }
    }

    pub fn with_config(self, config: FilterConfig) -> Result<Self, FilterError> {
        Ok(Self {
            core: Some(Core::new(config)?),
            store: self.store,
            _state: std::marker::PhantomData,
        })
    }

    pub fn with_storage(self, backend: Arc<dyn StorageBackend>) -> Self {
        Self {
            core: self.core,
            store: Some(RecordStore::new(Some(backend))),
            _state: std::marker::PhantomData,
        }
    }

    /// Finalise construction. Loads persisted records into the bit array.
    pub fn build(mut self) -> Result<TgOutputFilter<Ready>, FilterError> {
        let core = self
            .core
            .get_or_insert_with(|| Core::new(FilterConfig::default()).unwrap());

        let store = self
            .store
            .get_or_insert_with(|| RecordStore::new(None));

        // Replay persisted hashes into the bit array
        let hashes = store.load_from_storage(&core.config)?;
        for hash in &hashes {
            let _ = core.insert(hash); // errors only on rate limit; safe to ignore here
        }

        if !hashes.is_empty() {
            info!(
                "TgOutputFilter: restored {} outputs from storage",
                hashes.len()
            );
        }

        Ok(TgOutputFilter {
            core: self.core,
            store: self.store,
            _state: std::marker::PhantomData,
        })
    }
}

// ─────────────────────────────────────────────────────────────────
// Public API — operational
// ─────────────────────────────────────────────────────────────────

impl TgOutputFilter<Ready> {
    // ── Mining hot path ────────────────────────────────────────────

    /// Fast pre-filter for mining candidates.
    /// Returns `true` if the hash is definitely new (worth pursuing).
    /// Returns `false` if it has likely been seen before (skip).
    ///
    /// This is the hot-path call — no storage I/O, no metadata.
    #[inline]
    pub fn is_candidate(&self, hash: &[u8; 32]) -> bool {
        !self.core().might_contain(hash)
    }

    /// Record a confirmed solution. Inserts into the bit array AND
    /// persists the full record with epoch metadata.
    pub fn record_solution(
        &mut self,
        output_hash: [u8; 32],
        nonce: u64,
        miner: &str,
    ) -> Result<bool, FilterError> {
        let config = self.core().config.clone();
        let record = OutputRecord::new(output_hash, nonce, miner, config.epoch_duration_secs);
        let is_new = self.core_mut().insert(&output_hash)?;
        if is_new {
            self.store().insert(record)?;
        }
        Ok(is_new)
    }

    // ── Deduplication ──────────────────────────────────────────────

    /// Definitive check: was this output hash ever recorded?
    /// Unlike `is_candidate`, this validates the full metadata record.
    pub fn is_duplicate(&self, output_hash: &[u8; 32]) -> bool {
        if !self.core().might_contain(output_hash) {
            return false; // definitely new
        }
        // Confirm with metadata store (eliminates false positives)
        self.store().get(output_hash).is_some()
    }

    // ── Audit / dashboard ──────────────────────────────────────────

    /// Full record for a known output. Returns None if not found.
    pub fn get_record(&self, output_hash: &[u8; 32]) -> Option<OutputRecord> {
        self.store().get(output_hash)
    }

    /// How many unique outputs have been recorded.
    pub fn output_count(&self) -> u64 {
        self.core().item_count()
    }

    /// Current false-positive rate of the bit array.
    pub fn false_positive_rate(&self) -> f64 {
        self.core().false_positive_rate()
    }

    /// Per-epoch output counts — powers the dashboard timeline.
    pub fn epoch_stats(&self) -> HashMap<u64, u64> {
        self.store().epoch_stats()
    }

    /// Memory used by the bit array only (bytes).
    pub fn memory_bytes(&self) -> usize {
        self.core().memory_bytes()
    }

    // ── Maintenance ────────────────────────────────────────────────

    /// Remove outputs older than `max_output_age_secs`.
    /// Returns number of records removed.
    pub fn garbage_collect(&mut self) -> u64 {
        let config = self.core().config.clone();
        let removed = self.store().garbage_collect(&config);
        if removed > 0 {
            // Subtract from atomic count; bit array is append-only
            // (standard bloom filter — no individual bit clearing)
            self.core().item_count.fetch_sub(removed, Ordering::Relaxed);
            info!("TgOutputFilter: GC removed {} expired records", removed);
        }
        removed
    }

    /// Log a summary — useful on startup and in the dashboard.
    pub fn log_summary(&self) {
        info!(
            "TgOutputFilter | outputs={} | fp_rate={:.6} | mem={}KB | epochs={}",
            self.output_count(),
            self.false_positive_rate(),
            self.memory_bytes() / 1024,
            self.epoch_stats().len(),
        );
        if self.false_positive_rate() > self.core().config.max_fp_rate {
            warn!(
                "TgOutputFilter: fp_rate {:.6} exceeds limit {:.6} — consider resize",
                self.false_positive_rate(),
                self.core().config.max_fp_rate
            );
        }
    }

    // ── Internal helpers ───────────────────────────────────────────

    fn core(&self) -> &Core {
        self.core.as_ref().unwrap()
    }

    fn core_mut(&mut self) -> &mut Core {
        self.core.as_mut().unwrap()
    }

    fn store(&self) -> &RecordStore {
        self.store.as_ref().unwrap()
    }
}

// ─────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────

#[inline]
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

// ─────────────────────────────────────────────────────────────────
// Integration with existing runtime
//
// In runtime.rs, replace the per-solution duplicate check with:
//
//   // Before submitting to chain:
//   if !filter.is_candidate(&candidate.solution_hash) {
//       continue; // fast reject — seen before
//   }
//
//   // After successful reveal:
//   filter.record_solution(
//       output_hash,
//       candidate.nonce,
//       &hex_string(&miner_address),
//   )?;
//
// In the telemetry snapshot, add:
//   "output_count":        filter.output_count(),
//   "filter_fp_rate":      filter.false_positive_rate(),
//   "filter_memory_kb":    filter.memory_bytes() / 1024,
//   "epoch_stats":         filter.epoch_stats(),
//
// ─────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;
    use std::time::Instant;

    fn build_filter() -> TgOutputFilter<Ready> {
        TgOutputFilter::new()
            .with_config(FilterConfig {
                size: 1 << 16,
                num_hashes: 4,
                tweak: 42,
                max_fp_rate: 0.001,
                max_output_age_secs: Some(3600),
                epoch_duration_secs: 60,
            })
            .unwrap()
            .build()
            .unwrap()
    }

    fn rand_hash() -> [u8; 32] {
        rand::thread_rng().gen()
    }

    #[test]
    fn new_hash_is_candidate() {
        let filter = build_filter();
        let hash = rand_hash();
        assert!(filter.is_candidate(&hash));
    }

    #[test]
    fn recorded_hash_is_not_candidate() {
        let mut filter = build_filter();
        let hash = rand_hash();
        filter.record_solution(hash, 1, "0x1234").unwrap();
        assert!(!filter.is_candidate(&hash));
    }

    #[test]
    fn is_duplicate_returns_false_for_unknown() {
        let filter = build_filter();
        assert!(!filter.is_duplicate(&rand_hash()));
    }

    #[test]
    fn is_duplicate_returns_true_after_record() {
        let mut filter = build_filter();
        let hash = rand_hash();
        filter.record_solution(hash, 99, "0xabcd").unwrap();
        assert!(filter.is_duplicate(&hash));
    }

    #[test]
    fn record_solution_returns_false_on_duplicate() {
        let mut filter = build_filter();
        let hash = rand_hash();
        assert!(filter.record_solution(hash, 1, "0x1").unwrap());
        assert!(!filter.record_solution(hash, 2, "0x1").unwrap());
    }

    #[test]
    fn epoch_stats_track_insertions() {
        let mut filter = build_filter();
        for i in 0u64..5 {
            filter.record_solution(rand_hash(), i, "0x1").unwrap();
        }
        let stats: u64 = filter.epoch_stats().values().sum();
        assert_eq!(stats, 5);
    }

    #[test]
    fn output_count_increments() {
        let mut filter = build_filter();
        for i in 0..10u64 {
            filter.record_solution(rand_hash(), i, "0x1").unwrap();
        }
        assert_eq!(filter.output_count(), 10);
    }

    #[test]
    fn fp_rate_stays_low_at_scale() {
        let mut filter = TgOutputFilter::new()
            .with_config(FilterConfig {
                size: 1 << 22,
                num_hashes: 4,
                tweak: 7,
                max_fp_rate: 0.01,
                max_output_age_secs: None,
                epoch_duration_secs: 3600,
            })
            .unwrap()
            .build()
            .unwrap();

        let start = Instant::now();
        for i in 0..100_000u64 {
            let mut hash = [0u8; 32];
            hash[..8].copy_from_slice(&i.to_le_bytes());
            filter.record_solution(hash, i, "0x1").unwrap();
        }
        println!("100k inserts in {:?}", start.elapsed());
        assert!(
            filter.false_positive_rate() < 0.01,
            "fp_rate={:.6}",
            filter.false_positive_rate()
        );
    }

    #[test]
    fn memory_backend_persists_across_rebuild() {
        let backend = Arc::new(MemoryBackend::default());
        let hash = rand_hash();

        {
            let mut f = TgOutputFilter::new()
                .with_storage(backend.clone())
                .build()
                .unwrap();
            f.record_solution(hash, 1, "0x1").unwrap();
        }

        // Rebuild from same backend
        let f2 = TgOutputFilter::new()
            .with_storage(backend.clone())
            .build()
            .unwrap();

        assert!(f2.is_duplicate(&hash), "record should survive rebuild");
    }

    #[test]
    fn get_record_returns_correct_metadata() {
        let mut filter = build_filter();
        let hash = rand_hash();
        filter.record_solution(hash, 42, "0xminer").unwrap();
        let record = filter.get_record(&hash).unwrap();
        assert_eq!(record.nonce, 42);
        assert_eq!(record.miner, "0xminer");
        record.validate().unwrap();
    }
}