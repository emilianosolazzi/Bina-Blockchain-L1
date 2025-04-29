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
            p.miner
        ));
        if (expected != c.commitHash) revert InvalidCommitment();
    }

    function processMiningReveal(
        bytes32 previousOutput,
        bytes memory temporalSeed,
        uint64 nonce,
        bytes memory signature,
        bytes32 secretValue,
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
        if (input.length == 0) revert MalformedInput(SEVERITY_HIGH);
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
}
