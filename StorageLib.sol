// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title StorageLib
 * @notice Library for managing historical block storage in the Temporal Gradient Beacon
 * @dev Extracted from EnhancedTemporalGradientBeacon to reduce contract size
 */
library StorageLib {
    // Events
    event BlockArchived(uint256 indexed blockIndex, bytes32 output, address indexed miner);
    event HistoricalStorageConfigChanged(bool enabled, uint256 maxBlocks);
    
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
    
    /**
     * @notice Archives a new block in the historical storage
     * @param storage_ Historical storage state
     * @param output Block output
     * @param previousOutput Previous block output
     * @param nonce Block nonce
     * @param miner Block miner address
     * @param actualDifficulty Block difficulty value
     * @param reward Mining reward
     * @param timestamp Block timestamp
     * @param poolId Mining pool ID
     * @return blockIndex Index of the archived block
     */
    function archiveBlock(
        HistoricalStorage storage storage_,
        bytes32 output,
        bytes32 previousOutput,
        uint64 nonce,
        address miner,
        uint256 actualDifficulty,
        uint256 reward,
        uint256 timestamp,
        uint256 poolId
    ) internal returns (uint256 blockIndex) {
        // Skip if historical storage is disabled
        if (!storage_.enabled) return 0;
        
        // If we've reached max capacity, remove oldest block (index 0)
        if (storage_.blocks.length >= storage_.maxBlocks && storage_.maxBlocks > 0) {
            // Shift array elements (remove first element)
            for (uint256 i = 0; i < storage_.blocks.length - 1; i++) {
                storage_.blocks[i] = storage_.blocks[i + 1];
            }
            storage_.blocks.pop(); // Remove last duplicate
        }
        
        // Add new block
        storage_.blocks.push(
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
        
        blockIndex = storage_.blocks.length - 1;
        emit BlockArchived(blockIndex, output, miner);
        
        return blockIndex;
    }
    
    /**
     * @notice Configures the historical block storage
     * @param storage_ Historical storage state
     * @param enabled Whether to store historical blocks
     * @param maxBlocks Maximum number of historical blocks to store
     * @param genesisOutput Genesis block output for initialization
     * @param genesisTimestamp Genesis block timestamp
     * @param sender The sender of the transaction
     */
    function configureHistoricalStorage(
        HistoricalStorage storage storage_,
        bool enabled,
        uint256 maxBlocks,
        bytes32 genesisOutput,
        uint256 genesisTimestamp,
        address sender
    ) internal {
        storage_.enabled = enabled;
        storage_.maxBlocks = maxBlocks;
        
        // If enabling and we have no genesis block, add it
        if (enabled && storage_.blocks.length == 0) {
            storage_.blocks.push(
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
    
    /**
     * @notice Gets multiple historical blocks in a range
     * @param storage_ Historical storage state
     * @param startIndex Start index (inclusive)
     * @param endIndex End index (exclusive)
     * @return blocks Array of BeaconBlock structs
     */
    function getHistoricalBlockRange(
        HistoricalStorage storage storage_,
        uint256 startIndex,
        uint256 endIndex
    ) internal view returns (BeaconBlock[] memory blocks) {
        if (endIndex > storage_.blocks.length) {
            endIndex = storage_.blocks.length;
        }
        if (startIndex >= endIndex) {
            return new BeaconBlock[](0);
        }
        
        uint256 resultLength = endIndex - startIndex;
        BeaconBlock[] memory result = new BeaconBlock[](resultLength);
        
        for (uint256 i = 0; i < resultLength; i++) {
            result[i] = storage_.blocks[startIndex + i];
        }
        
        return result;
    }
}
