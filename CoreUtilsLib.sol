// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ECDSAUpgradeable } from "@openzeppelin/contracts-upgradeable/utils/cryptography/ECDSAUpgradeable.sol";

/**
 * @title CoreUtilsLib
 * @notice Consolidated utility functions for EnhancedTemporalGradientBeacon
 * @dev Provides core utilities for randomness generation, output history management, and cryptographic operations
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
        mapping(uint256 => uint8) contributionCount;
        uint256 requestCount;
    }

    /**
     * @notice Creates a new bytes32 array of specified size
     * @param size Length of the array to create
     * @return A new bytes32 array
     */
    function createBytes32Array(uint256 size) internal pure returns (bytes32[] memory) {
        return new bytes32[](size);
    }

    /**
     * @notice Computes a hash of the output history for verification
     * @param outputHistory Storage reference to the output history array
     * @return The keccak256 hash of all outputs in the history
     */
    function getHistoricalOutputsHash(bytes32[32] storage outputHistory) internal view returns (bytes32) {
        bytes32[] memory outputs = new bytes32[](32);
        for (uint256 i = 0; i < 32; i++) {
            outputs[i] = outputHistory[i];
        }
        return keccak256(abi.encodePacked(outputs));
    }

    /**
     * @notice Updates the circular output history buffer
     * @param outputHistory Storage reference to the output history array
     * @param currentOutputIndex Current index in the circular buffer
     * @param newOutput The new output to add to history
     * @return newIndex The updated index after insertion
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
     * @notice Creates a new randomness request
     * @param state Storage reference to the RandomnessState
     * @param requester Address of the requesting account
     * @param userSeed User-provided seed value
     * @param historicalHash Hash of current output history
     * @return requestId The ID of the new request
     */
    function createRequest(
        RandomnessState storage state,
        address requester,
        bytes32 userSeed,
        bytes32 historicalHash
    ) internal returns (uint256) {
        uint256 requestId = state.requestCount++;
        state.requests[requestId] = keccak256(
            abi.encodePacked(
                requester,
                userSeed,
                block.timestamp,
                historicalHash
            )
        );
        state.contributionCount[requestId] = 0;
        return requestId;
    }

    /**
     * @notice Processes an entropy contribution for a randomness request
     * @param state Storage reference to the RandomnessState
     * @param requestId ID of the request being contributed to
     * @param entropyContribution The entropy value being contributed
     * @param entropySignature Cryptographic signature of the contribution
     * @param contributor Address of the contributing account
     * @param historicalHash Hash of current output history
     * @return fulfilled Whether the request is now fulfilled
     * @return randomValue The final random value if fulfilled
     */
    function contributeEntropy(
        RandomnessState storage state,
        uint256 requestId,
        bytes32 entropyContribution,
        bytes calldata entropySignature,
        address contributor,
        bytes32 historicalHash
    ) internal returns (bool fulfilled, bytes32 randomValue) {
        // Validate request exists
        bytes32 requestHash = state.requests[requestId];
        require(requestHash != bytes32(0), "InvalidRequestId");

        // Verify cryptographic signature
        bytes32 signedMessage = entropyContribution.toEthSignedMessageHash();
        address recovered = signedMessage.recover(entropySignature);
        require(recovered == contributor, "InvalidSignature");

        // Enforce contribution limits
        uint8 contributions = state.contributionCount[requestId];
        require(contributions < state.maxContributions, "MaxContributionsReached");

        // Update state
        state.contributionCount[requestId] = contributions + 1;
        state.entropyAccumulator = uint256(
            keccak256(
                abi.encodePacked(
                    state.entropyAccumulator,
                    entropyContribution,
                    contributor,
                    requestId
                )
            )
        );
        state.entropyMerkleRoot = keccak256(
            abi.encodePacked(
                state.entropyAccumulator,
                historicalHash
            )
        );

        // Check fulfillment conditions
        if (state.contributionCount[requestId] >= state.minContributions) {
            randomValue = keccak256(
                abi.encodePacked(
                    state.entropyAccumulator,
                    historicalHash,
                    requestId
                )
            );
            fulfilled = true;
            state.requests[requestId] = randomValue;
            state.contributionCount[requestId] = 0;
        }
    }

    /**
     * @notice Retrieves the result of a randomness request
     * @param state Storage reference to the RandomnessState
     * @param requestId ID of the request to query
     * @return The random value if available, or zero if not fulfilled
     */
    function getRandomness(
        RandomnessState storage state,
        uint256 requestId
    ) internal view returns (bytes32) {
        return state.requests[requestId];
    }

    /**
     * @notice Validates if a previous output exists in history
     * @param output The output to validate
     * @param history Storage reference to the output history
     * @param historySize Size of the history buffer
     * @return exists Whether the output was found in history
     */
    function validatePreviousOutput(
        bytes32 output,
        bytes32[32] storage history,
        uint256 historySize
    ) internal view returns (bool exists) {
        for (uint256 i = 0; i < historySize; i++) {
            if (history[i] == output) {
                return true;
            }
        }
        return false;
    }
}