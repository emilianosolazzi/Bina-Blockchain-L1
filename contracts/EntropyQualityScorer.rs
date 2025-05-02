use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use num_traits::clamp;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EntropyScoreReport {
    pub contributor: String,
    pub entropy_hex: String,
    pub score: u32,
    pub bit_score: u32,
    pub byte_distribution_score: u32,
    pub run_length_score: u32,
    pub pattern_score: u32,
    pub contribution_count: u32,
}

pub struct EntropyQualityScorer {
    pub min_entropy_bits: u32,
    pub ideal_entropy_bits: u32,
    pub contributor_scores: HashMap<String, u32>,
    pub contributor_counts: HashMap<String, u32>,
}

impl EntropyQualityScorer {
    pub fn new(min_entropy_bits: u32, ideal_entropy_bits: u32) -> Self {
        Self {
            min_entropy_bits,
            ideal_entropy_bits,
            contributor_scores: HashMap::new(),
            contributor_counts: HashMap::new(),
        }
    }

    pub fn score_entropy(&mut self, contributor: &str, entropy: &[u8; 32]) -> EntropyScoreReport {
        let bit_score = Self::bit_distribution_score(entropy);
        let byte_score = Self::byte_distribution_score(entropy);
        let run_score = Self::run_length_score(entropy);
        let pattern_score = Self::pattern_score(entropy);

        let total_score = bit_score + byte_score + run_score + pattern_score;

        let entry = self.contributor_scores.entry(contributor.to_string()).or_default();
        *entry += total_score;

        let count = self.contributor_counts.entry(contributor.to_string()).or_default();
        *count += 1;

        EntropyScoreReport {
            contributor: contributor.to_string(),
            entropy_hex: hex::encode(entropy),
            score: total_score,
            bit_score,
            byte_distribution_score: byte_score,
            run_length_score: run_score,
            pattern_score,
            contribution_count: *count,
        }
    }

    fn bit_distribution_score(entropy: &[u8; 32]) -> u32 {
        let bit_count = entropy.iter().map(|b| b.count_ones()).sum::<u32>();
        let deviation = if bit_count > 128 { bit_count - 128 } else { 128 - bit_count };
        clamp(20 - (deviation * 20 / 128), 0, 20)
    }

    fn byte_distribution_score(entropy: &[u8; 32]) -> u32 {
        let mut frequencies = [0u8; 256];
        for &b in entropy.iter() {
            frequencies[b as usize] += 1;
        }

        let unique_bytes = frequencies.iter().filter(|&&f| f == 1).count() as u32;
        let expected = 1.0;
        let chi_sq: f64 = frequencies
            .iter()
            .map(|&f| {
                if f == 0 { 0.0 } else {
                    let diff = f as f64 - expected;
                    diff * diff / expected
                }
            })
            .sum();

        let unique_score = clamp((unique_bytes * 15) / 32, 0, 15);
        let chi_score = if chi_sq < 10.0 {
            10
        } else if chi_sq > 40.0 {
            0
        } else {
            clamp((10.0 - (chi_sq - 10.0) / 3.0).round() as u32, 0, 10)
        };

        unique_score + chi_score
    }

    fn run_length_score(entropy: &[u8; 32]) -> u32 {
        let mut max_run = 1;
        let mut current_run = 1;
        let mut run_count = 0;
        let mut last_bit = (entropy[0] & 1) != 0;

        for i in 0..256 {
            let byte = entropy[i / 8];
            let bit = (byte >> (i % 8)) & 1 != 0;

            if bit == last_bit {
                current_run += 1;
            } else {
                if current_run > max_run {
                    max_run = current_run;
                }
                run_count += 1;
                current_run = 1;
            }
            last_bit = bit;
        }

        if current_run > max_run {
            max_run = current_run;
        }

        let run_score = if max_run < 8 {
            15
        } else if max_run > 20 {
            0
        } else {
            clamp(15 - ((max_run - 8) * 15 / 12), 0, 15)
        };

        let count_score = if run_count >= 64 {
            10
        } else {
            (run_count * 10 / 64).min(10)
        };

        run_score + count_score
    }

    fn pattern_score(entropy: &[u8; 32]) -> u32 {
        let mut score = 30;

        // Repeating nibble patterns
        let mut repeated = 0;
        let mut seen = std::collections::HashSet::new();
        for i in 0..31 {
            let nibble = entropy[i] & 0xF;
            if seen.contains(&nibble) {
                repeated += 1;
            } else {
                seen.insert(nibble);
            }
        }

        if repeated > 2 {
            score = score.saturating_sub((repeated - 2) * 5);
        }

        // Arithmetic sequence detection
        for i in 0..30 {
            let a = entropy[i];
            let b = entropy[i + 1];
            let c = entropy[i + 2];
            if b == a.wrapping_add(1) && c == a.wrapping_add(2) {
                score = score.saturating_sub(10);
                break;
            }
        }

        clamp(score, 0, 30)
    }
}
