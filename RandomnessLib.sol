// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ECDSAUpgradeable } from "@openzeppelin/contracts-upgradeable/utils/cryptography/ECDSAUpgradeable.sol";
import { BytesArrayLib } from "./BytesArrayLib.sol";

/**
 * @title RandomnessLib
 * @notice Library for handling randomness-related functionality
 */
library RandomnessLib {
    using ECDSAUpgradeable for bytes32;
    using BytesArrayLib for bytes32[];
    
    struct Request {
        address requester;
        bytes32 userSeed;
        uint256 timestamp;
        bool fulfilled;
        bytes32 result;
        bytes32[] entropyContributions;
        uint256 contributionsCount;
    }
    
    struct State {
        uint256 fee;
        uint256 expiryBlocks;
        uint256 minContributions;
        uint256 maxContributions;
        uint256 maxBatchSize;
        uint256 requestCount;
        uint256 lastProcessedId;
        mapping(uint256 => Request) requests;
        mapping(uint256 => mapping(address => bool)) contributors;
        mapping(bytes32 => bool) usedValues;
    }
    
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
    error InvalidBatchSize();
    
    // Create a new randomness request
    function createRequest(
        State storage self,
        address requester,
        bytes32 userSeed
    ) internal returns (uint256 requestId) {
        requestId = self.requestCount++;
        
        bytes32[] memory contributions = BytesArrayLib.createBytes32Array(self.maxContributions);
        
        self.requests[requestId] = Request({
            requester: requester,
            userSeed: userSeed,
            timestamp: block.timestamp,
            fulfilled: false,
            result: bytes32(0),
            entropyContributions: contributions,
            contributionsCount: 0
        });
        
        return requestId;
    }
    
    // Add a contribution to a randomness request
    function addContribution(
        State storage self,
        uint256 requestId,
        address contributor,
        bytes32 entropyContribution
    ) internal returns (bool shouldFulfill) {
        if (requestId >= self.requestCount) revert InvalidRequestID();
        Request storage request = self.requests[requestId];
        
        if (request.fulfilled) revert RequestFulfilled();
        if (block.number > request.timestamp + self.expiryBlocks) revert RequestExpired();
        if (self.contributors[requestId][contributor]) revert AlreadyContributed();
        if (request.contributionsCount >= self.maxContributions) revert MaxContributionsReached();
        
        uint256 currentCount = request.contributionsCount;
        request.entropyContributions[currentCount] = entropyContribution;
        request.contributionsCount = currentCount + 1;
        self.contributors[requestId][contributor] = true;
        
        return (currentCount + 1 >= self.minContributions);
    }
    
    // Fulfill a randomness request
    function fulfillRequest(
        State storage self,
        uint256 requestId,
        bytes32 historicalOutputsHash,
        uint256 entropyAccumulator
    ) internal returns (bytes32 randomValue) {
        Request storage request = self.requests[requestId];
        
        if (requestId >= self.requestCount ||
            request.fulfilled ||
            request.requester == address(0) ||
            block.number > request.timestamp + self.expiryBlocks ||
            request.contributionsCount < self.minContributions) {
            revert InvalidRequest();
        }
        
        // Collect contributions
        uint256 contributionsCount = request.contributionsCount;
        bytes32[] memory contributions = BytesArrayLib.createBytes32Array(contributionsCount);
        
        for (uint i = 0; i < contributionsCount; i++) {
            contributions[i] = request.entropyContributions[i];
        }
        
        // Generate input for hash
        bytes memory randomnessInput = abi.encodePacked(
            historicalOutputsHash,
            request.userSeed,
            request.requester,
            request.timestamp,
            blockhash(block.number - 1),
            block.prevrandao,
            block.timestamp,
            entropyAccumulator,
            keccak256(abi.encodePacked(contributions))
        );
        
        // Generate random value
        randomValue = _generateRandomValue(self, randomnessInput);
        
        // Update request
        request.fulfilled = true;
        request.result = randomValue;
        
        return randomValue;
    }
    
    // Generate a random value and handle potential collisions
    function _generateRandomValue(
        State storage self,
        bytes memory input
    ) private returns (bytes32) {
        // Apply quantum resistant hashing
        bytes32 randomValue = keccak256(input);
        
        for (uint256 i = 1; i <= 3; i++) {
            randomValue = keccak256(abi.encodePacked(randomValue, i, input));
        }
        
        // Check for unlikely random value collisions
        if (self.usedValues[randomValue]) {
            // Add additional entropy and rehash in the rare case of collision
            randomValue = keccak256(abi.encodePacked(
                randomValue,
                block.prevrandao,
                gasleft()
            ));
        }
        
        // Mark the random value as used
        self.usedValues[randomValue] = true;
        
        return randomValue;
    }
    
    // Get a randomness result
    function getRandomness(
        State storage self, 
        uint256 requestId
    ) internal view returns (bytes32) {
        Request storage request = self.requests[requestId];
        if (request.requester == address(0)) revert RequestDoesNotExist();
        if (!request.fulfilled) revert RequestNotFulfilled();
        return request.result;
    }
    
    // Process pending randomness requests
    function processPendingRequests(
        State storage self,
        bytes32 historicalOutputsHash,
        uint256 entropyAccumulator
    ) internal {
        uint256 reqCount = self.requestCount;
        uint256 lastId = self.lastProcessedId;
        
        if (reqCount <= lastId) return;
        
        // Optimize toProcess calculation
        uint256 remaining = reqCount - lastId;
        uint256 toProcess = remaining > 3 ? 3 : remaining;
        
        // Process eligible requests
        for (uint256 i = 0; i < toProcess; ++i) {
            uint256 requestId = lastId + i;
            Request storage request = self.requests[requestId];
            
            if (!request.fulfilled && 
                request.requester != address(0) && 
                block.number <= request.timestamp + self.expiryBlocks &&
                request.contributionsCount >= self.minContributions) {
                
                fulfillRequest(self, requestId, historicalOutputsHash, entropyAccumulator);
            }
        }
        
        // Update processed ID
        self.lastProcessedId = lastId + toProcess;
    }
    
    // Check request status
    function checkRequestStatus(
        State storage self,
        uint256 requestId
    ) internal view returns (bool canFulfill, uint256 pendingContributions) {
        if (requestId >= self.requestCount) return (false, 0);
        
        Request storage request = self.requests[requestId];
        
        if (request.fulfilled) return (true, 0);
        if (request.requester == address(0)) return (false, 0);
        if (block.number > request.timestamp + self.expiryBlocks) return (false, 0);
        
        uint256 current = request.contributionsCount;
        if (current >= self.minContributions) return (true, 0);
        
        return (false, self.minContributions - current);
    }
    
    // Create a proof of randomness request
    function createProof(
        State storage self,
        uint256 requestId
    ) internal view returns (bytes memory proof) {
        if (requestId >= self.requestCount) revert InvalidRequestID();
        Request storage request = self.requests[requestId];
        
        // Encode the essential request data for verification
        return abi.encode(
            requestId,
            request.requester,
            request.userSeed,
            request.timestamp,
            request.fulfilled,
            request.result
        );
    }
    
    // Verify a request proof
    function verifyProof(
        State storage self,
        bytes calldata proof
    ) internal view returns (bool valid, uint256 requestId, address requester, bytes32 result) {
        (uint256 id, address req, bytes32 seed, uint256 timestamp, bool fulfilled, bytes32 res) = 
            abi.decode(proof, (uint256, address, bytes32, uint256, bool, bytes32));
            
        if (id >= self.requestCount) return (false, id, req, res);
        
        Request storage request = self.requests[id];
        
        bool isValid = request.requester == req &&
                      request.userSeed == seed &&
                      request.timestamp == timestamp &&
                      request.fulfilled == fulfilled &&
                      (fulfilled ? request.result == res : true);
                      
        return (isValid, id, req, res);
    }
    
    // Emergency fulfill a randomness request
    function emergencyFulfill(
        State storage self,
        uint256 requestId,
        bytes32 historicalOutputsHash,
        uint256 entropyAccumulator,
        bytes32 entropyMerkleRoot,
        address contractAddress
    ) internal returns (bytes32 randomValue) {
        if (requestId >= self.requestCount) revert InvalidRequestID();
        Request storage request = self.requests[requestId];
        
        if (request.fulfilled) revert RequestFulfilled();
        if (request.requester == address(0)) revert InvalidRequest();
        
        // Generate special emergency randomness using all available entropy
        bytes memory randomnessInput = abi.encodePacked(
            historicalOutputsHash,
            request.userSeed,
            request.requester,
            request.timestamp,
            blockhash(block.number - 1),
            block.prevrandao,
            block.timestamp,
            entropyAccumulator,
            entropyMerkleRoot,
            contractAddress
        );
        
        // Generate random value
        randomValue = _generateRandomValue(self, randomnessInput);
        
        // Update request
        request.fulfilled = true;
        request.result = randomValue;
        
        return randomValue;
    }
    
    // Process batch randomness requests
    function processBatch(
        State storage self,
        uint256 batchSize,
        bytes32 historicalOutputsHash,
        uint256 entropyAccumulator
    ) internal returns (uint256 processed, uint256 fulfilled) {
        if (batchSize == 0 || batchSize > self.maxBatchSize) revert InvalidBatchSize();
        
        uint256 startId = self.lastProcessedId;
        uint256 endId = self.requestCount;
        uint256 remaining = batchSize;
        uint256 processedCount = 0;
        uint256 fulfilledCount = 0;
        
        for (uint256 i = startId; i < endId && remaining > 0; i++) {
            Request storage request = self.requests[i];
            
            if (!request.fulfilled && 
                request.requester != address(0) && 
                block.number <= request.timestamp + self.expiryBlocks &&
                request.contributionsCount >= self.minContributions) {
                
                fulfillRequest(self, i, historicalOutputsHash, entropyAccumulator);
                fulfilledCount++;
            }
            processedCount++;
            remaining--;
        }
        
        if (processedCount > 0) {
            self.lastProcessedId = startId + processedCount;
        }
        
        // Reset to beginning if we've processed all requests
        if (self.lastProcessedId >= self.requestCount) {
            self.lastProcessedId = 0;
        }
        
        return (processedCount, fulfilledCount);
    }
    
    // Helper function to process individual contribution to reduce stack depth
    function _processContribution(
        State storage self,
        uint256 requestId,
        bytes32 contribution,
        bytes calldata signature,
        address sender,
        bytes32 historicalOutputsHash,
        uint256 entropyAccumulator
    ) private returns (bool success) {
        // Skip already invalid scenarios
        if (requestId >= self.requestCount) return false;
        
        Request storage request = self.requests[requestId];
        
        if (request.fulfilled) return false;
        if (block.number > request.timestamp + self.expiryBlocks) return false;
        if (self.contributors[requestId][sender]) return false;
        if (request.contributionsCount >= self.maxContributions) return false;
        
        // Verify signature to prevent contribution spoofing
        bytes32 messageHash = keccak256(abi.encodePacked(
            requestId,
            contribution,
            sender
        ));
        address recovered = messageHash.toEthSignedMessageHash().recover(signature);
        if (recovered != sender) return false;
        
        // Record contribution
        request.entropyContributions[request.contributionsCount] = contribution;
        request.contributionsCount++;
        self.contributors[requestId][sender] = true;
        
        // Check if we have enough contributions to fulfill
        if (request.contributionsCount >= self.minContributions) {
            fulfillRequest(self, requestId, historicalOutputsHash, entropyAccumulator);
        }
        
        return true;
    }
    
    // Batch contribute entropy - refactored to avoid stack too deep
    function batchContribute(
        State storage self,
        uint256[] calldata requestIds,
        bytes32[] calldata entropyContributions,
        bytes[] calldata entropySignatures,
        address sender,
        bytes32 historicalOutputsHash,
        uint256 entropyAccumulator
    ) internal returns (uint256 successCount) {
        // Input validation
        if (requestIds.length != entropyContributions.length || 
            requestIds.length != entropySignatures.length) revert ArrayLengthMismatch();
        if (requestIds.length > self.maxBatchSize) revert BatchTooLarge();
        
        // Process each contribution
        uint256 count = 0;
        
        for (uint256 i = 0; i < requestIds.length; i++) {
            bool success = _processContribution(
                self,
                requestIds[i],
                entropyContributions[i],
                entropySignatures[i],
                sender,
                historicalOutputsHash,
                entropyAccumulator
            );
            
            if (success) {
                count++;
            }
        }
        
        return count;
    }
    
    // Get batch randomness results
    function getBatchResults(
        State storage self,
        uint256[] calldata requestIds
    ) internal view returns (bytes32[] memory results, bool[] memory fulfilled) {
        // Use optimized array creation
        results = BytesArrayLib.createBytes32Array(requestIds.length);
        fulfilled = new bool[](requestIds.length);
        
        for (uint256 i = 0; i < requestIds.length; i++) {
            if (requestIds[i] >= self.requestCount) {
                fulfilled[i] = false;
                continue;
            }
            
            Request storage request = self.requests[requestIds[i]];
            fulfilled[i] = request.fulfilled;
            
            if (request.fulfilled) {
                results[i] = request.result;
            }
        }
        
        return (results, fulfilled);
    }
    
    // Get request state
    function getRequestState(
        State storage self,
        uint256 requestId
    ) internal view returns (address requester, uint256 timestamp, bool fulfilled, uint256 contributionsCount) {
        Request storage request = self.requests[requestId];
        return (
            request.requester,
            request.timestamp,
            request.fulfilled,
            request.contributionsCount
        );
    }
    
    // Get estimated storage size
    function getStorageEstimate(State storage self) internal view returns (uint256) {
        unchecked {
            return 100 * self.requestCount;
        }
    }
}
