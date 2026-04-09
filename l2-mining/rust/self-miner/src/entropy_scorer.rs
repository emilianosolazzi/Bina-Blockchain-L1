//! # Entropy Quality Scorer
//!
//! Standalone copy adapted from `temporal_gradient_core::entropy_quality_scorer`.
//!
//! Evaluates statistical quality of 32-byte entropy samples and tracks
//! per-contributor quality trends over time.
//!
//! ## Scoring dimensions (0–100 total)
//!
//! | Dimension         | Max | What it measures                                |
//! |-------------------|-----|-------------------------------------------------|
//! | Bit distribution  |  25 | How close the 0/1 ratio is to 50:50             |
//! | Byte distribution |  30 | Chi-squared fit + unique byte count             |
//! | Run length        |  20 | Longest bit-run and total run count              |
//! | Pattern detection |  25 | Absence of sequential / repeating / counter runs |

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────
// Report
// ─────────────────────────────────────────────────────────────────

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
    pub detected_flaws: Vec<String>,
    pub is_acceptable: bool,
    pub quality_tier: u8,
}

// ─────────────────────────────────────────────────────────────────
// Scorer
// ─────────────────────────────────────────────────────────────────

pub struct EntropyQualityScorer {
    min_acceptable_score: u32,
    #[allow(dead_code)]
    ideal_score: u32,
    quality_tiers: [u32; 3],
    contributor_counts: HashMap<String, u32>,
    contributor_scores: HashMap<String, u32>,
    historical_scores: HashMap<String, Vec<u32>>,
}

impl EntropyQualityScorer {
    pub fn new(min_acceptable: u32, ideal: u32) -> Self {
        Self {
            min_acceptable_score: min_acceptable,
            ideal_score: ideal,
            quality_tiers: [40, 60, 80],
            contributor_counts: HashMap::new(),
            contributor_scores: HashMap::new(),
            historical_scores: HashMap::new(),
        }
    }

    pub fn score_entropy(&mut self, contributor: &str, entropy: &[u8; 32]) -> EntropyScoreReport {
        let bit = Self::bit_distribution_score(entropy);
        let byte = Self::byte_distribution_score(entropy);
        let run = Self::run_length_score(entropy);
        let pattern = Self::pattern_score(entropy);

        let score = (bit + byte + run + pattern).min(100);
        let flaws = Self::detect_common_flaws(entropy);
        let tier = self.determine_quality_tier(score);
        let acceptable = self.is_acceptable(score);

        let count = self.contributor_counts.entry(contributor.to_string()).or_default();
        *count += 1;
        let current_count = *count;

        *self.contributor_scores.entry(contributor.to_string()).or_default() += score;

        let history = self.historical_scores.entry(contributor.to_string()).or_default();
        history.push(score);
        if history.len() > 100 {
            history.remove(0);
        }

        EntropyScoreReport {
            contributor: contributor.to_string(),
            entropy_hex: hex::encode(entropy),
            score,
            bit_score: bit,
            byte_distribution_score: byte,
            run_length_score: run,
            pattern_score: pattern,
            contribution_count: current_count,
            detected_flaws: flaws,
            is_acceptable: acceptable,
            quality_tier: tier,
        }
    }

    // ── Scoring dimensions ──────────────────────────────────────

    pub fn bit_distribution_score(entropy: &[u8; 32]) -> u32 {
        let ones = entropy.iter().map(|b| b.count_ones()).sum::<u32>();
        let total = 256u32;
        let zeros = total - ones;

        if ones == 0 || zeros == 0 {
            return 0;
        }

        let ratio = ones.min(zeros) as f64 / ones.max(zeros) as f64;
        // Perfect balance = 1.0 → 25, complete imbalance → 0
        (ratio * 25.0).round() as u32
    }

    pub fn byte_distribution_score(entropy: &[u8; 32]) -> u32 {
        let mut counts = [0u32; 256];
        for &b in entropy {
            counts[b as usize] += 1;
        }

        // Chi-squared against uniform
        let expected = 32.0 / 256.0; // 0.125
        let chi2: f64 = counts
            .iter()
            .map(|&c| {
                let diff = c as f64 - expected;
                diff * diff / expected
            })
            .sum();

        // Lower chi-squared = more uniform. chi2=0 → perfect.
        // For 32 samples over 256 bins, chi2 ~ 224–256 is typical for random data.
        let chi_score = if chi2 < 200.0 {
            20
        } else if chi2 < 300.0 {
            15
        } else if chi2 < 500.0 {
            10
        } else {
            (20.0 * (1000.0 - chi2).max(0.0) / 1000.0) as u32
        };

        // Unique byte count bonus (max 10)
        let unique = counts.iter().filter(|&&c| c > 0).count() as u32;
        let unique_score = if unique >= 28 {
            10
        } else if unique >= 20 {
            8
        } else if unique >= 12 {
            5
        } else {
            (unique * 10 / 32).min(10)
        };

        (chi_score + unique_score).min(30)
    }

    pub fn run_length_score(entropy: &[u8; 32]) -> u32 {
        let mut max_run = 0u32;
        let mut current_run = 1u32;
        let mut run_count = 1u32;
        let mut prev_bit: Option<bool> = None;

        for &byte in entropy {
            for bit_pos in (0..8).rev() {
                let bit = (byte >> bit_pos) & 1 == 1;
                match prev_bit {
                    Some(p) if p == bit => current_run += 1,
                    Some(_) => {
                        max_run = max_run.max(current_run);
                        current_run = 1;
                        run_count += 1;
                    }
                    None => {}
                }
                prev_bit = Some(bit);
            }
        }
        max_run = max_run.max(current_run);

        let run_score = if max_run < 8 {
            12
        } else if max_run > 20 {
            0
        } else {
            12u32.saturating_sub((max_run - 8) * 12 / 12)
        };

        let count_score = if run_count >= 80 {
            8
        } else {
            (run_count * 8 / 80).min(8)
        };

        run_score + count_score
    }

    pub fn pattern_score(entropy: &[u8; 32]) -> u32 {
        let mut score: u32 = 25;

        for window in entropy.windows(3) {
            if window[1] == window[0].wrapping_add(1)
                && window[2] == window[0].wrapping_add(2)
            {
                score = score.saturating_sub(8);
                break;
            }
        }

        let mut pair_repeats = 0u32;
        if entropy.len() >= 4 {
            for window in entropy.windows(4) {
                if window[0] == window[2]
                    && window[1] == window[3]
                    && window[0] != window[1]
                {
                    pair_repeats += 1;
                }
            }
        }
        if pair_repeats > 3 {
            score = score.saturating_sub(((pair_repeats - 3) * 3).min(10));
        }

        let counter_pattern =
            (1..entropy.len()).all(|i| entropy[i] == entropy[0].wrapping_add(i as u8));
        if counter_pattern {
            score = score.saturating_sub(15);
        }

        score.min(25)
    }

    // ── Acceptability ───────────────────────────────────────────

    pub fn is_acceptable(&self, score: u32) -> bool {
        score >= self.min_acceptable_score
    }

    // ── Contributor tracking ────────────────────────────────────

    pub fn contributor_trend(&self, contributor: &str) -> (f32, f32) {
        let scores = match self.historical_scores.get(contributor) {
            Some(history) if !history.is_empty() => history,
            _ => return (0.0, 0.0),
        };

        let avg_score = scores.iter().sum::<u32>() as f32 / scores.len() as f32;

        let trend = if scores.len() >= 3 {
            let n = scores.len() as f32;
            let x_sum: f32 = (0..scores.len()).map(|i| i as f32).sum();
            let y_sum: f32 = scores.iter().map(|&s| s as f32).sum();
            let xy_sum: f32 = scores
                .iter()
                .enumerate()
                .map(|(i, &score)| i as f32 * score as f32)
                .sum();
            let x2_sum: f32 = (0..scores.len()).map(|i| (i as f32).powi(2)).sum();

            let denominator = n * x2_sum - x_sum * x_sum;
            if denominator.abs() > f32::EPSILON {
                (n * xy_sum - x_sum * y_sum) / denominator
            } else {
                0.0
            }
        } else {
            0.0
        };

        (avg_score, trend)
    }

    pub fn determine_quality_tier(&self, score: u32) -> u8 {
        if score >= self.quality_tiers[2] {
            4
        } else if score >= self.quality_tiers[1] {
            3
        } else if score >= self.quality_tiers[0] {
            2
        } else {
            1
        }
    }

    // ── Flaw detection ──────────────────────────────────────────

    pub fn detect_common_flaws(entropy: &[u8; 32]) -> Vec<String> {
        let mut flaws = Vec::new();

        let ones = entropy.iter().map(|b| b.count_ones()).sum::<u32>() as usize;
        let zeros = 256 - ones;

        if ones == 0 {
            flaws.push("All bits are zero".to_string());
        } else if zeros == 0 {
            flaws.push("All bits are one".to_string());
        } else {
            let bit_ratio = zeros as f64 / ones as f64;
            if !(0.7..=1.3).contains(&bit_ratio) {
                flaws.push(format!("Biased bit distribution ({}:{})", zeros, ones));
            }
        }

        let mut byte_counts = [0u8; 256];
        for &byte in entropy {
            byte_counts[byte as usize] = byte_counts[byte as usize].saturating_add(1);
        }

        let mut highest_byte_count: u8 = 0;
        let mut highest_byte: usize = 0;
        for (byte, &count) in byte_counts.iter().enumerate() {
            if count > highest_byte_count {
                highest_byte_count = count;
                highest_byte = byte;
            }
        }

        if highest_byte_count > 4 {
            flaws.push(format!(
                "Byte 0x{:02x} appears {} times",
                highest_byte, highest_byte_count
            ));
        }

        let unique_bytes = byte_counts.iter().filter(|&&count| count > 0).count();
        if unique_bytes < 16 {
            flaws.push(format!("Low byte diversity ({} unique bytes)", unique_bytes));
        }

        let has_sequential = entropy
            .windows(3)
            .any(|w| w[1] == w[0].wrapping_add(1) && w[2] == w[0].wrapping_add(2));
        if has_sequential {
            flaws.push("Contains sequential byte patterns".to_string());
        }

        let all_same = entropy.windows(2).all(|w| w[0] == w[1]);
        if all_same {
            flaws.push("All bytes identical".to_string());
        }

        let has_alternating = entropy
            .windows(4)
            .any(|w| w[0] == w[2] && w[1] == w[3] && w[0] != w[1]);
        if has_alternating {
            flaws.push("Contains alternating byte patterns".to_string());
        }

        let counter_pattern =
            (1..entropy.len()).all(|i| entropy[i] == entropy[0].wrapping_add(i as u8));
        if counter_pattern {
            flaws.push("Entropy resembles counter output".to_string());
        }

        flaws
    }

    // ── Statistics ───────────────────────────────────────────────

    #[allow(dead_code)]
    pub fn contributor_statistics(&self, contributor: &str) -> Option<ContributorStats> {
        if !self.contributor_counts.contains_key(contributor) {
            return None;
        }

        let count = *self.contributor_counts.get(contributor).unwrap_or(&0);
        let total_score = *self.contributor_scores.get(contributor).unwrap_or(&0);
        let (avg_score, trend) = self.contributor_trend(contributor);

        let history = self
            .historical_scores
            .get(contributor)
            .cloned()
            .unwrap_or_default();

        let quality_distribution = if !history.is_empty() {
            let mut distribution = [0u32; 4];
            for &score in &history {
                let tier = self.determine_quality_tier(score) as usize - 1;
                distribution[tier] += 1;
            }
            distribution
        } else {
            [0u32; 4]
        };

        Some(ContributorStats {
            contributor: contributor.to_string(),
            contribution_count: count,
            total_score,
            average_score: avg_score,
            trend,
            quality_distribution,
            latest_n_scores: history.iter().rev().take(5).cloned().collect(),
        })
    }
}

// ─────────────────────────────────────────────────────────────────
// Contributor statistics
// ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct ContributorStats {
    pub contributor: String,
    pub contribution_count: u32,
    pub total_score: u32,
    pub average_score: f32,
    pub trend: f32,
    pub quality_distribution: [u32; 4],
    pub latest_n_scores: Vec<u32>,
}
