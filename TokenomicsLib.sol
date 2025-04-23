// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title TokenomicsLib
 * @notice Library for tokenomics-related functionality in the Temporal Gradient Beacon
 * @dev Extracted from EnhancedTemporalGradientBeacon to reduce contract size
 */
library TokenomicsLib {
    // Errors
    error InvalidEpochParameters();
    
    // Events
    event EpochChanged(uint256 indexed newEpoch, uint256 blockReward);
    event Halving(uint256 indexed epochNumber, uint256 newBlockReward, uint256 blockNumber);

    struct EpochState {
        uint256 currentEpoch;
        uint256 blocksPerEpoch;
        uint256 epochStartBlock;
        uint256 lastHalvingBlock;
        uint256 halvingInterval;
        uint256 rewardAmount;
    }
    
    /**
     * @notice Checks if an epoch transition should occur and updates state
     * @param state Current epoch state
     * @return newReward Updated reward amount after potential halving
     */
    function checkEpochTransition(EpochState storage state) internal returns (uint256 newReward) {
        uint256 blocksSinceEpochStart = block.number - state.epochStartBlock;
        
        if (blocksSinceEpochStart >= state.blocksPerEpoch) {
            uint256 epochsPassed = blocksSinceEpochStart / state.blocksPerEpoch;
            state.currentEpoch += epochsPassed;
            state.epochStartBlock = block.number;

            if (block.number - state.lastHalvingBlock >= state.halvingInterval) {
                state.rewardAmount = state.rewardAmount / 2;
                state.lastHalvingBlock = block.number;
                emit Halving(state.currentEpoch, state.rewardAmount, block.number);
            }

            emit EpochChanged(state.currentEpoch, state.rewardAmount);
        }
        
        return state.rewardAmount;
    }
    
    /**
     * @notice Updates epoch parameters
     * @param state Current epoch state
     * @param newBlocksPerEpoch New blocks per epoch value
     */
    function setEpochBlocks(EpochState storage state, uint256 newBlocksPerEpoch) internal {
        if (newBlocksPerEpoch == 0) revert InvalidEpochParameters();
        state.blocksPerEpoch = newBlocksPerEpoch;
    }
    
    /**
     * @notice Updates halving interval
     * @param state Current epoch state
     * @param newHalvingInterval New halving interval value
     */
    function setHalvingInterval(EpochState storage state, uint256 newHalvingInterval) internal {
        if (newHalvingInterval == 0) revert InvalidEpochParameters();
        state.halvingInterval = newHalvingInterval;
    }
    
    /**
     * @notice Gets tokenomics information
     * @param state Current epoch state
     * @param totalSupplyCap Total token supply cap
     * @param miningAllocation Total mining allocation
     * @param totalMined Total tokens mined so far
     * @return Tuple of tokenomics information
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
