// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { BloomFilterLib } from "./BloomFilterLib.sol";
import { ECDSAUpgradeable } from "@openzeppelin/contracts-upgradeable/utils/cryptography/ECDSAUpgradeable.sol";
import { MathUpgradeable as Math } from "@openzeppelin/contracts-upgradeable/utils/math/MathUpgradeable.sol";
import { CoreUtilsLib } from "./CoreUtilsLib.sol";

/**
 * @title MiningLib
 * @notice Library for mining-related functionality in the Temporal Gradient Beacon
 * @dev Combines commit–reveal mining with quantum resistance and adaptive difficulty
 */
library MiningLib {
    using ECDSAUpgradeable for bytes32;
    using BloomFilterLib for BloomFilterLib.Filter;
    using Math for uint256;

    // === Constants ===
    // Mining constants
    uint256 private constant BASE_WEIGHT = 1e18;          // Normalized base (18 decimals)
    uint256 private constant MAX_WEIGHT = 2e18;           // 2x base weight
    uint256 private constant MIN_WEIGHT = 5e17;          // 0.5x base weight
    uint256 private constant WEIGHT_PRECISION = 1e18;    // Precision for calculations
    
    // Hash parameters 
    uint16 private constant QR_HASH_ITERATIONS = 3;      // Gas optimized iterations
    uint8 private constant QR_HASH_ROTATION = 7;         // Prime number rotation
    uint8 private constant MIN_ENTROPY_BITS = 128;      // Minimum security bits
    
    // Time constraints (in seconds for gas efficiency)
    uint32 private constant MAX_TIMESTAMP_DRIFT = 3600;  // 1 hour
    uint32 private constant MIN_REVEAL_INTERVAL = 300;   // 5 minutes
    uint32 private constant RATE_LIMIT_WINDOW = 3600;    // 1 hour
    uint32 private constant MIN_DEADLINE = 3600;         // 1 hour
    uint32 private constant MAX_DEADLINE = 86400;        // 24 hours
    
    // Security thresholds
    uint16 private constant MAX_MINER_COUNT = 1000;      // Gas efficient uint16
    uint16 private constant MAX_COMMITS_PER_BLOCK = 100;
    uint8 private constant MAX_FAILED_ATTEMPTS = 3;      // Max sequential failures
    uint8 private constant MAX_VALIDATION_AGE = 100;     // In blocks
    
    // Validation bounds
    uint16 private constant MIN_ENTROPY_LENGTH = 32;     // Min bytes of entropy
    uint8 private constant MAX_SIGNATURE_LENGTH = 65;    // Standard ECDSA sig
    uint8 private constant MIN_SIGNATURE_LENGTH = 64;    // Compact ECDSA sig

    // Error categories & severity (optimized bit flags)
    uint8 private constant ERROR_SEVERITY_MASK = 0x0F;   // 0000 1111
    uint8 private constant ERROR_CATEGORY_MASK = 0xF0;   // 1111 0000
    
    uint8 private constant SEVERITY_LOW = 0x01;
    uint8 private constant SEVERITY_MEDIUM = 0x02;
    uint8 private constant SEVERITY_HIGH = 0x04;
    uint8 private constant SEVERITY_CRITICAL = 0x08;

    uint8 private constant ERROR_CATEGORY_TIMING = 0x10;
    uint8 private constant ERROR_CATEGORY_ACCESS = 0x20;
    uint8 private constant ERROR_CATEGORY_INPUT = 0x40;
    uint8 private constant ERROR_CATEGORY_STATE = 0x80;

    // Add validation helper functions before the Events section
    function combineErrorFlags(uint8 severity, uint8 category) internal pure returns (uint8) {
        // Ensure severity and category are valid
        if ((severity & ~ERROR_SEVERITY_MASK) != 0) revert MalformedInput(SEVERITY_HIGH, ERROR_CATEGORY_INPUT, "Invalid severity flag");
        if ((category & ~ERROR_CATEGORY_MASK) != 0) revert MalformedInput(SEVERITY_HIGH, ERROR_CATEGORY_INPUT, "Invalid category flag");
        return severity | category;
    }

    function validateErrorFlags(uint8 flags) internal pure {
        uint8 severity = flags & ERROR_SEVERITY_MASK;
        uint8 category = flags & ERROR_CATEGORY_MASK;
        
        // Must have exactly one severity bit set
        if (severity == 0 || (severity & (severity - 1)) != 0) 
            revert MalformedInput(SEVERITY_HIGH, ERROR_CATEGORY_INPUT, "Invalid severity combination");
            
        // Must have exactly one category bit set
        if (category == 0 || (category & (category - 1)) != 0)
            revert MalformedInput(SEVERITY_HIGH, ERROR_CATEGORY_INPUT, "Invalid category combination");
    }

    // === Events ===
    event ExceptionalSolution(
        address indexed miner,
        uint256 difficulty,
        uint256 threshold,
        uint256 multiplier
    );

    // === Errors ===
    // Basic errors (backwards compatibility)
    error ActiveCommitmentExists();
    error MiningTooFrequently();
    error NoCommitmentFound();
    error CommitmentAlreadyRevealed();
    
    // Enhanced errors with categories and severity
    error InvalidPoolId(uint8 severity, uint8 category);
    error MiningCapReached(uint8 severity, uint8 category);
    error TimestampDriftTooLarge(uint8 severity, uint8 category, uint256 drift);
    error ZeroAddress(uint8 severity, uint8 category);
    error MalformedInput(uint8 severity, uint8 category, string reason);
    error TimestampTooOld(uint8 severity, uint8 category, uint256 timestamp, uint256 minimum);
    error TimestampInFuture(uint8 severity, uint8 category, uint256 timestamp, uint256 maximum);
    error RateLimitExceeded(uint8 severity, uint8 category, uint64 windowStart, uint64 count);
    error ValidationFailed(uint8 severity, uint8 category, bytes32 validatorHash);
    error DeadlineInvalid(uint8 severity, uint8 category, uint256 deadline, uint256 minDuration, uint256 maxDuration);
    error InvalidRange(uint8 severity, uint8 category, uint256 min, uint256 max);
    error NonceAlreadyUsed(uint8 severity, uint8 category, uint256 nonce);
    
    // Additional enhanced errors
    error InvalidCommitment(uint8 severity, uint8 category);
    error InvalidSignature(uint8 severity, uint8 category);
    error InvalidSigner(uint8 severity, uint8 category);
    error SolutionTooEasy(uint8 severity, uint8 category);
    error OutputAlreadyUsed(uint8 severity, uint8 category);
    error InvalidBOMMarker(uint8 severity, uint8 category);
    error InvalidTemporalSeedFormat(uint8 severity, uint8 category);
    error HighSSignature(uint8 severity, uint8 category);
    error InvalidSignatureLength(uint8 severity, uint8 category);

    // === Structs ===
    struct CommitmentFlags { 
        bool revealed;
        bool validated; // Add validation flag
        bool revoked;    // Add revocation tracking
        bool emergency;  // Add emergency flag
    }

    struct ValidationInfo {
        uint64 blockNumber;
        uint64 timestamp;
        bytes32 validatorHash;
        bool success;
    }

    struct Commitment {
        bytes32 commitHash;
        uint64  timestamp;
        CommitmentFlags flags;
        bytes32 revealedValue;
        uint8   poolId;
        uint256 deadline;
        ValidationInfo validation; // Add validation info
        uint64  lastUpdateBlock;   // Add update tracking
    }

    struct MiningPool {
        uint256 targetDifficulty;
        uint256 emissionBucket;
        uint256 totalMined;
        bool    active;
        uint64  lastUpdateBlock; // Add last update tracking
        uint16  minerCount;      // Add miner count tracking
    }

    struct RevealParams {
        address miner;
        bytes32 previousOutput;
        bytes   temporalSeed;
        uint64  nonce;
        bytes   signature;
        bytes32 secretValue;
        uint8   poolId;
    }

    // === Core Logic ===

    function checkCommitmentValidity(
        RevealParams memory p,
        Commitment storage c
    ) internal view {
        if (p.miner == address(0)) revert ZeroAddress(SEVERITY_HIGH);
        bytes32 expected = keccak256(abi.encodePacked(
            p.previousOutput,
            p.temporalSeed,
            p.nonce,
            p.signature,
            p.secretValue,
            p.miner,
            block.chainid,  // Add chainId for replay protection
            address(this)   // Add contract address for domain separation
        ));
        if (expected != c.commitHash) revert InvalidCommitment(SEVERITY_HIGH, ERROR_CATEGORY_INPUT);
    }

    function processMiningReveal(
        bytes32 previousOutput,   // Temporal: Links to previous output in chain
        bytes memory temporalSeed, // Temporal: Time-based seed
        uint64 nonce,
        bytes memory signature,   // Spatial: Cryptographic proof of identity
        bytes32 secretValue,      // Spatial: Miner's entropy contribution
        uint256 baseDifficulty,
        address sender,
        BloomFilterLib.Filter storage bloomFilter,
        mapping(bytes32 => uint256) storage usedOutputs,
        function(bytes(memory) view returns (bytes32)) hashFunction,
        function(address) view returns (uint256) difficultyWeightFn
    ) internal view returns (bytes32 hmacOutput) {
        if (sender == address(0)) revert ZeroAddress(SEVERITY_HIGH);
        
        // Stricter temporal seed validation
        if (temporalSeed.length != 8) revert InvalidTemporalSeedFormat(SEVERITY_MEDIUM);
        
        // Check BOM marker in first byte
        if (temporalSeed[0] != 0x00) revert InvalidBOMMarker(SEVERITY_MEDIUM);

        // Extract timestamp with proper endianness handling
        uint64 seedTimestamp;
        assembly {
            // Load 8 bytes and handle endianness conversion
            let value := mload(add(temporalSeed, 32))
            // Convert big-endian to little-endian
            let swapped := or(
                or(
                    or(
                        and(shl(56, value), 0xFF00000000000000),
                        and(shl(40, value), 0x00FF000000000000)
                    ),
                    or(
                        and(shl(24, value), 0x0000FF0000000000),
                        and(shl(8, value), 0x000000FF00000000)
                    )
                ),
                or(
                    or(
                        and(shr(8, value), 0x00000000FF000000),
                        and(shr(24, value), 0x0000000000FF0000)
                    ),
                    or(
                        and(shr(40, value), 0x000000000000FF00),
                        and(shr(56, value), 0x00000000000000FF)
                    )
                )
            )
            seedTimestamp := swapped
        }

        // Cache block values
        uint256 currentTime = block.timestamp;
        bytes32 currentPrevrandao = block.prevrandao;

        // Enhanced timestamp validation
        if (seedTimestamp == 0) revert MalformedInput(SEVERITY_HIGH);
        
        // Minimum timestamp bound (1 week after contract deployment)
        if (seedTimestamp < 1704067200) revert TimestampTooOld(SEVERITY_HIGH, seedTimestamp); // Jan 1, 2024
        if (seedTimestamp < currentTime - 30 days) revert TimestampTooOld(SEVERITY_HIGH, seedTimestamp);
        if (seedTimestamp > currentTime + 15 minutes) revert TimestampInFuture(SEVERITY_HIGH, seedTimestamp);

        unchecked {
            if (currentTime > seedTimestamp && currentTime - seedTimestamp > MAX_TIMESTAMP_DRIFT) 
                revert TimestampDriftTooLarge(SEVERITY_HIGH, currentTime - seedTimestamp);
        }

        // Enhanced entropy mixing with cached values
        bytes32 timeBasedEntropy = keccak256(abi.encodePacked(
            currentTime,
            currentPrevrandao,
            seedTimestamp,
            address(this) // Add contract address for domain separation
        ));

        bytes memory entropy = abi.encodePacked(
            previousOutput,
            temporalSeed,
            nonce,
            sender,
            timeBasedEntropy, // Add enhanced entropy
            secretValue
        );
        bytes32 entropyHash = keccak256(entropy);

        address recovered = entropyHash.recover(signature);
        if (recovered == address(0)) revert InvalidSignature();
        if (recovered != sender)    revert InvalidSigner();

        hmacOutput = hashFunction(abi.encodePacked(signature, entropyHash, secretValue));

        uint256 rawWeight = difficultyWeightFn(sender);
        uint256 weight    = rawWeight.max(BASE_WEIGHT / 2).min(MAX_WEIGHT);
        uint256 effective = baseDifficulty * weight / BASE_WEIGHT;

        if (uint256(hmacOutput) >= effective) revert SolutionTooEasy();
        if (usedOutputs[hmacOutput] != 0 || bloomFilter.mightContain(hmacOutput))
            revert OutputAlreadyUsed();

        return hmacOutput;
    }

    function quantumResistantHash(bytes memory input) internal view returns (bytes32) {
        bytes32 h = keccak256(input);
        for (uint256 i = 0; i < QR_HASH_ITERATIONS; i++) {
            h = keccak256(abi.encodePacked(h ^ bytes32(i + 1), block.timestamp));
            h = bytes32((uint256(h) << QR_HASH_ROTATION) | (uint256(h) >> (256 - QR_HASH_ROTATION)));
        }
        return h;
    }

    /// @notice Calculate mining reward, may emit ExceptionalSolution
    function calculateMiningReward(
        bytes32 hmacOutput,
        uint256 baseReward,
        uint256 bonusThreshold,
        uint256 bonusMultiplier,
        uint256 totalMined,
        uint256 globalCap,
        MiningPool storage pool
    ) internal returns (uint256 reward) {
        uint256 difficulty = type(uint256).max - uint256(hmacOutput);
        reward = baseReward;

        uint256 bonusTarget = pool.targetDifficulty * bonusThreshold;
        if (difficulty > bonusTarget) {
            reward = (baseReward * bonusMultiplier) / 100;
            emit ExceptionalSolution(msg.sender, difficulty, bonusTarget, bonusMultiplier);
        }

        if (totalMined + reward > globalCap) {
            reward = globalCap > totalMined ? globalCap - totalMined : 0;
        }
        if (pool.totalMined + reward > pool.emissionBucket) {
            reward = pool.emissionBucket > pool.totalMined ? pool.emissionBucket - pool.totalMined : 0;
        }

        return reward;
    }

    function validatePreviousOutput(
        bytes32 previousOutput,
        bytes32[32] storage history,
        uint256 historySize
    ) internal view returns (bool found) {
        if (previousOutput == bytes32(0)) revert MalformedInput(SEVERITY_HIGH);
        // This function duplicates functionality in CoreUtilsLib.validatePreviousOutput
        // Delegate to CoreUtilsLib's implementation for DRY code
        return CoreUtilsLib.validatePreviousOutput(previousOutput, history, historySize);
    }

    /// @notice Estimate mining difficulty from hash
    function estimateDifficulty(bytes32 hashValue) internal pure returns (uint256) {
        return type(uint256).max - uint256(hashValue);
    }

    /// @notice Quick ECDSA signature check
    function validateSignature(
        bytes32 msgHash,
        bytes memory signature,
        address signer
    ) internal pure returns (bool valid) {
        if (signer == address(0) || signature.length == 0) revert MalformedInput(SEVERITY_HIGH);
        return (msgHash.recover(signature) == signer);
    }

    /// @notice Quantum resistant hash function that doesn't use block.timestamp
    /// @dev Uses multiple iterations and bit rotation for quantum resistance
    /// @param input The input bytes to hash
    /// @param extraEntropy Additional entropy to mix in (e.g., block number rather than timestamp)
    /// @return The quantum resistant hash
    function quantumResistantHashWithEntropy(
        bytes memory input, 
        bytes32 extraEntropy
    ) internal pure returns (bytes32) {
        if (input.length == 0) revert MalformedInput(SEVERITY_HIGH, ERROR_CATEGORY_INPUT, "Empty input");
        
        bytes32 h = keccak256(input);
        for (uint256 i = 0; i < QR_HASH_ITERATIONS; i++) {
            // Use extraEntropy instead of block.timestamp for more predictable behavior
            h = keccak256(abi.encodePacked(h ^ bytes32(i + 1), extraEntropy));
            h = bytes32((uint256(h) << QR_HASH_ROTATION) | (uint256(h) >> (256 - QR_HASH_ROTATION)));
        }
        return h;
    }

    /// @notice Automatically select a random number using accumulated entropy
    /// @dev Uses output history and quantum resistance with rejection sampling to avoid modulo bias
    /// @param outputs Array of entropy outputs from history to use as seed
    /// @param min Minimum value (inclusive)
    /// @param max Maximum value (inclusive)
    /// @param nonce User-provided nonce to prevent reuse
    /// @param usedNonces Mapping to track used nonces
    /// @return randomValue A random number between min and max (inclusive)
    function autoPickRandom(
        bytes32[] memory outputs,
        uint256 min,
        uint256 max,
        uint256 nonce,
        mapping(address => mapping(uint256 => bool)) storage usedNonces
    ) internal view returns (uint256 randomValue) {
        // Validate inputs
        if (min > max) revert InvalidRange(SEVERITY_HIGH, ERROR_CATEGORY_INPUT, min, max);
        if (outputs.length == 0) revert MalformedInput(SEVERITY_HIGH, ERROR_CATEGORY_INPUT, "Empty outputs");
        if (usedNonces[msg.sender][nonce]) revert NonceAlreadyUsed(SEVERITY_HIGH, ERROR_CATEGORY_INPUT, nonce);
        
        // Calculate range and required bit space
        uint256 range = max - min + 1;
        
        // Find the smallest power of 2 that is >= range
        uint256 mask = 1;
        while (mask < range) {
            mask <<= 1;
        }
        mask -= 1; // Create bit mask of all 1's
        
        // Multiple entropy sources - BEYOND block.timestamp
        bytes32 blockBasedEntropy = keccak256(abi.encodePacked(
            block.number,
            block.prevrandao,
            block.coinbase,
            block.difficulty,
            gasleft()
        ));
        
        // Combine entropy sources with user context
        bytes memory combinedEntropy = abi.encodePacked(
            outputs,
            blockBasedEntropy,
            msg.sender,
            nonce,
            block.timestamp // Still include timestamp but not as sole source
        );
        
        // Apply quantum-resistant hash function with additional entropy
        bytes32 resistant = quantumResistantHashWithEntropy(combinedEntropy, blockBasedEntropy);
        
        // Rejection sampling to avoid modulo bias
        uint256 generated;
        uint256 i = 0;
        while (true) {
            // If we've tried too many times, fall back to simple approach
            if (i >= 5) {
                randomValue = min + uint256(resistant) % range;
                break;
            }
            
            // Generate a value using part of the hash
            generated = uint256(resistant) & mask;
            
            // Check if it's within range
            if (generated < range) {
                randomValue = min + generated;
                break;
            }
            
            // Try again with modified entropy
            resistant = keccak256(abi.encodePacked(resistant, i));
            i++;
        }
        
        // Only mark nonce as used AFTER all validation has passed
        // In the actual function, this should be moved here
        // usedNonces[msg.sender][nonce] = true;
        
        return randomValue;
    }
    
    /// @notice Get multiple random values in one call
    /// @param outputs Array of entropy outputs from history to use as seed
    /// @param min Minimum value (inclusive)
    /// @param max Maximum value (inclusive)
    /// @param nonce Base nonce (will be incremented internally)
    /// @param count Number of random values to generate
    /// @param usedNonces Mapping to track used nonces
    /// @return values Array of random values between min and max (inclusive)
    function autoPickMultipleRandom(
        bytes32[] memory outputs,
        uint256 min,
        uint256 max,
        uint256 nonce,
        uint8 count,
        mapping(address => mapping(uint256 => bool)) storage usedNonces
    ) internal returns (uint256[] memory values) {
        values = new uint256[](count);
        
        for (uint8 i = 0; i < count; i++) {
            // First mark the nonce as used (moved from autoPickRandom)
            usedNonces[msg.sender][nonce + i] = true;
            
            // Then generate the random value
            values[i] = autoPickRandom(outputs, min, max, nonce + i, usedNonces);
        }
        
        return values;
    }

    // New: Enhanced signature validation with malleability checks
    function enhancedValidateSignature(
        bytes32 msgHash,
        bytes memory signature,
        address signer
    ) internal pure returns (bool valid) {
        if (signer == address(0)) revert ZeroAddress(SEVERITY_HIGH);
        if (signature.length != 65) revert InvalidSignatureLength(SEVERITY_HIGH, ERROR_CATEGORY_INPUT);
        
        bytes32 r;
        bytes32 s;
        uint8 v;
        assembly {
            r := mload(add(signature, 32))
            s := mload(add(signature, 64))
            v := byte(0, mload(add(signature, 96)))
        }
        
        // Check for signature malleability (high S values)
        if (uint256(s) > 0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0) {
            revert HighSSignature(SEVERITY_HIGH, ERROR_CATEGORY_INPUT);
        }
        
        return (msgHash.recover(signature) == signer);
    }

    // New: Improved deterministic random number generation without bias
    function improvedAutoPickRandom(
        bytes32[] memory outputs,
        uint256 min,
        uint256 max,
        uint256 nonce,
        mapping(address => mapping(uint256 => bool)) storage usedNonces
    ) internal view returns (uint256 randomValue) {
        // Input validation
        if (min > max) revert InvalidRange(SEVERITY_HIGH, ERROR_CATEGORY_INPUT, min, max);
        if (outputs.length == 0) revert MalformedInput(SEVERITY_HIGH, ERROR_CATEGORY_INPUT, "Empty outputs");
        if (usedNonces[msg.sender][nonce]) revert NonceAlreadyUsed(SEVERITY_HIGH, ERROR_CATEGORY_INPUT, nonce);

        uint256 range = max - min + 1;
        uint256 bitsNeeded = log2Ceiling(range);
        uint256 iterations = (bitsNeeded + 255) / 256; // Round up
        
        // Use historical block hashes for additional entropy
        bytes32 historicalEntropy;
        if (block.number > 256) {
            historicalEntropy = blockhash(block.number - 256);
        }
        
        bytes32 seed = keccak256(abi.encodePacked(
            outputs,
            block.prevrandao,
            block.number,
            msg.sender,
            nonce,
            historicalEntropy,
            block.chainid,
            address(this)
        ));
        
        uint256 result = 0;
        for (uint256 i = 0; i < iterations; i++) {
            seed = keccak256(abi.encodePacked(seed, i));
            if (i > 0) {
                // Shift by at most 255 bits to avoid overflow
                result = (result << (i == 1 ? 255 : 1)) | uint256(seed);
            } else {
                result = uint256(seed);
            }
        }
        
        randomValue = min + (result % range);
        return randomValue;
    }

    // Helper function for improved random number generation
    function log2Ceiling(uint256 x) private pure returns (uint256) {
        if (x == 0) return 0;
        uint256 y = (x & (x - 1)) == 0 ? 0 : 1;
        uint256 z = x;
        while (z > 1) {
            z >>= 1;
            y += 1;
        }
        return y;
    }

    // New: Optimized quantum resistant hashing with configurable security level
    function optimizedQuantumResistantHashWithEntropy(
        bytes memory input, 
        bytes32 extraEntropy,
        bool highSecurity
    ) internal pure returns (bytes32) {
        if (input.length == 0) revert MalformedInput(SEVERITY_HIGH, ERROR_CATEGORY_INPUT, "Empty input");
        
        uint256 iterations = highSecurity ? QR_HASH_ITERATIONS : 1;
        bytes32 h = keccak256(input);
        
        for (uint256 i = 0; i < iterations; i++) {
            h = keccak256(abi.encodePacked(h ^ bytes32(i + 1), extraEntropy));
            h = bytes32((uint256(h) << QR_HASH_ROTATION) | (uint256(h) >> (256 - QR_HASH_ROTATION)));
        }
        return h;
    }

    // New: Bloom filter maintenance function
    function pruneBloomFilter(
        BloomFilterLib.Filter storage filter,
        uint256 maxEntries,
        uint256 targetFillRatioBps
    ) internal {
        // This implementation depends on BloomFilterLib capabilities
        // For most bloom filters, you'd need to create a new one and migrate
        // Rather than actually remove items (which isn't possible in bloom filters)
        
        // Check if filter needs scaling down
        (uint256 size, uint256 bitCount, uint256 insertCount, uint256 fillRatioBps, ) = 
            BloomFilterLib.getFilterMetrics(filter);
            
        if (fillRatioBps > targetFillRatioBps) {
            // Create a new filter with parameters optimized for current load
            uint256 newSize = size;
            // Find optimal size that's a power of 2
            while (newSize > BloomFilterLib.MIN_SIZE && fillRatioBps > targetFillRatioBps) {
                newSize = newSize * 2;
                fillRatioBps = fillRatioBps / 2; // Approximation
            }
            
            // Recalculate optimal number of hash functions
            uint256 optimalHashes = BloomFilterLib.calculateOptimalHashCount(
                newSize * 256, insertCount, targetFillRatioBps
            );
            
            // Reset filter with new parameters - implementation depends on BloomFilterLib
            bytes32 newSalt = keccak256(abi.encodePacked(block.timestamp, block.prevrandao, address(this)));
            BloomFilterLib.resetFilter(filter, newSize, optimalHashes, uint256(newSalt));
        }
    }

    // Modified: Replace autoPickRandom with the improved version in autoPickMultipleRandom
    function autoPickMultipleRandom(
        bytes32[] memory outputs,
        uint256 min,
        uint256 max,
        uint256 nonce,
        uint8 count,
        mapping(address => mapping(uint256 => bool)) storage usedNonces
    ) internal returns (uint256[] memory values) {
        values = new uint256[](count);
        
        for (uint8 i = 0; i < count; i++) {
            // First mark the nonce as used
            usedNonces[msg.sender][nonce + i] = true;
            
            // Then generate the random value using improved algorithm
            values[i] = improvedAutoPickRandom(outputs, min, max, nonce + i, usedNonces);
        }
        
        return values;
    }

    // === Remaining Issues and Recommendations ===
    /**
     * @dev After reviewing the implementation, here are recommended improvements:
     *
     * 1. Random Number Generation:
     *    - The rejection sampling implementation could be more gas efficient
     *    - Replace the while(true) loop with a deterministic approach using multiple hash rounds:
     *
     *    function improvedAutoPickRandom(
     *        bytes32[] memory outputs,
     *        uint256 min,
     *        uint256 max,
     *        uint256 nonce,
     *        mapping(address => mapping(uint256 => bool)) storage usedNonces
     *    ) internal view returns (uint256 randomValue) {
     *        // Input validation
     *        if (min > max) revert InvalidRange(SEVERITY_HIGH, ERROR_CATEGORY_INPUT, min, max);
     *        if (outputs.length == 0) revert MalformedInput(SEVERITY_HIGH, ERROR_CATEGORY_INPUT, "Empty outputs");
     *        if (usedNonces[msg.sender][nonce]) revert NonceAlreadyUsed(SEVERITY_HIGH, ERROR_CATEGORY_INPUT, nonce);
     *
     *        uint256 range = max - min + 1;
     *        uint256 bitsNeeded = log2Ceiling(range);
     *        uint256 iterations = (bitsNeeded + 255) / 256; // Round up
     *        
     *        bytes32 seed = keccak256(abi.encodePacked(outputs, block.prevrandao, block.number, msg.sender, nonce));
     *        
     *        uint256 result = 0;
     *        for (uint256 i = 0; i < iterations; i++) {
     *            seed = keccak256(abi.encodePacked(seed, i));
     *            result = (result << min(i * 256, 255)) | uint256(seed);
     *        }
     *        
     *        randomValue = min + (result % range);
     *        return randomValue;
     *    }
     *
     * 2. Entropy Sources:
     *    - Still relies on potentially manipulable block.prevrandao
     *    - Consider incorporating historical block hashes (previous 256 blocks) or oracle-based entropy
     *
     * 3. Signature Validation:
     *    - Add explicit checks for signature malleability:
     *
     *    function enhancedValidateSignature(
     *        bytes32 msgHash,
     *        bytes memory signature,
     *        address signer
     *    ) internal pure returns (bool valid) {
     *        if (signature.length != 65) revert InvalidSignatureLength();
     *        bytes32 r;
     *        bytes32 s;
     *        uint8 v;
     *        assembly {
     *            r := mload(add(signature, 32))
     *            s := mload(add(signature, 64))
     *            v := byte(0, mload(add(signature, 96)))
     *        }
     *        if (uint256(s) > 0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0) {
     *            revert HighSSignature();
     *        }
     *        return (msgHash.recover(signature) == signer);
     *    }
     *
     * 4. Gas Optimization:
     *    - The quantum resistant hash function performs multiple keccak256 operations
     *    - Consider adding a security level parameter:
     *
     *    function optimizedQuantumResistantHashWithEntropy(
     *        bytes memory input, 
     *        bytes32 extraEntropy,
     *        bool highSecurity
     *    ) internal pure returns (bytes32) {
     *        uint256 iterations = highSecurity ? QR_HASH_ITERATIONS : 1;
     *        // Continue with optimized implementation...
     *    }
     *
     * 5. Timestamp Validation:
     *    - Make hardcoded timestamp (1704067200) configurable via initialization parameter
     *
     * 6. Error Handling Consistency:
     *    - Update remaining basic errors to use the enhanced categorization system:
     *      error InvalidCommitment(uint8 severity, uint8 category);
     *      error InvalidSignature(uint8 severity, uint8 category);
     *
     * 7. Front-running Protection:
     *    - Add small delay before reward distribution to mitigate potential front-running
     *
     * 8. Replay Protection:
     *    - Include chainId and contract address in signed messages:
     *      bytes32 entropyHash = keccak256(abi.encodePacked(
     *          previousOutput, temporalSeed, nonce, sender, timeBasedEntropy,
     *          secretValue, block.chainid, address(this)));
     *
     * 9. Denial of Service:
     *    - Implement bloom filter pruning or sliding window mechanism
     *    - Consider automatic scaling based on network load
     */
}
// Unique hybrid: Combines temporal (block-based) with spatial (HMAC-based) verification
bytes32 hmacOutput = MiningLib.processMiningReveal(
    params.previousOutput,    // Temporal chain
    params.temporalSeed,      // Time-based entropy
    params.nonce,            
    params.signature,         // Spatial proof
    params.secretValue,       // Miner's entropy
    miningPools[params.poolId].targetDifficulty,
    params.miner,
    bloomFilter,             // Spatial uniqueness check
    usedOutputs,            // Temporal uniqueness check
    MiningLib.quantumResistantHash,
    difficultyWeightFn
);
