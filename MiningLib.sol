// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { BloomFilterLib } from "./BloomFilterLib.sol";
import { ECDSAUpgradeable } from "@openzeppelin/contracts-upgradeable/utils/cryptography/ECDSAUpgradeable.sol";
import { MathUpgradeable as Math } from "@openzeppelin/contracts-upgradeable/utils/math/MathUpgradeable.sol";
import { CoreUtilsLib } from "./CoreUtilsLib.sol";

/**
 * @title MiningLib
 * @notice Library for mining-related functionality in the Temporal Gradient Beacon
 * @dev Combines commit–reveal mining with quantum resistance and adaptive difficulty
 */
library MiningLib {
    using ECDSAUpgradeable for bytes32;
    using BloomFilterLib for BloomFilterLib.Filter;
    using Math for uint256;

    // === Constants ===
    uint256 public constant BASE_WEIGHT         = 1e18;      // baseline compute capacity
    uint256 public constant MAX_WEIGHT          = 2e18;      // high-end compute cap
    uint256 public constant QR_HASH_ITERATIONS  = 3;          // rounds in quantumResistantHash
    uint256 public constant QR_HASH_ROTATION    = 7;          // bits to rotate each round
    uint256 public constant MAX_TIMESTAMP_DRIFT = 1 hours;   // replay protection

    // === Events ===
    event ExceptionalSolution(
        address indexed miner,
        uint256 difficulty,
        uint256 threshold,
        uint256 multiplier
    );

    // === Errors ===
    error ActiveCommitmentExists();
    error MiningTooFrequently();
    error NoCommitmentFound();
    error CommitmentAlreadyRevealed();
    error CommitmentTooRecent();
    error CommitmentExpired();
    error InvalidCommitment();
    error InvalidSigner();
    error InvalidSignature();
    error SolutionTooEasy();
    error OutputAlreadyUsed();
    error InsufficientStake();
    error InvalidPoolId();
    error MiningCapReached();
    error TimestampDriftTooLarge();
    error ZeroAddress();
    error MalformedInput();

    // === Structs ===
    struct CommitmentFlags { bool revealed; }

    struct Commitment {
        bytes32 commitHash;
        uint64  timestamp;
        CommitmentFlags flags;
        bytes32 revealedValue;
        uint8   poolId;
        uint256 deadline;
    }

    struct MiningPool {
        uint256 targetDifficulty;
        uint256 emissionBucket;
        uint256 totalMined;
        bool    active;
    }

    struct RevealParams {
        address miner;
        bytes32 previousOutput;
        bytes   temporalSeed;
        uint64  nonce;
        bytes   signature;
        bytes32 secretValue;
        uint8   poolId;
    }

    // === Core Logic ===

    function checkCommitmentValidity(
        RevealParams memory p,
        Commitment storage c
    ) internal view {
        if (p.miner == address(0)) revert ZeroAddress();
        bytes32 expected = keccak256(abi.encodePacked(
            p.previousOutput,
            p.temporalSeed,
            p.nonce,
            p.signature,
            p.secretValue,
            p.miner
        ));
        if (expected != c.commitHash) revert InvalidCommitment();
    }

    function processMiningReveal(
        bytes32 previousOutput,
        bytes memory temporalSeed,
        uint64 nonce,
        bytes memory signature,
        bytes32 secretValue,
        uint256 baseDifficulty,
        address sender,
        BloomFilterLib.Filter storage bloomFilter,
        mapping(bytes32 => uint256) storage usedOutputs,
        function(bytes memory) view returns (bytes32) hashFunction,
        function(address) view returns (uint256) difficultyWeightFn
    ) internal view returns (bytes32 hmacOutput) {
        if (sender == address(0)) revert ZeroAddress();
        if (signature.length == 0) revert MalformedInput();

        bytes memory entropy = abi.encodePacked(
            previousOutput,
            temporalSeed,
            nonce,
            sender,
            block.prevrandao,
            block.timestamp,
            secretValue
        );
        bytes32 entropyHash = keccak256(entropy);

        address recovered = entropyHash.recover(signature);
        if (recovered == address(0)) revert InvalidSignature();
        if (recovered != sender)    revert InvalidSigner();

        hmacOutput = hashFunction(abi.encodePacked(signature, entropyHash, secretValue));

        uint256 rawWeight = difficultyWeightFn(sender);
        uint256 weight    = rawWeight.max(BASE_WEIGHT / 2).min(MAX_WEIGHT);
        uint256 effective = baseDifficulty * weight / BASE_WEIGHT;

        if (uint256(hmacOutput) >= effective) revert SolutionTooEasy();
        if (usedOutputs[hmacOutput] != 0 || bloomFilter.mightContain(hmacOutput))
            revert OutputAlreadyUsed();

        if (block.timestamp > entropy.length + MAX_TIMESTAMP_DRIFT)
            revert TimestampDriftTooLarge();

        return hmacOutput;
    }

    function quantumResistantHash(bytes memory input) internal view returns (bytes32) {
        if (input.length == 0) revert MalformedInput();
        bytes32 h = keccak256(input);
        for (uint256 i = 0; i < QR_HASH_ITERATIONS; i++) {
            h = keccak256(abi.encodePacked(h ^ bytes32(i + 1), block.timestamp));
            h = bytes32((uint256(h) << QR_HASH_ROTATION) | (uint256(h) >> (256 - QR_HASH_ROTATION)));
        }
        return h;
    }

    /// @notice Calculate mining reward, may emit ExceptionalSolution
    function calculateMiningReward(
        bytes32 hmacOutput,
        uint256 baseReward,
        uint256 bonusThreshold,
        uint256 bonusMultiplier,
        uint256 totalMined,
        uint256 globalCap,
        MiningPool storage pool
    ) internal returns (uint256 reward) {
        uint256 difficulty = type(uint256).max - uint256(hmacOutput);
        reward = baseReward;

        uint256 bonusTarget = pool.targetDifficulty * bonusThreshold;
        if (difficulty > bonusTarget) {
            reward = (baseReward * bonusMultiplier) / 100;
            emit ExceptionalSolution(msg.sender, difficulty, bonusTarget, bonusMultiplier);
        }

        if (totalMined + reward > globalCap) {
            reward = globalCap > totalMined ? globalCap - totalMined : 0;
        }
        if (pool.totalMined + reward > pool.emissionBucket) {
            reward = pool.emissionBucket > pool.totalMined ? pool.emissionBucket - pool.totalMined : 0;
        }

        return reward;
    }

    function validatePreviousOutput(
        bytes32 previousOutput,
        bytes32[32] storage history,
        uint256 historySize
    ) internal view returns (bool found) {
        if (previousOutput == bytes32(0)) revert MalformedInput();
        // This function duplicates functionality in CoreUtilsLib.validatePreviousOutput
        // Delegate to CoreUtilsLib's implementation for DRY code
        return CoreUtilsLib.validatePreviousOutput(previousOutput, history, historySize);
    }

    /// @notice Estimate mining difficulty from hash
    function estimateDifficulty(bytes32 hashValue) internal pure returns (uint256) {
        return type(uint256).max - uint256(hashValue);
    }

    /// @notice Quick ECDSA signature check
    function validateSignature(
        bytes32 msgHash,
        bytes memory signature,
        address signer
    ) internal pure returns (bool valid) {
        if (signer == address(0) || signature.length == 0) revert MalformedInput();
        return (msgHash.recover(signature) == signer);
    }
}
