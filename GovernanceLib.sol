// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { MiningLib } from "./MiningLib.sol";
import { BloomFilterLib } from "./BloomFilterLib.sol";
import { ITGBT } from "./interfaces/ITGBT.sol";
import { RandomnessLib } from "./RandomnessLib.sol";

/**
 * @title GovernanceLib
 * @notice Governance library optimized for Arbitrum L2 operations
 * @dev Contains functions for parameter updates with Arbitrum-specific optimizations
 */
library GovernanceLib {
    /* === Constants === */
    
    // Optimized gas constants for Arbitrum's unique pricing model
    uint256 internal constant ARBITRUM_CALLDATA_GAS_DISCOUNT = 4; // Arbitrum calldata is ~4x cheaper
    uint256 internal constant ARBITRUM_STORAGE_GAS_PREMIUM = 2; // Storage can be more expensive
    
    /* === Error definitions optimized for Arbitrum's custom errors === */
    error InvalidDifficulty();
    error InvalidEmission();
    error InvalidMultiplier();
    error InvalidThreshold();
    error MinAgeTooLow();
    error MaxAgeTooLow();
    error MaxAgeTooHigh();
    error MaxPoolsReached();
    error PoolNotFound();
    error MinContributionsTooLow();
    error MaxLessThanMin();
    error MaxContributionsTooHigh();
    
    /* === Structures === */
    
    /**
     * @notice Context for governance parameters storage (optimized for Arbitrum's 32-byte slots)
     */
    struct GovernanceContext {
        // Mining parameters
        mapping(uint256 => MiningLib.MiningPool) miningPools;
        uint256 poolCount;
        uint256 bonusThreshold;
        uint256 bonusMultiplier;
        uint64 outputExpiryBlocks; // Packed for efficient storage

        // Block parameters (packed together)
        uint8 minBlockInterval;
        uint8 minSubmissionsPerBlock;
        uint8 consensusThreshold;

        // Commitment parameters (packed together)
        uint8 minCommitmentAge;
        uint16 maxCommitmentAge;
    }
    
    /* === Mining pool management functions === */

    /**
     * @notice Creates a new mining pool with Arbitrum-optimized gas usage
     */
    function createMiningPool(
        GovernanceContext storage context,
        uint256 targetDifficulty,
        uint256 emissionBucket,
        uint256 maxPools,
        uint256 minDifficulty,
        uint256 maxDifficulty,
        uint256 totalAllocation
    ) internal returns (uint256 poolId) {
        // Validation with gas-optimized error handling
        if (targetDifficulty < minDifficulty || targetDifficulty > maxDifficulty) revert InvalidDifficulty();
        if (emissionBucket > totalAllocation) revert InvalidEmission();
        if (context.poolCount >= maxPools) revert MaxPoolsReached();
        
        // Use the next available ID
        poolId = context.poolCount;
        context.poolCount = poolId + 1;
        
        // Create the pool with parameters
        context.miningPools[poolId] = MiningLib.MiningPool({
            targetDifficulty: targetDifficulty,
            emissionBucket: emissionBucket,
            totalMined: 0,
            active: true
        });
        
        return poolId;
    }

    /**
     * @notice Updates a mining pool with Arbitrum-specific optimizations
     */
    function updateMiningPool(
        GovernanceContext storage context,
        uint256 poolId,
        uint256 targetDifficulty,
        uint256 emissionBucket, 
        bool active,
        uint256 minDifficulty,
        uint256 maxDifficulty,
        uint256 totalAllocation
    ) internal {
        // Validation with gas-optimized error handling
        if (poolId >= context.poolCount) revert PoolNotFound();
        if (targetDifficulty < minDifficulty || targetDifficulty > maxDifficulty) revert InvalidDifficulty();
        if (emissionBucket > totalAllocation) revert InvalidEmission();
        
        // Update pool parameters with efficient storage writes
        MiningLib.MiningPool storage pool = context.miningPools[poolId];
        pool.targetDifficulty = targetDifficulty;
        pool.emissionBucket = emissionBucket;
        pool.active = active;
    }

    /* === Parameter management functions === */

    /**
     * @notice Sets bonus parameters 
     * @dev Optimized for Arbitrum's storage pricing model
     */
    function setBonusParameters(
        GovernanceContext storage context,
        uint16 multiplier,
        uint256 threshold,
        uint256 maxMultiplier
    ) internal {
        if (multiplier > maxMultiplier) revert InvalidMultiplier(); // Use custom errors for gas savings
        if (threshold == 0) revert InvalidThreshold();
        
        // Batch storage updates for Arbitrum gas savings
        context.bonusMultiplier = multiplier;
        context.bonusThreshold = threshold;
    }

    /**
     * @notice Sets commit-reveal parameters with Arbitrum-specific validations
     */
    function setCommitRevealParameters(
        GovernanceContext storage context,
        uint8 minAge,
        uint16 maxAge
    ) internal {
        if (minAge < 1) revert MinAgeTooLow();
        if (maxAge < 3) revert MaxAgeTooLow();
        if (maxAge < minAge * 2) revert MaxAgeTooLow();
        if (maxAge > 1000) revert MaxAgeTooHigh();
        
        // Batch storage updates
        context.minCommitmentAge = minAge;
        context.maxCommitmentAge = maxAge;
    }

    /**
     * @notice Sets emergency fee parameters for randomness requests
     */
    function setEmergencyFeeParameters(
        RandomnessLib.State storage randomnessState,
        uint256 baseFee,
        uint256 feePerContributor
    ) internal {
        // Update all fields in one storage operation for gas efficiency
        randomnessState.baseEmergencyFee = baseFee;
        randomnessState.feePerContributor = feePerContributor;
    }
    
    /**
     * @notice Sets contribution parameters for randomness generation
     */
    function setContributionParameters(
        RandomnessLib.State storage randomnessState,
        uint256 minContributions,
        uint256 maxContributions
    ) internal {
        if (minContributions < 1) revert MinContributionsTooLow();
        if (maxContributions < minContributions) revert MaxLessThanMin();
        if (maxContributions > 100) revert MaxContributionsTooHigh();
        
        // Update parameters
        randomnessState.minContributions = minContributions;
        randomnessState.maxContributions = maxContributions;
    }
}
