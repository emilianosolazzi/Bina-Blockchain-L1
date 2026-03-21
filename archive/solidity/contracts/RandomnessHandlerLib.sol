// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { RandomnessLib } from "./RandomnessLib.sol";
import { ECDSAUpgradeable } from "@openzeppelin/contracts-upgradeable/utils/cryptography/ECDSAUpgradeable.sol";

/**
 * @title RandomnessHandlerLib
 * @notice Library for randomness-related functionality
 */
library RandomnessHandlerLib {
    using ECDSAUpgradeable for bytes32;
    
    // Events
    event EntropyContributed(uint256 indexed requestId, address contributor, bytes32 contribution);
    event RandomnessRequested(uint256 indexed requestId, address indexed requester, bytes32 userSeed);
    event RandomnessFulfilled(uint256 indexed requestId, bytes32 result);
    event MerkleRootUpdated(bytes32 newRoot);
    
    // Errors
    error InvalidSigner();
    error FeeNotSet();
    
    struct RandomnessContext {
        uint256 entropyAccumulator;
        bytes32 entropyMerkleRoot;
        RandomnessLib.State state;
    }
    
    /**
     * @notice Updates the entropy accumulator and merkle root
     * @dev Now compatible with RandomnessLib structure, processes pending requests
     * @param self The randomness context
     * @param historicalHash Historical entropy for additional randomness
     */
    function processRandomnessAndUpdateMerkle(
        RandomnessContext storage self, 
        bytes32 historicalHash
    ) internal {
        // Process up to 3 pending requests instead of using non-existent processBatch
        uint256 startId = self.state.nextRequestId > 3 ? self.state.nextRequestId - 3 : 0;
        uint256 endId = self.state.nextRequestId;
        uint256 processedCount = 0;
        
        // Try to fulfill any pending requests that have enough contributions
        for (uint256 i = startId; i < endId && processedCount < 3; i++) {
            // Skip if request doesn't exist or is already fulfilled
            (address requester, , bool fulfilled, uint256 contributions) = 
                RandomnessLib.getRequestState(self.state, i);
                
            if (requester != address(0) && !fulfilled && 
                contributions >= self.state.minContributions) {
                // Attempt to fulfill the request
                try RandomnessLib.fulfillRequest(
                    self.state,
                    i,
                    historicalHash,
                    bytes32(self.entropyAccumulator)
                ) returns (bytes32 result) {
                    emit RandomnessFulfilled(i, result);
                    processedCount++;
                } catch {
                    // Silently continue if fulfillment fails
                }
            }
        }

        // Update merkle root with accumulated entropy
        bytes32 newRoot = keccak256(abi.encodePacked(
            self.entropyMerkleRoot, 
            self.entropyAccumulator, 
            historicalHash, 
            block.timestamp, 
            block.prevrandao
        ));
        
        self.entropyMerkleRoot = newRoot;
        emit MerkleRootUpdated(newRoot);
    }
    
    /**
     * @notice Contributes entropy to a randomness request
     * @param self The randomness context
     * @param requestId The ID of the request to contribute to
     * @param entropyContribution The entropy contribution
     * @param entropySignature Signature of the contributor
     * @param sender The message sender
     * @param historicalHash Additional entropy from historical outputs
     * @return fulfilled Whether the request was fulfilled
     * @return randomValue The random value if fulfilled, otherwise zero
     */
    function contributeEntropy(
        RandomnessContext storage self,
        uint256 requestId,
        bytes32 entropyContribution,
        bytes calldata entropySignature,
        address sender,
        bytes32 historicalHash
    ) internal returns (bool fulfilled, bytes32 randomValue) {
        bytes32 messageHash = keccak256(abi.encodePacked(requestId, entropyContribution, sender));
        address recovered = messageHash.toEthSignedMessageHash().recover(entropySignature);
        if (recovered != sender) revert InvalidSigner();

        // Call the function correctly using direct library call
        bool shouldFulfill = RandomnessLib.addContribution(
            self.state, 
            requestId, 
            sender, 
            entropyContribution
        );
        emit EntropyContributed(requestId, sender, entropyContribution);

        if (shouldFulfill) {
            randomValue = fulfillRandomness(self, requestId, historicalHash);
            fulfilled = true;
        } else {
            fulfilled = false;
            randomValue = bytes32(0);
        }
    }
    
    /**
     * @notice Fulfills a randomness request
     * @param self The randomness context
     * @param requestId The ID of the request to fulfill
     * @param historicalHash Additional entropy
     * @return randomValue The generated random value
     */
    function fulfillRandomness(
        RandomnessContext storage self,
        uint256 requestId,
        bytes32 historicalHash
    ) internal returns (bytes32 randomValue) {
        // Call the function correctly using direct library call
        randomValue = RandomnessLib.fulfillRequest(
            self.state, 
            requestId, 
            historicalHash, 
            bytes32(self.entropyAccumulator)
        );
        emit RandomnessFulfilled(requestId, randomValue);
        return randomValue;
    }
    
    /**
     * @notice Requests a new random value
     * @param self The randomness context
     * @param userSeed The user-provided seed
     * @param requester The address requesting randomness
     * @return requestId The ID of the created request
     */
    function requestRandomness(
        RandomnessContext storage self,
        bytes32 userSeed,
        address requester
    ) internal returns (uint256 requestId) {
        // Check if fees are configured - using baseEmergencyFee instead of non-existent 'fee'
        if (self.state.baseEmergencyFee == 0 && self.state.feePerContributor == 0) revert FeeNotSet();
        
        // Call the function correctly using direct library call
        requestId = RandomnessLib.createRequest(
            self.state,
            requester, 
            userSeed
        );
        emit RandomnessRequested(requestId, requester, userSeed);
        return requestId;
    }
}
