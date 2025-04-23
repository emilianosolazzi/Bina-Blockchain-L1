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
    
    function processRandomnessAndUpdateMerkle(
        RandomnessContext storage self, 
        bytes32 historicalHash
    ) internal {
        // Call the function correctly using direct library call
        RandomnessLib.processBatch(
            self.state,
            3, // Process a small batch each time
            historicalHash,
            self.entropyAccumulator
        );

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
            self.entropyAccumulator
        );
        emit RandomnessFulfilled(requestId, randomValue);
        return randomValue;
    }
    
    function requestRandomness(
        RandomnessContext storage self,
        bytes32 userSeed,
        address requester
    ) internal returns (uint256 requestId) {
        if (self.state.fee == 0) revert FeeNotSet();
        
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
