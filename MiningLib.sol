// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { BloomFilterLib } from "./BloomFilterLib.sol";
import { ECDSAUpgradeable } from "@openzeppelin/contracts-upgradeable/utils/cryptography/ECDSAUpgradeable.sol";

/**
 * @title MiningLib
 * @notice Library for mining-related functionality in the Temporal Gradient Beacon
 */
library MiningLib {
    using ECDSAUpgradeable for bytes32;
    using BloomFilterLib for BloomFilterLib.Filter;

    // === Constants ===
    uint256 public constant BASE_WEIGHT = 1e18; // i3 or low-end baseline
    uint256 public constant MAX_WEIGHT = 2e18; // Ryzen-tier cap

    // === Errors ===
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

    // === Structs ===
    struct CommitmentFlags {
        bool revealed;
    }

    struct Commitment {
        bytes32 commitHash;
        uint64 timestamp;
        CommitmentFlags flags;
        bytes32 revealedValue;
        uint8 poolId;
    }

    struct MiningPool {
        uint256 targetDifficulty;
        uint256 emissionBucket;
        uint256 totalMined;
        bool active;
    }

    struct RevealParams {
        address miner;
        bytes32 previousOutput;
        bytes temporalSeed;
        uint64 nonce;
        bytes signature;
        bytes32 secretValue;
        uint8 poolId;
    }

    // === Core Logic ===

    function checkCommitmentValidity(
        RevealParams memory params,
        Commitment storage commitment
    ) internal view {
        bytes32 expected = keccak256(abi.encodePacked(
            params.previousOutput,
            params.temporalSeed,
            params.nonce,
            params.signature,
            params.secretValue,
            params.miner
        ));
        if (expected != commitment.commitHash) revert InvalidCommitment();
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
        bytes memory entropy = abi.encodePacked(previousOutput, temporalSeed, nonce, sender, block.prevrandao, block.timestamp, secretValue);
        bytes32 entropyHash = keccak256(entropy);

        address recovered = entropyHash.recover(signature);
        if (recovered != sender) revert InvalidSigner();

        hmacOutput = hashFunction(abi.encodePacked(signature, entropyHash, secretValue));

        uint256 weight = difficultyWeightFn(sender); // typically scaled to 1e18
        uint256 effectiveDifficulty = baseDifficulty * weight / 1e18;

        if (uint256(hmacOutput) >= effectiveDifficulty) revert SolutionTooEasy();
        if (usedOutputs[hmacOutput] != 0 || bloomFilter.mightContain(hmacOutput)) revert OutputAlreadyUsed();

        return hmacOutput;
    }

    function quantumResistantHash(bytes memory input) internal view returns (bytes32) {
        bytes32 hash = keccak256(input);
        for (uint256 i = 0; i < 3; i++) {
            hash = keccak256(abi.encodePacked(hash ^ bytes32(uint256(i + 1)), block.timestamp));
            hash = bytes32((uint256(hash) << 7) | (uint256(hash) >> 249)); // rotate left 7
        }
        return hash;
    }

    function calculateMiningReward(
        bytes32 hmacOutput,
        uint256 baseReward,
        uint256 bonusThreshold,
        uint256 bonusMultiplier,
        uint256 totalMined,
        uint256 globalCap,
        MiningPool storage pool
    ) internal view returns (uint256 reward) {
        uint256 difficulty = type(uint256).max - uint256(hmacOutput);
        reward = baseReward;

        if (difficulty > pool.targetDifficulty * bonusThreshold) {
            reward = (baseReward * bonusMultiplier) / 100;
        }

        if (totalMined + reward > globalCap) {
            reward = globalCap - totalMined;
        }
        if (pool.totalMined + reward > pool.emissionBucket) {
            reward = pool.emissionBucket - pool.totalMined;
        }

        return reward;
    }

    function validatePreviousOutput(
        bytes32 previousOutput,
        bytes32[32] storage outputHistory,
        uint256 historySize
    ) internal view returns (bool found) {
        assembly {
            let i := 0
            let base := outputHistory.slot
            for { } lt(i, historySize) { i := add(i, 1) } {
                if eq(sload(add(base, i)), previousOutput) {
                    found := 1
                    break
                }
            }
        }
    }
}
