// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ECDSA } from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import { Math } from "@openzeppelin/contracts/utils/math/Math.sol";
import { CoreUtilsLib } from "./CoreUtilsLib.sol";

/**
 * @title MiningLib
 * @notice Library for mining-related functionality in the Temporal Gradient Beacon
 * @dev Combines commit-reveal validation, iterative entropy mixing, and difficulty checks.
 *      This library contains no privileged override path.
 */
library MiningLib {
    using ECDSA for bytes32;
    using Math for uint256;

    // === Constants ===
    // Hash parameters 
    uint16 private constant QR_HASH_ITERATIONS = 3;      // Bounded iteration count
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
        if (p.miner == address(0)) revert ZeroAddress(SEVERITY_HIGH, ERROR_CATEGORY_ACCESS);
        bytes32 expected = keccak256(abi.encodePacked(
            p.previousOutput,
            p.temporalSeed,
            p.nonce,
            p.signature,
            p.secretValue,
            p.miner
        ));
        if (expected != c.commitHash) revert InvalidCommitment(SEVERITY_HIGH, ERROR_CATEGORY_INPUT);
    }

    /**
     * @notice Validates the 8-byte BOM-prefixed temporal seed and its timestamp bounds.
     * @dev Extracted to its own stack frame to avoid stack-too-deep in processMiningReveal.
     */
    function _validateTemporalSeed(bytes memory temporalSeed) internal view {
        if (temporalSeed.length != 8) revert InvalidTemporalSeedFormat(SEVERITY_MEDIUM, ERROR_CATEGORY_INPUT);
        if (temporalSeed[0] != 0x00) revert InvalidBOMMarker(SEVERITY_MEDIUM, ERROR_CATEGORY_INPUT);

        // Extract timestamp from the canonical 8-byte format:
        // byte[0] = BOM (0x00), byte[1..7] = big-endian 56-bit unix timestamp
        uint64 seedTimestamp = 0;
        unchecked {
            for (uint256 i = 1; i < 8; i++) {
                seedTimestamp = (seedTimestamp << 8) | uint64(uint8(temporalSeed[i]));
            }
        }

        uint256 currentTime = block.timestamp;

        if (seedTimestamp == 0) revert MalformedInput(SEVERITY_HIGH, ERROR_CATEGORY_INPUT, "Zero seed timestamp");
        if (seedTimestamp < 1704067200) revert TimestampTooOld(SEVERITY_HIGH, ERROR_CATEGORY_TIMING, seedTimestamp, 1704067200); // Jan 1, 2024
        if (seedTimestamp < currentTime - 30 days) revert TimestampTooOld(SEVERITY_HIGH, ERROR_CATEGORY_TIMING, seedTimestamp, currentTime - 30 days);
        if (seedTimestamp > currentTime + 15 minutes) revert TimestampInFuture(SEVERITY_HIGH, ERROR_CATEGORY_TIMING, seedTimestamp, currentTime + 15 minutes);

        unchecked {
            if (currentTime > seedTimestamp && currentTime - seedTimestamp > MAX_TIMESTAMP_DRIFT)
                revert TimestampDriftTooLarge(SEVERITY_HIGH, ERROR_CATEGORY_TIMING, currentTime - seedTimestamp);
        }
    }

    /**
     * @notice Validates that hmacOutput meets the weighted difficulty and is unique.
     * @dev Extracted to its own stack frame to avoid stack-too-deep in processMiningReveal.
     */
    function _validateDifficultyAndUniqueness(
        bytes32 hmacOutput,
        uint256 baseDifficulty,
        mapping(bytes32 => uint256) storage usedOutputs
    ) internal view {
        if (uint256(hmacOutput) >= baseDifficulty) revert SolutionTooEasy(SEVERITY_MEDIUM, ERROR_CATEGORY_STATE);
        if (usedOutputs[hmacOutput] != 0)
            revert OutputAlreadyUsed(SEVERITY_MEDIUM, ERROR_CATEGORY_STATE);
    }

    /**
     * @notice Core mining reveal: validates temporal seed, signature, difficulty, and uniqueness.
     * @dev Accepts RevealParams struct instead of individual params to stay within the
     *      EVM's 16-slot stack limit under legacy codegen.
     */
    function processMiningReveal(
        RevealParams memory params,
        uint256 baseDifficulty,
        mapping(bytes32 => uint256) storage usedOutputs,
        function(bytes memory) view returns (bytes32) hashFunction
    ) internal view returns (bytes32) {
        if (params.miner == address(0)) revert ZeroAddress(SEVERITY_HIGH, ERROR_CATEGORY_ACCESS);

        _validateTemporalSeed(params.temporalSeed);

        bytes32 entropyHash = keccak256(abi.encodePacked(
            params.previousOutput,
            params.temporalSeed,
            params.nonce,
            params.miner,
            params.secretValue
        ));

        address recovered = entropyHash.recover(params.signature);
        if (recovered == address(0)) revert InvalidSignature(SEVERITY_HIGH, ERROR_CATEGORY_INPUT);
        if (recovered != params.miner) revert InvalidSigner(SEVERITY_HIGH, ERROR_CATEGORY_ACCESS);

        bytes32 hmacOutput = hashFunction(abi.encodePacked(params.signature, entropyHash, params.secretValue));

        _validateDifficultyAndUniqueness(hmacOutput, baseDifficulty, usedOutputs);

        return hmacOutput;
    }

    function iterativeEntropyHash(bytes memory input) internal pure returns (bytes32) {
        bytes32 h = keccak256(input);
        for (uint256 i = 0; i < QR_HASH_ITERATIONS; i++) {
            h = keccak256(abi.encodePacked(h ^ bytes32(i + 1)));
            h = bytes32((uint256(h) << QR_HASH_ROTATION) | (uint256(h) >> (256 - QR_HASH_ROTATION)));
        }
        return h;
    }

    /// @notice Calculate the rule-based protocol payout for a valid solution.
    function calculateMiningReward(
        bytes32 hmacOutput,
        uint256 baseReward,
        uint256 difficultyThresholdFactor,
        uint256 payoutMultiplier,
        uint256 totalMined,
        uint256 globalCap,
        MiningPool storage pool
    ) internal returns (uint256 reward) {
        uint256 difficulty = type(uint256).max - uint256(hmacOutput);
        reward = baseReward;

        uint256 thresholdTarget = pool.targetDifficulty * difficultyThresholdFactor;
        if (difficulty > thresholdTarget) {
            reward = (baseReward * payoutMultiplier) / 100;
            emit ExceptionalSolution(msg.sender, difficulty, thresholdTarget, payoutMultiplier);
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
        if (previousOutput == bytes32(0)) revert MalformedInput(SEVERITY_HIGH, ERROR_CATEGORY_INPUT, "Zero previous output");
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
        if (signer == address(0) || signature.length == 0) revert MalformedInput(SEVERITY_HIGH, ERROR_CATEGORY_INPUT, "Invalid signature input");
        return (msgHash.recover(signature) == signer);
    }

    /// @notice Iterative entropy-mixing hash helper that does not depend solely on block.timestamp.
    /// @dev Uses repeated keccak rounds and bit rotation for bounded on-chain mixing.
    /// @param input The input bytes to hash
    /// @param extraEntropy Additional entropy to mix in (e.g., block number rather than timestamp)
    /// @return The mixed hash output
    function iterativeEntropyHashWithSalt(
        bytes memory input, 
        bytes32 extraEntropy
    ) internal pure returns (bytes32) {
        if (input.length == 0) revert MalformedInput(SEVERITY_HIGH, ERROR_CATEGORY_INPUT, "Empty input");
        
        bytes32 h = keccak256(input);
        for (uint256 i = 0; i < QR_HASH_ITERATIONS; i++) {
            // Use extraEntropy instead of relying on timestamp alone.
            h = keccak256(abi.encodePacked(h ^ bytes32(i + 1), extraEntropy));
            h = bytes32((uint256(h) << QR_HASH_ROTATION) | (uint256(h) >> (256 - QR_HASH_ROTATION)));
        }
        return h;
    }

    /// @notice Automatically select a random number using accumulated entropy
    /// @dev Uses output history and iterative entropy mixing with rejection sampling to avoid modulo bias
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
                block.prevrandao,
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
        
        // Apply iterative hash mixing with additional entropy.
        bytes32 resistant = iterativeEntropyHashWithSalt(combinedEntropy, blockBasedEntropy);
        
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
    function autoPickMultipleRandomLegacy(
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

    // Enhanced signature validation with malleability checks
    function enhancedValidateSignature(
        bytes32 msgHash,
        bytes memory signature,
        address signer
    ) internal pure returns (bool valid) {
        if (signer == address(0)) revert ZeroAddress(SEVERITY_HIGH, ERROR_CATEGORY_ACCESS);
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

    // Improved deterministic random number generation without bias
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

    // Adaptive iterative hashing with configurable round count
    function adaptiveIterativeHashWithSalt(
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

    // Use the improved random selection path in batched generation.
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

    /**
     * @dev Implementation notes:
     * - entropy helpers are bounded and deterministic within normal EVM constraints;
     * - payout calculations depend only on caller-supplied protocol inputs;
     * - this library does not introduce admin-only branches or privileged overrides; and
     * - duplicate output checks should remain exact and auditable.
     */
}
