// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title TokenomicsLib
 * @notice Manages tokenomics for the Temporal Gradient Beacon, including epochs and reward halving
 * @dev Library extracted to reduce EnhancedTemporalGradientBeacon contract size
 */
library TokenomicsLib {
    /// @notice Thrown when epoch or halving parameters are invalid (e.g., zero)
    error InvalidEpochParameters();

    /// @notice Emitted on epoch changes or halving events
    /// @param epochNumber Current epoch number
    /// @param blockReward Updated block reward
    /// @param blockNumber Block number of the update
    /// @param isHalving True if a halving occurred
    event TokenomicsUpdate(
        uint256 indexed epochNumber,
        uint256 blockReward,
        uint256 blockNumber,
        bool isHalving
    );

    /// @notice Stores tokenomics state
    struct EpochState {
        uint256 currentEpoch; // Current epoch number
        uint256 blocksPerEpoch; // Blocks per epoch
        uint256 epochStartBlock; // Block when current epoch started
        uint256 lastHalvingBlock; // Block of last halving
        uint256 halvingInterval; // Blocks between halvings
        uint256 rewardAmount; // Current block reward
    }

    /**
     * @notice Checks for epoch transitions and updates reward if halving occurs
     * @param state Epoch state (stored in main contract)
     * @return newReward Updated block reward
     */
    function checkEpochTransition(EpochState storage state) internal returns (uint256 newReward) {
        uint256 blocksSinceEpochStart = block.number - state.epochStartBlock;
        newReward = state.rewardAmount;

        if (blocksSinceEpochStart >= state.blocksPerEpoch) {
            uint256 epochsPassed = blocksSinceEpochStart / state.blocksPerEpoch;
            state.currentEpoch += epochsPassed;
            state.epochStartBlock = block.number;

            // Calculate how many halving intervals (in blocks) have passed
            uint256 intervals = (block.number - state.lastHalvingBlock) / state.halvingInterval;

            // Apply a 35% reduction (i.e. keep 65%) each time one interval has passed
            if (intervals > 0) {
                for (uint256 i = 0; i < intervals; i++) {
                    newReward = (newReward * 65) / 100;
                }
                state.rewardAmount = newReward;
                state.lastHalvingBlock += intervals * state.halvingInterval;
            }

            // Emit event with isHalving=true if any halving occurred
            emit TokenomicsUpdate(state.currentEpoch, newReward, block.number, intervals > 0);
        }

        return newReward;
    }

    /**
     * @notice Updates blocks per epoch
     * @param state Epoch state
     * @param newBlocksPerEpoch New blocks per epoch (non-zero)
     */
    function setEpochBlocks(EpochState storage state, uint256 newBlocksPerEpoch) internal {
        if (newBlocksPerEpoch == 0) revert InvalidEpochParameters();
        state.blocksPerEpoch = newBlocksPerEpoch;
    }

    /**
     * @notice Updates halving interval
     * @param state Epoch state
     * @param newHalvingInterval New halving interval (non-zero)
     */
    function setHalvingInterval(EpochState storage state, uint256 newHalvingInterval) internal {
        if (newHalvingInterval == 0) revert InvalidEpochParameters();
        state.halvingInterval = newHalvingInterval;
    }

    /**
     * @notice Retrieves tokenomics information
     * @param state Epoch state
     * @param totalSupplyCap Total token supply cap
     * @param miningAllocation Total mining allocation
     * @param totalMined Tokens mined so far
     * @return cap Total supply cap
     * @return miningAlloc Mining allocation
     * @return currentBlockReward Current block reward
     * @return epoch Current epoch number
     * @return totalMinedToDate Total tokens mined
     * @return remaining Tokens remaining in mining allocation
     * @return nextHalvingBlock Next halving block number
     */
    function getTokenomicsInfo(
        EpochState storage state,
        uint256 totalSupplyCap,
        uint256 miningAllocation,
        uint256 totalMined
    ) internal view returns (
        uint256 cap,
        uint256 miningAlloc,
        uint256 currentBlockReward,
        uint256 epoch,
        uint256 totalMinedToDate,
        uint256 remaining,
        uint256 nextHalvingBlock
    ) {
        return (
            totalSupplyCap,
            miningAllocation,
            state.rewardAmount,
            state.currentEpoch,
            totalMined,
            miningAllocation - totalMined,
            state.lastHalvingBlock + state.halvingInterval
        );
    }
}