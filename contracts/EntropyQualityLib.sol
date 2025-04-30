// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title EntropyQualityLib
 * @notice Library for assessing entropy quality and patterns
 * @dev Used by ZKEntropyVerifier to evaluate quality of entropy sources
 */
library EntropyQualityLib {
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
}
