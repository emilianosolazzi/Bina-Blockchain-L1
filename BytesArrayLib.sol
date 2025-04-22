// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title BytesArrayLib
 * @notice Gas-optimized library for managing fixed-size bytes32 arrays in memory
 * @dev Avoids dynamic array allocation overhead using inline assembly
 */
library BytesArrayLib {
    /**
     * @notice Creates a fixed-length bytes32 array in memory
     * @param size Number of elements in the array
     * @return arr A freshly allocated bytes32[] array
     */
    function createBytes32Array(uint256 size) internal pure returns (bytes32[] memory arr) {
        assembly {
            // Allocate memory for array length + data
            arr := mload(0x40)
            mstore(arr, size) // Set array length
            let dataStart := add(arr, 0x20)
            let totalBytes := mul(size, 0x20)
            let end := add(dataStart, totalBytes)
            for { let ptr := dataStart } lt(ptr, end) { ptr := add(ptr, 0x20) } {
                mstore(ptr, 0) // Zero-initialize
            }
            mstore(0x40, add(end, 0x20)) // Update free memory pointer
        }
    }

    /**
     * @notice Sets a value in a bytes32 array
     * @dev Unsafe by default: assumes index is within bounds
     * @param arr The array to update
     * @param index Position to write
     * @param value The bytes32 value to insert
     */
    function set(bytes32[] memory arr, uint256 index, bytes32 value) internal pure {
        assembly {
            mstore(add(add(arr, 0x20), mul(index, 0x20)), value)
        }
    }

    /**
     * @notice Gets a value from a bytes32 array
     * @dev Unsafe by default: assumes index is within bounds
     * @param arr The array to read
     * @param index The index to read from
     * @return value The bytes32 value at the given index
     */
    function get(bytes32[] memory arr, uint256 index) internal pure returns (bytes32 value) {
        assembly {
            value := mload(add(add(arr, 0x20), mul(index, 0x20)))
        }
    }

    /**
     * @notice Safely sets a value in a bytes32 array with bounds checking
     * @dev Will revert if index is out of range
     * @param arr The array to update
     * @param index Position to write
     * @param value The bytes32 value to insert
     */
    function safeSet(bytes32[] memory arr, uint256 index, bytes32 value) internal pure {
        if (index >= arr.length) revert IndexOutOfBounds(index, arr.length);
        set(arr, index, value);
    }

    /**
     * @notice Safely gets a value from a bytes32 array with bounds checking
     * @dev Will revert if index is out of range
     * @param arr The array to read
     * @param index The index to read from
     * @return value The bytes32 value at the given index
     */
    function safeGet(bytes32[] memory arr, uint256 index) internal pure returns (bytes32 value) {
        if (index >= arr.length) revert IndexOutOfBounds(index, arr.length);
        return get(arr, index);
    }

    /**
     * @notice Returns the length of the array
     * @param arr The array to read
     * @return len Number of elements in the array
     */
    function length(bytes32[] memory arr) internal pure returns (uint256 len) {
        assembly {
            len := mload(arr)
        }
    }

    /// @notice Revert thrown when index is out of array bounds
    error IndexOutOfBounds(uint256 index, uint256 length);
}
