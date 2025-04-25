// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title BloomFilterLib
 * @notice A gas-efficient bloom filter library for probabilistic membership testing
 * @dev Implements a bloom filter with configurable hash functions, salt, and bit manipulation for storage efficiency.
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
     * @param salt A random value added during hashing to prevent certain attacks
     */
    struct Filter {
        bytes32[] buckets;
        uint256 size;
        uint256 numHashes;
        uint256 salt; // Added salt parameter
    }

    // Event for debugging and monitoring
    event FilterUpdated(bytes32 indexed entry, uint256 indexed bucketIndex);
    event FilterCleared(uint256 size, uint256 numHashes);
    event FilterReset(uint256 newSize, uint256 newNumHashes, uint256 newSalt); // Added event

    /**
     * @notice Creates a new bloom filter with the specified size, number of hash functions, and salt
     * @param size The number of buckets (must be a power of 2 and non-zero)
     * @param numHashes The number of hash functions (1 to MAX_HASHES)
     * @param salt A value used to randomize hash functions (e.g., block timestamp or random number)
     * @return filter The newly created filter
     */
    function createFilter(uint256 size, uint256 numHashes, uint256 salt) internal pure returns (Filter memory filter) {
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
        filter.salt = salt; // Store the salt
    }

    /**
     * @notice Updates the bloom filter with a new entry using bit manipulation
     * @param filter The filter to update
     * @param entry The entry to add
     */
    function updateFilter(Filter storage filter, bytes32 entry) internal {
        // --- Arbitrum Optimization: Cache storage reads ---
        uint256 size = filter.size;
        uint256 numHashes = filter.numHashes;
        uint256 salt = filter.salt;
        // --- End Optimization ---

        // Generate hashes and set bits in buckets
        // Use unchecked for loop counter
        unchecked {
            for (uint256 i = 0; i < numHashes; i++) { // Use cached numHashes
                // Incorporate salt into hash calculation
                uint256 hash = uint256(keccak256(abi.encodePacked(entry, i + 1, salt))) % (size * 256); // Use cached size, salt
                uint256 bucketIndex = hash / 256;
                uint256 bitIndex = hash % 256;

                // Bounds check is still necessary before storage write
                if (bucketIndex >= size) revert IndexOutOfBounds(); // Use cached size

                // Set the specific bit in the bucket
                bytes32 currentBucket = filter.buckets[bucketIndex];
                filter.buckets[bucketIndex] = currentBucket | bytes32(1 << bitIndex);

                emit FilterUpdated(entry, bucketIndex);
            }
        }
    }

    /**
     * @notice Checks if an entry might exist in the filter
     * @param filter The filter to check
     * @param entry The entry to check for
     * @return exists Whether the entry might exist (false positives possible, no false negatives)
     */
    function mightContain(Filter storage filter, bytes32 entry) internal view returns (bool exists) {
        // --- Arbitrum Optimization: Cache storage reads ---
        uint256 size = filter.size;
        uint256 numHashes = filter.numHashes;
        uint256 salt = filter.salt;
        bytes32[] storage buckets = filter.buckets; // Cache storage array pointer
        // --- End Optimization ---

        // Use unchecked for loop counter
        unchecked {
            for (uint256 i = 0; i < numHashes; i++) { // Use cached numHashes
                // Incorporate salt into hash calculation
                uint256 hash = uint256(keccak256(abi.encodePacked(entry, i + 1, salt))) % (size * 256); // Use cached size, salt
                uint256 bucketIndex = hash / 256;
                uint256 bitIndex = hash % 256;

                // Bounds check before reading from storage array
                if (bucketIndex >= size) return false; // Use cached size

                bytes32 bucket = buckets[bucketIndex]; // Read from cached pointer
                if ((bucket & bytes32(1 << bitIndex)) == 0) {
                    return false;
                }
            }
        }

        return true;
    }

    /**
     * @notice Clears all buckets in the filter, resetting it to an empty state
     * @param filter The filter to clear
     */
    function clearFilter(Filter storage filter) internal {
        uint256 size = filter.size; // Cache size
        // Use unchecked for loop counter
        unchecked {
            for (uint256 i = 0; i < size; i++) {
                filter.buckets[i] = bytes32(0);
            }
        }
        emit FilterCleared(size, filter.numHashes);
    }

    /**
     * @notice Resets the filter with new parameters (size, hashes, salt), effectively creating a new empty filter.
     * @dev This is generally preferred over dynamic resizing due to the complexity of re-hashing.
     * @param filter The filter storage reference to reset.
     * @param newSize The new number of buckets (must be power of 2, non-zero).
     * @param newNumHashes The new number of hash functions (1 to MAX_HASHES).
     * @param newSalt The new salt value.
     */
    function resetFilter(Filter storage filter, uint256 newSize, uint256 newNumHashes, uint256 newSalt) internal {
        if (newSize == 0) revert ZeroSize();
        if ((newSize & (newSize - 1)) != 0) revert InvalidFilterSize();
        if (newNumHashes == 0 || newNumHashes > MAX_HASHES) revert InvalidNumHashes();

        // Deallocate old buckets (important for storage refunds if applicable)
        delete filter.buckets;

        // Allocate new buckets (assembly might be slightly cheaper but less clear here)
        filter.buckets = new bytes32[](newSize);
        // Note: New array elements are zero-initialized by default.

        filter.size = newSize;
        filter.numHashes = newNumHashes;
        filter.salt = newSalt;

        emit FilterReset(newSize, newNumHashes, newSalt);
    }

    /**
     * @notice Counts the total number of set bits in the filter.
     * @dev Gas intensive: Use primarily for off-chain analysis or infrequent on-chain checks.
     * @param filter The filter to analyze.
     * @return count The total number of set bits.
     */
    function countSetBits(Filter storage filter) internal view returns (uint256 count) {
        // --- Arbitrum Optimization: Cache storage reads ---
        uint256 size = filter.size;
        bytes32[] storage buckets = filter.buckets; // Cache storage array pointer
        // --- End Optimization ---
        count = 0;
        // Use unchecked block for loop counters
        unchecked {
            for (uint256 i = 0; i < size; i++) { // Use cached size
                bytes32 bucket = buckets[i]; // Read from cached pointer
                // Iterate through the 256 bits of the bucket
                for (uint256 j = 0; j < 256; j++) {
                    if ((bucket & bytes32(1 << j)) != 0) {
                        count++;
                    }
                }
            }
        }
    }

    /**
     * @notice Estimates the false-positive rate of the filter.
     * @param filter The filter to analyze.
     * @param numEntries The number of entries added to the filter.
     * @return rateBps The approximate false-positive rate in basis points (1 bps = 0.01%).
     */
    function estimateFalsePositiveRate(Filter storage filter, uint256 numEntries) internal view returns (uint256 rateBps) {
        // --- Arbitrum Optimization: Cache storage reads ---
        uint256 m = filter.size * 256; // Total bits
        uint256 k = filter.numHashes;
        // --- End Optimization ---
        uint256 n = numEntries;

        if (m == 0) return 10000; // 100% FP rate if filter size is zero

        // Approximate false-positive rate: (1 - e^(-k*n/m))^k
        // Using simplified approximation for gas efficiency: (k*n/m)^k
        // Scale to basis points (10000 = 100%)
        uint256 scale = 10000; // Basis points scale
        uint256 kn = k * n;

        // Calculate (kn / m) scaled by `scale`
        // Use uint256 division, ensuring intermediate multiplication doesn't overflow easily
        // (kn * scale) / m
        uint256 fractionScaled;
        if (kn > type(uint256).max / scale) {
             // Handle potential overflow if kn * scale is too large
             // Divide first, then multiply (loses precision but safer)
             fractionScaled = (kn / m) * scale;
        } else {
             fractionScaled = (kn * scale) / m;
        }

        uint256 result = fractionScaled;

        // Raise to power k (approximate for small k)
        // Use unchecked block for potential gas savings
        unchecked {
            for (uint256 i = 1; i < k; i++) {
                // (result * fractionScaled) / scale
                if (result > type(uint256).max / fractionScaled) {
                     // Handle potential overflow
                     result = (result / scale) * fractionScaled; // Divide first
                } else {
                     result = (result * fractionScaled) / scale;
                }

            }
        }

        // Ensure rate doesn't exceed 100% (10000 bps)
        return result > scale ? scale : result;
    }
}