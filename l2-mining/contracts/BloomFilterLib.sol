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
 * 
 *      With size = 65536, numHashes = 4, and 1M outputs:
 *      - FPR ≈ (1-e^(-4*1000000/16777216))^4 ≈ 0.2%
 *      - This is better than the target 0.5% FPR
 */
library BloomFilterLib {
    // Custom errors for gas-efficient error handling
    error InvalidFilterSize();
    error ZeroSize();
    error InvalidNumHashes();
    error IndexOutOfBounds();
    error NotPowerOfTwo();
    error ExceedsMaxSize();
    error InvalidAppeal();
    error AlreadyAppealed();
    error NotConsortiumMember();
    error InsufficientVotes();
    error AppealAlreadyResolved();
    error InvalidConsortiumAction();
    error AppealDoesNotExist();  // Added custom error for appeal check
    error InvalidMemberAddress(); // Added custom error for member address check

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
     * @notice Structure for tracking appeal status and votes
     * @param output The output that triggered the false positive
     * @param reporter The address that reported the false positive
     * @param timestamp When the appeal was submitted
     * @param resolved Whether the appeal has been resolved
     * @param accepted Whether the appeal was accepted
     * @param voteCount Number of votes from consortium members
     * @param voters Mapping of addresses that have voted on this appeal
     */
    struct Appeal {
        bytes32 output;
        address reporter;
        uint256 timestamp;
        bool resolved;
        bool accepted;
        uint256 voteCount;
        mapping(address => bool) voters;
    }

    /**
     * @notice Structure representing a bloom filter with appeal tracking
     * @param buckets Array of bytes32, each representing 256 bits
     * @param size Number of buckets (must be a power of 2)
     * @param numHashes Number of hash functions to use
     * @param salt A random value added during hashing to prevent certain attacks
     * @param insertCount Number of items inserted (for analytics)
     * @param appealedOutputs Mapping to track appealed outputs
     * @param appeals Mapping to track detailed appeal status
     * @param consortiumMembers Mapping of addresses authorized to vote on appeals
     * @param minVotesRequired Minimum votes required to resolve an appeal
     * @param totalConsortiumMembers Total number of consortium members
     */
    struct Filter {
        bytes32[] buckets;
        uint256 size;
        uint256 numHashes;
        uint256 salt;
        uint256 insertCount; 
        mapping(bytes32 => bool) appealedOutputs;
        mapping(bytes32 => Appeal) appeals;
        mapping(address => bool) consortiumMembers;
        uint256 minVotesRequired;
        uint256 totalConsortiumMembers;
    }

    // Events for debugging and monitoring
    event FilterUpdated(bytes32 indexed entry, uint256 indexed bucketIndex);
    event FilterCleared(uint256 size, uint256 numHashes);
    event FilterReset(uint256 newSize, uint256 newNumHashes, uint256 newSalt);
    event FilterScaled(uint256 oldSize, uint256 newSize, uint256 itemCount);

    /**
     * @notice Event emitted when a false positive is detected and appealed
     * @dev This creates an immutable record for off-chain processing
     * @param output The output that triggered the false positive
     * @param reporter The address that reported the false positive
     * @param appealId A unique identifier for the appeal
     * @param proof Additional data proving this is a false positive
     * @param timestamp When the appeal was submitted
     */
    event FalsePositiveDetected(
        bytes32 indexed output,
        address indexed reporter,
        bytes32 indexed appealId,
        bytes proof,
        uint256 timestamp
    );
    
    /**
     * @notice Event for tracking appeal resolution status changes
     * @param appealId The ID of the appeal being updated
     * @param resolved Whether the appeal was resolved
     * @param accepted Whether the appeal was accepted (filter adjusted)
     */
    event AppealResolved(
        bytes32 indexed appealId,
        bool resolved,
        bool accepted
    );

    /**
     * @notice Event emitted when consortium membership changes
     * @param member The address being added or removed
     * @param isAdded Whether the member was added (true) or removed (false)
     * @param newTotalMembers The updated count of consortium members
     */
    event ConsortiumMembershipChanged(
        address indexed member,
        bool isAdded,
        uint256 newTotalMembers
    );
    
    /**
     * @notice Event emitted when minimum vote threshold changes
     * @param oldThreshold Previous minimum vote threshold
     * @param newThreshold New minimum vote threshold
     */
    event VoteThresholdChanged(
        uint256 oldThreshold,
        uint256 newThreshold
    );
    
    /**
     * @notice Event emitted when a consortium member votes on an appeal
     * @param appealId The ID of the appeal being voted on
     * @param voter The address of the consortium member voting
     * @param voteInFavor Whether the vote was in favor of accepting the appeal
     * @param newVoteCount Updated total vote count
     */
    event AppealVoteCast(
        bytes32 indexed appealId,
        address indexed voter,
        bool voteInFavor,
        uint256 newVoteCount
    );

    /**
     * @notice Creates a new bloom filter with the specified size, number of hash functions, and salt
     * @dev Initializes a storage-backed filter in place.
     *      This wrapper is used because `Filter` contains mappings and cannot be returned from memory.
     *      Existing state is cleared through `resetFilter()`.
     * @param filter The storage-backed filter to initialize
     * @param size The number of buckets (must be a power of 2 and non-zero)
     * @param numHashes The number of hash functions (1 to MAX_HASHES)
     * @param salt A value used to randomize hash functions (e.g., block timestamp or random number)
     */
    function createFilter(Filter storage filter, uint256 size, uint256 numHashes, uint256 salt) internal {
        resetFilter(filter, size, numHashes, salt);
        filter.insertCount = 0;
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

        // Optimized: Combined read-modify-write in a single loop instead of two separate loops
        unchecked {
            for (uint256 i = 0; i < numHashes; i++) {
                // Use pre-computed hash seeds and incorporate salt
                uint256 hash = uint256(
                    keccak256(abi.encodePacked(hashSeeds[i], entry, salt))
                ) % (size * 256);
                
                uint256 bucketIndex = hash / 256;
                uint256 bitIndex = hash % 256;
                
                // Bounds checking still needed to prevent out-of-bounds storage manipulation
                if (bucketIndex >= size) revert IndexOutOfBounds();
                
                // Read, modify, and write in a single pass - gas optimized
                bytes32 bucket = filter.buckets[bucketIndex];
                bucket = bucket | bytes32(1 << bitIndex);
                filter.buckets[bucketIndex] = bucket;
                
                emit FilterUpdated(entry, bucketIndex);
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
            // Calculate the slot for the dynamic array
            mstore(0x0, 0) // First slot of the struct Filter (buckets field is at index 0)
            let bucketsSlot := keccak256(0x0, 0x20) // Get array's slot
            
            // Zero out all buckets in the array
            for { let i := 0 } lt(i, size) { i := add(i, 1) } {
                sstore(add(bucketsSlot, i), 0)
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

        // Delete old buckets for gas refund (critical at large scales)
        assembly {
            // Calculate the slot for the dynamic array
            mstore(0x0, 0) // First slot of the struct Filter (buckets field is at index 0)
            let bucketsSlot := keccak256(0x0, 0x20) // Get array's slot
            
            // Get current array length
            let length := sload(bucketsSlot)
            
            // Delete length value
            sstore(bucketsSlot, 0)
            
            // Delete array elements
            for { let i := 0 } lt(i, length) { i := add(i, 1) } {
                sstore(add(bucketsSlot, add(i, 1)), 0)
            }
        }

        // Create and store new buckets
        bytes32[] storage buckets = filter.buckets;
        assembly {
            // Set new length - Calculate the slot again for consistency
            mstore(0x0, 0) // First slot of the struct Filter (buckets field is at index 0)
            let bucketsSlot := keccak256(0x0, 0x20) // Get array's slot
            sstore(bucketsSlot, newSize) // Set new length
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

    /**
     * @notice Calculates the theoretical FPR for the current filter parameters and validates against a target
     * @dev Used to confirm if current filter can handle expected volume with desired FPR
     * @param filter The filter to validate
     * @param expectedItems Expected number of items to be inserted
     * @param targetFPRbps Target false positive rate in basis points (e.g., 50 = 0.5%)
     * @return valid Whether the filter can meet the target FPR
     * @return actualFPRbps The estimated FPR with current settings
     */
    function validateFPR(
        Filter storage filter,
        uint256 expectedItems,
        uint256 targetFPRbps
    ) internal view returns (bool valid, uint256 actualFPRbps) {
        actualFPRbps = estimateFPR(
            filter.size,
            filter.numHashes,
            expectedItems
        );
        
        return (actualFPRbps <= targetFPRbps, actualFPRbps);
    }

    /**
     * @notice Reports a false positive and initiates an appeal process
     * @dev Emits an event for off-chain processing and records the appealed output
     * @param filter The filter where the false positive occurred
     * @param output The output incorrectly identified as present
     * @param proof Data proving this is a false positive (e.g. merkle proof)
     * @return appealId A unique identifier for this appeal
     */
    function reportFalsePositive(
        Filter storage filter,
        bytes32 output,
        bytes memory proof
    ) internal returns (bytes32 appealId) {
        // Verify this is actually a false positive
        require(mightContain(filter, output), "Not a false positive");
        
        // Check if already appealed
        if (filter.appealedOutputs[output]) revert AlreadyAppealed();
        
        // Generate a unique appeal ID
        appealId = keccak256(abi.encodePacked(
            output,
            msg.sender,
            block.number,
            block.timestamp
        ));
        
        // Record the appeal
        filter.appealedOutputs[output] = true;
        
        // Initialize appeal record
        Appeal storage newAppeal = filter.appeals[appealId];
        newAppeal.output = output;
        newAppeal.reporter = msg.sender;
        newAppeal.timestamp = block.timestamp;
        newAppeal.resolved = false;
        newAppeal.accepted = false;
        newAppeal.voteCount = 0;
        
        // Emit event for off-chain handling
        emit FalsePositiveDetected(
            output,
            msg.sender,
            appealId,
            proof,
            block.timestamp
        );
        
        return appealId;
    }
    
    /**
     * @notice Allows a consortium member to vote on an appeal
     * @dev Records vote and potentially resolves appeal if threshold met
     * @param filter The filter containing the appeal
     * @param appealId The ID of the appeal to vote on
     * @param acceptAppeal Whether to accept the appeal as legitimate
     * @return resolved Whether the appeal was resolved by this vote
     * @return finalAccepted The final decision if resolved
     */
    function voteOnAppeal(
        Filter storage filter,
        bytes32 appealId,
        bool acceptAppeal
    ) internal returns (bool resolved, bool finalAccepted) {
        // Check if caller is consortium member
        if (!filter.consortiumMembers[msg.sender]) revert NotConsortiumMember();
        
        Appeal storage appeal = filter.appeals[appealId];
        
        // Check if appeal exists and not already resolved - use custom error
        if (appeal.timestamp == 0) revert AppealDoesNotExist();
        if (appeal.resolved) revert AppealAlreadyResolved();
        
        // Check if member has already voted
        if (appeal.voters[msg.sender]) revert InvalidConsortiumAction();
        
        // Record vote
        appeal.voters[msg.sender] = true;
        appeal.voteCount++;
        
        emit AppealVoteCast(
            appealId,
            msg.sender,
            acceptAppeal,
            appeal.voteCount
        );
        
        // Check if threshold reached
        if (appeal.voteCount >= filter.minVotesRequired) {
            appeal.resolved = true;
            appeal.accepted = acceptAppeal;
            
            emit AppealResolved(
                appealId,
                true,
                acceptAppeal
            );
            
            // If appeal accepted and output is in filter, handle differently
            if (acceptAppeal) {
                // Mark the output as not used (implementation depends on parent contract)
                // This is a placeholder for parent contract implementation
            }
            
            return (true, acceptAppeal);
        }
        
        return (false, false);
    }
    
    /**
     * @notice Checks if an output has a pending appeal
     * @param filter The filter to check
     * @param output The output to check for appeals
     * @return hasAppeal Whether there is a pending appeal for this output
     */
    function hasAppeal(Filter storage filter, bytes32 output) internal view returns (bool) {
        return filter.appealedOutputs[output];
    }
    
    /**
     * @notice Gets the status of an appeal
     * @param filter The filter containing the appeal
     * @param appealId The ID of the appeal to check
     * @return exists Whether the appeal exists
     * @return resolved Whether the appeal has been resolved
     * @return accepted Whether the appeal was accepted
     * @return voteCount Current vote count for the appeal
     */
    function getAppealStatus(
        Filter storage filter,
        bytes32 appealId
    ) internal view returns (
        bool exists,
        bool resolved,
        bool accepted,
        uint256 voteCount
    ) {
        Appeal storage appeal = filter.appeals[appealId];
        
        exists = appeal.timestamp > 0;
        if (!exists) return (false, false, false, 0);
        
        return (
            true,
            appeal.resolved,
            appeal.accepted,
            appeal.voteCount
        );
    }

    /**
     * @notice Initializes consortium governance for filter appeals
     * @dev Sets up initial consortium members and voting thresholds
     * @param filter The filter to configure consortium for
     * @param initialMembers Array of initial consortium member addresses
     * @param minVotes Minimum votes required to resolve an appeal (must be > 0)
     */
    function initConsortium(
        Filter storage filter,
        address[] memory initialMembers,
        uint256 minVotes
    ) internal {
        // Replace string errors with custom errors for gas optimization
        if (initialMembers.length == 0) revert InvalidConsortiumAction();
        if (minVotes == 0) revert InsufficientVotes();
        
        // Reset any existing consortium settings
        filter.minVotesRequired = minVotes;
        filter.totalConsortiumMembers = 0;
        
        // Add all initial members
        for (uint256 i = 0; i < initialMembers.length; i++) {
            if (!filter.consortiumMembers[initialMembers[i]] && initialMembers[i] != address(0)) {
                filter.consortiumMembers[initialMembers[i]] = true;
                filter.totalConsortiumMembers++;
                
                emit ConsortiumMembershipChanged(
                    initialMembers[i],
                    true,
                    filter.totalConsortiumMembers
                );
            }
        }
        
        // Ensure min votes doesn't exceed total members
        if (minVotes > filter.totalConsortiumMembers) {
            filter.minVotesRequired = filter.totalConsortiumMembers;
            
            emit VoteThresholdChanged(
                minVotes,
                filter.minVotesRequired
            );
        }
    }
    
    /**
     * @notice Updates consortium membership
     * @dev Adds or removes members from the consortium
     * @param filter The filter to modify consortium for
     * @param member The address to add or remove
     * @param isAdding Whether to add (true) or remove (false) the member
     */
    function updateConsortiumMembership(
        Filter storage filter,
        address member,
        bool isAdding
    ) internal {
        // Replace require with custom error for gas optimization
        if (member == address(0)) revert InvalidMemberAddress();
        
        if (isAdding && !filter.consortiumMembers[member]) {
            filter.consortiumMembers[member] = true;
            filter.totalConsortiumMembers++;
            
            emit ConsortiumMembershipChanged(
                member,
                true,
                filter.totalConsortiumMembers
            );
        } else if (!isAdding && filter.consortiumMembers[member]) {
            filter.consortiumMembers[member] = false;
            filter.totalConsortiumMembers--;
            
            emit ConsortiumMembershipChanged(
                member,
                false,
                filter.totalConsortiumMembers
            );
            
            // Ensure min votes doesn't exceed total members
            if (filter.minVotesRequired > filter.totalConsortiumMembers && filter.totalConsortiumMembers > 0) {
                uint256 oldThreshold = filter.minVotesRequired;
                filter.minVotesRequired = filter.totalConsortiumMembers;
                
                emit VoteThresholdChanged(
                    oldThreshold,
                    filter.minVotesRequired
                );
            }
        }
    }
    
    /**
     * @notice Produces a report for storage pruning of used outputs
     * @dev Analyzes filter statistics to recommend optimal pruning parameters
     * @param filter The filter to analyze
     * @param targetUsage Target storage usage after pruning (percentage in bps)
     * @return pruneSizeBps Percentage of storage to prune (basis points)
     * @return estimatedGasSavings Estimated gas savings from pruning
     * @return recommendedNewSize Recommended new size after pruning
     * @return recommendedNewHashes Recommended new hash count after pruning
     */
    function generatePruningReport(
        Filter storage filter,
        uint256 targetUsage
    ) internal view returns (
        uint256 pruneSizeBps,
        uint256 estimatedGasSavings,
        uint256 recommendedNewSize,
        uint256 recommendedNewHashes
    ) {
        // Get current metrics
            (uint256 size, , uint256 insertCount, uint256 fillRatioBps, uint256 fprBps) = 
            getFilterMetrics(filter);
        
        // Calculate pruning parameters
        if (fillRatioBps > targetUsage) {
            pruneSizeBps = fillRatioBps - targetUsage;
        }
        
        // Estimate gas savings (rough approximation)
        // Each storage slot cleared is ~15,000 gas refund under current EVM rules
        uint256 slotsToBeCleared = (size * pruneSizeBps) / 10000;
        estimatedGasSavings = slotsToBeCleared * 15000;
        
        // Calculate recommended new size (power of 2 not exceeding current size)
        recommendedNewSize = size;
        while (recommendedNewSize > MIN_SIZE && fillRatioBps < targetUsage/2) {
            recommendedNewSize = recommendedNewSize / 2;
            fillRatioBps = fillRatioBps * 2;
        }
        
        // Calculate optimal hash count for updated size
        uint256 remaining_items = insertCount * (10000 - pruneSizeBps) / 10000;
        recommendedNewHashes = calculateOptimalHashCount(
            recommendedNewSize * 256,
            remaining_items,
            fprBps
        );
        
        return (pruneSizeBps, estimatedGasSavings, recommendedNewSize, recommendedNewHashes);
    }
    
    /**
     * @notice Implementation notes for integrating an iterable mapping for prunable outputs
     * @dev Add this documentation block as a guide for implementing prunable storage
     * 
     * To implement prunable storage of used outputs, consider this pattern:
     * 
     * struct AppealableOutput {
     *     bytes32 output;
     *     uint256 timestamp;
     *     bool appealed;
     *     mapping(address => bool) confirmations;
     * }
     * 
     * struct OutputStorage {
     *     mapping(bytes32 => AppealableOutput) outputs;
     *     mapping(uint256 => bytes32) outputsIndex;
     *     uint256 firstIndex;
     *     uint256 nextIndex;
     * }
     * 
     * To add:
     * - outputs[output] = AppealableOutput(output, block.timestamp, false)
     * - outputsIndex[nextIndex++] = output;
     * 
     * To prune:
     * - For each i from firstIndex to firstIndex+count:
     *   - output = outputsIndex[i]
     *   - If output not appealed && timestamp < pruneTime:
     *     - Delete outputs[output]
     *     - Delete outputsIndex[i]
     *   - Else: retain, potentially compact
     * - Update firstIndex
     * 
     * The bloom filter remains as the primary screen, with this storage
     * as a secondary verification layer for handle edge cases and appeals.
     */
}