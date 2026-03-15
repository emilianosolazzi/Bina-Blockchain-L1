// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";

/**
 * @title RandomnessLib
 * @notice Library for secure randomness generation in Arbitrum L2 environment
 * @dev Optimized for Arbitrum's gas model and block execution environment
 */
library RandomnessLib {
    using EntropyUtils for bytes32;

    // Constants tuned for Arbitrum's transaction throughput and confirmation times
    uint256 internal constant MAX_BATCH_SIZE = 50; // Default batch size limit on Arbitrum (gas-optimized)
    uint256 internal constant DEFAULT_MIN_CONTRIBUTIONS = 3; // Default minimum contributions
    uint256 internal constant DEFAULT_MAX_CONTRIBUTIONS = 10; // Default maximum
    uint256 internal constant DEFAULT_EXPIRY_BLOCKS = 50000; // ~1 week on Arbitrum (15s block time)
    
    // Arbitrum-specific gas optimizations
    uint256 internal constant CACHE_LINE_SIZE = 32; // Optimize for 32-byte reads/writes
    uint256 internal constant ARBITRUM_CALLDATA_COMPRESSION_THRESHOLD = 112; // Bytes before Arbitrum's brotli compression helps

    // Errors optimized for Arbitrum's custom error encoding
    error InvalidTGBTAddress();
    error TGBTTransferFailed();
    error InvalidRequestID();
    error RequestFulfilled();
    error RequestExpired();
    error RequestNotExpired();
    error AlreadyContributed();
    error MaxContributionsReached();
    error RequestDoesNotExist();
    error RequestNotFulfilled();
    error InvalidRequest();
    error BatchTooLarge();
    error ArrayLengthMismatch();
    error InvalidSigner();
    error InvalidBatchSize();
    error InvalidParameters();
    error ContributionLimitExceeded();

    // Structs for storage optimization on Arbitrum (packing for 32-byte slots)
    struct RandomnessRequest {
        address requester;
        uint64 timestamp;
        bool fulfilled;
        bytes32 result;
        bytes32 userSeed;
        // Maps are separate to optimize gas on Arbitrum
    }

    // Context tracking map of contributor to bool to optimize gas costs
    struct ContributionContext {
        mapping(address => bool) hasContributed;
        address[] contributors;
        bytes32[] contributions;
    }

    // State struct with fee parameters designed for Arbitrum's fee model
    struct State {
        address tgbtTokenAddress;
        uint256 baseEmergencyFee;
        uint256 feePerContributor;
        uint256 expiryBlocks;
        uint256 minContributions;
        uint256 maxContributions;
        uint256 maxBatchSize;
        uint256 nextRequestId;
        mapping(uint256 => RandomnessRequest) requests;
        mapping(uint256 => ContributionContext) contributions;
    }

    /* === Core Functions === */
    
    /**
     * @notice Creates a new randomness request
     * @dev Optimized for Arbitrum's sequencer and aggregator model
     * @param state Library state
     * @param requester User requesting randomness
     * @param userSeed Seed provided by the requester
     * @return requestId Unique identifier for the request
     */
    function createRequest(
        State storage state,
        address requester,
        bytes32 userSeed
    ) internal returns (uint256 requestId) {
        if (requester == address(0)) revert InvalidRequest();

        requestId = state.nextRequestId++;
        
        state.requests[requestId] = RandomnessRequest({
            requester: requester,
            timestamp: uint64(block.timestamp), // Use uint64 for gas optimization on Arbitrum
            fulfilled: false,
            result: bytes32(0),
            userSeed: userSeed
        });
        
        return requestId;
    }

    /**
     * @notice Adds entropy contribution to a pending request
     * @param state Library state
     * @param requestId Request to contribute to
     * @param contributor Address providing entropy
     * @param entropyContribution Contributor's entropy value
     * @return shouldFulfill Whether the request has enough contributions to fulfill
     */
    function addContribution(
        State storage state,
        uint256 requestId,
        address contributor,
        bytes32 entropyContribution
    ) internal returns (bool shouldFulfill) {
        RandomnessRequest storage request = state.requests[requestId];
        
        if (request.requester == address(0)) revert RequestDoesNotExist();
        if (request.fulfilled) revert RequestFulfilled();
        if (block.timestamp > request.timestamp + state.expiryBlocks) revert RequestExpired();
        
        ContributionContext storage context = state.contributions[requestId];
        
        if (context.hasContributed[contributor]) revert AlreadyContributed();
        if (context.contributors.length >= state.maxContributions) revert MaxContributionsReached();
        
        context.hasContributed[contributor] = true;
        context.contributors.push(contributor);
        context.contributions.push(entropyContribution);
        
        return context.contributors.length >= state.minContributions;
    }

    /**
     * @notice Fulfills a randomness request once enough contributions are received
     * @param state Library state
     * @param requestId Request to fulfill
     * @param historicalHash Additional entropy from beacon history
     * @param entropyAccumulator Optional pre-accumulated entropy
     * @return result The generated randomness
     */
    function fulfillRequest(
        State storage state,
        uint256 requestId,
        bytes32 historicalHash,
        bytes32 entropyAccumulator
    ) internal returns (bytes32 result) {
        RandomnessRequest storage request = state.requests[requestId];
        
        if (request.requester == address(0)) revert RequestDoesNotExist();
        if (request.fulfilled) revert RequestFulfilled();
        
        ContributionContext storage context = state.contributions[requestId];
        if (context.contributors.length < state.minContributions) revert InvalidRequest();
        
        // Combine entropy sources with optimized gas operations for Arbitrum
        bytes32 finalEntropy = combineEntropy(
            request.userSeed,
            historicalHash,
            entropyAccumulator,
            context.contributions,
            bytes32(block.prevrandao), // Use prevrandao on Arbitrum
            block.number,
            block.timestamp
        );
        
        // Set as fulfilled
        request.fulfilled = true;
        request.result = finalEntropy;
        
        return finalEntropy;
    }

    /**
     * @notice Emergency fulfillment for randomness with fee payment
     * @dev Specially optimized for Arbitrum's fee structure
     */
    function emergencyFulfill(
        State storage state,
        uint256 requestId,
        bytes32 historicalHash,
        bytes32 entropyAccumulator,
        bytes32,
        address receiver,
        IERC20 tgbtToken,
        address feePayingAccount
    ) internal returns (bytes32 result) {
        RandomnessRequest storage request = state.requests[requestId];
        if (request.requester == address(0)) revert RequestDoesNotExist();
        if (request.fulfilled) revert RequestFulfilled();
        if (block.timestamp <= request.timestamp + state.expiryBlocks) revert RequestNotExpired();

        // Logic to calculate and collect fees based on Arbitrum's fee model
        uint256 contributorCount = state.contributions[requestId].contributors.length;
        uint256 feeAmount = state.baseEmergencyFee + (state.feePerContributor * contributorCount);
        
        // Verify TGBT token address
        if (address(tgbtToken) != state.tgbtTokenAddress) revert InvalidTGBTAddress();
        
        // Transfer fee (optimized for Arbitrum's ERC20 handling)
        bool feeTransferSuccess = tgbtToken.transferFrom(
            feePayingAccount, 
            receiver,
            feeAmount
        );
        if (!feeTransferSuccess) revert TGBTTransferFailed();
        
        // Generate and return result
        return fulfillRequest(
            state,
            requestId,
            historicalHash,
            entropyAccumulator
        );
    }

    /* === View Functions === */

    /**
     * @notice Gets the state of a randomness request
     */
    function getRequestState(
        State storage state,
        uint256 requestId
    ) internal view returns (
        address requester,
        uint256 timestamp,
        bool fulfilled,
        uint256 contributionsCount
    ) {
        RandomnessRequest storage request = state.requests[requestId];
        ContributionContext storage context = state.contributions[requestId];
        
        return (
            request.requester,
            request.timestamp,
            request.fulfilled,
            context.contributors.length
        );
    }

    /**
     * @notice Gets a richer summary of a randomness request for wallet/app UX.
     */
    function getRequestReceipt(
        State storage state,
        uint256 requestId
    )
        internal
        view
        returns (
            address requester,
            uint256 timestamp,
            bool fulfilled,
            bytes32 userSeed,
            bytes32 result,
            uint256 contributionsCount,
            uint256 minContributions,
            uint256 maxContributions,
            uint256 emergencyFeeQuote
        )
    {
        RandomnessRequest storage request = state.requests[requestId];
        if (request.requester == address(0)) revert RequestDoesNotExist();

        ContributionContext storage context = state.contributions[requestId];
        uint256 contributions = context.contributors.length;

        return (
            request.requester,
            request.timestamp,
            request.fulfilled,
            request.userSeed,
            request.result,
            contributions,
            state.minContributions,
            state.maxContributions,
            state.baseEmergencyFee + (state.feePerContributor * contributions)
        );
    }

    /**
     * @notice Returns contribution-level proof inputs for off-chain verification.
     */
    function getContributionDetails(
        State storage state,
        uint256 requestId
    ) internal view returns (address[] memory contributors, bytes32[] memory contributions) {
        RandomnessRequest storage request = state.requests[requestId];
        if (request.requester == address(0)) revert RequestDoesNotExist();

        ContributionContext storage context = state.contributions[requestId];
        uint256 count = context.contributors.length;

        contributors = new address[](count);
        contributions = new bytes32[](count);

        for (uint256 i = 0; i < count; i++) {
            contributors[i] = context.contributors[i];
            contributions[i] = context.contributions[i];
        }
    }

    /**
     * @notice Gets the result of a fulfilled randomness request
     */
    function getRandomness(
        State storage state,
        uint256 requestId
    ) internal view returns (bytes32) {
        RandomnessRequest storage request = state.requests[requestId];
        
        if (request.requester == address(0)) revert RequestDoesNotExist();
        if (!request.fulfilled) revert RequestNotFulfilled();
        
        return request.result;
    }

    /* === Utility Functions === */

    /**
     * @notice Combines entropy from multiple sources using Arbitrum-optimized hashing
     * @dev Special implementation optimized for Arbitrum's gas metering
     */
    function combineEntropy(
        bytes32 userSeed,
        bytes32 historicalHash,
        bytes32 accumulator,
        bytes32[] storage contributions,
        bytes32 randaoValue,
        uint256 blockNumber,
        uint256 blockTimestamp
    ) private view returns (bytes32) {
        bytes memory packed = abi.encodePacked(
            userSeed,
            historicalHash,
            accumulator,
            randaoValue,
            blockNumber,
            blockTimestamp,
            contributions.length
        );

        for (uint256 i = 0; i < contributions.length; i++) {
            packed = abi.encodePacked(packed, contributions[i]);
        }

        return keccak256(packed);
    }

    /**
     * @dev Returns the maximum historical block range that can be used in Arbitrum
     * for secure randomness generation, accounting for Arbitrum's sequencer domain.
     */
        function getArbitrumSafeHistoricalBlocks() internal pure returns (uint256) {
        return 6500; // ~1 day of Arbitrum blocks with 15s average block time
    }
}

/**
 * @dev Utility library for entropy operations
 */
library EntropyUtils {
    /**
     * @dev Mixes two entropy sources efficiently for Arbitrum
     */
    function mix(bytes32 a, bytes32 b) internal pure returns (bytes32) {
        return keccak256(abi.encodePacked(a, b));
    }
    
    /**
     * @dev Derives multiple random values from a single seed
     */
    function derive(bytes32 seed, uint256 count) internal pure returns (bytes32[] memory) {
        bytes32[] memory results = new bytes32[](count);
        
        for (uint256 i = 0; i < count; i++) {
            results[i] = keccak256(abi.encodePacked(seed, i));
        }
        
        return results;
    }
}
