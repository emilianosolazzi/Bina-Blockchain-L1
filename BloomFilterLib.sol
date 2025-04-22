// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title BloomFilterLib
 * @notice A gas-efficient bloom filter library for probabilistic membership testing
 * @dev Implements a bloom filter with configurable hash functions and bit manipulation for storage efficiency.
 *      Optimized for use in contracts like EnhancedTemporalGradientBeacon to track used outputs.
 *      False-positive rate is approximately (1 - e^(-k*n/m))^k, where:
 *      - k: number of hash functions (numHashes)
 *      - n: number of inserted elements
 *      - m: number of bits (size * 256, as each bucket is a bytes32 with 256 bits)
 */
library BloomFilterLib {
    // Custom errors for gas-efficient error handling
    error InvalidFilterSize();
    error ZeroSize();
    error InvalidNumHashes();
    error IndexOutOfBounds();

    // Constants for hash function seeds
    uint256 private constant HASH_SEED_1 = 1;
    uint256 private constant HASH_SEED_2 = 2;
    uint256 private constant HASH_SEED_3 = 3;

    // Maximum number of hash functions to prevent excessive gas consumption
    uint256 private constant MAX_HASHES = 8;

    /**
     * @notice Structure representing a bloom filter
     * @param buckets Array of bytes32, each representing 256 bits
     * @param size Number of buckets (must be a power of 2)
     * @param numHashes Number of hash functions to use
     */
    struct Filter {
        bytes32[] buckets;
        uint256 size;
        uint256 numHashes;
    }

    // Event for debugging and monitoring
    event FilterUpdated(bytes32 indexed entry, uint256 indexed bucketIndex);
    event FilterCleared(uint256 size, uint256 numHashes);

    /**
     * @notice Creates a new bloom filter with the specified size and number of hash functions
     * @param size The number of buckets (must be a power of 2 and non-zero)
     * @param numHashes The number of hash functions (1 to MAX_HASHES)
     * @return filter The newly created filter
     */
    function createFilter(uint256 size, uint256 numHashes) internal pure returns (Filter memory filter) {
        if (size == 0) revert ZeroSize();
        if ((size & (size - 1)) != 0) revert InvalidFilterSize();
        if (numHashes == 0 || numHashes > MAX_HASHES) revert InvalidNumHashes();

        bytes32[] memory buckets;
        // Use assembly for efficient array allocation
        assembly {
            buckets := mload(0x40)
            mstore(buckets, size)
            mstore(0x40, add(buckets, add(0x20, mul(size, 0x20))))
            let dataPtr := add(buckets, 0x20)
            for { let i := 0 } lt(i, size) { i := add(i, 1) } {
                mstore(add(dataPtr, mul(i, 0x20)), 0)
            }
        }

        filter.buckets = buckets;
        filter.size = size;
        filter.numHashes = numHashes;
    }

    /**
     * @notice Updates the bloom filter with a new entry using bit manipulation
     * @param filter The filter to update
     * @param entry The entry to add
     */
    function updateFilter(Filter storage filter, bytes32 entry) internal {
        uint256 size = filter.size;
        uint256 numHashes = filter.numHashes;

        // Generate hashes and set bits in buckets
        for (uint256 i = 0; i < numHashes; i++) {
            // Use deterministic seed for each hash function
            uint256 hash = uint256(keccak256(abi.encodePacked(entry, i + 1))) % (size * 256); // 256 bits per bucket
            uint256 bucketIndex = hash / 256;
            uint256 bitIndex = hash % 256;

            if (bucketIndex >= size) revert IndexOutOfBounds();

            // Set the specific bit in the bucket
            bytes32 currentBucket = filter.buckets[bucketIndex];
            filter.buckets[bucketIndex] = currentBucket | bytes32(1 << bitIndex);

            emit FilterUpdated(entry, bucketIndex);
        }
    }

    /**
     * @notice Checks if an entry might exist in the filter
     * @param filter The filter to check
     * @param entry The entry to check for
     * @return exists Whether the entry might exist (false positives possible, no false negatives)
     */
    function mightContain(Filter storage filter, bytes32 entry) internal view returns (bool exists) {
        uint256 size = filter.size;
        uint256 numHashes = filter.numHashes;

        // Cache buckets to minimize SLOADs
        for (uint256 i = 0; i < numHashes; i++) {
            uint256 hash = uint256(keccak256(abi.encodePacked(entry, i + 1))) % (size * 256);
            uint256 bucketIndex = hash / 256;
            uint256 bitIndex = hash % 256;

            if (bucketIndex >= size) return false;

            bytes32 bucket = filter.buckets[bucketIndex];
            if ((bucket & bytes32(1 << bitIndex)) == 0) {
                return false;
            }
        }

        return true;
    }

    /**
     * @notice Clears all buckets in the filter, resetting it to an empty state
     * @param filter The filter to clear
     */
    function clearFilter(Filter storage filter) internal {
        uint256 size = filter.size;
        for (uint256 i = 0; i < size; i++) {
            filter.buckets[i] = bytes32(0);
        }
        emit FilterCleared(size, filter.numHashes);
    }

    /**
     * @notice Estimates the false-positive rate of the filter
     * @param filter The filter to analyze
     * @param numEntries The number of entries added to the filter
     * @return rate The approximate false-positive rate (as a percentage, scaled by 1e18)
     */
    function estimateFalsePositiveRate(Filter storage filter, uint256 numEntries) internal view returns (uint256 rate) {
        uint256 m = filter.size * 256; // Total bits
        uint256 k = filter.numHashes;
        uint256 n = numEntries;

        // Approximate false-positive rate: (1 - e^(-k*n/m))^k
        // Use simplified approximation for gas efficiency: (k*n/m)^k
        // Scale to avoid floating-point arithmetic
        uint256 kn = k * n * 1e18;
        uint256 mScaled = m * 1e18;
        uint256 fraction = kn / mScaled;
        uint256 result = fraction;

        // Raise to power k (approximate for small k)
        for (uint256 i = 1; i < k; i++) {
            result = (result * fraction) / 1e18;
        }

        return result;
    }
}
