#[cfg(test)]
mod tests {
    use super::{BitcoinBloomFilter, BloomConfig};
    use bitcoin::{Txid, hashes::Hash};
    use rand::Rng;
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::collections::HashSet;

    #[test]
    fn test_false_positive_rate_for_utxo_tokenization() {
        // Test configurations for different scales and scenarios
        let configs = [
            // Small-scale: Default settings for quick validation
            (36_000, 5, 1_000, 100_000, "Small-scale"),
            // Medium-scale: Larger filter for realistic UTXO sets
            (72_000, 5, 10_000, 100_000, "Medium-scale"),
            // Large-scale: Simulates full blockchain scanning
            (360_000, 7, 100_000, 1_000_000, "Large-scale"),
            // Saturated: Tests filter near capacity
            (36_000, 5, 5_000, 100_000, "Saturated"),
        ];

        for (size, num_hashes, num_insertions, num_queries, config_name) in configs {
            println!("\n=== Testing Configuration: {} ===", config_name);
            println!("Size: {} bits, Hashes: {}, Insertions: {}, Queries: {}", 
                     size, num_hashes, num_insertions, num_queries);

            // Initialize Bloom filter
            let config = BloomConfig {
                size,
                num_hashes,
                tweak: rand::random(),
                flags: 0,
            };
            let bloom_filter = BitcoinBloomFilter::new(Some(config));

            // Track inserted and spent UTXOs
            let mut inserted_utxos = HashSet::new();
            let mut spent_utxos = HashSet::new();
            let mut rng = rand::thread_rng();

            // Simulate realistic UTXOs with "lost" timestamps
            let current_time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let lost_threshold = current_time - 5 * 365 * 24 * 60 * 60; // 5 years ago

            // Measure insertion performance
            let start = SystemTime::now();
            for _ in 0..num_insertions {
                let txid_bytes: [u8; 32] = rng.gen();
                let txid = Txid::from_byte_array(txid_bytes);
                let vout = rng.gen::<u32>() % 100; // Realistic vout range (0-99)
                bloom_filter.insert_utxo(&txid, vout);
                inserted_utxos.insert((txid, vout));

                // Simulate timestamp for "lost" UTXOs (5-10 years old)
                let timestamp = lost_threshold - (rng.gen::<u64>() % (5 * 365 * 24 * 60 * 60));
                let mut preimage = Vec::with_capacity(36);
                preimage.extend_from_slice(&txid[..]);
                preimage.extend_from_slice(&vout.to_le_bytes());
                bloom_filter.timestamps.insert(preimage, timestamp);
            }
            let insertion_time = start.elapsed().unwrap().as_secs_f64();

            // Simulate spent UTXOs (10% of inserted UTXOs)
            let num_spent = (num_insertions / 10) as usize;
            for _ in 0..num_spent {
                if let Some(&(txid, vout)) = inserted_utxos.iter().next() {
                    spent_utxos.insert((txid, vout));
                    inserted_utxos.remove(&(txid, vout));
                    // Note: Bloom filter cannot unset bits; track externally
                }
            }

            // Query non-inserted and spent UTXOs for false positives
            let mut false_positives = 0;
            let mut spent_false_positives = 0;
            let start = SystemTime::now();
            for _ in 0..num_queries {
                let txid_bytes: [u8; 32] = rng.gen();
                let txid = Txid::from_byte_array(txid_bytes);
                let vout = rng.gen::<u32>() % 100;
                if !inserted_utxos.contains(&(txid, vout)) && bloom_filter.contains_utxo(&txid, vout) {
                    false_positives += 1;
                }
            }
            for (txid, vout) in spent_utxos.iter() {
                if bloom_filter.contains_utxo(txid, *vout) {
                    spent_false_positives += 1;
                }
            }
            let query_time = start.elapsed().unwrap().as_secs_f64();

            // Calculate empirical false positive rates
            let empirical_rate = false_positives as f64 / num_queries as f64;
            let spent_rate = if num_spent > 0 { spent_false_positives as f64 / num_spent as f64 } else { 0.0 };
            let theoretical_rate = bloom_filter.false_positive_rate();

            // Print results
            println!("Insertion time: {:.3} seconds", insertion_time);
            println!("Query time: {:.3} seconds", query_time);
            println!("Empirical false positive rate (non-inserted): {:.6}%", empirical_rate * 100.0);
            println!("Empirical false positive rate (spent UTXOs): {:.6}%", spent_rate * 100.0);
            println!("Theoretical false positive rate: {:.6}%", theoretical_rate * 100.0);

            // Assert empirical rate is close to theoretical
            let tolerance = 0.05; // 5% deviation due to randomness
            assert!(
                (empirical_rate - theoretical_rate).abs() < tolerance,
                "Empirical rate ({:.6}) deviates too much from theoretical ({:.6}) for {}",
                empirical_rate, theoretical_rate, config_name
            );

            // Verify compliance metadata (timestamps for "lost" UTXOs)
            let mut valid_timestamps = 0;
            for entry in bloom_filter.timestamps.iter() {
                if *entry.value() <= lost_threshold {
                    valid_timestamps += 1;
                }
            }
            println!("Valid 'lost' UTXO timestamps: {} / {}", valid_timestamps, num_insertions);
            assert!(
                valid_timestamps >= num_insertions * 3 / 4,
                "Too few valid 'lost' timestamps ({}/{} required) for {}",
                valid_timestamps, num_insertions * 3 / 4, config_name
            );
        }
    }
}