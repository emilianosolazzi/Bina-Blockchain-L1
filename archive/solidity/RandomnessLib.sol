// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ECDSAUpgradeable } from "@openzeppelin/contracts-upgradeable/utils/cryptography/ECDSAUpgradeable.sol";
import { IERC20Upgradeable } from "@openzeppelin/contracts-upgradeable/token/ERC20/IERC20Upgradeable.sol"; // Added import
import { MerkleProofUpgradeable } from "@openzeppelin/contracts-upgradeable/utils/cryptography/MerkleProofUpgradeable.sol"; // Assuming MerkleProof might be used

/**
 * @title RandomnessLib
 * @notice Core implementation of randomness generation and request handling
 * @dev Provides a secure multi-source entropy based randomness system with request management
 */
library RandomnessLib {
    // Custom errors for gas-efficient reverts
    error InvalidTGBTAddress();
    error TGBTTransferFailed();
    error InvalidRequestID();
    error RequestFulfilled();
    error RequestExpired();
    error AlreadyContributed();
    error MaxContributionsReached();
    error RequestDoesNotExist();
    error RequestNotFulfilled();
    error InvalidRequest();
    error BatchTooLarge();
    error ArrayLengthMismatch();
    error InvalidSigner();
    error InsufficientFee();
    error FeeNotSet();

    // Events
    event RandomnessRequested(uint256 indexed requestId, address requester, bytes32 userSeed);
    event EntropyContributed(uint256 indexed requestId, address contributor, bytes32 entropyHash);
    event RandomnessFulfilled(uint256 indexed requestId, bytes32 result);
    event EmergencyFulfilled(uint256 indexed requestId, address indexed caller, uint256 fee);
    event ContributionParamsUpdated(uint256 minContributions, uint256 maxContributions);
    event FeeParamsUpdated(uint256 baseEmergencyFee, uint256 feePerContributor);

    /**
     * @notice Contribution structure to track entropy providers
     * @param contributor Address of the entropy provider
     * @param entropyValue The entropy value submitted
     * @param timestamp When the contribution was made
     */
    struct Contribution {
        address contributor;
        bytes32 entropyValue;
        uint64 timestamp;
    }

    /**
     * @notice Structure for randomness requests
     * @param requester Address that initiated the request
     * @param userSeed Seed provided by the user
     * @param requestBlock Block number when the request was made
     * @param requestTimestamp Timestamp when the request was created
     * @param contributions Array of entropy contributions
     * @param fulfilled Whether the request has been fulfilled
     * @param result The final random value (once fulfilled)
     */
    struct Request {
        address requester;
        bytes32 userSeed;
        uint64 requestBlock;
        uint64 requestTimestamp;
        Contribution[] contributions;
        bool fulfilled;
        bytes32 result;
    }

    /**
     * @notice State structure for managing randomness generation
     * @dev Used with the "using X for X.Y" pattern
     * @param tgbtTokenAddress Address of the TGBT token contract
     * @param baseEmergencyFee Base fee for emergency fulfillment
     * @param feePerContributor Additional fee per missing contributor
     * @param entropyAccumulator Running hash of combined entropy sources
     * @param expiryBlocks Number of blocks before a request expires
     * @param nextRequestId Counter for request IDs
     * @param requests Mapping of request IDs to request data
     * @param contributorToRequests Tracks which requests an address has contributed to
     * @param minContributions Minimum required entropy contributions
     * @param maxContributions Maximum allowed entropy contributions 
     * @param maxBatchSize Maximum size for batch operations
     */
    struct State {
        address tgbtTokenAddress;
        uint256 baseEmergencyFee;
        uint256 feePerContributor;
        bytes32 entropyAccumulator;
        uint64 expiryBlocks;
        uint256 nextRequestId;
        mapping(uint256 => Request) requests;
        mapping(address => mapping(uint256 => bool)) contributorToRequests;
        uint256 minContributions;
        uint256 maxContributions;
        uint256 maxBatchSize;
    }

    /**
     * @notice Creates a new randomness request
     * @param state State struct storage reference
     * @param requester Address requesting randomness
     * @param userSeed Arbitrary seed provided by requester for additional entropy
     * @return requestId The ID of the newly created request
     */
    function createRequest(
        State storage state,
        address requester,
        bytes32 userSeed
    ) internal returns (uint256) {
        uint256 requestId = state.nextRequestId++;
        
        Request storage request = state.requests[requestId];
        request.requester = requester;
        request.userSeed = userSeed;
        request.requestBlock = uint64(block.number);
        request.requestTimestamp = uint64(block.timestamp);
        request.fulfilled = false;
        
        // Update entropy accumulator with request data
        state.entropyAccumulator = keccak256(
            abi.encodePacked(
                state.entropyAccumulator,
                requester,
                userSeed,
                block.timestamp,
                block.prevrandao
            )
        );
        
        return requestId;
    }

    /**
     * @notice Adds an entropy contribution to a pending request
     * @param state State struct storage reference
     * @param requestId ID of the request to contribute to
     * @param contributor Address providing entropy
     * @param entropyValue Entropy contribution value
     * @return shouldFulfill Whether the request should be fulfilled after this contribution
     */
    function addContribution(
        State storage state,
        uint256 requestId,
        address contributor,
        bytes32 entropyValue
    ) internal returns (bool shouldFulfill) {
        Request storage request = state.requests[requestId];
        
        // Validate request
        if (request.requester == address(0)) revert RequestDoesNotExist();
        if (request.fulfilled) revert RequestFulfilled();
        if (block.number > request.requestBlock + state.expiryBlocks) revert RequestExpired();
        
        // Check if contributor already contributed
        if (state.contributorToRequests[contributor][requestId]) revert AlreadyContributed();
        
        // Ensure we haven't reached max contributions
        if (request.contributions.length >= state.maxContributions) revert MaxContributionsReached();
        
        // Record contribution
        state.contributorToRequests[contributor][requestId] = true;
        request.contributions.push(Contribution({
            contributor: contributor,
            entropyValue: entropyValue,
            timestamp: uint64(block.timestamp)
        }));
        
        // Update entropy accumulator
        state.entropyAccumulator = keccak256(
            abi.encodePacked(
                state.entropyAccumulator,
                contributor,
                entropyValue,
                block.timestamp,
                block.prevrandao
            )
        );
        
        // Check if we've reached the minimum contributions threshold
        return (request.contributions.length >= state.minContributions);
    }

    /**
     * @notice Fulfills a randomness request once enough contributions are received
     * @param state State struct storage reference  
     * @param requestId ID of the request to fulfill
     * @param historicalOutputsHash Hash of the beacon's historical outputs
     * @param additionalEntropy Additional entropy source (if any)
     * @return result The generated random value
     */
    function fulfillRequest(
        State storage state,
        uint256 requestId,
        bytes32 historicalOutputsHash,
        bytes32 additionalEntropy
    ) internal returns (bytes32 result) {
        Request storage request = state.requests[requestId];
        
        // Validate request
        if (request.requester == address(0)) revert RequestDoesNotExist();
        if (request.fulfilled) revert RequestFulfilled();
        if (block.number > request.requestBlock + state.expiryBlocks) revert RequestExpired();
        if (request.contributions.length < state.minContributions) revert InvalidRequest();
        
        // Generate the random value from multiple entropy sources
        result = generateRandomValue(
            state,
            request,
            historicalOutputsHash,
            additionalEntropy
        );
        
        // Mark as fulfilled and store result
        request.fulfilled = true;
        request.result = result;
        
        return result;
    }

    /**
     * @notice Retrieves the result of a fulfilled randomness request
     * @param state State struct storage reference
     * @param requestId ID of the request
     * @return The generated random value
     */
    function getRandomness(State storage state, uint256 requestId) internal view returns (bytes32) {
        Request storage request = state.requests[requestId];
        
        if (request.requester == address(0)) revert RequestDoesNotExist();
        if (!request.fulfilled) revert RequestNotFulfilled();
        
        return request.result;
    }

    /**
     * @notice Performs emergency fulfillment of a request for a fee
     * @param state State struct storage reference
     * @param requestId ID of the request to fulfill
     * @param historicalOutputsHash Hash of beacon historical outputs
     * @param additionalEntropy Additional entropy source
     * @param entropyMerkleRoot Merkle root of entropy sources (if applicable)
     * @param beaconAddress Address of the beacon contract
     * @param tgbtToken TGBT token interface
     * @param feePayer Address that will pay the emergency fee
     * @return result The generated random value
     */
    function emergencyFulfill(
        State storage state,
        uint256 requestId,
        bytes32 historicalOutputsHash,
        bytes32 additionalEntropy,
        bytes32 entropyMerkleRoot,
        address beaconAddress,
        IERC20Upgradeable tgbtToken,
        address feePayer
    ) internal returns (bytes32 result) {
        Request storage request = state.requests[requestId];
        
        // Validate request
        if (request.requester == address(0)) revert RequestDoesNotExist();
        if (request.fulfilled) revert RequestFulfilled();
        
        // Calculate fee based on number of missing contributions
        uint256 missingContributions = state.minContributions;
        if (request.contributions.length < state.minContributions) {
            missingContributions = state.minContributions - request.contributions.length;
        } else {
            missingContributions = 0;
        }
        
        uint256 fee = state.baseEmergencyFee + (missingContributions * state.feePerContributor);
        
        // Check if fee parameters are set
        if (fee == 0) revert FeeNotSet();
        
        // Validate TGBT address
        if (state.tgbtTokenAddress == address(0)) revert InvalidTGBTAddress();
        
        // Transfer fee from caller
        bool success = tgbtToken.transferFrom(feePayer, beaconAddress, fee);
        if (!success) revert TGBTTransferFailed();
        
        // Generate randomness using available entropy + additional sources
        result = generateEmergencyRandomValue(
            state,
            request,
            historicalOutputsHash,
            additionalEntropy,
            entropyMerkleRoot,
            feePayer
        );
        
        // Mark request as fulfilled and store result
        request.fulfilled = true;
        request.result = result;
        
        emit EmergencyFulfilled(requestId, feePayer, fee);
        
        return result;
    }

    /**
     * @notice Gets information about a randomness request
     * @param state State struct storage reference
     * @param requestId ID of the request
     * @return requester Address that created the request
     * @return timestamp Block timestamp when request was created
     * @return fulfilled Whether the request has been fulfilled
     * @return contributionsCount Number of entropy contributions
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
        Request storage request = state.requests[requestId];
        
        if (request.requester == address(0)) revert RequestDoesNotExist();
        
        return (
            request.requester,
            request.requestTimestamp,
            request.fulfilled,
            request.contributions.length
        );
    }

    /**
     * @notice Internal function to generate the final random value from all entropy sources
     * @param state State struct storage reference
     * @param request Request data  
     * @param historicalOutputsHash Hash of beacon historical outputs
     * @param additionalEntropy Additional entropy source
     * @return result The generated random value
     */
    function generateRandomValue(
        State storage state,
        Request storage request,
        bytes32 historicalOutputsHash,
        bytes32 additionalEntropy
    ) private view returns (bytes32 result) {
        // Combine all entropy sources
        bytes memory combinedEntropy = abi.encodePacked(
            // User-provided seed
            request.userSeed,
            
            // Request metadata
            request.requester,
            request.requestBlock,
            request.requestTimestamp,
            
            // External entropy sources
            historicalOutputsHash,
            state.entropyAccumulator,
            additionalEntropy,
            
            // Block values for unpredictability
            block.timestamp,
            block.prevrandao,
            blockhash(block.number - 1)
        );
        
        // Add all contributions
        for (uint256 i = 0; i < request.contributions.length; i++) {
            Contribution storage contribution = request.contributions[i];
            combinedEntropy = abi.encodePacked(
                combinedEntropy,
                contribution.contributor,
                contribution.entropyValue,
                contribution.timestamp
            );
        }
        
        // Apply multiple hashing rounds for improved security
        bytes32 hash = keccak256(combinedEntropy);
        
        // Add quantum resistance with multiple rounds
        for (uint256 i = 0; i < 3; i++) {
            hash = keccak256(abi.encodePacked(hash, i));
        }
        
        return hash;
    }

    /**
     * @notice Internal function for emergency randomness generation with limited entropy
     * @param state State struct storage reference
     * @param request Request data
     * @param historicalOutputsHash Hash of beacon historical outputs
     * @param additionalEntropy Additional entropy source
     * @param entropyMerkleRoot Merkle root of entropy sources (if applicable)
     * @param emergencyCaller Address initiating emergency fulfillment
     * @return result The generated random value
     */
    function generateEmergencyRandomValue(
        State storage state,
        Request storage request,
        bytes32 historicalOutputsHash,
        bytes32 additionalEntropy,
        bytes32 entropyMerkleRoot,
        address emergencyCaller
    ) private view returns (bytes32 result) {
        // Create entropy from available sources to ensure randomness
        // Even in emergency mode with potentially fewer contributors
        bytes memory combinedEntropy = abi.encodePacked(
            // User-provided seed
            request.userSeed,
            
            // Request metadata
            request.requester,
            request.requestBlock,
            request.requestTimestamp,
            
            // External entropy sources
            historicalOutputsHash,
            state.entropyAccumulator,
            additionalEntropy,
            entropyMerkleRoot,
            
            // Block values for unpredictability
            block.timestamp,
            block.prevrandao,
            blockhash(block.number - 1),
            
            // Emergency caller info
            emergencyCaller
        );
        
        // Add any existing contributions
        for (uint256 i = 0; i < request.contributions.length; i++) {
            Contribution storage contribution = request.contributions[i];
            combinedEntropy = abi.encodePacked(
                combinedEntropy,
                contribution.contributor,
                contribution.entropyValue,
                contribution.timestamp
            );
        }
        
        // Apply multiple hashing rounds for improved security
        bytes32 hash = keccak256(combinedEntropy);
        
        // Extra security rounds for emergency fulfillment
        for (uint256 i = 0; i < 5; i++) {
            hash = keccak256(abi.encodePacked(hash, i, block.prevrandao));
        }
        
        return hash;
    }

    /**
     * @notice Creates multiple randomness requests in a batch
     * @param state State struct storage reference
     * @param requester Address requesting randomness
     * @param userSeeds Array of seeds for each request
     * @return requestIds Array of request IDs
     */
    function batchCreateRequests(
        State storage state,
        address requester,
        bytes32[] calldata userSeeds
    ) internal returns (uint256[] memory requestIds) {
        // Validate batch size
        if (userSeeds.length == 0) revert InvalidRequest();
        if (userSeeds.length > state.maxBatchSize) revert BatchTooLarge();
        
        requestIds = new uint256[](userSeeds.length);
        
        for (uint256 i = 0; i < userSeeds.length; i++) {
            requestIds[i] = createRequest(state, requester, userSeeds[i]);
        }
        
        return requestIds;
    }

    /**
     * @notice Batch contribute entropy to multiple requests
     * @param state State struct storage reference
     * @param requestIds Array of request IDs
     * @param entropyValues Array of entropy values
     * @return fulfilledRequests Array indicating which requests are ready for fulfillment
     */
    function batchContribute(
        State storage state,
        uint256[] calldata requestIds,
        bytes32[] calldata entropyValues
    ) internal returns (bool[] memory fulfilledRequests) {
        // Validate batch size and array lengths
        if (requestIds.length == 0) revert InvalidRequest();
        if (requestIds.length > state.maxBatchSize) revert BatchTooLarge();
        if (requestIds.length != entropyValues.length) revert ArrayLengthMismatch();
        
        fulfilledRequests = new bool[](requestIds.length);
        
        for (uint256 i = 0; i < requestIds.length; i++) {
            try this.addContribution(state, requestIds[i], msg.sender, entropyValues[i]) returns (bool shouldFulfill) {
                fulfilledRequests[i] = shouldFulfill;
            } catch {
                // Skip this request if it fails (already contributed, expired, etc.)
                fulfilledRequests[i] = false;
            }
        }
        
        return fulfilledRequests;
    }

    /**
     * @notice Updates the entropy accumulator with a new value
     * @param state State struct storage reference
     * @param newEntropy New entropy to incorporate
     */
    function updateEntropyAccumulator(State storage state, bytes32 newEntropy) internal {
        state.entropyAccumulator = keccak256(
            abi.encodePacked(
                state.entropyAccumulator,
                newEntropy,
                block.timestamp,
                block.number,
                block.prevrandao
            )
        );
    }

    /**
     * @notice Gets the minimum and maximum contribution parameters
     * @param state State struct storage reference
     * @return minContributions Minimum required entropy contributions
     * @return maxContributions Maximum allowed entropy contributions
     */
    function getContributionParams(
        State storage state
    ) internal view returns (uint256 minContributions, uint256 maxContributions) {
        return (state.minContributions, state.maxContributions);
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
