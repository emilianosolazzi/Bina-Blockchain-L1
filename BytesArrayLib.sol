// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title BytesArrayLib
 * @notice Gas-efficient library for managing fixed-size arrays of bytes32
 * @dev Avoids dynamic array allocation overhead
 */
library BytesArrayLib {
    /**
     * @notice Create a new bytes32 array in memory without dynamic allocation overhead
     * @param size The size of the array to create
     * @return arr The newly created array
     */
    function createBytes32Array(uint256 size) internal pure returns (bytes32[] memory arr) {
        assembly {
            // Allocate memory for the array
            // 0x20 bytes for the length + (size * 0x20) bytes for the data
            arr := mload(0x40)
            
            // Store the array length
            mstore(arr, size)
            
            // Update the free memory pointer
            // 0x20 (length) + (size * 0x20) bytes for items + 0x20 for padding
            mstore(0x40, add(arr, add(0x20, mul(size, 0x20))))
            
            // Initialize array elements to zero
            let dataPtr := add(arr, 0x20)
            for { let i := 0 } lt(i, size) { i := add(i, 1) } {
                mstore(add(dataPtr, mul(i, 0x20)), 0)
            }
        }
    }
    
    /**
     * @notice Sets a value in a bytes32 array
     * @param arr The array to modify
     * @param index The index to set
     * @param value The value to set
     */
    function set(bytes32[] memory arr, uint256 index, bytes32 value) internal pure {
        assembly {
            // Calculate the position in memory and store the value
            mstore(add(add(arr, 0x20), mul(index, 0x20)), value)
        }
    }
    
    /**
     * @notice Gets a value from a bytes32 array
     * @param arr The array to read from
     * @param index The index to read
     * @return value The value at the index
     */
    function get(bytes32[] memory arr, uint256 index) internal pure returns (bytes32 value) {
        assembly {
            // Calculate the position in memory and load the value
            value := mload(add(add(arr, 0x20), mul(index, 0x20)))
        }
    }
}

