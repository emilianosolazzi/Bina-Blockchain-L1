use std::sync::atomic::{AtomicU64, Ordering};
use rayon::prelude::*;
use siphasher::sip::SipHasher24;
use std::hash::{Hash as _, Hasher};
use std::simd::{u64x8, Simd};
use bitcoin::{Block, Txid};
use bitcoin::hashes::Hash;
use serde::{Serialize, Deserialize};
use rand::rngs::OsRng;
use rand::RngCore;
use std::time::{SystemTime, UNIX_EPOCH};
use std::collections::HashMap;

#[derive(Clone, Serialize, Deserialize)]
pub struct BloomConfig {
    size: usize,
    num_hashes: u8,
    tweak: u32,
    flags: u8,
}

impl Default for BloomConfig {
    fn default() -> Self {
        let mut tweak = [0u8; 4];
        OsRng.fill_bytes(&mut tweak);
        BloomConfig {
            size: 1 << 22,
            num_hashes: 5,
            tweak: u32::from_le_bytes(tweak),
            flags: 0,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct BitcoinBloomFilter {
    buckets: Vec<AtomicU64>,
    config: BloomConfig,
    item_count: AtomicU64,
    hash_seeds: [u64; 8],
    timestamps: HashMap<Vec<u8>, u64>,
}

impl BitcoinBloomFilter {
    pub fn new(config: Option<BloomConfig>) -> Self {
        let cfg = config.unwrap_or_default();
        assert!(cfg.size.is_power_of_two(), "Size must be power of two");
        assert!((2..=7).contains(&cfg.num_hashes), "Use 2-7 hashes for Bitcoin");

        let bucket_count = (cfg.size + 63) / 64;
        let mut hash_seeds = [0u64; 8];
        let mut hasher = SipHasher24::new_with_keys(cfg.tweak as u64, 0);
        cfg.tweak.hash(&mut hasher);

        for seed in &mut hash_seeds {
            *seed = hasher.finish();
            hasher.write_u64(*seed);
        }

        BitcoinBloomFilter {
            buckets: (0..bucket_count).map(|_| AtomicU64::new(0)).collect(),
            config: cfg,
            item_count: AtomicU64::new(0),
            hash_seeds,
            timestamps: HashMap::new(),
        }
    }

    pub fn insert_utxo(&mut self, txid: &Txid, vout: u32) {
        let mut preimage = Vec::with_capacity(36);
        preimage.extend_from_slice(&txid[..]);
        preimage.extend_from_slice(&vout.to_le_bytes());
        self.insert(&preimage);
    }

    pub fn insert_witness(&mut self, witness_program: &[u8], node_salt: u64) {
        let mut hasher = SipHasher24::new_with_keys(node_salt, self.config.tweak as u64);
        witness_program.hash(&mut hasher);
        self.insert(&hasher.finish().to_le_bytes());
    }

    fn insert(&mut self, data: &[u8]) {
        let hashes = self.compute_hashes(data);
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

        (0..self.config.num_hashes).into_par_iter().for_each(|i| {
            let bit_pos = hashes[i as usize] % (self.config.size as u64);
            let bucket_idx = (bit_pos >> 6) as usize;
            let bit_mask = 1u64 << (bit_pos & 0x3F);

            if bucket_idx >= self.buckets.len() {
                panic!("Bucket index out of bounds");
            }
            self.buckets[bucket_idx].fetch_or(bit_mask, Ordering::SeqCst);
        });

        self.item_count.fetch_add(1, Ordering::SeqCst);
        self.timestamps.insert(data.to_vec(), timestamp);
    }

    pub fn load_block(&mut self, block: &Block) {
        block.txdata.par_iter().for_each(|tx| {
            let txid = tx.txid().to_byte_array();
            let mut hashes = [0u64; 8];
            let seeds = u64x8::from_array(self.hash_seeds);
            let mut hasher = SipHasher24::new_with_keys(seeds[0], self.config.tweak as u64);
            txid.hash(&mut hasher);
            let base_hash = hasher.finish();
            (0..8).for_each(|i| hashes[i] = base_hash ^ self.hash_seeds[i]);
            self.insert(&txid);

            tx.output.iter().enumerate().for_each(|(vout, _)| {
                self.insert_utxo(&tx.txid(), vout as u32);
            });
        });
    }

    pub fn contains_utxo(&self, txid: &Txid, vout: u32) -> bool {
        let mut preimage = Vec::with_capacity(36);
        preimage.extend_from_slice(&txid[..]);
        preimage.extend_from_slice(&vout.to_le_bytes());
        self.contains(&preimage)
    }

    pub fn contains(&self, data: &[u8]) -> bool {
        let hashes = self.compute_hashes(data);
        (0..self.config.num_hashes).into_par_iter().all(|i| {
            let bit_pos = hashes[i as usize] % (self.config.size as u64);
            let bucket_idx = (bit_pos >> 6) as usize;
            let bit_mask = 1u64 << (bit_pos & 0x3F);

            if bucket_idx >= self.buckets.len() {
                return false;
            }
            (self.buckets[bucket_idx].load(Ordering::SeqCst) & bit_mask) != 0
        })
    }

    fn compute_hashes(&self, data: &[u8]) -> [u64; 8] {
        let mut hashes = [0u64; 8];
        (0..8).into_par_iter().for_each(|i| {
            let mut hasher = SipHasher24::new_with_keys(self.hash_seeds[i], self.config.tweak as u64);
            data.hash(&mut hasher);
            hashes[i] = hasher.finish();
        });
        hashes
    }

    pub fn to_bip37(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(9 + self.buckets.len() * 8);
        data.push(self.config.num_hashes);
        data.extend_from_slice(&self.config.tweak.to_le_bytes());
        data.push(self.config.flags);
        self.buckets.iter().for_each(|bucket| {
            data.extend_from_slice(&bucket.load(Ordering::SeqCst).to_le_bytes());
        });
        data
    }

    pub fn resize(&mut self, expected_items: u64, target_fpr: f64) {
        let m = self.config.size as f64;
        let optimal_hashes = (-(m / expected_items as f64) * target_fpr.ln() / 2.0_f64.ln()).round() as u8;
        let new_hashes = optimal_hashes.clamp(2, 7);
        let new_size = (self.config.size as u64 * 2).next_power_of_two() as usize;

        let mut new_config = self.config.clone();
        new_config.size = new_size;
        new_config.num_hashes = new_hashes;
        let mut tweak = [0u8; 4];
        OsRng.fill_bytes(&mut tweak);
        new_config.tweak = u32::from_le_bytes(tweak);

        let mut new_filter = BitcoinBloomFilter::new(Some(new_config));
        self.timestamps.iter().for_each(|(data, _)| {
            new_filter.insert(data);
        });

        *self = new_filter;
    }

    pub fn prune(&mut self, threshold_timestamp: u64) {
        self.timestamps.retain(|_, &mut ts| ts >= threshold_timestamp);
        let bucket_count = (self.config.size + 63) / 64;
        self.buckets = (0..bucket_count).map(|_| AtomicU64::new(0)).collect();
        self.item_count.store(0, Ordering::SeqCst);
        for (data, _) in self.timestamps.iter() {
            self.insert(data);
        }
    }

    pub fn false_positive_rate(&self) -> f64 {
        let n = self.item_count.load(Ordering::SeqCst) as f64;
        let m = self.config.size as f64;
        let k = self.config.num_hashes as f64;
        (1.0 - (-k * n / m).exp()).powf(k)
    }

    pub fn to_p2p_message(&self) -> Vec<u8> {
        let mut msg = Vec::new();
        msg.extend_from_slice(b"bloomfilter");
        msg.extend_from_slice(&self.to_bip37());
        msg
    }

    pub fn to_grpc(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap()
    }

    pub fn log_stats(&self) -> String {
        format!(
            "FPR: {:.2}%, Items: {}, Size: {} bits",
            self.false_positive_rate() * 100.0,
            self.item_count.load(Ordering::SeqCst),
            self.config.size
        )
    }
}

extern "C" {
    BloomFilterHandle bf_create(size_t size, uint8_t num_hashes, uint32_t tweak);
    void bf_insert_utxo(void* handle, const unsigned char* txid, uint32_t vout);
    void bf_insert_witness(void* handle, const unsigned char* witness, size_t len, uint64_t node_salt);
    void bf_load_block(void* handle, const unsigned char* block, size_t len);
    bool bf_contains_utxo(void* handle, const unsigned char* txid, uint32_t vout);
    void bf_resize(void* handle, uint64_t expected_items, double target_fpr, BloomFilterHandle* out);
    void bf_prune(void* handle, uint64_t threshold_timestamp);
    double bf_false_positive_rate(void* handle);
    char* bf_log_stats(void* handle);
    void bf_free(void* handle);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_contains() {
        let mut filter = BitcoinBloomFilter::new(None);
        let txid = Txid::all_zeros();
        filter.insert_utxo(&txid, 0);
        assert!(filter.contains_utxo(&txid, 0));
    }

    #[test]
    fn test_resize() {
        let mut filter = BitcoinBloomFilter::new(None);
        let old_size = filter.config.size;
        filter.resize(1_000_000, 0.002);
        assert!(filter.config.size > old_size);
    }
}