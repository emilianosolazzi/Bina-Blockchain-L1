// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title StorageLib
 * @notice Library for managing historical block storage
 */
library StorageLib {
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
        BeaconBlock[] blocks;
        uint256 maxBlocks;
        bool enabled;
    }
    
    event HistoricalStorageConfigChanged(bool enabled, uint256 maxBlocks);
    event BlockArchived(uint256 indexed blockIndex, bytes32 output, address indexed miner);
    
    function configureHistoricalStorage(
        HistoricalStorage storage self,
        bool enabled,
        uint256 maxBlocks,
        bytes32 genesisOutput,
        uint256 genesisTimestamp,
        address sender
    ) internal {
        self.enabled = enabled;
        self.maxBlocks = maxBlocks;
        
        // If enabling and we have no genesis block, add it
        if (enabled && self.blocks.length == 0) {
            self.blocks.push(
                BeaconBlock({
                    output: genesisOutput,
                    previousOutput: bytes32(0),
                    nonce: 0,
                    miner: sender,
                    actualDifficulty: 0,
                    reward: 0, 
                    timestamp: genesisTimestamp,
                    poolId: 0
                })
            );
        }
        
        emit HistoricalStorageConfigChanged(enabled, maxBlocks);
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
        
        // If we've reached max capacity, remove oldest block
        if (self.blocks.length >= self.maxBlocks && self.maxBlocks > 0) {
            // Shift array elements (remove first element)
            for (uint256 i = 0; i < self.blocks.length - 1; i++) {
                self.blocks[i] = self.blocks[i + 1];
            }
            self.blocks.pop(); // Remove last duplicate
        }
        
        // Add new block
        self.blocks.push(
            BeaconBlock({
                output: output,
                previousOutput: previousOutput,
                nonce: nonce,
                miner: miner,
                actualDifficulty: actualDifficulty,
                reward: reward,
                timestamp: timestamp,
                poolId: poolId
            })
        );
        
        blockIndex = self.blocks.length - 1;
        emit BlockArchived(blockIndex, output, miner);
        return blockIndex;
    }
    
    function getHistoricalBlockRange(
        HistoricalStorage storage self,
        uint256 startIndex,
        uint256 endIndex
    ) internal view returns (BeaconBlock[] memory blocks) {
        if (endIndex > self.blocks.length) {
            endIndex = self.blocks.length;
        }
        if (startIndex >= endIndex) {
            return new BeaconBlock[](0);
        }
        
        uint256 resultLength = endIndex - startIndex;
        BeaconBlock[] memory result = new BeaconBlock[](resultLength);
        
        for (uint256 i = 0; i < resultLength; i++) {
            result[i] = self.blocks[startIndex + i];
        }
        
        return result;
    }
}
