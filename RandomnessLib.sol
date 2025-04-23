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
    
    // BeaconBlock definition to support archive functions
    struct BeaconBlock {
        bytes32 output;
        bytes32 previousOutput;
        uint64 nonce;
        address miner;
        uint256 actualDifficulty;
        uint256 reward;
        uint256 timestamp;
        uint256 poolId;
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
        // Historical blocks storage (optional usage)
        BeaconBlock[] historicalBlocks;
        uint256 maxHistoricalBlocks;
        bool historicalStorageEnabled;
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
            
            unchecked { // Safe as loop bounds are controlled
                processedCount++;
                remaining--;
            }
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
        // Explicitly cast to resolve ambiguity
        uint256 requestIdsLen = requestIds.length;
        uint256 entropyContributionsLen = (uint256[])(entropyContributions).length;
        uint256 entropySignaturesLen = entropySignatures.length;

        if (requestIdsLen != entropyContributionsLen || requestIdsLen != entropySignaturesLen) revert ArrayLengthMismatch();
        if (requestIdsLen > self.maxBatchSize) revert BatchTooLarge();

        uint256 count = 0;
        unchecked {
            for (uint256 i = 0; i < requestIdsLen; i++) {
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
        }
        return count;
    }
    
    // Get batch randomness results
    function getBatchResults(
        State storage self,
        uint256[] calldata requestIds
    ) internal view returns (bytes32[] memory results, bool[] memory fulfilled) {
        // Use optimized array creation - Access length directly for calldata arrays using .length
        uint256 requestIdLen = requestIds.length;
        results = BytesArrayLib.createBytes32Array(requestIdLen);
        fulfilled = new bool[](requestIdLen);
        
        // Use unchecked for gas optimization in Arbitrum
        unchecked {
            for (uint256 i = 0; i < requestIdLen; i++) {
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
    
    // New functions to support historical blocks without modifying main contract
    
    /**
     * @notice Configures the historical block storage in the RandomnessLib
     * @dev Can be called but won't affect the contract unless it explicitly uses these values
     * @param self Storage reference
     * @param enabled Whether to enable historical storage
     * @param maxBlocks Maximum number of blocks to store
     */
    function configureHistoricalStorage(
        State storage self,
        bool enabled,
        uint256 maxBlocks
    ) internal {
        self.historicalStorageEnabled = enabled;
        self.maxHistoricalBlocks = maxBlocks;
    }
    
    /**
     * @notice Archives a block in the historical storage
     * @dev No-op if historical storage is disabled
     * @param self Storage reference
     * @param output Block output
     * @param previousOutput Previous block output
     * @param nonce Block nonce
     * @param miner Block miner
     * @param difficulty Block difficulty
     * @param reward Mining reward
     * @param timestamp Block timestamp
     * @param poolId Mining pool ID
     */
    function archiveBlock(
        State storage self,
        bytes32 output,
        bytes32 previousOutput,
        uint64 nonce,
        address miner,
        uint256 difficulty,
        uint256 reward,
        uint256 timestamp,
        uint256 poolId
    ) internal returns (uint256 blockIndex) {
        // Skip if historical storage is disabled
        if (!self.historicalStorageEnabled) return 0;
        
        // If we've reached max capacity, remove oldest block
        if (self.historicalBlocks.length >= self.maxHistoricalBlocks && 
            self.maxHistoricalBlocks > 0) {
            // For Arbitrum, optimize array shifting by using pop and push
            // This avoids excessive storage manipulation
            uint256 historyLength = self.historicalBlocks.length;
            
            // Store the last block temporarily
            BeaconBlock memory lastBlock = self.historicalBlocks[historyLength - 1];
            
            // Remove first element (shift by popping from end and manipulating indices)
            self.historicalBlocks.pop();
            
            // Ensure we're not dealing with an empty array
            if (historyLength > 1) {
                // Copy elements one position back (pop last element)
                for (uint256 i = 0; i < historyLength - 2; i++) {
                    self.historicalBlocks[i] = self.historicalBlocks[i + 1];
                }
                // Put the last block in the second-to-last position
                self.historicalBlocks[historyLength - 2] = lastBlock;
            }
        }
        
        // Add new block
        self.historicalBlocks.push(
            BeaconBlock({
                output: output,
                previousOutput: previousOutput,
                nonce: nonce,
                miner: miner,
                actualDifficulty: difficulty,
                reward: reward,
                timestamp: timestamp,
                poolId: poolId
            })
        );
        
        return self.historicalBlocks.length - 1;
    }
    
    /**
     * @notice Gets the count of historical blocks
     * @param self Storage reference
     * @return count Number of historical blocks
     */
    function getHistoricalBlockCount(
        State storage self
    ) internal view returns (uint256 count) {
        return self.historicalBlocks.length;
    }
    
    /**
     * @notice Gets a single historical block
     * @param self Storage reference
     * @param index Block index
     * @return blockData The historical block data
     */
    function getHistoricalBlock(
        State storage self,
        uint256 index
    ) internal view returns (BeaconBlock memory blockData) {
        require(index < self.historicalBlocks.length, "Invalid block index");
        return self.historicalBlocks[index];
    }
    
    /**
     * @notice Gets multiple historical blocks
     * @param self Storage reference
     * @param startIndex Start index (inclusive)
     * @param endIndex End index (exclusive)
     * @return blocks Array of historical blocks
     */
    function getHistoricalBlockRange(
        State storage self,
        uint256 startIndex,
        uint256 endIndex
    ) internal view returns (BeaconBlock[] memory blocks) {
        // Avoid unnecessary storage reads on Arbitrum by using local variables
        uint256 historyLength = self.historicalBlocks.length;
        
        if (endIndex > historyLength) {
            endIndex = historyLength;
        }
        if (startIndex >= endIndex) {
            return new BeaconBlock[](0);
        }
        
        uint256 resultLength = endIndex - startIndex;
        BeaconBlock[] memory result = new BeaconBlock[](resultLength);
        
        // Use unchecked for gas optimization in Arbitrum
        unchecked {
            for (uint256 i = 0; i < resultLength; i++) {
                result[i] = self.historicalBlocks[startIndex + i];
            }
        }
        
        return result;
    }
    
    /**
     * @notice Uses historical blocks as an additional source of randomness
     * @dev Can be used to enhance randomness quality when historical blocks are available
     * @param self Storage reference
     * @param userSeed User-provided seed
     * @param blockCount Number of recent blocks to use (0 for all available)
     * @return enhancedSeed A seed enhanced with historical block data
     */
    function enhanceRandomnessWithHistory(
        State storage self,
        bytes32 userSeed,
        uint256 blockCount
    ) internal view returns (bytes32 enhancedSeed) {
        // Early return to avoid storage reads on Arbitrum
        if (!self.historicalStorageEnabled) return userSeed;
        
        uint256 historyLength = self.historicalBlocks.length;
        if (historyLength == 0) return userSeed;
        
        uint256 count;
        if (blockCount == 0) {
            count = historyLength;
        } else {
            count = blockCount > historyLength ? historyLength : blockCount;
        }
        
        bytes memory combinedData = abi.encodePacked(userSeed);
        
        // Use the most recent blocks for enhanced randomness
        uint256 startIdx;
        if (historyLength > count) {
            startIdx = historyLength - count;
        } else {
            startIdx = 0;
        }
        
        // Avoid excessive storage reads on Arbitrum by reading once per iteration
        unchecked {
            for (uint256 i = startIdx; i < historyLength; i++) {
                BeaconBlock storage currentBlock = self.historicalBlocks[i];
                combinedData = abi.encodePacked(
                    combinedData,
                    currentBlock.output,
                    currentBlock.nonce,
                    currentBlock.timestamp
                );
            }
        }
        
        return keccak256(combinedData);
    }
    
    /**
     * @notice Clears historical blocks to save gas/storage
     * @param self Storage reference
     * @return count Number of blocks cleared
     */
    function clearHistoricalBlocks(
        State storage self
    ) internal returns (uint256 count) {
        uint256 blocksCount = self.historicalBlocks.length;
        delete self.historicalBlocks;
        return blocksCount;
    }
    
    /**
     * @notice Optimized version of fulfillRequest specifically for Arbitrum
     * @dev Reduces gas costs by optimizing storage access patterns
     */
    function fulfillRequestOptimized(
        State storage self,
        uint256 requestId,
        bytes32 historicalOutputsHash,
        uint256 entropyAccumulator
    ) internal returns (bytes32 randomValue) {
        // Cache request in memory to avoid multiple SLOAD operations
        Request storage requestStorage = self.requests[requestId];
        
        // Validate request (combine checks to reduce jumps)
        bool isValid = requestId < self.requestCount &&
                      !requestStorage.fulfilled &&
                      requestStorage.requester != address(0) &&
                      block.number <= requestStorage.timestamp + self.expiryBlocks &&
                      requestStorage.contributionsCount >= self.minContributions;
        
        if (!isValid) revert InvalidRequest();
        
        // Collect contributions - optimize by reading length once
        uint256 contribCount = requestStorage.contributionsCount;
        bytes32[] memory contributions = BytesArrayLib.createBytes32Array(contribCount);
        
        unchecked {
            for (uint i = 0; i < contribCount; i++) {
                contributions[i] = requestStorage.entropyContributions[i];
            }
        }
        
        // Generate random value (same as original function)
        bytes memory randomnessInput = abi.encodePacked(
            historicalOutputsHash,
            requestStorage.userSeed,
            requestStorage.requester,
            requestStorage.timestamp,
            blockhash(block.number - 1),
            block.prevrandao,
            block.timestamp,
            entropyAccumulator,
            keccak256(abi.encodePacked(contributions))
        );
        
        // Generate random value
        randomValue = _generateRandomValue(self, randomnessInput);
        
        // Update request
        requestStorage.fulfilled = true;
        requestStorage.result = randomValue;
        
        return randomValue;
    }
}
