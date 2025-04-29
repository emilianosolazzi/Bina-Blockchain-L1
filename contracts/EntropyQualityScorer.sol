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
     * @notice Calculate quality metrics for entropy
     * @dev Implementation would use bit distribution, autocorrelation, etc.
     */
    function calculateEntropyQuality(bytes32 entropy) internal pure returns (uint256) {
        // Simplified implementation - would use more sophisticated tests in production
        uint256 bitCount = countBits(entropy);
        uint256 idealBits = 128; // Perfect entropy would have 128 bits in 256-bit value
        
        // Score based on deviation from ideal bit count (closer to 128 is better)
        uint256 deviation = bitCount > idealBits ? 
            bitCount - idealBits : 
            idealBits - bitCount;
            
        // 0-100 score, 100 being perfect
        return 100 - (deviation * 100 / idealBits);
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
