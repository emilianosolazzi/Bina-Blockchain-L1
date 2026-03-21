// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title EntropyQualityLib
 * @notice Library for assessing entropy quality and patterns
 * @dev Used by ZKEntropyVerifier to evaluate quality of entropy sources
 */
library EntropyQualityLib {
    // --- Constants for ZK integration ---
    uint256 internal constant ENTROPY_MIN_SCORE = 100;
    uint256 internal constant ENTROPY_MAX_SCORE = 1000;
    uint256 internal constant ENTROPY_TIER_1_THRESHOLD = 300;
    uint256 internal constant ENTROPY_TIER_2_THRESHOLD = 600;
    uint256 internal constant ENTROPY_TIER_3_THRESHOLD = 850;
    
    // --- Error codes aligned with ZKEntropyVerifier ---
    uint8 internal constant ERROR_INSUFFICIENT_ENTROPY = 1;
    uint8 internal constant ERROR_PATTERN_DETECTED = 2;
    uint8 internal constant ERROR_PREDICTABILITY_RISK = 3;
    uint8 internal constant ERROR_DISTRIBUTION_ANOMALY = 4;
    
    /**
     * @notice Assesses the pattern quality of entropy based on byte distribution
     * @dev Returns a score between -100 and +100 to adjust base entropy score
     * @param entropy The entropy value to assess
     * @return qualityAdjustment Score adjustment (-100 to +100)
     */
    function assessPatternQuality(bytes32 entropy) internal pure returns (int256) {
        // Count byte frequencies
        uint8[256] memory frequencies;
        
        for (uint256 i = 0; i < 32; i++) {
            uint8 b = uint8(entropy[i]);
            frequencies[b]++;
        }
        
        // Calculate chi-squared statistic to detect non-randomness
        // Expected frequency for perfect uniformity is 32/256 = 0.125 per byte value
        // But since we only have 32 bytes total, most should be 0, with some being 1
        uint256 chiSquared = 0;
        
        for (uint256 i = 0; i < 256; i++) {
            if (frequencies[i] > 0) {
                // Each present byte should ideally occur only once or not at all
                if (frequencies[i] > 2) {
                    // Penalize repeated byte patterns
                    chiSquared += (frequencies[i] - 1) * (frequencies[i] - 1);
                }
            }
        }
        
        // Count runs of sequential or repeating bytes
        uint256 runCount = 0;
        for (uint256 i = 1; i < 32; i++) {
            // Check for sequential bytes (x, x+1) or repeating bytes (x, x)
            if (entropy[i] == entropy[i-1] || uint8(entropy[i]) == uint8(entropy[i-1]) + 1) {
                runCount++;
            }
        }
        
        // Calculate final quality adjustment
        int256 adjustment = 0;
        
        // Adjust based on chi-squared (lower is better)
        if (chiSquared < 5) {
            adjustment += 50; // Very good distribution
        } else if (chiSquared < 10) {
            adjustment += 20; // Good distribution
        } else if (chiSquared > 20) {
            adjustment -= 50; // Poor distribution
        } else if (chiSquared > 15) {
            adjustment -= 20; // Below average distribution
        }
        
        // Adjust based on runs (lower is better)
        if (runCount < 3) {
            adjustment += 50; // Very few runs, good randomness
        } else if (runCount < 5) {
            adjustment += 20; // Few runs, decent randomness
        } else if (runCount > 10) {
            adjustment -= 50; // Many runs, poor randomness
        } else if (runCount > 8) {
            adjustment -= 20; // Above average runs, below average randomness
        }
        
        // Limit adjustment to -100 to +100 range
        if (adjustment > 100) {
            adjustment = 100;
        } else if (adjustment < -100) {
            adjustment = -100;
        }
        
        return adjustment;
    }
    
    /**
     * @notice Calculates approximate Shannon entropy of a byte array
     * @dev Returns value in bits scaled by 1e18
     * @param data The data to analyze
     * @return entropy The Shannon entropy in bits (scaled by 1e18)
     */
    function calculateShannonEntropy(bytes memory data) internal pure returns (uint256) {
        uint256 len = data.length;
        if (len == 0) return 0;
        
        // Count byte frequencies
        uint256[256] memory frequencies;
        
        for (uint256 i = 0; i < len; i++) {
            uint8 b = uint8(data[i]);
            frequencies[b]++;
        }
        
        // Calculate Shannon entropy: -sum(p_i * log2(p_i))
        // where p_i is the probability of byte value i
        uint256 entropy = 0;
        
        for (uint256 i = 0; i < 256; i++) {
            if (frequencies[i] == 0) continue;
            
            // Calculate p_i with high precision (scaled by 1e18)
            uint256 probability = (frequencies[i] * 1e18) / len;
            
            // Calculate -p_i * log2(p_i) with high precision
            // log2(x) = log(x)/log(2)
            // Using the approximation: log(x) ≈ (x - 1) - (x - 1)^2/2 + (x - 1)^3/3 - ...
            // for x near 1, which is sufficient for our probability calculations
            uint256 logTerm = approximateLog2Scaled(probability);
            
            // Add -p_i * log2(p_i) to entropy
            entropy += (probability * logTerm) / 1e18;
        }
        
        // Return the calculated entropy (already scaled by 1e18)
        return entropy;
    }
    
    /**
     * @notice Approximates log2(x) for a value x scaled by 1e18
     * @dev Uses a rational approximation suitable for values between 0 and 1
     * @param x The value to calculate log2 for, scaled by 1e18
     * @return log2Value The log2 of the value, also scaled by 1e18
     */
    function approximateLog2Scaled(uint256 x) internal pure returns (uint256) {
        // For x = probability * 1e18, we want log2(probability)
        // Convert back to actual probability
        uint256 probability = x;
        
        // Handle edge cases
        if (probability == 0) return 0;
        if (probability == 1e18) return 0;
        
        // Constants for log2 approximation
        uint256 LOG2_E_SCALED = 1_442695040; // log2(e) * 1e9
        
        // Calculate ln(probability) using Taylor series
        uint256 y = (1e18 - probability) * 1e9 / probability; // (1-p)/p * 1e9
        // ln(p) = -ln(1 + (1-p)/p) ≈ -(y - y^2/2 + y^3/3 - ...)
        uint256 y2 = (y * y) / 1e9;
        uint256 y3 = (y2 * y) / 1e9;
        uint256 y4 = (y3 * y) / 1e9;
        
        // Calculate -ln(probability) * 1e9 with Taylor series
        uint256 lnScaled = y - y2/2 + y3/3 - y4/4;
        
        // Convert ln to log2: log2(x) = ln(x) * log2(e)
        // Return log2 scaled by 1e18
        return (lnScaled * LOG2_E_SCALED) / 1e9;
    }
    
    /**
     * @notice Maps an entropy score to a ZK-compatible tier
     * @param score The entropy quality score (0-1000)
     * @return tier The tier level (1-4)
     */
    function entropyScoreToTier(uint256 score) internal pure returns (uint8) {
        if (score >= ENTROPY_TIER_3_THRESHOLD) {
            return 4; // Exceptional
        } else if (score >= ENTROPY_TIER_2_THRESHOLD) {
            return 3; // High
        } else if (score >= ENTROPY_TIER_1_THRESHOLD) {
            return 2; // Medium
        } else {
            return 1; // Basic
        }
    }
    
    /**
     * @notice Validates entropy against ZKEntropyVerifier requirements
     * @param entropyBytes The entropy to validate
     * @param minShannonEntropy Minimum required Shannon entropy (scaled)
     * @param minMinEntropy Minimum required min-entropy (scaled)
     * @return valid Whether the entropy passes validation
     * @return errorCode Error code if validation fails, 0 if successful
     */
    function validateEntropyForZKProof(
        bytes memory entropyBytes, 
        uint256 minShannonEntropy,
        uint256 minMinEntropy
    ) internal pure returns (bool valid, uint8 errorCode) {
        // Check minimum length
        if (entropyBytes.length < 16) {
            return (false, ERROR_INSUFFICIENT_ENTROPY);
        }
        
        // Calculate Shannon entropy
        uint256 shannonEntropy = calculateShannonEntropy(entropyBytes);
        if (shannonEntropy < minShannonEntropy) {
            return (false, ERROR_INSUFFICIENT_ENTROPY);
        }
        
        // Test for patterns (simplified check)
        int256 patternScore = assessPatternQuality(bytes32(entropyBytes));
        if (patternScore < -50) {
            return (false, ERROR_PATTERN_DETECTED);
        }
        
        // Check min-entropy (approximated)
        uint256 minEntropy = approximateMinEntropy(entropyBytes);
        if (minEntropy < minMinEntropy) {
            return (false, ERROR_PREDICTABILITY_RISK);
        }
        
        return (true, 0);
    }
    
    /**
     * @notice Approximates min-entropy of a byte array (most conservative entropy measure)
     * @dev Min-entropy focuses on the most likely outcome rather than average information
     * @param data The data to analyze
     * @return minEntropy The min-entropy in bits (scaled by 1e18)
     */
    function approximateMinEntropy(bytes memory data) internal pure returns (uint256) {
        uint256 len = data.length;
        if (len == 0) return 0;
        
        // Count byte frequencies to find most common byte
        uint256[256] memory frequencies;
        uint256 maxFreq = 0;
        
        for (uint256 i = 0; i < len; i++) {
            uint8 b = uint8(data[i]);
            frequencies[b]++;
            if (frequencies[b] > maxFreq) {
                maxFreq = frequencies[b];
            }
        }
        
        // Calculate probability of most likely byte
        uint256 pMax = (maxFreq * 1e18) / len;
        
        // Min-entropy = -log2(pMax)
        uint256 logTerm = approximateLog2Scaled(pMax);
        
        return logTerm;
    }
    
    /**
     * @notice Generates a ZK-compatible proof preparation summary
     * @param entropy Input entropy bytes
     * @return score Overall quality score (0-1000)
     * @return tier Entropy tier (1-4)
     * @return shannonEntropy Shannon entropy measurement (scaled)
     * @return minEntropy Min-entropy measurement (scaled) 
     * @return suitable Whether it's suitable for ZK proofs
     */
    function prepareForZKProof(bytes32 entropy) internal pure returns (
        uint256 score,
        uint8 tier,
        uint256 shannonEntropy,
        uint256 minEntropy,
        bool suitable
    ) {
        // Convert to bytes for entropy calculations
        bytes memory entropyBytes = new bytes(32);
        for (uint i = 0; i < 32; i++) {
            entropyBytes[i] = entropy[i];
        }
        
        // Calculate entropy metrics
        shannonEntropy = calculateShannonEntropy(entropyBytes);
        minEntropy = approximateMinEntropy(entropyBytes);
        
        // Calculate base score (50% Shannon entropy, 50% min entropy)
        uint256 shannonScore = (shannonEntropy * 500) / (8 * 1e18); // Max 8 bits per byte
        uint256 minEntropyScore = (minEntropy * 500) / (8 * 1e18);
        
        // Cap scores
        shannonScore = shannonScore > 500 ? 500 : shannonScore;
        minEntropyScore = minEntropyScore > 500 ? 500 : minEntropyScore;
        
        // Get pattern quality adjustment
        int256 patternAdjustment = assessPatternQuality(entropy);
        
        // Calculate final score
        uint256 baseScore = shannonScore + minEntropyScore;
        score = baseScore;
        
        // Apply pattern adjustment
        if (patternAdjustment > 0) {
            // Positive adjustment (up to +10%)
            score = score + ((score * uint256(patternAdjustment)) / 1000);
        } else if (patternAdjustment < 0) {
            // Negative adjustment (up to -10%)
            score = score - ((score * uint256(-patternAdjustment)) / 1000);
        }
        
        // Cap final score
        if (score > ENTROPY_MAX_SCORE) {
            score = ENTROPY_MAX_SCORE;
        }
        
        // Determine tier
        tier = entropyScoreToTier(score);
        
        // Determine if suitable for ZK proofs
        suitable = (
            shannonEntropy >= 6 * 1e18 && // At least 6 bits of Shannon entropy
            minEntropy >= 4 * 1e18 &&     // At least 4 bits of min-entropy
            patternAdjustment >= -25      // Not too many patterns
        );
        
        return (score, tier, shannonEntropy, minEntropy, suitable);
    }
}
