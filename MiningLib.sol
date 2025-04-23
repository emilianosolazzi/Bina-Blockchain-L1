// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { BloomFilterLib } from "./BloomFilterLib.sol";
import { ECDSAUpgradeable } from "@openzeppelin/contracts-upgradeable/utils/cryptography/ECDSAUpgradeable.sol";

/**
 * @title MiningLib
 * @notice Library for mining-related functionality in the Temporal Gradient Beacon
 * @dev Extracted from EnhancedTemporalGradientBeacon to reduce contract size
 */
library MiningLib {
    using ECDSAUpgradeable for bytes32;
    using BloomFilterLib for BloomFilterLib.Filter;
    
    // Errors
    error ActiveCommitmentExists();
    error MiningTooFrequently();
    error NoCommitmentFound();
    error CommitmentAlreadyRevealed();
    error CommitmentTooRecent();
    error CommitmentExpired();
    error InvalidCommitment();
    error InvalidSigner();
    error SolutionTooEasy();
    error OutputAlreadyUsed();
    error InsufficientStake();
    error InvalidPoolId();
    error BloomFilterNotInitialized();
    error MiningCapReached();
    
    // Structs
    struct CommitmentFlags {
        bool revealed;
    }
    
    struct Commitment {
        bytes32 commitHash;
        uint64 timestamp;
        CommitmentFlags flags;
        bytes32 revealedValue;
        uint256 poolId;
    }
    
    struct MiningPool {
        uint256 targetDifficulty;
        uint256 emissionBucket;
        uint256 totalMined;
        bool active;
    }
    
    // Struct to group reveal parameters
    struct RevealParams {
        address miner;
        bytes32 previousOutput;
        bytes temporalSeed;
        uint64 nonce;
        bytes signature;
        bytes32 secretValue;
        uint256 poolId;
    }
    
    /**
     * @notice Validates a mining commitment for a reveal operation
     * @param params Reveal parameters
     * @param commitment The commitment to validate
     */
    function checkCommitmentValidity(
        RevealParams memory params,
        Commitment storage commitment
    ) internal view {
        if (keccak256(abi.encodePacked(
            params.previousOutput,
            params.temporalSeed,
            params.nonce,
            params.signature,
            params.secretValue,
            params.miner
        )) != commitment.commitHash) {
            revert InvalidCommitment();
        }
    }
    
    /**
     * @notice Process a mining reveal to generate an HMAC output
     * @param previousOutput Previous beacon output
     * @param temporalSeed Temporal seed data
     * @param nonce Miner-provided nonce
     * @param signature Signed data to prove commitment
     * @param secretValue Secret revealed during reveal phase
     * @param poolDifficulty The difficulty target for this mining pool
     * @param sender The message sender
     * @param bloomFilter The bloom filter to check for used outputs
     * @param usedOutputs Mapping of outputs to the block number they were used
     * @return hmacOutput The generated HMAC output value
     */
    function processMiningReveal(
        bytes32 previousOutput,
        bytes memory temporalSeed,
        uint64 nonce,
        bytes memory signature,
        bytes32 secretValue,
        uint256 poolDifficulty,
        address sender,
        BloomFilterLib.Filter storage bloomFilter,
        mapping(bytes32 => uint256) storage usedOutputs,
        function(bytes memory) view returns (bytes32) hashFunction  // Changed parameter type from bytes32 to bytes memory
    ) internal view returns (bytes32 hmacOutput) {
        bytes memory input = abi.encodePacked(previousOutput, temporalSeed, nonce, sender, block.prevrandao, block.timestamp, secretValue);
        bytes32 inputHash = keccak256(input);

        // Recover signer from signature - note this needs to be provided by the caller
        address recovered = ECDSAUpgradeable.recover(inputHash, signature);
        if (recovered != sender) revert InvalidSigner();

        // We use a custom hash function provided by the caller to allow quantum resistance logic
        hmacOutput = hashFunction(abi.encodePacked(signature, inputHash, secretValue));

        if (uint256(hmacOutput) >= poolDifficulty) revert SolutionTooEasy();
        if (usedOutputs[hmacOutput] != 0 || BloomFilterLib.mightContain(bloomFilter, hmacOutput)) revert OutputAlreadyUsed();

        return hmacOutput;
    }

    /**
     * @notice Simple quantum resistant hash function example
     * @dev Can be replaced with more sophisticated quantum resistant algorithms
     * @param input The input to hash
     * @return A quantum resistant hash output
     */
    function quantumResistantHash(bytes memory input) internal view returns (bytes32) {
        bytes32 state = keccak256(input);
        for (uint256 i = 0; i < 3; i++) {
            state = keccak256(abi.encodePacked(state ^ bytes32(uint256(i + 1)), block.timestamp));
            state = bytes32((uint256(state) << 7) | (uint256(state) >> 249));
        }
        return state;
    }
    /**
     * @notice Calculate mining reward based on difficulty of solution
     * @param hmacOutput The mining output hash
     * @param rewardAmount Base reward amount
     * @param bonusThreshold Minimum difficulty multiplier for bonus
     * @param bonusMultiplier Reward multiplier percentage (100 = 100%)
     * @param totalMined Total tokens mined so far
     * @param miningAllocation Total mining allocation
     * @param pool The mining pool data
     * @return calculatedReward The calculated reward amount
     */
    function calculateMiningReward(
        bytes32 hmacOutput,
        uint256 rewardAmount,
        uint256 bonusThreshold,
        uint256 bonusMultiplier,
        uint256 totalMined,
        uint256 miningAllocation,
        MiningPool storage pool
    ) internal view returns (uint256 calculatedReward) {
        uint256 _poolDifficulty = pool.targetDifficulty;
        
        uint256 actualDifficulty = type(uint256).max - uint256(hmacOutput);
        calculatedReward = rewardAmount;

        // Apply bonus for exceeding difficulty threshold
        if (actualDifficulty > _poolDifficulty * bonusThreshold) {
            calculatedReward = (rewardAmount * bonusMultiplier) / 100;
        }

        // Cap reward to remaining allocation
        if (totalMined + calculatedReward > miningAllocation) {
            calculatedReward = miningAllocation - totalMined;
        }
        
        // Cap reward to pool's remaining emission
        if (pool.totalMined + calculatedReward > pool.emissionBucket) {
            calculatedReward = pool.emissionBucket - pool.totalMined;
        }

        return calculatedReward;
    }

    /**
     * @notice Validates an assembly-optimized check if previousOutput exists in history
     * @param previousOutput The output to check
     * @param outputHistory The array of historical outputs
     * @param historySize The size of the history array
     * @return isValid True if the output exists in history
     */
    function validatePreviousOutput(
        bytes32 previousOutput,
        bytes32[32] storage outputHistory,  // Fixed size to match constant
        uint256 historySize
    ) internal view returns (bool isValid) {
        assembly {
            let i := 0
            let size := historySize
            let baseSlot := outputHistory.slot
            for { } lt(i, size) { i := add(i, 1) } {
                let slot := add(baseSlot, i)
                if eq(sload(slot), previousOutput) {
                    isValid := 1
                    break
                }
            }
        }
    }
}
