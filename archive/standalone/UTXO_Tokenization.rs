use std::sync::atomic::{AtomicU64, Ordering};
use rayon::prelude::*;
use bitcoin::{Block, Txid};
use bitcoin::hashes::Hash;
use std::time::{SystemTime, UNIX_EPOCH, Duration, Instant};
use dashmap::DashMap;
use std::sync::Arc;
use blake3;
use log::{info, warn, error};
use serde::{Serialize, Deserialize};
use thiserror::Error;
use bitvec::prelude::*;
use std::path::Path;
use std::collections::HashMap;
use prometheus::{Registry, Counter, Gauge};
use proptest::prelude::*;
use std::mem;

/// Custom error type for Bloom filter operations
#[derive(Debug, Error)]
pub enum BloomError {
    #[error("Invalid filter size: {0} (must be power of two)")]
    InvalidSize(usize),
    #[error("Invalid number of hash functions: {0}")]
    InvalidNumHashes(u8),
    #[error("Invalid false positive rate: {0}")]
    InvalidFPRate(f64),
    #[error("Storage error: {0}")]
    StorageError(String),
    #[error("Serialization error: {0}")]
    SerializationError(String),
    #[error("Metadata validation failed for UTXO {0}:{1}")]
    MetadataValidationFailed(Txid, u32),
    #[error("Concurrent modification detected")]
    ConcurrentModification,
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    #[error("Rate limit exceeded")]
    RateLimitExceeded,
    #[error("Checksum validation failed")]
    ChecksumFailed,
}

/// Configuration for Bitcoin Bloom Filter
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BloomConfig {
    pub size: usize,
    pub num_hashes: u8,
    pub tweak: u32,
    pub flags: u8,
    pub max_fp_rate: f64,
    pub max_utxo_age_secs: Option<u64>,
    pub epoch_duration_secs: u64,
}

impl BloomConfig {
    pub fn validate(&self) -> Result<(), BloomError> {
        if !self.size.is_power_of_two() {
            return Err(BloomError::InvalidSize(self.size));
        }
        if !(2..=7).contains(&self.num_hashes) {
            return Err(BloomError::InvalidNumHashes(self.num_hashes));
        }
        if !(0.0..1.0).contains(&self.max_fp_rate) {
            return Err(BloomError::InvalidFPRate(self.max_fp_rate));
        }
        Ok(())
    }

    pub fn calculate_optimal_size(expected_items: usize, fp_rate: f64) -> usize {
        let ln2 = std::f64::consts::LN_2;
        let size = (-(expected_items as f64) * fp_rate.ln() / (ln2 * ln2)).ceil() as usize;
        size.next_power_of_two()
    }

    pub fn calculate_optimal_hashes(size: usize, expected_items: usize) -> u8 {
        let k = (size as f64 / expected_items as f64) * std::f64::consts::LN_2;
        k.round().clamp(2.0, 7.0) as u8
    }
}

impl Default for BloomConfig {
    fn default() -> Self {
        let expected_items = 100_000;
        let fp_rate = 0.0001;
        let size = Self::calculate_optimal_size(expected_items, fp_rate);
        let num_hashes = Self::calculate_optimal_hashes(size, expected_items);

        Self {
            size,
            num_hashes,
            tweak: rand::random(),
            flags: 0,
            max_fp_rate: fp_rate,
            max_utxo_age_secs: None,
            epoch_duration_secs: 30 * 24 * 60 * 60,
        }
    }
}

/// UTXO metadata with versioning
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UtxoMetadata {
    pub txid: Txid,
    pub vout: u32,
    pub timestamp: u64,
    pub metadata_hash: [u8; 32],
    pub epoch: u64,
    #[serde(default = "default_version")]
    pub version: u32,
}

fn default_version() -> u32 { 1 }

impl UtxoMetadata {
    pub fn new(txid: Txid, vout: u32, epoch_duration: u64) -> Self {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let epoch = timestamp / epoch_duration;
        let metadata_hash = Self::compute_metadata_hash(&txid, vout, timestamp);

        Self {
            txid,
            vout,
            timestamp,
            metadata_hash,
            epoch,
            version: CURRENT_VERSION,
        }
    }

    fn compute_metadata_hash(txid: &Txid, vout: u32, timestamp: u64) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&txid[..]);
        hasher.update(&vout.to_le_bytes());
        hasher.update(&timestamp.to_le_bytes());
        hasher.finalize().into()
    }

    pub fn validate(&self) -> Result<(), BloomError> {
        let computed_hash = Self::compute_metadata_hash(&self.txid, self.vout, self.timestamp);
        if computed_hash != self.metadata_hash {
            Err(BloomError::MetadataValidationFailed(self.txid, self.vout))
        } else {
            Ok(())
        }
    }
}

/// Snapshot structure with metadata and checksum
#[derive(Serialize, Deserialize)]
struct FilterSnapshot {
    version: u32,
    data: Vec<u64>,
    metadata: Vec<UtxoMetadata>,
    config: BloomConfig,
    checksum: [u8; 32],
}

const CURRENT_VERSION: u32 = 1;
const MAX_REASONABLE_VOUT: u32 = 1_000_000;

/// Fixed-size key types
type StorageKey = [u8; 16];
type PreimageKey = [u8; 36];

/// Cache for frequent hash calculations
#[derive(Clone)]
struct HashCache {
    tweak: u32,
    seeds: [u32; 8],
    num_hashes: u8,
}

/// Rate limiter for operations
pub struct RateLimiter {
    last_op: AtomicU64,
    min_interval: Duration,
}

impl RateLimiter {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            last_op: AtomicU64::new(0),
            min_interval,
        }
    }

    pub fn allow(&self) -> bool {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let last = self.last_op.load(Ordering::Relaxed);
        if now >= last + self.min_interval.as_secs() {
            self.last_op.compare_exchange(
                last,
                now,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ).is_ok()
        } else {
            false
        }
    }
}

/// Monitoring trait
pub trait Monitor: Send + Sync {
    fn on_insert(&self, txid: &Txid, vout: u32);
    fn on_false_positive(&self);
}

/// Storage backend trait
pub trait StorageBackend: Send + Sync {
    fn insert(&self, key: &[u8], value: &[u8]) -> Result<(), String>;
    fn remove(&self, key: &[u8]) -> Result<(), String>;
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, String>;
    fn iter(&self) -> Box<dyn Iterator<Item = Result<(Vec<u8>, Vec<u8>), String>>>;
    fn flush(&self) -> Result<(), String>;
}

/// Sled-based storage backend
pub struct SledBackend {
    db: sled::Db,
}

impl SledBackend {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Self {
            db: sled::open(path).expect("Failed to open sled database"),
        }
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
        self.db.get(key).map_err(|e| e.to_string()).map(|opt| opt.map(|ivec| ivec.to_vec()))
    }

    fn iter(&self) -> Box<dyn Iterator<Item = Result<(Vec<u8>, Vec<u8>), String>> {
        Box::new(self.db.iter().map(|res| {
            res.map_err(|e| e.to_string())
                .map(|(k, v)| (k.to_vec(), v.to_vec()))
        }))
    }

    fn flush(&self) -> Result<(), String> {
        self.db.flush().map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// Storage layer for persistence
struct StorageLayer {
    backend: Arc<dyn StorageBackend>,
}

impl StorageLayer {
    pub fn new(backend: Arc<dyn StorageBackend>) -> Self {
        Self { backend }
    }

    pub fn store_metadata(&self, key: &StorageKey, metadata: &UtxoMetadata) -> Result<(), BloomError> {
        let serialized = bincode::serialize(&StorageEntry::V1(metadata.clone()))
            .map_err(|e| BloomError::SerializationError(e.to_string()))?;
        self.backend.insert(key, &serialized)
            .map_err(|e| BloomError::StorageError(e))?;
        Ok(())
    }

    pub fn remove_metadata(&self, key: &StorageKey) -> Result<(), BloomError> {
        self.backend.remove(key)
            .map_err(|e| BloomError::StorageError(e))?;
        Ok(())
    }

    pub fn load_metadata(&self, key: &StorageKey) -> Result<Option<UtxoMetadata>, BloomError> {
        let data = self.backend.get(key)
            .map_err(|e| BloomError::StorageError(e))?;
        match data {
            Some(bytes) => {
                let entry = bincode::deserialize::<StorageEntry>(&bytes)
                    .map_err(|e| BloomError::SerializationError(e.to_string()))?;
                match entry {
                    StorageEntry::V1(metadata) => Ok(Some(metadata)),
                }
            }
            None => Ok(None),
        }
    }

    pub fn flush(&self) -> Result<(), BloomError> {
        self.backend.flush()
            .map_err(|e| BloomError::StorageError(e))?;
        Ok(())
    }
}

/// Metadata manager for UTXO metadata
struct MetadataManager {
    metadata: Arc<DashMap<PreimageKey, UtxoMetadata>>,
    epoch_stats: DashMap<u64, u64>,
    storage: Option<StorageLayer>,
}

impl MetadataManager {
    pub fn new(storage: Option<StorageLayer>) -> Self {
        Self {
            metadata: Arc::new(DashMap::with_capacity(1024)),
            epoch_stats: DashMap::new(),
            storage,
        }
    }

    pub fn insert(&self, preimage: PreimageKey, metadata: UtxoMetadata) -> Result<(), BloomError> {
        metadata.validate()?;
        if let Some(storage) = &self.storage {
            let key = BloomFilterCore::compute_storage_key(&metadata.txid, metadata.vout);
            storage.store_metadata(&key, &metadata)?;
        }
        self.metadata.insert(preimage, metadata.clone());
        *self.epoch_stats.entry(metadata.epoch).or_insert(0) += 1;
        Ok(())
    }

    pub fn remove(&self, preimage: &PreimageKey) -> Result<Option<UtxoMetadata>, BloomError> {
        if let Some((_, metadata)) = self.metadata.remove(preimage) {
            if let Some(storage) = &self.storage {
                let key = BloomFilterCore::compute_storage_key(&metadata.txid, metadata.vout);
                storage.remove_metadata(&key)?;
            }
            *self.epoch_stats.entry(metadata.epoch).or_insert(0) -= 1;
            Ok(Some(metadata))
        } else {
            Ok(None)
        }
    }

    pub fn get(&self, preimage: &PreimageKey) -> Option<UtxoMetadata> {
        self.metadata.get(preimage).map(|m| m.clone())
    }

    pub fn garbage_collect(&self, config: &BloomConfig, now: u64) -> Result<u64, BloomError> {
        let current_epoch = now / config.epoch_duration_secs;
        let mut removed = 0;

        let epochs_to_clean: Vec<u64> = self.epoch_stats.iter()
            .filter(|entry| {
                if let Some(max_age) = config.max_utxo_age_secs {
                    let epoch_time = entry.key() * config.epoch_duration_secs;
                    now.saturating_sub(epoch_time) > max_age
                } else {
                    current_epoch.saturating_sub(*entry.key()) > 2
                }
            })
            .map(|entry| *entry.key())
            .collect();

        removed = epochs_to_clean.par_iter().map(|&epoch| {
            let mut epoch_removed = 0;
            let entries: Vec<(PreimageKey, StorageKey)> = self.metadata.iter()
                .filter(|entry| entry.value().epoch == epoch)
                .map(|entry| (
                    *entry.key(),
                    BloomFilterCore::compute_storage_key(&entry.value().txid, entry.value().vout),
                ))
                .collect();

            for (preimage, storage_key) in entries {
                if self.metadata.remove(&preimage).is_some() {
                    if let Some(storage) = &self.storage {
                        if let Err(e) = storage.remove_metadata(&storage_key) {
                            error!("Failed to remove expired UTXO: {}", e);
                            continue;
                        }
                    }
                    epoch_removed += 1;
                }
            }
            self.epoch_stats.remove(&epoch);
            epoch_removed
        }).sum();

        Ok(removed)
    }

    pub fn epoch_stats(&self) -> HashMap<u64, u64> {
        self.epoch_stats.iter().map(|entry| (*entry.key(), *entry.value())).collect()
    }
}

/// Core Bloom filter logic
struct BloomFilterCore {
    filter_data: Arc<BitVec<u64, Lsb0>>,
    config: BloomConfig,
    hash_cache: HashCache,
    item_count: AtomicU64,
    rate_limiter: RateLimiter,
    metrics: Metrics,
}

impl BloomFilterCore {
    pub fn new(config: BloomConfig, monitor: Option<Arc<dyn Monitor>>) -> Result<Self, BloomError> {
        config.validate()?;
        let bit_capacity = config.size.next_power_of_two();
        let mut bits = BitVec::with_capacity(bit_capacity);
        bits.resize(bit_capacity, false);

        let mut seeds = [0u32; 8];
        let mut hasher = blake3::Hasher::new();
        hasher.update(&config.tweak.to_le_bytes());
        let hash = hasher.finalize();
        for i in 0..8 {
            seeds[i] = u32::from_le_bytes([
                hash[i * 4],
                hash[i * 4 + 1],
                hash[i * 4 + 2],
                hash[i * 4 + 3],
            ]);
        }

        let metrics = Metrics::new();
        Ok(Self {
            filter_data: Arc::new(bits),
            config,
            hash_cache: HashCache {
                tweak: config.tweak,
                seeds,
                num_hashes: config.num_hashes,
            },
            item_count: AtomicU64::new(0),
            rate_limiter: RateLimiter::new(Duration::from_millis(10)),
            metrics,
        })
    }

    #[inline]
    fn compute_storage_key(txid: &Txid, vout: u32) -> StorageKey {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&txid[..]);
        hasher.update(&vout.to_le_bytes());
        let hash = hasher.finalize();
        hash.as_bytes()[0..16].try_into().unwrap()
    }

    #[inline]
    fn build_preimage(txid: &Txid, vout: u32) -> PreimageKey {
        let mut data = [0u8; 36];
        data[0..32].copy_from_slice(&txid[..]);
        data[32..36].copy_from_slice(&vout.to_le_bytes());
        data
    }

    #[inline]
    fn compute_hashes(&self, data: &[u8]) -> [u64; 2] {
        let key = self.hash_cache.tweak.to_le_bytes().repeat(8);
        let mut hasher = blake3::Hasher::new_keyed(&key[..32]);
        hasher.update(data);
        let hash1 = hasher.finalize();
        hasher.reset();
        hasher.update(hash1.as_bytes());
        let hash2 = hasher.finalize();
        [
            u64::from_le_bytes(hash1.as_bytes()[0..8].try_into().unwrap()),
            u64::from_le_bytes(hash2.as_bytes()[0..8].try_into().unwrap()),
        ]
    }

    #[inline]
    fn murmur_hash3(&self, hashes: [u64; 2], hash_num: u32) -> u64 {
        let seed = hash_num.wrapping_mul(0xFBA4C795).wrapping_add(self.hash_cache.tweak);
        let mut v = seed as u64 ^ hashes[1];
        v = v.wrapping_mul(0xFF51AFD7ED558CCD);
        v = v.wrapping_mul(0xC4CEB9FE1A85EC53);
        v ^= v >> 32;
        v ^ hashes[0]
    }

    #[inline]
    fn compute_positions(&self, preimage: &[u8]) -> Vec<usize> {
        let hashes = self.compute_hashes(preimage);
        (0..self.hash_cache.num_hashes)
            .map(|i| {
                let bit_pos = self.murmur_hash3(hashes, i as u32) % self.config.size as u64;
                bit_pos as usize
            })
            .collect()
    }

    #[inline]
    fn contains(&self, preimage: &[u8]) -> bool {
        let positions = self.compute_positions(preimage);
        positions.iter().all(|&pos| self.filter_data[pos])
    }

    pub fn insert(&self, preimage: &PreimageKey) -> Result<(), BloomError> {
        if !self.rate_limiter.allow() {
            return Err(BloomError::RateLimitExceeded);
        }
        let positions = self.compute_positions(preimage);
        positions.par_iter().for_each(|&pos| {
            Arc::make_mut(&mut self.filter_data.clone()).set(pos, true);
        });
        self.item_count.fetch_add(1, Ordering::Relaxed);
        self.metrics.inserts.inc();
        Ok(())
    }

    pub fn false_positive_rate(&self) -> f64 {
        let n = self.item_count.load(Ordering::Relaxed) as f64;
        let m = self.config.size as f64;
        let k = self.config.num_hashes as f64;
        let exponent = -k * n / m;
        if exponent < -20.0 {
            0.0
        } else {
            (1.0 - exponent.exp()).powf(k)
        }
    }

    pub fn memory_usage(&self) -> usize {
        let bitvec_size = self.filter_data.capacity() / 8;
        let metadata_size = self.item_count.load(Ordering::Relaxed) as usize * mem::size_of::<UtxoMetadata>();
        bitvec_size + metadata_size
    }
}

/// Metrics for monitoring
struct Metrics {
    inserts: Counter,
    false_positives: Counter,
    memory_usage: Gauge,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let inserts = Counter::new("bloom_inserts_total", "Total UTXO insertions").unwrap();
        let false_positives = Counter::new("bloom_false_positives_total", "Total false positives").unwrap();
        let memory_usage = Gauge::new("bloom_memory_usage_bytes", "Memory usage in bytes").unwrap();
        registry.register(Box::new(inserts.clone())).unwrap();
        registry.register(Box::new(false_positives.clone())).unwrap();
        registry.register(Box::new(memory_usage.clone())).unwrap();
        Self { inserts, false_positives, memory_usage }
    }
}

/// Typestate for initialization
pub struct Uninitialized;
pub struct Initialized;

pub struct BitcoinBloomFilter<State = Uninitialized> {
    core: Option<BloomFilterCore>,
    metadata_manager: Option<MetadataManager>,
    monitor: Option<Arc<dyn Monitor>>,
    state: std::marker::PhantomData<State>,
}

impl BitcoinBloomFilter<Uninitialized> {
    pub fn new() -> Self {
        Self {
            core: None,
            metadata_manager: None,
            monitor: None,
            state: std::marker::PhantomData,
        }
    }

    pub fn with_config(self, config: BloomConfig) -> Result<Self, BloomError> {
        let core = BloomFilterCore::new(config, self.monitor.clone())?;
        Ok(Self {
            core: Some(core),
            metadata_manager: self.metadata_manager,
            monitor: self.monitor,
            state: std::marker::PhantomData,
        })
    }

    pub fn with_storage(self, storage: Arc<dyn StorageBackend>) -> Self {
        let storage_layer = StorageLayer::new(storage);
        let metadata_manager = MetadataManager::new(Some(storage_layer));
        Self {
            core: self.core,
            metadata_manager: Some(metadata_manager),
            monitor: self.monitor,
            state: std::marker::PhantomData,
        }
    }

    pub fn with_monitor(self, monitor: Arc<dyn Monitor>) -> Self {
        Self {
            core: self.core,
            metadata_manager: self.metadata_manager,
            monitor: Some(monitor),
            state: std::marker::PhantomData,
        }
    }

    pub fn build(self) -> Result<BitcoinBloomFilter<Initialized>, BloomError> {
        let core = self.core.ok_or_else(|| BloomError::InvalidInput("Core not initialized".to_string()))?;
        let metadata_manager = self.metadata_manager.unwrap_or_else(|| MetadataManager::new(None));
        if let Some(storage) = &metadata_manager.storage {
            for entry in storage.backend.iter() {
                let (key, value) = entry.map_err(|e| BloomError::StorageError(e))?;
                if let Ok(StorageEntry::V1(metadata)) = bincode::deserialize(&value) {
                    if metadata.validate().is_ok() {
                        let preimage = BloomFilterCore::build_preimage(&metadata.txid, metadata.vout);
                        metadata_manager.metadata.insert(preimage, metadata.clone());
                        *metadata_manager.epoch_stats.entry(metadata.epoch).or_insert(0) += 1;
                    }
                }
            }
        }
        Ok(BitcoinBloomFilter {
            core: Some(core),
            metadata_manager: Some(metadata_manager),
            monitor: self.monitor,
            state: std::marker::PhantomData,
        })
    }
}

impl BitcoinBloomFilter<Initialized> {
    pub fn insert_utxo(&self, txid: &Txid, vout: u32) -> Result<(), BloomError> {
        if vout > MAX_REASONABLE_VOUT {
            return Err(BloomError::InvalidInput("Vout value too large".to_string()));
        }
        let core = self.core.as_ref().unwrap();
        let metadata_manager = self.metadata_manager.as_ref().unwrap();
        let preimage = BloomFilterCore::build_preimage(txid, vout);
        let metadata = UtxoMetadata::new(*txid, vout, core.config.epoch_duration_secs);

        core.insert(&preimage)?;
        metadata_manager.insert(preimage, metadata.clone())?;
        if let Some(monitor) = &self.monitor {
            monitor.on_insert(txid, vout);
        }
        if core.false_positive_rate() > core.config.max_fp_rate {
            warn!("False positive rate ({:.6}) exceeds max ({:.6})", 
                 core.false_positive_rate(), core.config.max_fp_rate);
        }
        Ok(())
    }

    pub fn contains_utxo(&self, txid: &Txid, vout: u32) -> bool {
        let core = self.core.as_ref().unwrap();
        let metadata_manager = self.metadata_manager.as_ref().unwrap();
        let preimage = BloomFilterCore::build_preimage(txid, vout);
        let present = core.contains(&preimage);
        if present {
            if let Some(metadata) = metadata_manager.get(&preimage) {
                if metadata.validate().is_ok() {
                    return true;
                }
            }
            if let Some(monitor) = &self.monitor {
                monitor.on_false_positive();
            }
            core.metrics.false_positives.inc();
        }
        false
    }

    pub fn load_block(&self, block: &Block) -> Result<(), BloomError> {
        let core = self.core.as_ref().unwrap();
        block.txdata.par_iter().try_for_each(|tx| {
            tx.output.iter().enumerate().try_for_each(|(vout, _)| {
                self.insert_utxo(&tx.txid(), vout as u32)
            })
        })?;
        info!("Loaded block: hash={}", block.block_hash());
        Ok(())
    }

    pub fn load_blocks(&self, blocks: &[Block]) -> Result<(), BloomError> {
        blocks.par_iter().try_for_each(|block| self.load_block(block))?;
        Ok(())
    }

    pub fn is_tokenizable(&self, txid: &Txid, vout: u32, min_age_secs: u64) -> bool {
        let core = self.core.as_ref().unwrap();
        let metadata_manager = self.metadata_manager.as_ref().unwrap();
        let preimage = BloomFilterCore::build_preimage(txid, vout);
        if !core.contains(&preimage) {
            return false;
        }
        if let Some(metadata) = metadata_manager.get(&preimage) {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
            let is_old_enough = now.saturating_sub(metadata.timestamp) >= min_age_secs;
            if is_old_enough {
                info!("Tokenizable UTXO: txid={}, vout={}, age={}s", txid, vout, 
                     now.saturating_sub(metadata.timestamp));
            }
            is_old_enough
        } else {
            false
        }
    }

    pub fn get_metadata(&self, txid: &Txid, vout: u32) -> Option<UtxoMetadata> {
        let metadata_manager = self.metadata_manager.as_ref().unwrap();
        let preimage = BloomFilterCore::build_preimage(txid, vout);
        metadata_manager.get(&preimage)
    }

    pub fn mark_spent_utxo(&self, txid: &Txid, vout: u32) -> Result<(), BloomError> {
        let metadata_manager = self.metadata_manager.as_ref().unwrap();
        let preimage = BloomFilterCore::build_preimage(txid, vout);
        metadata_manager.remove(&preimage)?;
        Ok(())
    }

    pub fn garbage_collect(&self) -> Result<u64, BloomError> {
        let core = self.core.as_ref().unwrap();
        let metadata_manager = self.metadata_manager.as_ref().unwrap();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let removed = metadata_manager.garbage_collect(&core.config, now)?;
        if removed > 0 {
            core.item_count.fetch_sub(removed, Ordering::Relaxed);
            info!("Garbage collected {} expired UTXOs", removed);
        }
        Ok(removed)
    }

    pub fn resize_if_needed(&mut self) -> Result<bool, BloomError> {
        let core = self.core.as_ref().unwrap();
        if core.false_positive_rate() <= core.config.max_fp_rate {
            return Ok(false);
        }

        let new_size = BloomConfig::calculate_optimal_size(
            core.item_count.load(Ordering::Relaxed) as usize,
            core.config.max_fp_rate
        );
        let new_hashes = BloomConfig::calculate_optimal_hashes(
            new_size,
            core.item_count.load(Ordering::Relaxed) as usize
        );

        let new_config = BloomConfig {
            size: new_size,
            num_hashes: new_hashes,
            tweak: rand::random(),
            ..core.config.clone()
        };

        let metadata_manager = self.metadata_manager.take().unwrap();
        let mut new_filter = BitcoinBloomFilter::new()
            .with_config(new_config)?
            .with_monitor(self.monitor.clone())
            .build()?;

        metadata_manager.metadata.par_iter().try_for_each(|entry| {
            let metadata = entry.value();
            new_filter.insert_utxo(&metadata.txid, metadata.vout)
        })?;

        *self = new_filter;
        self.metadata_manager = Some(metadata_manager);
        info!("Resized filter: new_size={}, new_hashes={}", new_config.size, new_hashes);

        Ok(true)
    }

    pub fn save_to_storage(&self) -> Result<(), BloomError> {
        let core = self.core.as_ref().unwrap();
        let metadata_manager = self.metadata_manager.as_ref().unwrap();
        if let Some(storage) = &metadata_manager.storage {
            let mut snapshot = FilterSnapshot {
                version: CURRENT_VERSION,
                data: core.filter_data.as_raw_slice().to_vec(),
                metadata: metadata_manager.metadata.iter().map(|entry| entry.value().clone()).collect(),
                config: core.config.clone(),
                checksum: [0u8; 32],
            };
            let serialized = bincode::serialize(&snapshot)
                .map_err(|e| BloomError::SerializationError(e.to_string()))?;
            snapshot.checksum = blake3::hash(&serialized).into();
            let final_serialized = bincode::serialize(&snapshot)
                .map_err(|e| BloomError::SerializationError(e.to_string()))?;
            storage.backend.insert(b"filter_data", &final_serialized)
                .map_err(|e| BloomError::StorageError(e))?;
            storage.flush()?;
            info!("Saved filter_data to storage (version={})", CURRENT_VERSION);
        }
        Ok(())
    }

    pub fn export_snapshot(&self) -> Result<Vec<u8>, BloomError> {
        let core = self.core.as_ref().unwrap();
        let metadata_manager = self.metadata_manager.as_ref().unwrap();
        let snapshot = FilterSnapshot {
            version: CURRENT_VERSION,
            data: core.filter_data.as_raw_slice().to_vec(),
            metadata: metadata_manager.metadata.iter().map(|entry| entry.value().clone()).collect(),
            config: core.config.clone(),
            checksum: [0u8; 32],
        };
        let serialized = bincode::serialize(&snapshot)
            .map_err(|e| BloomError::SerializationError(e.to_string()))?;
        let checksum = blake3::hash(&serialized).into();
        let final_snapshot = FilterSnapshot {
            checksum,
            ..snapshot
        };
        bincode::serialize(&final_snapshot)
            .map_err(|e| BloomError::SerializationError(e.to_string()))
    }

    pub fn memory_usage(&self) -> usize {
        self.core.as_ref().unwrap().memory_usage()
    }

    pub fn epoch_stats(&self) -> HashMap<u64, u64> {
        self.metadata_manager.as_ref().unwrap().epoch_stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::blockdata::transaction::{Transaction, TxOut};
    use rand::Rng;
    use proptest::prelude::*;

    struct TestMonitor;

    impl Monitor for TestMonitor {
        fn on_insert(&self, _txid: &Txid, _vout: u32) {}
        fn on_false_positive(&self) {}
    }

    proptest! {
        #[test]
        fn test_insert_contains_property(txid in any::<[u8; 32]>(), vout in 0..1000u32) {
            let config = BloomConfig::default();
            let filter = BitcoinBloomFilter::new()
                .with_config(config)
                .unwrap()
                .build()
                .unwrap();
            let txid = Txid::from_byte_array(txid);
            filter.insert_utxo(&txid, vout).unwrap();
            prop_assert!(filter.contains_utxo(&txid, vout));
        }
    }

    #[test]
    fn test_high_volume_performance() {
        let config = BloomConfig {
            size: 1 << 22,
            ..Default::default()
        };
        let filter = BitcoinBloomFilter::new()
            .with_config(config)
            .build()
            .unwrap();

        let start = Instant::now();
        for _ in 0..100_000 {
            let txid = Txid::from_byte_array(rand::random());
            filter.insert_utxo(&txid, 0).unwrap();
        }
        println!("Inserted 100K UTXOs in {:?}", start.elapsed());
        assert!(filter.false_positive_rate() < 0.01);
    }

    #[test]
    fn test_edge_cases() {
        let config = BloomConfig::default();
        let filter = BitcoinBloomFilter::new()
            .with_config(config)
            .build()
            .unwrap();

        let mut rng = rand::thread_rng();
        let txid = Txid::from_byte_array(rng.gen());
        assert!(!filter.contains_utxo(&txid, 0));

        filter.insert_utxo(&txid, MAX_REASONABLE_VOUT - 1).unwrap();
        assert!(filter.contains_utxo(&txid, MAX_REASONABLE_VOUT - 1));

        assert!(filter.insert_utxo(&txid, MAX_REASONABLE_VOUT + 1).is_err());
    }

    #[test]
    fn test_rate_limiting() {
        let config = BloomConfig::default();
        let filter = BitcoinBloomFilter::new()
            .with_config(config)
            .build()
            .unwrap();

        let txid = Txid::from_byte_array(rand::random());
        filter.core.as_ref().unwrap(). |mut core| core.rate_limiter = RateLimiter::new(Duration::from_millis(1));

        assert!(filter.insert_utxo(&Txid::from_byte_array([0; 32]), 0).is_ok());
        assert!(filter.insert_utxo(&Txid::from_byte_array([1; 32]), 0).is_err());
    }
}