use std::time::{Instant, Duration};
use num_bigint::BigUint;
use num_traits::{Zero, One};
use rayon::prelude::*; // For parallel processing
use serde::{Serialize, Deserialize};
use fixed::types::U64F64; // Fixed-point arithmetic for improved precision
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;
use rand::RngCore;

// Constants aligned with Solidity
const ENTROPY_MIN_SCORE: u64 = 100;
const ENTROPY_MAX_SCORE: u64 = 1000;
const ENTROPY_TIER_1_THRESHOLD: u64 = 300;
const ENTROPY_TIER_2_THRESHOLD: u64 = 600;
const ENTROPY_TIER_3_THRESHOLD: u64 = 850;

const ERROR_INSUFFICIENT_ENTROPY: u8 = 1;
const ERROR_PATTERN_DETECTED: u8 = 2;
const ERROR_PREDICTABILITY_RISK: u8 = 3;
const ERROR_DISTRIBUTION_ANOMALY: u8 = 4;

// Enhanced error handling with an enum
#[derive(Debug)]
pub enum EntropyError {
    TooShort,
    LowShannonEntropy(u64, u64), // (actual, required)
    LowMinEntropy(u64, u64),     // (actual, required)
    PatternDetected(i64),        // pattern score
    DistributionAnomaly(f64),    // chi-square value
    InsufficientVariance(u32),   // unique byte count
}

// Struct for ZK proof preparation results
#[derive(Serialize, Deserialize, Debug)]
pub struct ZKProofPrepResult {
    score: u64,
    tier: u8,
    shannon_entropy: u64,
    min_entropy: u64,
    suitable: bool,
    // Add validation report
    pub validation_details: Option<ValidationDetails>,
}

// Added for more detailed validation reporting
#[derive(Serialize, Deserialize, Debug)]
pub struct ValidationDetails {
    pub unique_bytes: u32,
    pub chi_square: f64,
    pub autocorrelation: i32,
    pub bit_balance_ratio: f64,
    pub pattern_findings: Vec<String>,
}

// For cross-language validation
#[derive(Serialize, Deserialize)]
pub struct VerificationResult {
    pub score: u64,
    pub tier: u8,
    pub passed: bool,
    pub shannon_entropy: u64,
    pub min_entropy: u64,
}

// Offline EntropyQualityLib implementation
pub struct EntropyQualityLib;

impl EntropyQualityLib {
    /// Assess pattern quality of entropy (returns -100 to +100 adjustment)
    /// Enhanced with autocorrelation detection for subtle patterns
    pub fn assess_pattern_quality(entropy: &[u8; 32]) -> i64 {
        let mut frequencies = [0u8; 256];
        for &b in entropy.iter() {
            frequencies[b as usize] += 1;
        }

        // Count unique bytes (for variability check)
        let unique_bytes = frequencies.iter().filter(|&&freq| freq > 0).count() as u32;

        let mut chi_squared = 0;
        for &freq in frequencies.iter() {
            if freq > 0 && freq > 2 {
                chi_squared += (freq as u64 - 1) * (freq as u64 - 1);
            }
        }

        let mut run_count = 0;
        for i in 1..32 {
            if entropy[i] == entropy[i - 1] || entropy[i] == entropy[i - 1] + 1 {
                run_count += 1;
            }
        }

        // Add autocorrelation check for subtle patterns
        let mut autocorr = 0;
        for lag in 1..8 { // Check various lag values
            let mut lag_score = 0;
            for i in lag..32 {
                lag_score += (entropy[i] as i32 - entropy[i-lag] as i32).pow(2);
            }
            autocorr += lag_score / (32 - lag) as i32;
        }
        let autocorr_score = if autocorr < 100 { -50 } else if autocorr < 200 { -20 } else { 0 };

        // Check for sequential byte patterns
        let mut seq_patterns = 0;
        for i in 2..32 {
            if entropy[i] == entropy[i-2] {
                seq_patterns += 1;
            }
        }
        let seq_score = if seq_patterns > 10 { -30 } else if seq_patterns > 5 { -15 } else { 0 };

        // Standard adjustments
        let mut adjustment = 0;
        if chi_squared < 5 { adjustment += 50; } else if chi_squared < 10 { adjustment += 20; } else if chi_squared > 20 { adjustment -= 50; } else if chi_squared > 15 { adjustment -= 20; }
        if run_count < 3 { adjustment += 50; } else if run_count < 5 { adjustment += 20; } else if run_count > 10 { adjustment -= 50; } else if run_count > 8 { adjustment -= 20; }
        
        // Add new pattern scores
        adjustment += autocorr_score;
        adjustment += seq_score;
        
        // Penalize severely if very few unique bytes
        if unique_bytes < 16 { 
            adjustment -= 70; 
        }
        
        adjustment.clamp(-100, 100)
    }

    /// Calculate Shannon entropy (scaled by 1e18)
    pub fn calculate_shannon_entropy(data: &[u8]) -> u64 {
        let len = data.len() as u64;
        if len == 0 { return 0; }

        let mut frequencies = [0u64; 256];
        for &b in data {
            frequencies[b as usize] += 1;
        }

        let mut entropy = 0;
        for &freq in frequencies.iter() {
            if freq == 0 { continue; }
            let probability = (freq * 1_000_000_000_000_000_000) / len; // Scaled by 1e18
            let log_term = Self::approximate_log2_scaled(probability);
            entropy += (probability * log_term) / 1_000_000_000_000_000_000;
        }
        entropy
    }

    /// Improved log2 approximation using fixed-point arithmetic
    fn approximate_log2_scaled(x: u64) -> u64 {
        if x == 0 { return 0; }
        if x == 1_000_000_000_000_000_000 { return 0; }
        
        // Use fixed-point arithmetic for better precision
        let x_fp = U64F64::from_num(x) / U64F64::from_num(1_000_000_000_000_000_000);
        let log2 = x_fp.checked_log2().unwrap_or(U64F64::ZERO);
        (log2 * U64F64::from_num(1_000_000_000_000_000_000)).to_num()
    }

    /// Alternative implementation using Taylor series if fixed-point isn't available
    fn fallback_log2_scaled(x: u64) -> u64 {
        if x == 0 { return 0; }
        if x == 1_000_000_000_000_000_000 { return 0; }
        
        // Existing approximation as fallback
        let y = (1_000_000_000_000_000_000 - x) * 1_000_000_000 / x; // Scaled
        let y2 = (y * y) / 1_000_000_000;
        let y3 = (y2 * y) / 1_000_000_000;
        let ln_scaled = y - y2 / 2 + y3 / 3;
        let log2_e_scaled = 1_442695040; // log2(e) * 1e9
        (ln_scaled * log2_e_scaled) / 1_000_000_000
    }

    /// Improved min-entropy calculation
    pub fn approximate_min_entropy(data: &[u8]) -> u64 {
        let len = data.len() as u64;
        if len == 0 { return 0; }

        let mut frequencies = [0u64; 256];
        let mut max_freq = 0;
        for &b in data {
            frequencies[b as usize] += 1;
            if frequencies[b as usize] > max_freq { max_freq = frequencies[b as usize]; }
        }

        let p_max = (max_freq * 1_000_000_000_000_000_000) / len;
        Self::approximate_log2_scaled(p_max)
    }

    /// Map entropy score to tier (1-4)
    pub fn entropy_score_to_tier(score: u64) -> u8 {
        if score >= ENTROPY_TIER_3_THRESHOLD { 4 } else if score >= ENTROPY_TIER_2_THRESHOLD { 3 } else if score >= ENTROPY_TIER_1_THRESHOLD { 2 } else { 1 }
    }

    /// Enhanced entropy validation with richer error information
    pub fn validate_entropy_for_zk_proof(data: &[u8], min_shannon_entropy: u64, min_min_entropy: u64) -> Result<(), EntropyError> {
        if data.len() < 16 {
            return Err(EntropyError::TooShort);
        }

        let shannon_entropy = Self::calculate_shannon_entropy(data);
        if shannon_entropy < min_shannon_entropy {
            return Err(EntropyError::LowShannonEntropy(shannon_entropy, min_shannon_entropy));
        }

        let min_entropy = Self::approximate_min_entropy(data);
        if min_entropy < min_min_entropy {
            return Err(EntropyError::LowMinEntropy(min_entropy, min_min_entropy));
        }

        // Check byte distribution and variety
        let mut counts = [0u16; 256];
        for &b in data {
            counts[b as usize] += 1;
        }
        let unique_bytes = counts.iter().filter(|&&c| c > 0).count() as u32;
        if unique_bytes < 16 {
            return Err(EntropyError::InsufficientVariance(unique_bytes));
        }

        // Chi-square test for uniform distribution
        let expected = data.len() as f64 / 256.0;
        let chi_sq: f64 = counts.iter()
            .map(|&c| {
                let diff = c as f64 - expected;
                diff * diff / expected
            })
            .sum();
            
        if chi_sq > 350.0 {
            return Err(EntropyError::DistributionAnomaly(chi_sq));
        }

        // Pattern analysis
        if data.len() >= 32 {
            let pattern_score = Self::assess_pattern_quality(&data[0..32].try_into().unwrap());
            if pattern_score < -50 {
                return Err(EntropyError::PatternDetected(pattern_score));
            }
        }

        Ok(())
    }

    /// Enhanced ZK proof preparation with validation details
    pub fn prepare_for_zk_proof(entropy: &[u8; 32]) -> ZKProofPrepResult {
        let entropy_bytes = entropy.to_vec();
        let shannon_entropy = Self::calculate_shannon_entropy(&entropy_bytes);
        let min_entropy = Self::approximate_min_entropy(&entropy_bytes);

        // Calculate scores as before
        let shannon_score = (shannon_entropy * 500) / (8 * 1_000_000_000_000_000_000); // Scaled by 1e18
        let min_entropy_score = (min_entropy * 500) / (8 * 1_000_000_000_000_000_000);
        let shannon_score = if shannon_score > 500 { 500 } else { shannon_score };
        let min_entropy_score = if min_entropy_score > 500 { 500 } else { min_entropy_score };

        let base_score = shannon_score + min_entropy_score;
        let pattern_adjustment = Self::assess_pattern_quality(entropy);
        let mut score = base_score;
        if pattern_adjustment > 0 {
            score += (score * pattern_adjustment as u64) / 1000;
        } else if pattern_adjustment < 0 {
            score -= (score * (-pattern_adjustment) as u64) / 1000;
        }
        score = if score > ENTROPY_MAX_SCORE { ENTROPY_MAX_SCORE } else { score };

        let tier = Self::entropy_score_to_tier(score);
        let suitable = shannon_entropy >= 6 * 1_000_000_000_000_000_000 &&
                       min_entropy >= 4 * 1_000_000_000_000_000_000 &&
                       pattern_adjustment >= -25;

        // Calculate additional validation metrics
        let mut validation_details = ValidationDetails {
            unique_bytes: 0,
            chi_square: 0.0,
            autocorrelation: 0,
            bit_balance_ratio: 0.0,
            pattern_findings: Vec::new(),
        };

        // Count unique bytes and calculate chi-square
        let mut counts = [0u16; 256];
        for &b in entropy {
            counts[b as usize] += 1;
        }
        validation_details.unique_bytes = counts.iter().filter(|&&c| c > 0).count() as u32;
        
        let expected = entropy.len() as f64 / 256.0;
        validation_details.chi_square = counts.iter()
            .map(|&c| {
                let diff = c as f64 - expected;
                diff * diff / expected
            })
            .sum();

        // Calculate bit balance
        let mut zeros = 0;
        let mut ones = 0;
        for byte in entropy {
            zeros += byte.count_zeros();
            ones += byte.count_ones();
        }
        validation_details.bit_balance_ratio = if ones > 0 { zeros as f64 / ones as f64 } else { f64::MAX };

        // Calculate autocorrelation
        let mut autocorr = 0;
        for i in 1..32 {
            autocorr += (entropy[i] as i32 - entropy[i-1] as i32).pow(2);
        }
        validation_details.autocorrelation = autocorr;

        // Add findings
        if validation_details.unique_bytes < 16 {
            validation_details.pattern_findings.push(format!("Low entropy: Only {} unique bytes", validation_details.unique_bytes));
        }
        if validation_details.chi_square > 350.0 {
            validation_details.pattern_findings.push(format!("Distribution anomaly: Chi-square {:.2}", validation_details.chi_square));
        }
        if (validation_details.bit_balance_ratio - 1.0).abs() > 0.3 {
            validation_details.pattern_findings.push(format!("Bit imbalance: Ratio {:.2}", validation_details.bit_balance_ratio));
        }

        ZKProofPrepResult { 
            score, 
            tier, 
            shannon_entropy, 
            min_entropy, 
            suitable,
            validation_details: Some(validation_details) 
        }
    }

    /// Optimized batch processing with chunking for better memory management
    pub fn batch_prepare_for_zk_proof(entropy_batch: &[Vec<u8>]) -> Vec<ZKProofPrepResult> {
        // Process in chunks of 100 for better memory management
        entropy_batch.par_chunks(100)
            .flat_map(|chunk| {
                chunk.par_iter().map(|e| {
                    let entropy_array: [u8; 32] = e[0..32].try_into().unwrap_or([0; 32]);
                    Self::prepare_for_zk_proof(&entropy_array)
                }).collect::<Vec<_>>()
            })
            .collect()
    }

    /// Performance measurement
    pub fn measure_performance(batch_size: usize) -> (f64, f64) {
        let mut test_data = Vec::with_capacity(batch_size);
        for i in 0..batch_size {
            let mut data = [0u8; 32];
            for j in 0..32 {
                data[j] = ((i + j) % 256) as u8;
            }
            test_data.push(data.to_vec());
        }

        let start = Instant::now();
        let _results = Self::batch_prepare_for_zk_proof(&test_data);
        let duration = start.elapsed();

        let avg_time_ms = duration.as_secs_f64() * 1000.0 / batch_size as f64;
        let total_time_ms = duration.as_secs_f64() * 1000.0;
        (avg_time_ms, total_time_ms)
    }
    
    /// Convert result to JSON for cross-language validation
    pub fn to_json(result: &ZKProofPrepResult) -> String {
        serde_json::to_string(&VerificationResult {
            score: result.score,
            tier: result.tier,
            passed: result.suitable,
            shannon_entropy: result.shannon_entropy,
            min_entropy: result.min_entropy,
        }).unwrap_or_else(|_| "".to_string())
    }
    
    /// Export batch results to JSON for integration testing
    pub fn export_batch_results(results: &[ZKProofPrepResult]) -> String {
        let verification_results: Vec<VerificationResult> = results.iter()
            .map(|r| VerificationResult {
                score: r.score,
                tier: r.tier,
                passed: r.suitable,
                shannon_entropy: r.shannon_entropy,
                min_entropy: r.min_entropy,
            })
            .collect();
            
        serde_json::to_string(&verification_results).unwrap_or_else(|_| "[]".to_string())
    }
}

// Integration with existing ecosystem
pub fn integrate_with_ecosystem(entropy: Vec<u8>) -> ZKProofPrepResult {
    // Simulate integration with EntropyPQC, VDFVerifier, etc.
    let entropy_hash = EntropyPQC::hybrid_quantum_resistant_hash(&entropy, None); // Example
    let entropy_array: [u8; 32] = entropy_hash[0..32].try_into().unwrap();
    EntropyQualityLib::prepare_for_zk_proof(&entropy_array)
}

// Simulation setup
fn simulate_base_entropy(seed: u64, batch_size: usize) -> Vec<Vec<u8>> {
    let mut batch_data = Vec::with_capacity(batch_size);
    for i in 0..batch_size {
        let mut entropy = vec![0u8; 10_000];
        let mut rng = StdRng::seed_from_u64(seed + i as u64);
        rng.fill_bytes(&mut entropy);
        batch_data.push(entropy);
    }
    batch_data
}

// Enhanced entropy check
fn enhanced_entropy_check(data: &[u8]) -> bool {
    if data.is_empty() { return false; }
    let mut zeros = 0;
    let mut ones = 0;
    for byte in data {
        zeros += byte.count_zeros();
        ones += byte.count_ones();
    }
    let bit_balance_ok = (zeros as f64 / ones as f64).abs() - 1.0 < 0.3;

    let mut counts = [0u16; 256];
    for &b in data {
        counts[b as usize] += 1;
    }
    let expected = data.len() as f64 / 256.0;
    let chi_sq: f64 = counts.iter()
        .map(|&c| {
            let diff = c as f64 - expected;
            diff * diff / expected
        })
        .sum();
    let chi_square_ok = chi_sq < 310.0;

    bit_balance_ok && chi_square_ok
}

// Run simulation
fn run_offline_simulation(batch_size: usize) -> (f64, f64, f64, u8, f64, f64) {
    let mut passes = 0;
    let mut total_overhead_ms = 0.0;
    let mut total_score = 0.0;
    let mut suitable_count = 0;
    let iterations = 100;

    for i in 0..iterations {
        let start = Instant::now();
        let entropy_batch = simulate_base_entropy(i as u64, batch_size);
        let results = EntropyQualityLib::batch_prepare_for_zk_proof(&entropy_batch);

        for result in results {
            let entropy = entropy_batch[0].clone(); // Use first entropy for check
            let mut combined_entropy = entropy;
            combined_entropy.extend_from_slice(&result.score.to_le_bytes());
            combined_entropy.extend_from_slice(&[result.tier]);

            if enhanced_entropy_check(&combined_entropy) {
                passes += 1;
            }
            total_score += result.score as f64;
            if result.suitable { suitable_count += 1; }
        }

        let duration = start.elapsed();
        total_overhead_ms += duration.as_secs_f64() * 1000.0;
    }

    let pass_rate = passes as f64 / iterations as f64;
    let avg_overhead_ms = total_overhead_ms / iterations as f64;
    let avg_score = total_score / iterations as f64;
    let avg_tier = (total_score / iterations as f64 / 250.0).round() as u8; // Approximate tier
    let suitability_rate = suitable_count as f64 / iterations as f64;
    let (avg_time_per_item, total_time) = EntropyQualityLib::measure_performance(batch_size);
    (pass_rate, avg_overhead_ms, avg_score, avg_tier, suitability_rate, avg_time_per_item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{thread_rng, RngCore};
    use std::convert::TryInto;

    #[test]
    fn test_pattern_quality() {
        let mut entropy = [0u8; 32];
        entropy[0] = 1; entropy[1] = 1; // Repeated byte
        let adjustment = EntropyQualityLib::assess_pattern_quality(&entropy);
        assert!(adjustment < 0); // Should penalize repetition
    }

    #[test]
    fn test_shannon_entropy() {
        let data = vec![1u8; 32]; // Low entropy
        let entropy = EntropyQualityLib::calculate_shannon_entropy(&data);
        assert!(entropy < 1_000_000_000_000_000_000); // <1 bit
    }

    #[test]
    fn test_zk_preparation() {
        let mut entropy = [0u8; 32];
        for i in 0..32 { entropy[i] = i as u8; } // Varied data
        let result = EntropyQualityLib::prepare_for_zk_proof(&entropy);
        assert!(result.score > 300); // Should be at least Tier 2
        assert!(result.suitable); // Should be suitable
    }

    #[test]
    fn test_performance() {
        let (avg_time, total_time) = EntropyQualityLib::measure_performance(100);
        println!("Avg time per item: {:.3}ms, Total time: {:.3}ms", avg_time, total_time);
        assert!(avg_time < 1.0); // Should be efficient
    }
    
    // Additional edge case tests
    
    #[test]
    fn test_all_zeros() {
        let data = [0u8; 32];
        let result = EntropyQualityLib::prepare_for_zk_proof(&data);
        assert!(result.score < ENTROPY_TIER_1_THRESHOLD); // Should not reach tier 2
        assert!(!result.suitable); // Should not be suitable
    }
    
    #[test]
    fn test_all_ones() {
        let data = [255u8; 32];
        let result = EntropyQualityLib::prepare_for_zk_proof(&data);
        assert!(result.score < ENTROPY_TIER_1_THRESHOLD); // Should not reach tier 2
        assert!(!result.suitable); // Should not be suitable
    }
    
    #[test]
    fn test_alternating_pattern() {
        let mut data = [0u8; 32];
        for i in 0..32 {
            data[i] = if i % 2 == 0 { 0 } else { 255 };
        }
        let result = EntropyQualityLib::prepare_for_zk_proof(&data);
        let validation = result.validation_details.unwrap();
        assert!(validation.pattern_findings.len() > 0); // Should detect pattern
    }
    
    #[test]
    fn test_cryptographic_entropy() {
        let mut data = [0u8; 32];
        let mut rng = thread_rng();
        rng.fill_bytes(&mut data);
        let result = EntropyQualityLib::prepare_for_zk_proof(&data);
        assert!(result.suitable); // Should be suitable
        assert!(result.score >= ENTROPY_TIER_2_THRESHOLD); // Should be at least tier 2
    }
    
    #[test]
    fn test_incremental_bytes() {
        let mut data = [0u8; 32];
        for i in 0..32 {
            data[i] = i as u8;
        }
        let result = EntropyQualityLib::prepare_for_zk_proof(&data);
        println!("Incremental score: {}, tier: {}", result.score, result.tier);
        // This is predictable but has good distribution, so check validation details
        let validation = result.validation_details.unwrap();
        assert!(validation.unique_bytes == 32); // All bytes should be unique
    }
    
    #[test]
    fn test_fixed_point_log2() {
        // Test the fixed-point log2 implementation vs fallback
        let values = [1, 10, 100, 1000, 10000, 1000000, 1_000_000_000_000_000_000u64];
        for value in values {
            let scaled = value * 1_000_000_000_000_000_000;
            if value > 1 { // Skip 1 which is exactly 0
                let fixed_result = EntropyQualityLib::approximate_log2_scaled(scaled);
                let fallback_result = EntropyQualityLib::fallback_log2_scaled(scaled);
                
                // The fixed-point result should be more accurate, but within 5% of the fallback
                let difference = if fixed_result > fallback_result {
                    fixed_result - fallback_result
                } else {
                    fallback_result - fixed_result
                };
                
                let tolerance = if fallback_result > 0 { (fallback_result * 5) / 100 } else { 1 };
                assert!(difference <= tolerance, 
                    "Log2 results differ too much: fixed {}, fallback {}", 
                    fixed_result, fallback_result);
            }
        }
    }
}

fn main() {
    let (pass_rate, avg_overhead_ms, avg_score, avg_tier, suitability_rate, avg_time_per_item) = run_offline_simulation(100);
    println!(
        "Pass Rate: {:.2}%, Avg Overhead: {:.2}ms, Avg Score: {:.2}, Avg Tier: {}, Suitability Rate: {:.2}%, Avg Time per Item: {:.3}ms",
        pass_rate * 100.0, avg_overhead_ms, avg_score, avg_tier, suitability_rate * 100.0, avg_time_per_item
    );
}