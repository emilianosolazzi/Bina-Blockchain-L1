use std::sync::atomic::{AtomicU64, Ordering};
use rayon::prelude::*;
use bitcoin::{Block, TxOut, Txid};
use bitcoin::hashes::{Hash, HashEngine};
use bitcoin::hashes::sha256;
use std::time::{SystemTime, UNIX_EPOCH};
use dashmap::DashMap;
use std::sync::Arc;

/// Configuration matching Bitcoin Core's Bloom filter parameters
#[derive(Clone, Debug)]
pub struct BloomConfig {
    pub size: usize,          // Filter size in bits (must be power of two)
    pub num_hashes: u8,       // Number of hash functions (2-7)
    pub tweak: u32,           // Random value to modify hash functions
    pub flags: u8,            // Filter update flags
}

impl Default for BloomConfig {
    fn default() -> Self {
        BloomConfig {
            size: 36_000,      // Bitcoin Core default size
            num_hashes: 5,     // Optimal for Bitcoin's use cases
            tweak: rand::random(),
            flags: 0,
        }
    }
}

/// Optimized Bitcoin Bloom Filter implementation
pub struct BitcoinBloomFilter {
    filter_data: Vec<AtomicU64>,  // Bit array (atomic for thread safety)
    config: BloomConfig,
    item_count: AtomicU64,
    hash_seeds: [u32; 8],        // Pre-computed hash seeds
    timestamps: Arc<DashMap<Vec<u8>, u64>>, // Thread-safe timestamp tracking
}

impl BitcoinBloomFilter {
    /// Create a new Bloom filter with optional configuration
    pub fn new(config: Option<BloomConfig>) -> Self {
        let cfg = config.unwrap_or_default();
        assert!(cfg.size.is_power_of_two(), "Size must be power of two");
        assert!((2..=7).contains(&cfg.num_hashes), "Number of hashes must be 2-7");

        let bucket_count = (cfg.size + 63) / 64; // Round up to 64-bit chunks
        let mut hash_seeds = [0u32; 8];
        
        // Generate hash seeds using Bitcoin's standard method
        let mut engine = sha256::HashEngine::default();
        engine.input(&cfg.tweak.to_le_bytes());
        let hash = sha256::Hash::from_engine(engine);
        
        for i in 0..8 {
            hash_seeds[i] = u32::from_le_bytes([hash[i*4], hash[i*4+1], hash[i*4+2], hash[i*4+3]]);
        }

        BitcoinBloomFilter {
            filter_data: (0..bucket_count).map(|_| AtomicU64::new(0)).collect(),
            config: cfg,
            item_count: AtomicU64::new(0),
            hash_seeds,
            timestamps: Arc::new(DashMap::new()),
        }
    }

    /// Insert a UTXO into the filter (txid + vout)
    pub fn insert_utxo(&self, txid: &Txid, vout: u32) {
        let mut preimage = Vec::with_capacity(36);
        preimage.extend_from_slice(&txid[..]);
        preimage.extend_from_slice(&vout.to_le_bytes());
        self.insert(&preimage);
    }

    /// Core insertion method using Bitcoin's standard hash mixing
    fn insert(&self, data: &[u8]) {
        let hashes = self.compute_hashes(data);
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

        (0..self.config.num_hashes).into_par_iter().for_each(|i| {
            let bit_pos = self.murmur_hash3(hashes, i as u32) % (self.config.size as u64);
            let bucket_idx = (bit_pos >> 6) as usize;
            let bit_mask = 1u64 << (bit_pos & 0x3F);

            self.filter_data[bucket_idx].fetch_or(bit_mask, Ordering::Relaxed);
        });

        self.item_count.fetch_add(1, Ordering::Relaxed);
        self.timestamps.insert(data.to_vec(), timestamp);
    }

    /// Check if a UTXO might be in the filter
    pub fn contains_utxo(&self, txid: &Txid, vout: u32) -> bool {
        let mut preimage = Vec::with_capacity(36);
        preimage.extend_from_slice(&txid[..]);
        preimage.extend_from_slice(&vout.to_le_bytes());
        self.contains(&preimage)
    }

    /// Core membership check
    fn contains(&self, data: &[u8]) -> bool {
        let hashes = self.compute_hashes(data);
        (0..self.config.num_hashes).into_par_iter().all(|i| {
            let bit_pos = self.murmur_hash3(hashes, i as u32) % (self.config.size as u64);
            let bucket_idx = (bit_pos >> 6) as usize;
            let bit_mask = 1u64 << (bit_pos & 0x3F);

            (self.filter_data[bucket_idx].load(Ordering::Relaxed) & bit_mask) != 0
        })
    }

    /// Compute hash values using Bitcoin's standard method
    fn compute_hashes(&self, data: &[u8]) -> [u64; 2] {
        let mut engine = sha256::HashEngine::default();
        engine.input(data);
        let hash1 = sha256::Hash::from_engine(engine);
        
        let mut engine = sha256::HashEngine::default();
        engine.input(&hash1[..]);
        let hash2 = sha256::Hash::from_engine(engine);
        
        [
            u64::from_le_bytes([hash1[0], hash1[1], hash1[2], hash1[3], hash1[4], hash1[5], hash1[6], hash1[7]]),
            u64::from_le_bytes([hash2[0], hash2[1], hash2[2], hash2[3], hash2[4], hash2[5], hash2[6], hash2[7]]),
        ]
    }

    /// Bitcoin's MurmurHash3 implementation for final bit position
    fn murmur_hash3(&self, hash: [u64; 2], hash_num: u32) -> u64 {
        let h = hash_num.wrapping_mul(0xFBA4C795).wrapping_add(self.config.tweak);
        let mut v = h as u64 ^ hash[1];
        v = v.wrapping_mul(0xFF51AFD7ED558CCD);
        v = v.wrapping_mul(0xC4CEB9FE1A85EC53);
        v ^= v >> 32;
        v ^ hash[0]
    }

    /// Load all transactions from a block into the filter
    pub fn load_block(&self, block: &Block) {
        block.txdata.par_iter().for_each(|tx| {
            self.insert(&tx.txid().to_byte_array());
            
            tx.output.iter().enumerate().for_each(|(vout, _)| {
                self.insert_utxo(&tx.txid(), vout as u32);
            });
        });
    }

    /// Get current false positive rate
    pub fn false_positive_rate(&self) -> f64 {
        let n = self.item_count.load(Ordering::Relaxed) as f64;
        let m = self.config.size as f64;
        let k = self.config.num_hashes as f64;
        (1.0 - (-k * n / m).exp()).powf(k)
    }

    /// Serialize to Bitcoin's P2P message format
    pub fn to_p2p_message(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(9 + self.filter_data.len() * 8);
        data.push(self.config.num_hashes);
        data.extend_from_slice(&self.config.tweak.to_le_bytes());
        data.push(self.config.flags);
        
        self.filter_data.iter().for_each(|bucket| {
            data.extend_from_slice(&bucket.load(Ordering::Relaxed).to_le_bytes());
        });
        
        data
    }
}