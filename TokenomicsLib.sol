// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title TokenomicsLib
 * @notice Library for handling tokenomics-related functionality
 */
library TokenomicsLib {
    // Events
    event EpochChanged(uint256 indexed newEpoch, uint256 blockReward);
    event Halving(uint256 indexed epochNumber, uint256 newBlockReward, uint256 blockNumber);
    
    // Errors
    error InvalidEpochParameters();
    
    struct EpochState {
        uint256 currentEpoch;
        uint256 blocksPerEpoch;
        uint256 epochStartBlock;
        uint256 lastHalvingBlock;
        uint256 halvingInterval;
        uint256 rewardAmount;
    }
    
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
    
    function setEpochBlocks(EpochState storage state, uint256 newBlocksPerEpoch) internal {
        if (newBlocksPerEpoch == 0) revert InvalidEpochParameters();
        state.blocksPerEpoch = newBlocksPerEpoch;
    }
    
    function setHalvingInterval(EpochState storage state, uint256 newHalvingInterval) internal {
        if (newHalvingInterval == 0) revert InvalidEpochParameters();
        state.halvingInterval = newHalvingInterval;
    }
}
