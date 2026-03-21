// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title TokenomicsLib
 * @notice Manages a block-number-anchored emission schedule for the Temporal Gradient Beacon.
 * @dev Emission changes are derived from immutable initialization parameters and current L2 block height.
 */
library TokenomicsLib {
    // Add constants for bounds checking
    uint256 private constant MAX_BLOCKS_PER_EPOCH = 1_000_000;
    uint256 private constant MIN_BLOCKS_PER_EPOCH = 100;
    uint256 private constant MAX_HALVING_INTERVAL = 15_000_000;
    uint256 private constant MIN_HALVING_INTERVAL = 10_000;
    uint256 private constant MIN_REWARD = 1e6;
    uint256 private constant MAX_EPOCHS = type(uint64).max;

    // Add constants for safe math
    uint256 private constant MAX_REDUCTION_ROUNDS = 100; // Prevent infinite loops
    uint256 private constant REDUCTION_NUMERATOR = 65;
    uint256 private constant REDUCTION_DENOMINATOR = 100;

    // Add detailed error types
    error EpochOutOfBounds(uint256 provided, uint256 min, uint256 max);
    error HalvingIntervalOutOfBounds(uint256 provided, uint256 min, uint256 max);
    error RewardTooLow(uint256 provided, uint256 minimum);
    error InvalidInitialState();
    error EpochOverflow();

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
        uint256 epochStartBlock; // Fixed epoch anchor block
        uint256 lastHalvingBlock; // Fixed halving anchor block
        uint256 halvingInterval; // Blocks between halvings
        uint256 rewardAmount; // Current block reward
    }

    /**
     * @notice Initializes the fixed emission schedule.
     * @dev The schedule is anchored to the initialization block and should not be mutated afterward.
     */
    function initializeEpochState(
        EpochState storage state,
        uint256 initialReward,
        uint256 blocksPerEpoch,
        uint256 halvingInterval
    ) internal {
        if (state.blocksPerEpoch != 0 || state.halvingInterval != 0 || state.rewardAmount != 0) {
            revert InvalidInitialState();
        }
        if (blocksPerEpoch < MIN_BLOCKS_PER_EPOCH || blocksPerEpoch > MAX_BLOCKS_PER_EPOCH) {
            revert EpochOutOfBounds(blocksPerEpoch, MIN_BLOCKS_PER_EPOCH, MAX_BLOCKS_PER_EPOCH);
        }
        if (halvingInterval < MIN_HALVING_INTERVAL || halvingInterval > MAX_HALVING_INTERVAL) {
            revert HalvingIntervalOutOfBounds(halvingInterval, MIN_HALVING_INTERVAL, MAX_HALVING_INTERVAL);
        }
        if (initialReward < MIN_REWARD) revert RewardTooLow(initialReward, MIN_REWARD);

        state.currentEpoch = 0;
        state.blocksPerEpoch = blocksPerEpoch;
        state.epochStartBlock = block.number;
        state.lastHalvingBlock = block.number;
        state.halvingInterval = halvingInterval;
        state.rewardAmount = initialReward;
    }

    /**
     * @notice Checks for epoch transitions and updates reward if halving occurs
     * @param state Epoch state (stored in main contract)
     * @return newReward Updated block reward
     */
    function checkEpochTransition(EpochState storage state) internal returns (uint256 newReward) {
        (
            uint256 projectedReward,
            uint256 projectedEpoch,
            uint256 projectedEpochStartBlock,
            uint256 projectedLastHalvingBlock,
            bool epochAdvanced,
            bool halvingOccurred
        ) = _projectState(state);

        newReward = projectedReward;

        if (epochAdvanced || halvingOccurred) {
            state.currentEpoch = projectedEpoch;
            state.epochStartBlock = projectedEpochStartBlock;
            state.lastHalvingBlock = projectedLastHalvingBlock;
            state.rewardAmount = projectedReward;

            emit TokenomicsUpdate(projectedEpoch, projectedReward, block.number, halvingOccurred);
        }

        return newReward;
    }

    /**
     * @notice Computes the current schedule state without mutating storage.
     */
    function previewEpochState(EpochState storage state)
        internal
        view
        returns (
            uint256 currentReward,
            uint256 currentEpoch,
            uint256 nextEpochBlock,
            uint256 nextHalvingBlock
        )
    {
        uint256 projectedEpochStartBlock;
        uint256 projectedLastHalvingBlock;
        bool epochAdvanced;
        bool halvingOccurred;

        (
            currentReward,
            currentEpoch,
            projectedEpochStartBlock,
            projectedLastHalvingBlock,
            epochAdvanced,
            halvingOccurred
        ) = _projectState(state);

        epochAdvanced;
        halvingOccurred;

        nextEpochBlock = projectedEpochStartBlock + state.blocksPerEpoch;
        nextHalvingBlock = projectedLastHalvingBlock + state.halvingInterval;
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
        (currentBlockReward, epoch, , nextHalvingBlock) = previewEpochState(state);
        cap = totalSupplyCap;
        miningAlloc = miningAllocation;
        totalMinedToDate = totalMined;
        remaining = miningAllocation > totalMined ? miningAllocation - totalMined : 0;
    }

    function _projectState(EpochState storage state)
        private
        view
        returns (
            uint256 projectedReward,
            uint256 projectedEpoch,
            uint256 projectedEpochStartBlock,
            uint256 projectedLastHalvingBlock,
            bool epochAdvanced,
            bool halvingOccurred
        )
    {
        if (state.blocksPerEpoch == 0 || state.halvingInterval == 0 || state.rewardAmount < MIN_REWARD) {
            revert InvalidInitialState();
        }

        projectedReward = state.rewardAmount;
        projectedEpoch = state.currentEpoch;
        projectedEpochStartBlock = state.epochStartBlock;
        projectedLastHalvingBlock = state.lastHalvingBlock;

        uint256 blocksSinceEpochStart = block.number - projectedEpochStartBlock;
        if (blocksSinceEpochStart >= state.blocksPerEpoch) {
            uint256 epochsPassed = blocksSinceEpochStart / state.blocksPerEpoch;
            if (projectedEpoch + epochsPassed > MAX_EPOCHS) revert EpochOverflow();

            projectedEpoch += epochsPassed;
            projectedEpochStartBlock += epochsPassed * state.blocksPerEpoch;
            epochAdvanced = true;
        }

        uint256 blocksSinceHalving = block.number - projectedLastHalvingBlock;
        if (blocksSinceHalving >= state.halvingInterval) {
            uint256 intervals = blocksSinceHalving / state.halvingInterval;
            projectedReward = _applyRewardReductions(projectedReward, intervals);
            projectedLastHalvingBlock += intervals * state.halvingInterval;
            halvingOccurred = intervals > 0;
        }
    }

    function _applyRewardReductions(uint256 reward, uint256 intervals) private pure returns (uint256 reducedReward) {
        reducedReward = reward;
        uint256 reductionRounds = intervals > MAX_REDUCTION_ROUNDS ? MAX_REDUCTION_ROUNDS : intervals;

        for (uint256 i = 0; i < reductionRounds; i++) {
            uint256 reduced = (reducedReward * REDUCTION_NUMERATOR) / REDUCTION_DENOMINATOR;
            if (reduced < MIN_REWARD) {
                return MIN_REWARD;
            }
            reducedReward = reduced;
        }
    }
}