// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title CoreUtilsLib
 * @notice Consolidated utility functions for EnhancedTemporalGradientBeacon
 * @dev Provides core utilities for output history management and cryptographic operations
 */
library CoreUtilsLib {
    error InvalidOutputHistory();
    error OutputNotFound();
    error InvalidHistorySize(uint256 size);

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
        for (uint256 i = 0; i < 32;) {
            outputs[i] = outputHistory[i];
            unchecked { ++i; }
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
        if (output == bytes32(0)) return false;
        if (historySize > 32) revert InvalidHistorySize(historySize);

        for (uint256 i = 0; i < historySize;) {
            if (history[i] == output) {
                return true;
            }
            unchecked { ++i; }
        }

        return false;
    }
}