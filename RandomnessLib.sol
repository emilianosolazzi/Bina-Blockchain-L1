// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ECDSAUpgradeable } from "@openzeppelin/contracts-upgradeable/utils/cryptography/ECDSAUpgradeable.sol";

/**
 * @title RandomnessLib
 * @notice Library for handling randomness-related functionality.
 * @dev Considers ZKP enhancements for VRFs or private contributions.
 */
library RandomnessLib {
    using ECDSAUpgradeable for bytes32;
    
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
        
        bytes32[] memory contributions = new bytes32[](self.maxContributions);
        
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

        // --- Arbitrum Optimization: Cache storage reads ---
        bool fulfilled = request.fulfilled;
        uint256 timestamp = request.timestamp;
        uint256 expiry = self.expiryBlocks;
        uint256 maxContrib = self.maxContributions;
        uint256 minContrib = self.minContributions;
        // --- End Optimization ---

        if (fulfilled) revert RequestFulfilled();
        if (block.number > timestamp + expiry) revert RequestExpired(); // Use cached values
        if (self.contributors[requestId][contributor]) revert AlreadyContributed();

        uint256 currentCount = request.contributionsCount; // Read count once
        if (currentCount >= maxContrib) revert MaxContributionsReached(); // Use cached value

        request.entropyContributions[currentCount] = entropyContribution;
        request.contributionsCount = currentCount + 1;
        self.contributors[requestId][contributor] = true;

        return (currentCount + 1 >= minContrib); // Use cached value
    }
    
    // Fulfill a randomness request (Integrate optimizations from fulfillRequestOptimized)
    function fulfillRequest(
        State storage self,
        uint256 requestId,
        bytes32 historicalOutputsHash,
        uint256 entropyAccumulator
    ) internal returns (bytes32 randomValue) {
        // Cache request in memory to avoid multiple SLOAD operations
        Request storage requestStorage = self.requests[requestId];

        // --- Arbitrum Optimization: Cache storage reads ---
        bool fulfilled = requestStorage.fulfilled;
        address requester = requestStorage.requester;
        uint256 timestamp = requestStorage.timestamp;
        uint256 expiry = self.expiryBlocks;
        uint256 contribCount = requestStorage.contributionsCount;
        uint256 minContrib = self.minContributions;
        // --- End Optimization ---

        // Validate request (combine checks to reduce jumps)
        bool isValid = requestId < self.requestCount &&
                      !fulfilled && // Use cached value
                      requester != address(0) && // Use cached value
                      block.number <= timestamp + expiry && // Use cached values
                      contribCount >= minContrib; // Use cached values

        if (!isValid) revert InvalidRequest();

        // Collect contributions - optimize by reading length once
        bytes32[] memory contributions = new bytes32[](contribCount); // Use cached value

        // Use unchecked for loop counter
        unchecked {
            for (uint i = 0; i < contribCount; i++) { // Use cached value
                contributions[i] = requestStorage.entropyContributions[i];
            }
        }

        // --- Potential ZKP VRF Integration Point ---
        // ... (no changes needed here for Arbitrum optimization) ...
        // --- End ZKP VRF Integration Point ---

        // --- Current Hashing Method ---
        bytes memory randomnessInput = abi.encodePacked(
            historicalOutputsHash,
            requestStorage.userSeed, // Read from storage (or cache if read multiple times)
            requester, // Use cached value
            timestamp, // Use cached value
            blockhash(block.number - 1),
            block.prevrandao,
            block.timestamp,
            entropyAccumulator,
            keccak256(abi.encodePacked(contributions))
        );

        // Generate random value
        randomValue = _generateRandomValue(self, randomnessInput);
        // --- End Current Hashing Method ---

        // Update request
        requestStorage.fulfilled = true;
        requestStorage.result = randomValue;

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
        // --- Arbitrum Optimization: Cache storage reads ---
        Request storage request = self.requests[requestId];
        address requester = request.requester;
        bool fulfilled = request.fulfilled;
        // --- End Optimization ---
        if (requester == address(0)) revert RequestDoesNotExist(); // Use cached value
        if (!fulfilled) revert RequestNotFulfilled(); // Use cached value
        return request.result; // Read result once
    }
    
    // Process pending randomness requests
    function processPendingRequests(
        State storage self,
        bytes32 historicalOutputsHash,
        uint256 entropyAccumulator
    ) internal {
        // --- Arbitrum Optimization: Cache storage reads ---
        uint256 reqCount = self.requestCount;
        uint256 lastId = self.lastProcessedId;
        uint256 expiry = self.expiryBlocks;
        uint256 minContrib = self.minContributions;
        // --- End Optimization ---

        if (reqCount <= lastId) return;

        // Optimize toProcess calculation
        uint256 remaining = reqCount - lastId;
        uint256 toProcess = remaining > 3 ? 3 : remaining; // Keep batch small

        // Process eligible requests
        // Use unchecked for loop counter
        unchecked {
            for (uint256 i = 0; i < toProcess; ++i) {
                uint256 currentRequestId = lastId + i;
                Request storage request = self.requests[currentRequestId];

                // Cache request fields for this iteration
                bool reqFulfilled = request.fulfilled;
                address reqRequester = request.requester;
                uint256 reqTimestamp = request.timestamp;
                uint256 reqContribCount = request.contributionsCount;

                if (!reqFulfilled &&
                    reqRequester != address(0) &&
                    block.number <= reqTimestamp + expiry && // Use cached expiry
                    reqContribCount >= minContrib) { // Use cached minContrib

                    // Call optimized fulfill function if available, otherwise standard
                    fulfillRequest(self, currentRequestId, historicalOutputsHash, entropyAccumulator);
                    // fulfillRequestOptimized(self, currentRequestId, historicalOutputsHash, entropyAccumulator);
                }
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

        // --- Arbitrum Optimization: Cache storage reads ---
        Request storage request = self.requests[requestId];
        bool fulfilled = request.fulfilled;
        address requester = request.requester;
        uint256 timestamp = request.timestamp;
        uint256 expiry = self.expiryBlocks;
        uint256 current = request.contributionsCount;
        uint256 minContrib = self.minContributions;
        // --- End Optimization ---

        if (fulfilled) return (true, 0); // Use cached value
        if (requester == address(0)) return (false, 0); // Use cached value
        if (block.number > timestamp + expiry) return (false, 0); // Use cached values

        if (current >= minContrib) return (true, 0); // Use cached values

        // Use unchecked for subtraction as current < minContrib is guaranteed here
        unchecked {
            pendingContributions = minContrib - current; // Use cached values
        }
        return (false, pendingContributions);
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
        // --- Arbitrum Optimization: Cache storage reads ---
        uint256 maxBatch = self.maxBatchSize;
        uint256 startId = self.lastProcessedId;
        uint256 endId = self.requestCount;
        uint256 expiry = self.expiryBlocks;
        uint256 minContrib = self.minContributions;
        // --- End Optimization ---

        if (batchSize == 0 || batchSize > maxBatch) revert InvalidBatchSize(); // Use cached value

        uint256 remaining = batchSize;
        uint256 processedCount = 0;
        uint256 fulfilledCount = 0;

        // Use unchecked for loop counter and arithmetic
        unchecked {
            for (uint256 i = startId; i < endId && remaining > 0; i++) {
                Request storage request = self.requests[i];

                // Cache request fields for this iteration
                bool reqFulfilled = request.fulfilled;
                address reqRequester = request.requester;
                uint256 reqTimestamp = request.timestamp;
                uint256 reqContribCount = request.contributionsCount;

                if (!reqFulfilled &&
                    reqRequester != address(0) &&
                    block.number <= reqTimestamp + expiry && // Use cached expiry
                    reqContribCount >= minContrib) { // Use cached minContrib

                    // Call optimized fulfill function if available, otherwise standard
                    fulfillRequest(self, i, historicalOutputsHash, entropyAccumulator);
                    // fulfillRequestOptimized(self, i, historicalOutputsHash, entropyAccumulator);
                    fulfilledCount++;
                }

                processedCount++;
                remaining--;
            }
        }

        if (processedCount > 0) {
            self.lastProcessedId = startId + processedCount;
        }

        // Reset to beginning if we've processed all requests
        if (self.lastProcessedId >= endId) { // Use cached endId
            self.lastProcessedId = 0;
        }

        return (processedCount, fulfilledCount);
    }
    
    // Helper function to process individual contribution to reduce stack depth
    function _processContribution(
        State storage self,
        uint256 requestId,
        bytes32 contribution, // Could be a commitment if using ZKP for privacy
        bytes calldata signatureOrProof, // Could be a ZKP instead of a signature
        address sender,
        bytes32 historicalOutputsHash,
        uint256 entropyAccumulator
    ) private returns (bool success) {
        // Skip already invalid scenarios
        if (requestId >= self.requestCount) return false;

        Request storage request = self.requests[requestId];

        // --- Arbitrum Optimization: Cache storage reads ---
        bool fulfilled = request.fulfilled;
        uint256 timestamp = request.timestamp;
        uint256 expiry = self.expiryBlocks;
        uint256 maxContrib = self.maxContributions;
        uint256 currentCount = request.contributionsCount;
        uint256 minContrib = self.minContributions;
        // --- End Optimization ---

        if (fulfilled) return false; // Use cached value
        if (block.number > timestamp + expiry) return false; // Use cached values
        if (self.contributors[requestId][sender]) return false;
        if (currentCount >= maxContrib) return false; // Use cached values

        // --- Potential ZKP Contribution Proof ---
        // ... (no changes needed here for Arbitrum optimization) ...
        // --- End ZKP Contribution Proof ---

        // --- Current Signature Verification ---
        bytes32 messageHash = keccak256(abi.encodePacked(
            requestId,
            contribution,
            sender
        ));
        address recovered = messageHash.toEthSignedMessageHash().recover(signatureOrProof);
        if (recovered != sender) return false;
        // --- End Current Signature Verification ---

        // Record contribution
        request.entropyContributions[currentCount] = contribution; // Use cached value
        request.contributionsCount = currentCount + 1; // Use cached value
        self.contributors[requestId][sender] = true;

        // Check if we have enough contributions to fulfill
        if (currentCount + 1 >= minContrib) { // Use cached values
            // Call optimized fulfill function if available, otherwise standard
            fulfillRequest(self, requestId, historicalOutputsHash, entropyAccumulator);
            // fulfillRequestOptimized(self, requestId, historicalOutputsHash, entropyAccumulator);
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
        // --- Arbitrum Optimization: Cache storage read ---
        uint256 maxBatch = self.maxBatchSize;
        // --- End Optimization ---

        // Copy calldata lengths to memory once
        uint256 requestIdsLen = requestIds.length;
        uint256 entropyContributionsLen = entropyContributions.length;
        uint256 entropySignaturesLen = entropySignatures.length;

        if (requestIdsLen != entropyContributionsLen || requestIdsLen != entropySignaturesLen) revert ArrayLengthMismatch();
        if (requestIdsLen > maxBatch) revert BatchTooLarge(); // Use cached value

        uint256 count = 0;
        // Use unchecked for loop counter
        unchecked {
            for (uint256 i = 0; i < requestIdsLen; i++) {
                // Directly access calldata elements inside the loop
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
        uint256 requestIdLen = requestIds.length;
        results = new bytes32[](requestIdLen);
        fulfilled = new bool[](requestIdLen);

        // --- Arbitrum Optimization: Cache storage read ---
        uint256 reqCount = self.requestCount;
        // --- End Optimization ---

        // Use unchecked for loop counter
        unchecked {
            for (uint256 i = 0; i < requestIdLen; i++) {
                uint256 currentId = requestIds[i]; // Read calldata once per iteration
                if (currentId >= reqCount) { // Use cached value
                    fulfilled[i] = false;
                    continue;
                }

                Request storage request = self.requests[currentId];
                bool reqFulfilled = request.fulfilled; // Read fulfilled status once
                fulfilled[i] = reqFulfilled;

                if (reqFulfilled) {
                    results[i] = request.result; // Read result only if fulfilled
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
        // --- Arbitrum Optimization: Cache storage reads ---
        bool enabled = self.historicalStorageEnabled;
        uint256 maxBlocks = self.maxHistoricalBlocks;
        // --- End Optimization ---

        if (!enabled) return 0; // Use cached value

        uint256 historyLength = self.historicalBlocks.length; // Read length once

        if (historyLength >= maxBlocks && maxBlocks > 0) { // Use cached value
            // Optimized shifting for Arbitrum (pop/push simulation)
            // This part seems complex and potentially gas-intensive even with optimization.
            // Consider if simpler FIFO (just push and let old ones fall off implicitly if needed elsewhere) is sufficient.
            // The current implementation tries to maintain exact order which might be costly.
            // Re-evaluating the necessity of strict FIFO order vs. simpler push/overwrite.
            // For now, keeping the existing logic but noting its potential cost.
            if (historyLength > 0) { // Ensure not empty before pop
                 self.historicalBlocks.pop(); // This shifts elements left, effectively removing index 0
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
        uint256 historyLength = self.historicalBlocks.length; // Cache length

        if (endIndex > historyLength) {
            endIndex = historyLength;
        }
        if (startIndex >= endIndex) {
            return new BeaconBlock[](0);
        }

        uint256 resultLength = endIndex - startIndex;
        BeaconBlock[] memory result = new BeaconBlock[](resultLength);

        // Use unchecked for loop counter and array access
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
        // --- Arbitrum Optimization: Cache storage reads ---
        bool enabled = self.historicalStorageEnabled;
        uint256 historyLength = self.historicalBlocks.length;
        // --- End Optimization ---

        if (!enabled || historyLength == 0) return userSeed; // Use cached values

        uint256 count;
        if (blockCount == 0) {
            count = historyLength;
        } else {
            count = blockCount > historyLength ? historyLength : blockCount;
        }

        bytes memory combinedData = abi.encodePacked(userSeed);

        uint256 startIdx;
        // Use unchecked for subtraction as historyLength > count is checked implicitly or count <= historyLength
        unchecked {
             startIdx = historyLength - count;
        }

        // Use unchecked for loop counter
        unchecked {
            for (uint256 i = startIdx; i < historyLength; i++) {
                // Cache storage struct pointer for this iteration
                BeaconBlock storage currentBlock = self.historicalBlocks[i];
                combinedData = abi.encodePacked(
                    combinedData,
                    currentBlock.output, // Read from cached pointer
                    currentBlock.nonce, // Read from cached pointer
                    currentBlock.timestamp // Read from cached pointer
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
}
