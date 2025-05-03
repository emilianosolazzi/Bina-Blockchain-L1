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
    pub detected_flaws: Vec<String>,            // Added field for detected flaws
    pub is_acceptable: bool,                    // Added field to indicate if entropy meets minimum standards
    pub quality_tier: u8,                       // Added field for quality categorization (1-4)
}

pub struct EntropyQualityScorer {
    pub min_entropy_bits: u32,
    pub ideal_entropy_bits: u32,
    pub contributor_scores: HashMap<String, u32>,
    pub contributor_counts: HashMap<String, u32>,
    pub historical_scores: HashMap<String, Vec<u32>>,  // Added to track score history for trend analysis
    pub quality_tiers: [u32; 3],                // Thresholds for quality tiers (3 thresholds = 4 tiers)
}

impl EntropyQualityScorer {
    pub fn new(min_entropy_bits: u32, ideal_entropy_bits: u32) -> Self {
        Self {
            min_entropy_bits,
            ideal_entropy_bits,
            contributor_scores: HashMap::new(),
            contributor_counts: HashMap::new(),
            historical_scores: HashMap::new(),
            quality_tiers: [40, 60, 75],        // Default tier thresholds (can be customized)
        }
    }

    pub fn score_entropy(&mut self, contributor: &str, entropy: &[u8; 32]) -> EntropyScoreReport {
        let bit_score = Self::bit_distribution_score(entropy);
        let byte_score = Self::byte_distribution_score(entropy);
        let run_score = Self::run_length_score(entropy);
        let pattern_score = Self::pattern_score(entropy);

        let total_score = bit_score + byte_score + run_score + pattern_score;

        // Update contributor stats
        let entry = self.contributor_scores.entry(contributor.to_string()).or_default();
        *entry += total_score;

        let count = self.contributor_counts.entry(contributor.to_string()).or_default();
        *count += 1;

        // Track historical scores for trend analysis
        self.historical_scores
            .entry(contributor.to_string())
            .or_default()
            .push(total_score);

        // Detect common flaws in entropy
        let flaws = Self::detect_common_flaws(entropy);
        
        // Determine quality tier
        let quality_tier = self.determine_quality_tier(total_score);
        
        // Check if score is acceptable
        let is_acceptable = self.is_acceptable(total_score);

        EntropyScoreReport {
            contributor: contributor.to_string(),
            entropy_hex: hex::encode(entropy),
            score: total_score,
            bit_score,
            byte_distribution_score: byte_score,
            run_length_score: run_score,
            pattern_score,
            contribution_count: *count,
            detected_flaws: flaws,
            is_acceptable,
            quality_tier,
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

    /// Check if a score meets the minimum acceptable threshold
    pub fn is_acceptable(&self, score: u32) -> bool {
        score >= self.min_entropy_bits
    }

    /// Analyze contributor's historical trend
    /// Returns the average score and trend direction (positive/negative)
    pub fn contributor_trend(&self, contributor: &str) -> (f32, f32) {
        let scores = match self.historical_scores.get(contributor) {
            Some(history) if !history.is_empty() => history,
            _ => return (0.0, 0.0), // No history or empty history
        };
        
        let avg_score = scores.iter().sum::<u32>() as f32 / scores.len() as f32;
        
        // Calculate trend direction (if we have enough samples)
        let trend = if scores.len() >= 3 {
            // Simple linear regression slope calculation
            let n = scores.len() as f32;
            let x_sum: f32 = (0..scores.len()).map(|i| i as f32).sum();
            let y_sum: f32 = scores.iter().map(|&s| s as f32).sum();
            let xy_sum: f32 = scores.iter().enumerate()
                .map(|(i, &score)| i as f32 * score as f32)
                .sum();
            let x2_sum: f32 = (0..scores.len()).map(|i| (i as f32).powi(2)).sum();
            
            // Slope formula: (n*Σxy - Σx*Σy) / (n*Σx² - (Σx)²)
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

    /// Determine quality tier based on score (1=lowest, 4=highest)
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

    /// Set custom quality tier thresholds
    pub fn set_quality_tiers(&mut self, low: u32, medium: u32, high: u32) {
        self.quality_tiers = [low, medium, high];
    }

    /// Detect common flaws in entropy sources
    pub fn detect_common_flaws(entropy: &[u8; 32]) -> Vec<String> {
        let mut flaws = Vec::new();
        
        // Count zeros and ones for basic bit distribution check
        let zeros = entropy.iter().flat_map(|&byte| (0..8).map(move |i| (byte >> i) & 1 == 0))
            .filter(|&bit| bit).count();
        let ones = 256 - zeros;
        let bit_ratio = (zeros as f64 / ones as f64).abs();
        
        if bit_ratio > 1.3 || bit_ratio < 0.7 {
            flaws.push(format!("Biased bit distribution ({}:{})", zeros, ones));
        }
        
        // Check for too many repeating bytes
        let mut byte_counts = [0u8; 256];
        for &byte in entropy {
            byte_counts[byte as usize] += 1;
        }
        
        let mut highest_byte_count = 0;
        let mut highest_byte = 0;
        for (byte, &count) in byte_counts.iter().enumerate() {
            if count > highest_byte_count {
                highest_byte_count = count;
                highest_byte = byte;
            }
        }
        
        if highest_byte_count > 4 {
            flaws.push(format!("Byte 0x{:02x} appears {} times", highest_byte, highest_byte_count));
        }
        
        // Check for low byte diversity
        let unique_bytes = byte_counts.iter().filter(|&&count| count > 0).count();
        if unique_bytes < 16 {
            flaws.push(format!("Low byte diversity ({} unique bytes)", unique_bytes));
        }
        
        // Check for sequential patterns
        let mut has_sequential = false;
        for i in 0..30 {
            if entropy[i+1] == entropy[i].wrapping_add(1) && 
               entropy[i+2] == entropy[i].wrapping_add(2) {
                has_sequential = true;
                break;
            }
        }
        
        if has_sequential {
            flaws.push("Contains sequential byte patterns".to_string());
        }
        
        // Check for common weak entropy patterns
        let all_same = entropy.windows(2).all(|w| w[0] == w[1]);
        if all_same {
            flaws.push("All bytes identical".to_string());
        }
        
        // Check for alternating patterns (like 0xAA 0x55 repeating)
        let mut has_alternating = false;
        if entropy.len() >= 4 {
            for offset in 0..entropy.len()-4 {
                if entropy[offset] == entropy[offset+2] && 
                   entropy[offset+1] == entropy[offset+3] &&
                   entropy[offset] != entropy[offset+1] {
                    has_alternating = true;
                    break;
                }
            }
        }
        
        if has_alternating {
            flaws.push("Contains alternating byte patterns".to_string());
        }
        
        // Check if data looks like simple counter output
        let mut counter_pattern = true;
        for i in 1..entropy.len() {
            if entropy[i] != entropy[0].wrapping_add(i as u8) {
                counter_pattern = false;
                break;
            }
        }
        
        if counter_pattern {
            flaws.push("Entropy resembles counter output".to_string());
        }

        flaws
    }

    /// Get contributor statistics including trends and quality metrics
    pub fn contributor_statistics(&self, contributor: &str) -> Option<ContributorStats> {
        if !self.contributor_counts.contains_key(contributor) {
            return None;
        }
        
        let count = *self.contributor_counts.get(contributor).unwrap_or(&0);
        let total_score = *self.contributor_scores.get(contributor).unwrap_or(&0);
        let (avg_score, trend) = self.contributor_trend(contributor);
        
        let history = self.historical_scores.get(contributor)
            .map(|h| h.clone())
            .unwrap_or_default();
            
        let quality_distribution = if !history.is_empty() {
            let mut distribution = [0; 4];
            for &score in &history {
                let tier = self.determine_quality_tier(score) as usize - 1;
                distribution[tier] += 1;
            }
            distribution
        } else {
            [0; 4]
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

#[derive(Debug, Serialize, Deserialize)]
pub struct ContributorStats {
    pub contributor: String,
    pub contribution_count: u32,
    pub total_score: u32,
    pub average_score: f32,
    pub trend: f32,
    pub quality_distribution: [u32; 4],  // Count of contributions in each tier
    pub latest_n_scores: Vec<u32>,       // Most recent scores (newest first)
}
