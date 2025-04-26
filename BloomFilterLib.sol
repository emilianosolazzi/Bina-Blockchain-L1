// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title BloomFilterLib
 * @notice A gas-efficient bloom filter library optimized for Arbitrum L2 with 10M+ user scalability
 * @dev Implements a bloom filter with configurable hash functions, salt, and bit manipulation for storage efficiency.
 *      Optimized for the EnhancedTemporalGradientBeacon to track used outputs at massive scale.
 *      False-positive rate is approximately (1 - e^(-k*n/m))^k, where:
 *      - k: number of hash functions (numHashes)
 *      - n: number of inserted elements
 *      - m: number of bits (size * 256, as each bucket is a bytes32 with 256 bits)
 *      For 10M+ users, recommended parameters:
 *      - size: 65536 (maximum) for ~16.7M bits capacity
 *      - numHashes: 4-5 (optimal for expected load)
 *      - Dynamic scaling for different network conditions
 */
library BloomFilterLib {
    // Custom errors for gas-efficient error handling
    error InvalidFilterSize();
    error ZeroSize();
    error InvalidNumHashes();
    error IndexOutOfBounds();
    error NotPowerOfTwo();
    error ExceedsMaxSize();

    // Constants for hash function seeds
    uint256 private constant HASH_SEED_1 = 0x734f6e50;    // "sO_P"
    uint256 private constant HASH_SEED_2 = 0x46724062;    // "Fr@b"
    uint256 private constant HASH_SEED_3 = 0x34a2e4d9;    // Unique value for third hash function
    uint256 private constant HASH_SEED_4 = 0xb76c9e13;    // Added for >10M scaling
    uint256 private constant HASH_SEED_5 = 0x5a4e718c;    // Added for >10M scaling

    // Maximum number of hash functions to prevent excessive gas consumption
    uint256 private constant MAX_HASHES = 8;
    
    // Minimum and maximum filter sizes (must be powers of 2)
    uint256 private constant MIN_SIZE = 128;          // 128 * 256 = 32,768 bits minimum
    uint256 private constant MAX_SIZE = 65536;        // ~16.7M bits maximum

    /**
     * @notice Structure representing a bloom filter
     * @param buckets Array of bytes32, each representing 256 bits
     * @param size Number of buckets (must be a power of 2)
     * @param numHashes Number of hash functions to use
     * @param salt A random value added during hashing to prevent certain attacks
     * @param insertCount Number of items inserted (for analytics)
     */
    struct Filter {
        bytes32[] buckets;
        uint256 size;
        uint256 numHashes;
        uint256 salt;
        uint256 insertCount; // Track insertions for FPR estimation
    }

    // Events for debugging and monitoring
    event FilterUpdated(bytes32 indexed entry, uint256 indexed bucketIndex);
    event FilterCleared(uint256 size, uint256 numHashes);
    event FilterReset(uint256 newSize, uint256 newNumHashes, uint256 newSalt);
    event FilterScaled(uint256 oldSize, uint256 newSize, uint256 itemCount);

    /**
     * @notice Creates a new bloom filter with the specified size, number of hash functions, and salt
     * @dev Optimized for Arbitrum's gas model - allocates to precise size and performs all zeroing in a single assembly block
     * @param size The number of buckets (must be a power of 2 and non-zero)
     * @param numHashes The number of hash functions (1 to MAX_HASHES)
     * @param salt A value used to randomize hash functions (e.g., block timestamp or random number)
     * @return filter The newly created filter
     */
    function createFilter(uint256 size, uint256 numHashes, uint256 salt) internal pure returns (Filter memory filter) {
        validateFilterParams(size, numHashes);

        bytes32[] memory buckets;
        // Use assembly for gas-efficient allocation and zeroing on Arbitrum
        assembly {
            buckets := mload(0x40)
            mstore(buckets, size)
            mstore(0x40, add(buckets, add(0x20, mul(size, 0x20))))
            let dataPtr := add(buckets, 0x20)
            
            // Zero out buckets with 32-byte writes for maximum efficiency
            for { let i := 0 } lt(i, size) { i := add(i, 1) } {
                mstore(add(dataPtr, mul(i, 0x20)), 0)
            }
        }

        filter.buckets = buckets;
        filter.size = size;
        filter.numHashes = numHashes;
        filter.salt = salt;
        filter.insertCount = 0; // Initialize insert count
    }

    /**
     * @notice Updates the bloom filter with a new entry using bit manipulation
     * @dev Optimized for Arbitrum's storage costs with minimal reads and careful write batching
     * @param filter The filter to update
     * @param entry The entry to add
     */
    function updateFilter(Filter storage filter, bytes32 entry) internal {
        // --- Arbitrum Optimization: Cache storage reads ---
        uint256 size = filter.size;
        uint256 numHashes = filter.numHashes;
        uint256 salt = filter.salt;
        // --- End Optimization ---

        // For 10M+ scaling: Pre-compute hash seeds based on number of hash functions
        uint256[MAX_HASHES] memory hashSeeds;
        unchecked {
            for (uint256 i = 0; i < numHashes && i < MAX_HASHES; i++) {
                // Create unique hash seed for each function using the base seeds
                if (i == 0) hashSeeds[i] = HASH_SEED_1;
                else if (i == 1) hashSeeds[i] = HASH_SEED_2;
                else if (i == 2) hashSeeds[i] = HASH_SEED_3;
                else if (i == 3) hashSeeds[i] = HASH_SEED_4;
                else if (i == 4) hashSeeds[i] = HASH_SEED_5;
                else hashSeeds[i] = uint256(keccak256(abi.encodePacked(i, salt)));
            }
        }

        // Generate hashes and set bits in buckets, pre-allocate updated buckets array for gas savings
        bytes32[] memory updatedBuckets = new bytes32[](numHashes);
        uint256[] memory bucketIndices = new uint256[](numHashes);
        uint256[] memory bitIndices = new uint256[](numHashes);

        // Pre-compute all hash positions to batch storage operations
        unchecked {
            for (uint256 i = 0; i < numHashes; i++) {
                // Use pre-computed hash seeds and incorporate salt
                uint256 hash = uint256(
                    keccak256(abi.encodePacked(hashSeeds[i], entry, salt))
                ) % (size * 256);
                
                bucketIndices[i] = hash / 256;
                bitIndices[i] = hash % 256;
                
                // Bounds checking still needed to prevent out-of-bounds storage manipulation
                if (bucketIndices[i] >= size) revert IndexOutOfBounds();
                
                // Read buckets in one pass
                updatedBuckets[i] = filter.buckets[bucketIndices[i]];
            }
        }
        
        // Set all bits and update storage in one pass
        unchecked {
            for (uint256 i = 0; i < numHashes; i++) {
                // Set the bit
                bytes32 updatedBucket = updatedBuckets[i] | bytes32(1 << bitIndices[i]);
                filter.buckets[bucketIndices[i]] = updatedBucket;
                
                emit FilterUpdated(entry, bucketIndices[i]);
            }
        }
        
        // Increment insertion counter for FPR tracking
        filter.insertCount++;
    }

    /**
     * @notice Checks if an entry might exist in the filter
     * @dev Optimized for read-heavy workflows in high volume systems
     * @param filter The filter to check
     * @param entry The entry to check for
     * @return exists Whether the entry might exist (false positives possible, no false negatives)
     */
    function mightContain(Filter storage filter, bytes32 entry) internal view returns (bool exists) {
        // --- Arbitrum Optimization: Cache storage reads ---
        uint256 size = filter.size;
        uint256 numHashes = filter.numHashes;
        uint256 salt = filter.salt;
        // --- End Optimization ---

        // For 10M+ scaling: Pre-compute hash seeds based on number of hash functions
        uint256[MAX_HASHES] memory hashSeeds;
        unchecked {
            for (uint256 i = 0; i < numHashes && i < MAX_HASHES; i++) {
                // Create unique hash seed for each function using the base seeds
                if (i == 0) hashSeeds[i] = HASH_SEED_1;
                else if (i == 1) hashSeeds[i] = HASH_SEED_2;
                else if (i == 2) hashSeeds[i] = HASH_SEED_3;
                else if (i == 3) hashSeeds[i] = HASH_SEED_4;
                else if (i == 4) hashSeeds[i] = HASH_SEED_5;
                else hashSeeds[i] = uint256(keccak256(abi.encodePacked(i, salt)));
            }
        }

        unchecked {
            for (uint256 i = 0; i < numHashes; i++) {
                // Use pre-computed hash seed and incorporate salt
                uint256 hash = uint256(
                    keccak256(abi.encodePacked(hashSeeds[i], entry, salt))
                ) % (size * 256);
                
                uint256 bucketIndex = hash / 256;
                uint256 bitIndex = hash % 256;

                // Bounds check before reading
                if (bucketIndex >= size) return false;

                bytes32 bucket = filter.buckets[bucketIndex];
                if ((bucket & bytes32(1 << bitIndex)) == 0) {
                    return false;
                }
            }
        }

        return true;
    }

    /**
     * @notice Clears all buckets in the filter, resetting it to an empty state
     * @dev Optimized for gas efficiency with assembly-driven zeroing
     * @param filter The filter to clear
     */
    function clearFilter(Filter storage filter) internal {
        uint256 size = filter.size;
        
        // Use assembly for efficient loop zeroing
        assembly {
            let slot := filter.buckets.slot
            for { let i := 0 } lt(i, size) { i := add(i, 1) } {
                sstore(add(slot, i), 0)
            }
        }
        
        // Reset insertion counter
        filter.insertCount = 0;
        
        emit FilterCleared(size, filter.numHashes);
    }

    /**
     * @notice Resets the filter with new parameters, effectively creating a new empty filter
     * @dev Optimized for 10M+ user scale with proper storage manipulation for gas refunds
     * @param filter The filter storage reference to reset
     * @param newSize The new number of buckets (must be power of 2, non-zero)
     * @param newNumHashes The new number of hash functions (1 to MAX_HASHES)
     * @param newSalt The new salt value
     */
    function resetFilter(Filter storage filter, uint256 newSize, uint256 newNumHashes, uint256 newSalt) internal {
        validateFilterParams(newSize, newNumHashes);
        uint256 oldSize = filter.size;

        // Delete old buckets for gas refund (critical at large scales)
        assembly {
            let slot := filter.buckets.slot
            // Get current array length
            let length := sload(slot)
            
            // Delete length value
            sstore(slot, 0)
            
            // Delete array elements
            for { let i := 0 } lt(i, length) { i := add(i, 1) } {
                sstore(add(slot, add(i, 1)), 0)
            }
        }

        // Create and store new buckets
        bytes32[] storage buckets = filter.buckets;
        assembly {
            // Set new length
            sstore(buckets.slot, newSize)
        }
        
        // Initialize new array elements to zero
        unchecked {
            for (uint256 i = 0; i < newSize; i++) {
                buckets.push(bytes32(0));
            }
        }

        // Update filter parameters
        filter.size = newSize;
        filter.numHashes = newNumHashes;
        filter.salt = newSalt;
        filter.insertCount = 0; // Reset insertion count

        emit FilterReset(newSize, newNumHashes, newSalt);
    }
    
    /**
     * @notice Scale the filter up or down to handle changing user loads (10M+ optimization)
     * @dev Creates new filter with optimal parameters based on desired false positive rate and expected items
     * @param filter The filter storage reference to scale
     * @param newSize The new number of buckets (must be power of 2, non-zero)
     * @param expectedItems Expected number of items to store (helps tune parameters)
     * @param targetFPR Target false positive rate in basis points (e.g., 100 = 1%)
     */
    function scaleFilter(
        Filter storage filter,
        uint256 newSize,
        uint256 expectedItems,
        uint256 targetFPR
    ) internal {
        validateFilterParams(newSize, 0); // Validate size only, we'll calculate optimal hash count
        uint256 oldSize = filter.size;
        
        // Compute optimal number of hash functions for target FPR
        uint256 m = newSize * 256; // Total bits
        uint256 n = expectedItems;
        uint256 optimalHashes = calculateOptimalHashCount(m, n, targetFPR);
        
        // Constrain to library limits
        optimalHashes = optimalHashes > MAX_HASHES ? MAX_HASHES : optimalHashes;
        optimalHashes = optimalHashes == 0 ? 1 : optimalHashes;
        
        // Generate new salt for security
        uint256 newSalt = uint256(keccak256(abi.encodePacked(block.timestamp, block.prevrandao, filter.salt)));
        
        // Reset the filter with new parameters
        resetFilter(filter, newSize, optimalHashes, newSalt);
        
        emit FilterScaled(oldSize, newSize, expectedItems);
    }

    /**
     * @notice Counts the total number of set bits in the filter
     * @dev Gas intensive: Use primarily for off-chain analysis or infrequent on-chain checks
     * @param filter The filter to analyze
     * @return count The total number of set bits
     */
    function countSetBits(Filter storage filter) internal view returns (uint256 count) {
        // --- Arbitrum Optimization: Cache storage reads ---
        uint256 size = filter.size;
        // --- End Optimization ---
        
        count = 0;
        unchecked {
            for (uint256 i = 0; i < size; i++) {
                bytes32 bucket = filter.buckets[i];
                
                // Use Brian Kernighan's algorithm for counting bits - optimal for sparse filters
                uint256 value = uint256(bucket);
                while (value > 0) {
                    value &= value - 1; // Clear the least significant bit
                    count++;
                }
            }
        }
    }

    /**
     * @notice Estimates the false-positive rate (FPR) of the filter based on current parameters and fill level
     * @dev Provides more accurate real-time estimate based on actual insertion count
     * @param filter The filter to analyze
     * @return rateBps The approximate false-positive rate in basis points (1 bps = 0.01%)
     */
    function estimateCurrentFPR(Filter storage filter) internal view returns (uint256 rateBps) {
        // --- Arbitrum Optimization: Cache storage reads ---
        uint256 m = filter.size * 256; // Total bits
        uint256 k = filter.numHashes;
        uint256 n = filter.insertCount;
        // --- End Optimization ---

        if (m == 0) return 10000; // 100% FP rate if filter size is zero
        if (n == 0) return 0;     // 0% FP rate if nothing inserted

        // Approximate false-positive rate: (1 - e^(-k*n/m))^k
        // Using simplified approximation for gas efficiency: (1 - (1 - k*n/m)^k)
        // For small probabilities, this approximation works well
        uint256 scale = 10000; // Basis points scale
        uint256 kn = k * n;

        // Calculate (kn / m) scaled by `scale`
        uint256 fractionScaled;
        if (kn > type(uint256).max / scale) {
            // Handle potential overflow if kn * scale is too large
            fractionScaled = (kn / m) * scale;
        } else {
            fractionScaled = (kn * scale) / m;
        }

        // Ensure fraction doesn't exceed scale
        if (fractionScaled > scale) return scale;

        // Calculate (1 - fraction)^k using binomial approximation for small k
        uint256 complement = scale - fractionScaled;
        uint256 result = scale;

        unchecked {
            for (uint256 i = 0; i < k; i++) {
                // result = result * complement / scale;
                if (result > type(uint256).max / complement) {
                    // Handle potential overflow
                    result = (result / scale) * complement;
                } else {
                    result = (result * complement) / scale;
                }
            }
        }

        // Final result is (scale - result)
        return scale - result;
    }

    /**
     * @notice Estimates the false-positive rate for specified parameters and item count
     * @dev For planning filter size and hash count during scaling operations
     * @param size The bucket size to evaluate
     * @param numHashes The number of hash functions to use
     * @param itemCount The expected number of items to be inserted
     * @return rateBps The approximate false-positive rate in basis points (1 bps = 0.01%)
     */
    function estimateFPR(
        uint256 size,
        uint256 numHashes,
        uint256 itemCount
    ) internal pure returns (uint256 rateBps) {
        uint256 m = size * 256; // Total bits
        uint256 k = numHashes;
        uint256 n = itemCount;
        
        if (m == 0) return 10000; // 100% FP rate if filter size is zero
        if (n == 0) return 0;     // 0% FP rate if nothing inserted

        // Same calculation as estimateCurrentFPR but with passed parameters
        uint256 scale = 10000;
        uint256 kn = k * n;
        
        uint256 fractionScaled;
        if (kn > type(uint256).max / scale) {
            fractionScaled = (kn / m) * scale;
        } else {
            fractionScaled = (kn * scale) / m;
        }
        
        if (fractionScaled > scale) return scale;
        
        uint256 complement = scale - fractionScaled;
        uint256 result = scale;
        
        unchecked {
            for (uint256 i = 0; i < k; i++) {
                if (result > type(uint256).max / complement) {
                    result = (result / scale) * complement;
                } else {
                    result = (result * complement) / scale;
                }
            }
        }
        
        return scale - result;
    }

    /**
     * @notice Calculate optimal hash function count for target FPR
     * @dev Uses approximation formula derivation from bloom filter theory
     * @param m Total bits in filter
     * @param n Expected number of items
     * @param targetFPRbps Target FPR in basis points
     * @return k Optimal number of hash functions
     */
    function calculateOptimalHashCount(
        uint256 m,
        uint256 n, 
        uint256 targetFPRbps
    ) internal pure returns (uint256) {
        if (m == 0 || n == 0) return 1;
        
        // k = -ln(p) * (m/n) / ln(2) where p is target FPR
        // Simplified using basis points and integer math
        // k ≈ ln(10000/targetFPR) * (m/n) / ln(2)
        
        // Scale factor for precision
        uint256 scale = 1000;
        
        // p = targetFPRbps / 10000
        // ln(1/p) = ln(10000/targetFPRbps) ≈ ln(10000) - ln(targetFPRbps)
        // We approximate ln(10000) ≈ 9.21 and scale by 1000: 9210
        // And ln(2) ≈ 0.693 scaled: 693
        
        uint256 lnInvP;
        if (targetFPRbps <= 10) { // Very low FPR (≤0.1%)
            lnInvP = 9210; // ln(10000) * scale
        } else {
            // Approximate log using reference points
            if (targetFPRbps < 100) { // 0.1% to 1%
                lnInvP = 6908; // ln(1000) * scale
            } else if (targetFPRbps < 1000) { // 1% to 10%
                lnInvP = 4605; // ln(100) * scale
            } else { // 10% to 100%
                lnInvP = 2303; // ln(10) * scale
            }
        }
        
        // k = lnInvP * (m/n) / ln(2)
        uint256 mOverN;
        if (m > type(uint256).max / scale) {
            mOverN = (m / n) * scale;
        } else {
            mOverN = (m * scale) / n;
        }
        
        uint256 k = (lnInvP * mOverN) / (693 * scale);
        
        // Ensure k is at least 1 and at most MAX_HASHES
        return k == 0 ? 1 : (k > MAX_HASHES ? MAX_HASHES : k);
    }

    /**
     * @notice Extracts filter metrics for monitoring and scaling decisions
     * @dev Provides key metrics to determine when scaling is needed
     * @param filter The filter to analyze
     * @return size Number of buckets in the filter
     * @return bitCount Total bit capacity (size * 256)
     * @return insertCount Number of items inserted
     * @return fillRatioBps Fill percentage in basis points (100 = 1%)
     * @return estimatedFPRbps Estimated false positive rate in basis points
     */
    function getFilterMetrics(Filter storage filter) internal view returns (
        uint256 size,
        uint256 bitCount,
        uint256 insertCount,
        uint256 fillRatioBps,
        uint256 estimatedFPRbps
    ) {
        size = filter.size;
        bitCount = size * 256;
        insertCount = filter.insertCount;
        
        // Calculate fill ratio (setbits/total bits) in basis points
        uint256 setbits = countSetBits(filter);
        fillRatioBps = bitCount > 0 ? (setbits * 10000) / bitCount : 0;
        
        // Get estimated FPR
        estimatedFPRbps = estimateCurrentFPR(filter);
        
        return (size, bitCount, insertCount, fillRatioBps, estimatedFPRbps);
    }

    /**
     * @notice Helper function to validate filter parameters
     * @dev Centralizes all filter parameter validations
     * @param size The filter bucket size to validate
     * @param numHashes The number of hash functions to validate
     */
    function validateFilterParams(uint256 size, uint256 numHashes) private pure {
        if (size == 0) revert ZeroSize();
        if (size < MIN_SIZE) revert InvalidFilterSize();
        if (size > MAX_SIZE) revert ExceedsMaxSize();
        if ((size & (size - 1)) != 0) revert NotPowerOfTwo();
        if (numHashes > 0 && (numHashes == 0 || numHashes > MAX_HASHES)) revert InvalidNumHashes();
    }
}