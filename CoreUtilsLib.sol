// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ECDSAUpgradeable } from "@openzeppelin/contracts-upgradeable/utils/cryptography/ECDSAUpgradeable.sol";

/**
 * @title CoreUtilsLib
 * @notice Consolidated utility functions for EnhancedTemporalGradientBeacon
 */
library CoreUtilsLib {
    using ECDSAUpgradeable for bytes32;

    struct RandomnessState {
        uint256 fee;
        uint64 expiryBlocks;
        uint8 minContributions;
        uint8 maxContributions;
        uint256 entropyAccumulator;
        bytes32 entropyMerkleRoot;
        mapping(uint256 => bytes32) requests;
        mapping(uint256 => uint8) contributionCount; // Tracks contributions per request
        uint256 requestCount;
    }

    /**
     * @notice Creates a bytes32 array
     */
    function createBytes32Array(uint256 size) internal pure returns (bytes32[] memory) {
        return new bytes32[](size);
    }

    /**
     * @notice Computes a hash of the output history
     */
    function getHistoricalOutputsHash(bytes32[32] storage outputHistory) internal view returns (bytes32) {
        bytes32[] memory outputs = createBytes32Array(32);
        for (uint256 i = 0; i < 32; i++) {
            outputs[i] = outputHistory[i];
        }
        return keccak256(abi.encodePacked(outputs));
    }

    /**
     * @notice Updates the output history
     */
    function updateOutputHistory(
        bytes32[32] storage outputHistory,
        uint64 currentOutputIndex,
        bytes32 newOutput
    ) internal returns (uint64) {
        uint64 newIndex = (currentOutputIndex + 1) % 32;
        outputHistory[newIndex] = newOutput;
        return newIndex;
    }

    /**
     * @notice Creates a randomness request
     */
    function createRequest(
        RandomnessState storage state,
        address requester,
        bytes32 userSeed,
        bytes32 historicalHash
    ) internal returns (uint256) {
        uint256 requestId = state.requestCount++;
        state.requests[requestId] = keccak256(abi.encodePacked(requester, userSeed, block.timestamp, historicalHash));
        state.contributionCount[requestId] = 0; // Initialize contribution count
        return requestId;
    }

    /**
     * @notice Contributes entropy for randomness
     */
    function contributeEntropy(
        RandomnessState storage state,
        uint256 requestId,
        bytes32 entropyContribution,
        bytes calldata entropySignature,
        address contributor,
        bytes32 historicalHash
    ) internal returns (bool fulfilled, bytes32 randomValue) {
        // Verify request exists
        require(state.requests[requestId] != bytes32(0), "InvalidRequestId");

        // Verify signature
        bytes32 signedMessage = entropyContribution.toEthSignedMessageHash();
        require(signedMessage.recover(entropySignature) == contributor, "InvalidSignature");

        // Check contribution limits
        require(state.contributionCount[requestId] < state.maxContributions, "MaxContributionsReached");

        // Update contribution count
        state.contributionCount[requestId]++;

        // Update entropy
        state.entropyAccumulator = uint256(
            keccak256(abi.encodePacked(state.entropyAccumulator, entropyContribution, contributor, requestId))
        );
        state.entropyMerkleRoot = keccak256(abi.encodePacked(state.entropyAccumulator, historicalHash));

        // Check if fulfilled
        if (state.contributionCount[requestId] >= state.minContributions) {
            randomValue = keccak256(abi.encodePacked(state.entropyAccumulator, historicalHash, requestId));
            fulfilled = true;
            // Reset for next use
            state.requests[requestId] = randomValue;
            state.contributionCount[requestId] = 0;
        }
    }

    /**
     * @notice Gets randomness for a request
     */
    function getRandomness(RandomnessState storage state, uint256 requestId) internal view returns (bytes32) {
        return state.requests[requestId];
    }
}