// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title EntropyQualityScorer
 * @notice Scores and rewards high-quality entropy contributions
 */
contract EntropyQualityScorer {
    // Scoring parameters
    uint256 public minEntropyBits;
    uint256 public idealEntropyBits;
    
    // Score storage
    mapping(address => uint256) public contributorScores;
    mapping(address => uint256) public contributionCount;
    
    event EntropyScored(address indexed contributor, bytes32 entropy, uint256 score);
    
    /**
     * @notice Initialize parameters for scoring
     * @param _minEntropy Minimum acceptable entropy bits
     * @param _idealEntropy Ideal entropy bits target
     */
    function initialize(uint256 _minEntropy, uint256 _idealEntropy) external {
        minEntropyBits = _minEntropy;
        idealEntropyBits = _idealEntropy;
    }

    /**
     * @notice Score entropy based on statistical quality measures
     * @dev Uses entropy estimation algorithms to score randomness quality
     */
    function scoreEntropy(bytes32 entropy, address contributor) external returns (uint256 score) {
        // Calculate entropy quality score (0-100) using statistical tests
        score = calculateEntropyQuality(entropy);
        
        // Update contributor scores
        contributorScores[contributor] += score;
        contributionCount[contributor]++;
        
        emit EntropyScored(contributor, entropy, score);
        return score;
    }
    
    /**
     * @notice Get the score for a contributor
     * @param contributor Address of contributor to check
     * @return score Current score
     * @return count Number of contributions
     */
    function getContributorScore(address contributor) external view returns (uint256 score, uint256 count) {
        return (contributorScores[contributor], contributionCount[contributor]);
    }

    /**
     * @notice Calculate quality metrics for entropy
     * @dev Uses multiple statistical tests to evaluate entropy quality
     */
    function calculateEntropyQuality(bytes32 entropy) internal pure returns (uint256) {
        // Multiple statistical tests for comprehensive entropy assessment
        uint256 score = 0;
        
        // Test 1: Bit distribution (20 points max)
        uint256 bitCount = countBits(entropy);
        uint256 bitDeviation = bitCount > 128 ? bitCount - 128 : 128 - bitCount;
        uint256 bitScore = 20 - (bitDeviation * 20 / 128);
        
        // Test 2: Byte frequency distribution (25 points max)
        uint256 byteDistributionScore = calculateByteDistribution(entropy);
        
        // Test 3: Run length analysis (25 points max)
        uint256 runLengthScore = calculateRunMetrics(entropy);
        
        // Test 4: Pattern detection (30 points max)
        uint256 patternScore = detectPatterns(entropy);
        
        // Combined entropy quality score (0-100 total)
        score = bitScore + byteDistributionScore + runLengthScore + patternScore;
        
        return score;
    }
    
    /**
     * @notice Analyze byte frequency distribution using chi-squared test
     * @dev A uniform distribution of bytes is expected in high-quality entropy
     */
    function calculateByteDistribution(bytes32 entropy) internal pure returns (uint256) {
        // Count frequency of each byte (0-255)
        uint8[32] memory bytes_array;
        for (uint i = 0; i < 32; i++) {
            bytes_array[i] = uint8(entropy[i]);
        }
        
        // Check for repeated bytes - perfect entropy should have few repetitions
        uint256 uniqueBytes = 0;
        uint256 chiSquared = 0;
        
        for (uint i = 0; i < 32; i++) {
            uint256 count = 0;
            for (uint j = 0; j < 32; j++) {
                if (bytes_array[i] == bytes_array[j]) {
                    count++;
                }
            }
            
            if (count == 1) {
                uniqueBytes++;
            }
            
            // Simple chi-squared test component
            uint256 expected = 1; // With 32 bytes, expect ~1 occurrence of each value
            chiSquared += (count > expected) ? 
                (count - expected) * (count - expected) / expected : 0;
        }
        
        // Score based on unique byte percentage and chi-squared value
        uint256 uniqueScore = (uniqueBytes * 15) / 32; // 15 points max
        uint256 chiSquaredScore = chiSquared < 10 ? 10 : (chiSquared > 40 ? 0 : 10 - (chiSquared - 10) / 3);
        
        return uniqueScore + chiSquaredScore; // 25 points max
    }
    
    /**
     * @notice Analyze runs of consecutive bits
     * @dev Good entropy shouldn't have long runs of the same bit
     */
    function calculateRunMetrics(bytes32 entropy) internal pure returns (uint256) {
        uint256 maxRun = 0;
        uint256 runCount = 0;
        uint256 currentRun = 1;
        uint256 val = uint256(entropy);
        bool lastBit = (val & 1) == 1;
        
        // Scan through bits counting run lengths
        for (uint i = 1; i < 256; i++) {
            val = val >> 1;
            bool currentBit = (val & 1) == 1;
            
            if (currentBit == lastBit) {
                currentRun++;
            } else {
                if (currentRun > maxRun) {
                    maxRun = currentRun;
                }
                runCount++;
                currentRun = 1;
            }
            
            lastBit = currentBit;
        }
        
        // Final run
        if (currentRun > maxRun) {
            maxRun = currentRun;
        }
        
        // Score based on run metrics
        uint256 maxRunScore = maxRun < 8 ? 15 : (maxRun > 20 ? 0 : 15 - ((maxRun - 8) * 15) / 12);
        uint256 runCountScore = (runCount >= 64) ? 10 : (runCount * 10) / 64;
        
        return maxRunScore + runCountScore; // 25 points max
    }
    
    /**
     * @notice Detect repeating patterns in entropy
     * @dev Checks for common patterns that indicate low-quality entropy
     */  
    function detectPatterns(bytes32 entropy) internal pure returns (uint256) {
        uint256 val = uint256(entropy);
        uint256 score = 30; // Start with maximum score
        
        // Check for common patterns that indicate poor randomness
        
        // Check for repeating nibble patterns
        uint256 last4Nibbles = 0;
        uint256 repeatedNibblePatterns = 0;
        
        for (uint i = 0; i < 64; i++) {
            uint256 nibble = (val >> (i * 4)) & 0xF;
            uint256 currentPattern = (last4Nibbles << 4) | nibble;
            last4Nibbles = (last4Nibbles >> 4) | (nibble << 12);
            
            // Check for previously seen patterns using bit manipulation
            if ((currentPattern & 0xFFFF) == (currentPattern >> 8)) {
                repeatedNibblePatterns++;
            }
        }
        
        // Penalize for repeating nibble patterns
        if (repeatedNibblePatterns > 2) {
            score -= (repeatedNibblePatterns - 2) * 5;
        }
        
        // Check for potential arithmetic sequences
        uint8[8] memory byteSequence;
        for (uint i = 0; i < 8; i++) {
            byteSequence[i] = uint8(entropy[i]);
        }
        
        uint256 sequenceDetected = 0;
        for (uint i = 0; i < 6; i++) {
            if (byteSequence[i+1] == byteSequence[i] + 1 && 
                byteSequence[i+2] == byteSequence[i] + 2) {
                sequenceDetected++;
            }
        }
        
        // Penalize for arithmetic sequences
        if (sequenceDetected > 0) {
            score -= sequenceDetected * 10;
        }
        
        // Ensure score doesn't go negative
        if (score > 30) return 30;
        return score > 0 ? score : 0;
    }
    
    /**
     * @notice Count the number of set bits in a bytes32 value
     */
    function countBits(bytes32 value) internal pure returns (uint256) {
        uint256 count;
        uint256 val = uint256(value);
        
        // Count bits using Brian Kernighan's algorithm
        while (val > 0) {
            val &= (val - 1);
            count++;
        }
        
        return count;
    }
}
