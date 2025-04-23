// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { MiningLib } from "./MiningLib.sol";
import { BloomFilterLib } from "./BloomFilterLib.sol";
import { ITGBT } from "./interfaces/ITGBT.sol";
import { RandomnessLib } from "./RandomnessLib.sol";

/**
 * @title GovernanceLib
 * @notice Library for governance-related functionality
 */
library GovernanceLib {
    // Events
    event GovernanceParameterChanged(string paramName, uint256 newValue);
    event TokenUpdated(address newToken);
    event MiningPoolCreated(uint256 indexed poolId, uint256 targetDifficulty, uint256 emissionBucket);
    event MiningPoolUpdated(uint256 indexed poolId, uint256 targetDifficulty, uint256 emissionBucket);
    event MiningPoolDeactivated(uint256 indexed poolId);
    event BloomFilterReset(uint256 size, uint256 numHashes);
    event RandomnessFeeChanged(uint256 oldFee, uint256 newFee);
    
    // Errors
    error ZeroAddress();
    error InvalidDifficulty();
    error InvalidPoolId();
    error MaxPoolsReached();
    error InvalidEpochParameters();
    error InvalidThreshold();
    error InvalidMinSubmissions();
    error ExpiryTooShort();
    error MinAgeTooLow();
    error MaxAgeTooLow();
    error MaxAgeTooHigh();
    error InvalidMultiplier();
    error InvalidThresholdValue();
    error MinContributionsTooLow();
    error MaxLessThanMin();
    error MaxContributionsTooHigh();
    
    struct GovernanceContext {
        mapping(uint256 => MiningLib.MiningPool) miningPools;
        uint256 poolCount;
        uint256 totalMined;
        uint256 minCommitmentAge;
        uint256 maxCommitmentAge;
        uint256 bonusMultiplier;
        uint256 bonusThreshold;
        uint256 minBlockInterval;
        uint256 minSubmissionsPerBlock;
        uint256 consensusThreshold;
        uint256 outputExpiryBlocks;
    }
    
    function createMiningPool(
        GovernanceContext storage self,
        uint256 targetDifficulty,
        uint256 emissionBucket,
        uint256 maxPools,
        uint256 minDifficulty,
        uint256 maxDifficulty,
        uint256 miningAllocation
    ) internal returns (uint256 poolId) {
        if (self.poolCount >= maxPools) revert MaxPoolsReached();
        if (targetDifficulty < minDifficulty || targetDifficulty > maxDifficulty) revert InvalidDifficulty();
        if (emissionBucket == 0 || self.totalMined + emissionBucket > miningAllocation) revert InvalidEpochParameters();

        poolId = self.poolCount++;
        self.miningPools[poolId] = MiningLib.MiningPool({
            targetDifficulty: targetDifficulty,
            emissionBucket: emissionBucket,
            totalMined: 0,
            active: true
        });

        emit MiningPoolCreated(poolId, targetDifficulty, emissionBucket);
        return poolId;
    }
    
    function updateMiningPool(
        GovernanceContext storage self,
        uint256 poolId,
        uint256 targetDifficulty,
        uint256 emissionBucket,
        bool active,
        uint256 minDifficulty,
        uint256 maxDifficulty,
        uint256 miningAllocation
    ) internal {
        if (poolId >= self.poolCount) revert InvalidPoolId();
        if (targetDifficulty < minDifficulty || targetDifficulty > maxDifficulty) revert InvalidDifficulty();
        if (emissionBucket == 0 || self.totalMined + emissionBucket > miningAllocation) revert InvalidEpochParameters();

        self.miningPools[poolId].targetDifficulty = targetDifficulty;
        self.miningPools[poolId].emissionBucket = emissionBucket;
        self.miningPools[poolId].active = active;

        emit MiningPoolUpdated(poolId, targetDifficulty, emissionBucket);
        if (!active) {
            emit MiningPoolDeactivated(poolId);
        }
    }
    
    function setCommitRevealParameters(
        GovernanceContext storage self,
        uint256 minAge,
        uint256 maxAge
    ) internal {
        if (minAge < 3) revert MinAgeTooLow();
        if (maxAge < minAge * 2) revert MaxAgeTooLow();
        if (maxAge > 1000) revert MaxAgeTooHigh();
        self.minCommitmentAge = minAge;
        self.maxCommitmentAge = maxAge;
        emit GovernanceParameterChanged("minCommitmentAge", minAge);
        emit GovernanceParameterChanged("maxCommitmentAge", maxAge);
    }
    
    function setBonusParameters(
        GovernanceContext storage self,
        uint256 multiplier,
        uint256 threshold,
        uint256 maxMultiplier
    ) internal {
        if (multiplier < 100 || multiplier > maxMultiplier) revert InvalidMultiplier();
        if (threshold <= 1) revert InvalidThresholdValue();
        self.bonusMultiplier = multiplier;
        self.bonusThreshold = threshold;
        emit GovernanceParameterChanged("bonusMultiplier", multiplier);
        emit GovernanceParameterChanged("bonusThreshold", threshold);
    }
    
    function setConsensusParameters(
        GovernanceContext storage self,
        uint256 minSubmissions,
        uint256 threshold
    ) internal {
        if (threshold < 51 || threshold > 100) revert InvalidThreshold();
        if (minSubmissions < 1) revert InvalidMinSubmissions();
        self.minSubmissionsPerBlock = minSubmissions;
        self.consensusThreshold = threshold;
        emit GovernanceParameterChanged("minSubmissionsPerBlock", minSubmissions);
        emit GovernanceParameterChanged("consensusThreshold", threshold);
    }
    
    function setContributionParameters(
        RandomnessLib.State storage state,
        uint256 minContributions,
        uint256 maxContributions
    ) internal {
        if (minContributions < 2) revert MinContributionsTooLow();
        if (maxContributions < minContributions) revert MaxLessThanMin();
        if (maxContributions > 50) revert MaxContributionsTooHigh();
        state.minContributions = minContributions;
        state.maxContributions = maxContributions;
        emit GovernanceParameterChanged("minContributions", minContributions);
        emit GovernanceParameterChanged("maxContributions", maxContributions);
    }
    
    function setRandomnessFee(
        RandomnessLib.State storage state,
        uint256 fee
    ) internal returns (uint256 oldFee) {
        oldFee = state.fee;
        state.fee = fee;
        emit RandomnessFeeChanged(oldFee, fee);
        return oldFee;
    }
}
