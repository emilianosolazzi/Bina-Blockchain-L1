// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Math } from "@openzeppelin/contracts/utils/math/Math.sol";
import { SafeCast } from "@openzeppelin/contracts/utils/math/SafeCast.sol";

/**
 * @title TokenomicsLib
 * @notice Manages tokenomics for the Temporal Gradient Beacon, including epochs and reward halving
 * @dev Library extracted to reduce EnhancedTemporalGradientBeacon contract size
 */
library TokenomicsLib {
    using Math for uint256;
    using SafeCast for uint256;

    // Add constants for bounds checking
    uint256 private constant MAX_BLOCKS_PER_EPOCH = 1_000_000;
    uint256 private constant MIN_BLOCKS_PER_EPOCH = 100;
    uint256 private constant MAX_HALVING_INTERVAL = 10_000_000;
    uint256 private constant MIN_HALVING_INTERVAL = 10_000;
    uint256 private constant MIN_REWARD = 1e6;
    uint256 private constant MAX_EPOCHS = type(uint64).max;

    // Add constants for safe math
    uint256 private constant MAX_REDUCTION_ROUNDS = 100; // Prevent infinite loops
    uint256 private constant REDUCTION_NUMERATOR = 65;
    uint256 private constant REDUCTION_DENOMINATOR = 100;

    // Add detailed error types
    error InvalidEpochParameters();
    error EpochOutOfBounds(uint256 provided, uint256 min, uint256 max);
    error HalvingIntervalOutOfBounds(uint256 provided, uint256 min, uint256 max);
    error RewardTooLow(uint256 provided, uint256 minimum);
    error InvalidInitialState();
    error EpochOverflow();

    // Add events for parameter changes
    event EpochBlocksUpdated(uint256 oldValue, uint256 newValue);
    event HalvingIntervalUpdated(uint256 oldValue, uint256 newValue);

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
        if (state.blocksPerEpoch == 0 || state.halvingInterval == 0) revert InvalidInitialState();
        
        // Safe subtraction (will revert on underflow)
        uint256 blocksSinceEpochStart = block.number - state.epochStartBlock;
        newReward = state.rewardAmount;

        if (blocksSinceEpochStart >= state.blocksPerEpoch) {
            // Safe division (reverts on divide by zero, but we checked above)
            uint256 epochsPassed = blocksSinceEpochStart / state.blocksPerEpoch;
            
            // Explicit overflow check
            if (state.currentEpoch + epochsPassed > MAX_EPOCHS) revert EpochOverflow();
            
            unchecked {
                // Safe after overflow check
                state.currentEpoch += epochsPassed;
            }
            
            state.epochStartBlock = block.number;

            // Safe subtraction (will revert on underflow)
            uint256 blocksSinceHalving = block.number - state.lastHalvingBlock;
            uint256 intervals = blocksSinceHalving / state.halvingInterval;

            if (intervals > 0) {
                // Limit reduction rounds
                uint256 reductionRounds = intervals > MAX_REDUCTION_ROUNDS ? MAX_REDUCTION_ROUNDS : intervals;
                
                unchecked {
                    // Reduction can't underflow due to MIN_REWARD check
                    for (uint256 i = 0; i < reductionRounds; i++) {
                        uint256 reduced = (newReward * REDUCTION_NUMERATOR) / REDUCTION_DENOMINATOR;
                        if (reduced < MIN_REWARD) {
                            newReward = MIN_REWARD;
                            break;
                        }
                        newReward = reduced;
                    }
                    
                    // Safe after bounds check
                    state.lastHalvingBlock += intervals * state.halvingInterval;
                }
                state.rewardAmount = newReward;
            }

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
        if (newBlocksPerEpoch < MIN_BLOCKS_PER_EPOCH || newBlocksPerEpoch > MAX_BLOCKS_PER_EPOCH) {
            revert EpochOutOfBounds(newBlocksPerEpoch, MIN_BLOCKS_PER_EPOCH, MAX_BLOCKS_PER_EPOCH);
        }
        uint256 oldValue = state.blocksPerEpoch;
        state.blocksPerEpoch = newBlocksPerEpoch;
        emit EpochBlocksUpdated(oldValue, newBlocksPerEpoch);
    }

    /**
     * @notice Updates halving interval
     * @param state Epoch state
     * @param newHalvingInterval New halving interval (non-zero)
     */
    function setHalvingInterval(EpochState storage state, uint256 newHalvingInterval) internal {
        if (newHalvingInterval < MIN_HALVING_INTERVAL || newHalvingInterval > MAX_HALVING_INTERVAL) {
            revert HalvingIntervalOutOfBounds(newHalvingInterval, MIN_HALVING_INTERVAL, MAX_HALVING_INTERVAL);
        }
        uint256 oldValue = state.halvingInterval;
        state.halvingInterval = newHalvingInterval;
        emit HalvingIntervalUpdated(oldValue, newHalvingInterval);
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
        cap = totalSupplyCap;
        miningAlloc = miningAllocation;
        currentBlockReward = state.rewardAmount;
        epoch = state.currentEpoch;
        totalMinedToDate = totalMined;
        remaining = miningAllocation > totalMined ? miningAllocation - totalMined : 0;
        nextHalvingBlock = state.lastHalvingBlock + state.halvingInterval;
    }
}