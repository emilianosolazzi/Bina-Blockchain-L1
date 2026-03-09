// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Math} from "@openzeppelin/contracts/utils/math/Math.sol";
import {Strings} from "@openzeppelin/contracts/utils/Strings.sol";
import {StorageSlot} from "@openzeppelin/contracts/utils/StorageSlot.sol";

/**
 * @title StorageLib
 * @notice Library for managing historical block storage
 * @dev Uses a circular buffer pattern to optimize gas usage when storing beacon blocks
 */
library StorageLib {
    using Math for uint256;
    using Strings for uint256;

    // BeaconBlock structure for storing comprehensive block data
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

    struct HistoricalStorage {
        // Storage as a fixed-size mapping rather than a dynamic array
        mapping(uint256 => BeaconBlock) blockMap;
        uint256 maxBlocks;
        uint256 totalBlocks; // Total blocks ever archived (can exceed maxBlocks)
        uint256 headIndex;   // Current position in the circular buffer
        bool enabled;
        bool configured;     // Flag to prevent reconfiguration of maxBlocks
    }
    
    event HistoricalStorageConfigChanged(bool enabled, uint256 maxBlocks);
    event BlockArchived(uint256 indexed blockIndex, bytes32 output, address indexed miner);
    
    // Add custom errors
    error InvalidMaxBlocks(uint256 provided);
    error StorageDisabled();
    error BlockIndexOutOfRange(uint256 blockIndex, uint256 oldest, uint256 newest);
    error InvalidRange(uint256 start, uint256 end);
    error ConfigurationLocked();
    
    function configureHistoricalStorage(
        HistoricalStorage storage self,
        bool enabled,
        uint256 maxBlocks,
        bytes32 genesisOutput,
        uint256 genesisTimestamp,
        address sender
    ) internal {
        if (maxBlocks == 0) revert InvalidMaxBlocks(0);
        if (self.configured) revert ConfigurationLocked();
        
        require(maxBlocks > 0, "Max blocks must be > 0");
        
        // Prevent changing maxBlocks after initial configuration to avoid orphaned data
        if (self.configured) {
            // Can still toggle enabled/disabled state
            self.enabled = enabled;
            emit HistoricalStorageConfigChanged(enabled, self.maxBlocks);
            return;
        }
        
        self.enabled = enabled;
        self.maxBlocks = maxBlocks;
        self.configured = true; // Mark as configured to prevent future maxBlocks changes
        
        // If enabling and we have no blocks yet, add genesis block
        if (enabled && self.totalBlocks == 0) {
            // Initialize the circular buffer with genesis block at index 0
            self.blockMap[0] = BeaconBlock({
                output: genesisOutput,
                previousOutput: bytes32(0),
                nonce: 0,
                miner: sender,
                actualDifficulty: 0,
                reward: 0, 
                timestamp: genesisTimestamp,
                poolId: 0
            });
            
            self.totalBlocks = 1;
            self.headIndex = 0;
        }
        
        emit HistoricalStorageConfigChanged(enabled, maxBlocks);
    }
    
    /**
     * @notice Force reconfiguration of maxBlocks (should be used with extreme caution)
     * @dev This function can orphan data if maxBlocks is reduced
     * @param self The storage reference
     * @param newMaxBlocks New maximum number of blocks
     * @param force Whether to force the change even if data loss may occur
     */
    function forceReconfigure(
        HistoricalStorage storage self,
        uint256 newMaxBlocks,
        bool force
    ) internal returns (bool success) {
        require(newMaxBlocks > 0, "Max blocks must be > 0");
        
        // If reducing size, check if we'll lose data
        if (newMaxBlocks < self.maxBlocks && self.totalBlocks > newMaxBlocks) {
            // Will lose data - require force flag
            if (!force) {
                return false;
            }
            
            // With force=true, we accept data loss
            // Adjust head index to point within new bounds
            self.headIndex = self.headIndex % newMaxBlocks;
        }
        
        self.maxBlocks = newMaxBlocks;
        emit HistoricalStorageConfigChanged(self.enabled, newMaxBlocks);
        return true;
    }
    
    function archiveBlock(
        HistoricalStorage storage self,
        bytes32 output,
        bytes32 previousOutput,
        uint64 nonce,
        address miner,
        uint256 actualDifficulty,
        uint256 reward,
        uint256 timestamp,
        uint256 poolId
    ) internal returns (uint256 blockIndex) {
        if (!self.enabled) return 0;
        
        // Calculate the next position in the circular buffer
        uint256 nextIndex = (self.headIndex + 1) % self.maxBlocks;
        
        // Add new block at the calculated position
        self.blockMap[nextIndex] = BeaconBlock({
            output: output,
            previousOutput: previousOutput,
            nonce: nonce,
            miner: miner,
            actualDifficulty: actualDifficulty,
            reward: reward,
            timestamp: timestamp,
            poolId: poolId
        });
        
        // Update head index and total block count
        self.headIndex = nextIndex;
        self.totalBlocks += 1;
        
        // Return the absolute index (total blocks - 1)
        blockIndex = self.totalBlocks - 1;
        emit BlockArchived(blockIndex, output, miner);
        return blockIndex;
    }
    
    function getHistoricalBlockRange(
        HistoricalStorage storage self,
        uint256 startIndex,
        uint256 endIndex
    ) internal view returns (BeaconBlock[] memory blocks) {
        // Calculate how many blocks we can actually return
        uint256 availableBlocks = self.totalBlocks;
        uint256 oldestBlockIndex = 0;
        
        // If we have more blocks than maxBlocks, calculate the oldest available index
        if (availableBlocks > self.maxBlocks) {
            oldestBlockIndex = availableBlocks - self.maxBlocks;
        }
        
        // Ensure startIndex is not before the oldest available block
        if (startIndex < oldestBlockIndex) {
            startIndex = oldestBlockIndex;
        }
        
        // Ensure endIndex doesn't exceed total blocks
        if (endIndex > availableBlocks) {
            endIndex = availableBlocks;
        }
        
        // Return empty array if invalid range
        if (startIndex >= endIndex) {
            return new BeaconBlock[](0);
        }
        
        uint256 resultLength = endIndex - startIndex;
        BeaconBlock[] memory result = new BeaconBlock[](resultLength);
        
        for (uint256 i = 0; i < resultLength; i++) {
            // Calculate the physical position in the circular buffer
            uint256 actualIndex = (startIndex + i) % self.maxBlocks;
            result[i] = self.blockMap[actualIndex];
        }
        
        return result;
    }
    
    /**
     * @notice Get a single historical block by its absolute index
     * @dev Will revert if the index is out of bounds (older than maxBlocks)
     * @param self The storage reference
     * @param blockIndex The absolute index of the block to retrieve
     * @return block The requested block
     */
    function getHistoricalBlock(
        HistoricalStorage storage self,
        uint256 blockIndex
    ) internal view returns (BeaconBlock memory) {
        if (!self.enabled) revert StorageDisabled();
        
        // Check that the requested block is within available range
        uint256 oldestAvailable = Math.max(0, self.totalBlocks > self.maxBlocks ? 
            self.totalBlocks - self.maxBlocks : 0);
            
        if (blockIndex < oldestAvailable || blockIndex >= self.totalBlocks) {
            revert BlockIndexOutOfRange(blockIndex, oldestAvailable, self.totalBlocks);
        }
        
        // Calculate the physical index in the circular buffer
        uint256 actualIndex = blockIndex % self.maxBlocks;
        return self.blockMap[actualIndex];
    }
    
    /**
     * @notice Get information about the storage state
     * @param self The storage reference
     * @return enabled Whether historical storage is enabled
     * @return maxBlocks Maximum number of blocks to store
     * @return totalBlocks Total number of blocks ever archived
     * @return oldestAvailable Index of the oldest available block
     */
    function getStorageInfo(
        HistoricalStorage storage self
    ) internal view returns (bool enabled, uint256 maxBlocks, uint256 totalBlocks, uint256 oldestAvailable) {
        enabled = self.enabled;
        maxBlocks = self.maxBlocks;
        totalBlocks = self.totalBlocks;
        oldestAvailable = self.totalBlocks > self.maxBlocks ? 
            self.totalBlocks - self.maxBlocks : 0;
            
        return (enabled, maxBlocks, totalBlocks, oldestAvailable);
    }
    
    /**
     * @notice Check if changing maxBlocks would result in data loss
     * @param self The storage reference
     * @param newMaxBlocks Proposed new maximum blocks
     * @return wouldLoseData Whether changing to newMaxBlocks would lose data
     * @return dataLossCount Number of entries that would be lost
     */
    function checkReconfigurationImpact(
        HistoricalStorage storage self,
        uint256 newMaxBlocks
    ) internal view returns (bool wouldLoseData, uint256 dataLossCount) {
        if (newMaxBlocks >= self.maxBlocks) {
            // Increasing size never loses data
            return (false, 0);
        }
        
        uint256 actualStored = self.totalBlocks > self.maxBlocks ? self.maxBlocks : self.totalBlocks;
        
        if (actualStored <= newMaxBlocks) {
            // All current data fits in new size
            return (false, 0);
        }
        
        // Calculate how many entries would be lost
        dataLossCount = actualStored - newMaxBlocks;
        return (true, dataLossCount);
    }
}
