//! # Entropy Quality Scorer
//!
//! Evaluates the statistical quality of 32-byte entropy samples and tracks
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
//!
//! The 0–100 range matches the on-chain `qualityScore` field in
//! `IStaleBlockOracle.StaleProof` so scores are directly comparable.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────
// Report
// ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EntropyScoreReport {
    pub contributor: String,
    pub entropy_hex: String,
    /// Composite quality score, 0–100.
    pub score: u32,
    pub bit_score: u32,
    pub byte_distribution_score: u32,
    pub run_length_score: u32,
    pub pattern_score: u32,
    pub contribution_count: u32,
    pub detected_flaws: Vec<String>,
    pub is_acceptable: bool,
    /// Quality tier: 1 = poor, 2 = low, 3 = medium, 4 = high.
    pub quality_tier: u8,
}

// ─────────────────────────────────────────────────────────────────
// Scorer
// ─────────────────────────────────────────────────────────────────

pub struct EntropyQualityScorer {
    /// Minimum acceptable composite score (0–100).
    pub min_acceptable_score: u32,
    /// Ideal composite score (0–100) — informational only.
    pub ideal_score: u32,
    pub contributor_scores: HashMap<String, u32>,
    pub contributor_counts: HashMap<String, u32>,
    pub historical_scores: HashMap<String, Vec<u32>>,
    /// Tier boundaries: [low, medium, high].
    pub quality_tiers: [u32; 3],
}

impl EntropyQualityScorer {
    pub fn new(min_acceptable_score: u32, ideal_score: u32) -> Self {
        Self {
            min_acceptable_score,
            ideal_score,
            contributor_scores: HashMap::new(),
            contributor_counts: HashMap::new(),
            historical_scores: HashMap::new(),
            quality_tiers: [40, 60, 80],
        }
    }

    // Keep the old constructor name working for downstream callers.
    #[doc(hidden)]
    #[inline]
    pub fn with_min_and_ideal(min: u32, ideal: u32) -> Self {
        Self::new(min, ideal)
    }

    pub fn score_entropy(&mut self, contributor: &str, entropy: &[u8; 32]) -> EntropyScoreReport {
        let bit_score = Self::bit_distribution_score(entropy);
        let byte_score = Self::byte_distribution_score(entropy);
        let run_score = Self::run_length_score(entropy);
        let pattern_score = Self::pattern_score(entropy);

        let total_score = (bit_score + byte_score + run_score + pattern_score).min(100);

        let entry = self.contributor_scores.entry(contributor.to_string()).or_default();
        *entry += total_score;

        let contribution_count = {
            let count = self.contributor_counts.entry(contributor.to_string()).or_default();
            *count += 1;
            *count
        };

        self.historical_scores
            .entry(contributor.to_string())
            .or_default()
            .push(total_score);

        let flaws = Self::detect_common_flaws(entropy);
        let quality_tier = self.determine_quality_tier(total_score);
        let is_acceptable = self.is_acceptable(total_score);

        EntropyScoreReport {
            contributor: contributor.to_string(),
            entropy_hex: hex::encode(entropy),
            score: total_score,
            bit_score,
            byte_distribution_score: byte_score,
            run_length_score: run_score,
            pattern_score,
            contribution_count,
            detected_flaws: flaws,
            is_acceptable,
            quality_tier,
        }
    }

    // ── Scoring dimensions ──────────────────────────────────────

    /// Bit balance: how close the 0/1 bit ratio is to 50:50. Max 25.
    fn bit_distribution_score(entropy: &[u8; 32]) -> u32 {
        let bit_count = entropy.iter().map(|b| b.count_ones()).sum::<u32>();
        // Perfect = 128 ones out of 256 bits.
        let deviation = bit_count.abs_diff(128);
        // Linear scale: 0 deviation → 25, ≥128 deviation → 0.
        25u32.saturating_sub(deviation * 25 / 128)
    }

    /// Byte distribution: chi-squared goodness-of-fit + unique byte count.
    /// Max 30.
    ///
    /// For 32 bytes drawn uniformly over 256 bins, the expected frequency
    /// per bin is 32/256 = 0.125. Bins with zero observations don't
    /// contribute to chi-squared (observed == expected == very small).
    fn byte_distribution_score(entropy: &[u8; 32]) -> u32 {
        let mut frequencies = [0u32; 256];
        for &b in entropy.iter() {
            frequencies[b as usize] += 1;
        }

        // With only 32 samples over 256 bins, most bins are zero.
        // A practical measurement: count how many distinct byte values appear.
        let unique_bytes = frequencies.iter().filter(|&&f| f > 0).count() as u32;
        // Ideal for 32 random bytes: ~31 unique values (birthday bound).
        // Score: 0–20 based on unique count out of 32.
        let unique_score = (unique_bytes * 20 / 32).min(20);

        // Chi-squared over non-zero bins only (sparse-safe).
        // Expected frequency = 32 / 256 = 0.125 per bin.
        let expected: f64 = 32.0 / 256.0;
        let chi_sq: f64 = frequencies
            .iter()
            .filter(|&&f| f > 0)
            .map(|&f| {
                let diff = f as f64 - expected;
                diff * diff / expected
            })
            .sum();

        // Normalise chi-squared to a 0–10 score.
        // For truly random 32-byte input the non-zero-bin chi-sq is
        // typically 20–30. Values > 60 indicate strong bias.
        let chi_score = if chi_sq < 25.0 {
            10
        } else if chi_sq > 60.0 {
            0
        } else {
            ((60.0 - chi_sq) * 10.0 / 35.0).round() as u32
        };

        unique_score + chi_score
    }

    /// Run-length analysis: penalises long same-bit runs. Max 20.
    fn run_length_score(entropy: &[u8; 32]) -> u32 {
        let mut max_run: u32 = 1;
        let mut current_run: u32 = 1;
        let mut run_count: u32 = 0;
        let mut last_bit = (entropy[0] & 1) != 0;

        for i in 1..256u32 {
            let byte = entropy[(i / 8) as usize];
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

        // Max-run sub-score (0–12): short runs → full marks.
        let run_score = if max_run < 8 {
            12
        } else if max_run > 20 {
            0
        } else {
            12u32.saturating_sub((max_run - 8) * 12 / 12)
        };

        // Run-count sub-score (0–8): more transitions → better.
        // Ideal: ~128 transitions for 256 bits.
        let count_score = if run_count >= 80 {
            8
        } else {
            (run_count * 8 / 80).min(8)
        };

        run_score + count_score
    }

    /// Pattern detection: penalises sequential runs, repeated byte pairs,
    /// and counter-like sequences. Max 25.
    fn pattern_score(entropy: &[u8; 32]) -> u32 {
        let mut score: u32 = 25;

        // Check for sequential byte triplets (a, a+1, a+2).
        for window in entropy.windows(3) {
            if window[1] == window[0].wrapping_add(1)
                && window[2] == window[0].wrapping_add(2)
            {
                score = score.saturating_sub(8);
                break;
            }
        }

        // Check for repeated byte pairs (aa bb aa bb).
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
        // A few coincidental pair-repeats are normal; penalise > 3.
        if pair_repeats > 3 {
            score = score.saturating_sub(((pair_repeats - 3) * 3).min(10));
        }

        // Full counter pattern (each byte = first + index).
        let counter_pattern = (1..entropy.len())
            .all(|i| entropy[i] == entropy[0].wrapping_add(i as u8));
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

    pub fn set_quality_tiers(&mut self, low: u32, medium: u32, high: u32) {
        self.quality_tiers = [low, medium, high];
    }

    // ── Flaw detection ──────────────────────────────────────────

    pub fn detect_common_flaws(entropy: &[u8; 32]) -> Vec<String> {
        let mut flaws = Vec::new();

        // Bit balance.
        let ones = entropy.iter().map(|b| b.count_ones()).sum::<u32>() as usize;
        let zeros = 256 - ones;
        // Guard against division by zero when all bits are the same.
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

        // Byte frequency.
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

        // Sequential byte triplet.
        let has_sequential = entropy.windows(3).any(|w| {
            w[1] == w[0].wrapping_add(1) && w[2] == w[0].wrapping_add(2)
        });
        if has_sequential {
            flaws.push("Contains sequential byte patterns".to_string());
        }

        // All-same bytes.
        let all_same = entropy.windows(2).all(|w| w[0] == w[1]);
        if all_same {
            flaws.push("All bytes identical".to_string());
        }

        // Alternating pair pattern.
        let has_alternating = entropy.windows(4).any(|w| {
            w[0] == w[2] && w[1] == w[3] && w[0] != w[1]
        });
        if has_alternating {
            flaws.push("Contains alternating byte patterns".to_string());
        }

        // Counter output.
        let counter_pattern = (1..entropy.len())
            .all(|i| entropy[i] == entropy[0].wrapping_add(i as u8));
        if counter_pattern {
            flaws.push("Entropy resembles counter output".to_string());
        }

        flaws
    }

    // ── Statistics ───────────────────────────────────────────────

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

// ─────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn random_entropy() -> [u8; 32] {
        // Deterministic pseudo-random bytes for repeatable tests.
        let mut out = [0u8; 32];
        let mut state: u64 = 0xDEAD_BEEF_CAFE_BABE;
        for b in out.iter_mut() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *b = state as u8;
        }
        out
    }

    // ── Score range ──────────────────────────────────────────────

    #[test]
    fn random_input_scores_in_0_100() {
        let mut scorer = EntropyQualityScorer::new(40, 80);
        let report = scorer.score_entropy("test", &random_entropy());
        assert!(report.score <= 100, "score {} > 100", report.score);
    }

    #[test]
    fn all_zeros_scores_low() {
        let mut scorer = EntropyQualityScorer::new(40, 80);
        let report = scorer.score_entropy("test", &[0u8; 32]);
        assert!(report.score < 30, "all-zeros should score low, got {}", report.score);
        assert!(!report.is_acceptable);
        assert!(!report.detected_flaws.is_empty());
    }

    #[test]
    fn all_ones_scores_low() {
        let mut scorer = EntropyQualityScorer::new(40, 80);
        let report = scorer.score_entropy("test", &[0xFFu8; 32]);
        assert!(report.score < 30, "all-0xFF should score low, got {}", report.score);
    }

    #[test]
    fn counter_sequence_scores_low() {
        let mut scorer = EntropyQualityScorer::new(40, 80);
        let mut counter = [0u8; 32];
        for (i, b) in counter.iter_mut().enumerate() {
            *b = i as u8;
        }
        let report = scorer.score_entropy("test", &counter);
        assert!(report.score < 65, "counter should score below high-quality, got {}", report.score);
        assert!(report.detected_flaws.iter().any(|f| f.contains("counter")));
    }

    // ── Individual dimensions ────────────────────────────────────

    #[test]
    fn bit_score_max_for_balanced() {
        // 128 ones + 128 zeros: perfectly balanced.
        let mut buf = [0u8; 32];
        for (i, b) in buf.iter_mut().enumerate() {
            *b = if i < 16 { 0xFF } else { 0x00 };
        }
        let score = EntropyQualityScorer::bit_distribution_score(&buf);
        assert_eq!(score, 25, "perfectly balanced bits should get max 25");
    }

    #[test]
    fn byte_distribution_high_for_unique() {
        // All 32 distinct bytes → high unique count.
        let mut buf = [0u8; 32];
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (i * 7) as u8; // All distinct mod 256 for first 32
        }
        let score = EntropyQualityScorer::byte_distribution_score(&buf);
        assert!(score >= 15, "32 unique bytes should score well, got {}", score);
    }

    #[test]
    fn run_length_max_for_alternating_bits() {
        // 0xAA = 10101010 → max run = 1, many transitions.
        let buf = [0xAAu8; 32];
        let score = EntropyQualityScorer::run_length_score(&buf);
        assert!(score >= 15, "alternating bits should score high, got {}", score);
    }

    #[test]
    fn pattern_score_max_for_random() {
        let score = EntropyQualityScorer::pattern_score(&random_entropy());
        assert_eq!(score, 25, "random input should get max pattern score");
    }

    // ── Flaw detection ──────────────────────────────────────────

    #[test]
    fn detect_all_zero_flaws() {
        let flaws = EntropyQualityScorer::detect_common_flaws(&[0u8; 32]);
        assert!(flaws.iter().any(|f| f.contains("zero") || f.contains("Zero")));
        assert!(flaws.iter().any(|f| f.contains("identical")));
        assert!(flaws.iter().any(|f| f.contains("diversity")));
    }

    #[test]
    fn detect_counter_flaw() {
        let mut counter = [0u8; 32];
        for (i, b) in counter.iter_mut().enumerate() {
            *b = i as u8;
        }
        let flaws = EntropyQualityScorer::detect_common_flaws(&counter);
        assert!(flaws.iter().any(|f| f.contains("counter")));
        assert!(flaws.iter().any(|f| f.contains("sequential")));
    }

    #[test]
    fn detect_alternating_flaw() {
        let mut alt = [0u8; 32];
        for (i, b) in alt.iter_mut().enumerate() {
            *b = if i % 2 == 0 { 0xAA } else { 0x55 };
        }
        let flaws = EntropyQualityScorer::detect_common_flaws(&alt);
        assert!(flaws.iter().any(|f| f.contains("alternating")));
    }

    #[test]
    fn no_div_by_zero_on_all_ones() {
        // All 0xFF → zeros count is 0. Should not panic.
        let flaws = EntropyQualityScorer::detect_common_flaws(&[0xFF; 32]);
        assert!(flaws.iter().any(|f| f.contains("one") || f.contains("One")));
    }

    // ── Contributor tracking ─────────────────────────────────────

    #[test]
    fn contributor_count_increments() {
        let mut scorer = EntropyQualityScorer::new(40, 80);
        scorer.score_entropy("alice", &random_entropy());
        scorer.score_entropy("alice", &random_entropy());
        let stats = scorer.contributor_statistics("alice").unwrap();
        assert_eq!(stats.contribution_count, 2);
    }

    #[test]
    fn contributor_trend_needs_3_samples() {
        let mut scorer = EntropyQualityScorer::new(40, 80);
        scorer.score_entropy("bob", &random_entropy());
        scorer.score_entropy("bob", &random_entropy());
        let (avg, trend) = scorer.contributor_trend("bob");
        assert!(avg > 0.0);
        assert_eq!(trend, 0.0, "trend undefined with < 3 samples");
    }

    #[test]
    fn unknown_contributor_returns_none() {
        let scorer = EntropyQualityScorer::new(40, 80);
        assert!(scorer.contributor_statistics("nobody").is_none());
    }

    // ── Quality tiers ────────────────────────────────────────────

    #[test]
    fn tiers_cover_full_range() {
        let scorer = EntropyQualityScorer::new(40, 80);
        assert_eq!(scorer.determine_quality_tier(0), 1);
        assert_eq!(scorer.determine_quality_tier(39), 1);
        assert_eq!(scorer.determine_quality_tier(40), 2);
        assert_eq!(scorer.determine_quality_tier(59), 2);
        assert_eq!(scorer.determine_quality_tier(60), 3);
        assert_eq!(scorer.determine_quality_tier(79), 3);
        assert_eq!(scorer.determine_quality_tier(80), 4);
        assert_eq!(scorer.determine_quality_tier(100), 4);
    }

    #[test]
    fn custom_tiers_applied() {
        let mut scorer = EntropyQualityScorer::new(40, 80);
        scorer.set_quality_tiers(20, 50, 90);
        assert_eq!(scorer.determine_quality_tier(19), 1);
        assert_eq!(scorer.determine_quality_tier(20), 2);
        assert_eq!(scorer.determine_quality_tier(50), 3);
        assert_eq!(scorer.determine_quality_tier(90), 4);
    }

    // ── Acceptability ────────────────────────────────────────────

    #[test]
    fn acceptable_threshold() {
        let scorer = EntropyQualityScorer::new(50, 80);
        assert!(!scorer.is_acceptable(49));
        assert!(scorer.is_acceptable(50));
        assert!(scorer.is_acceptable(100));
    }
}
