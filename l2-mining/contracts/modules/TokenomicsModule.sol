// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ModuleBase } from "./ModuleBase.sol";
import { ITGBT } from "../interfaces/ITGBT.sol";
import { ITokenomicsModule } from "../interfaces/ITokenomicsModule.sol";
import { TokenomicsLib } from "../TokenomicsLib.sol";
import { Math } from "@openzeppelin/contracts/utils/math/Math.sol";

contract TokenomicsModule is ModuleBase, ITokenomicsModule {
    using TokenomicsLib for TokenomicsLib.EpochState;
    using Math for uint256;

    bytes32 public constant MODULE_MINING = keccak256("MINING_MODULE");
    bytes32 public constant MODULE_BATCH_MINING = keccak256("BATCH_MINING_MODULE");
    bytes32 public constant MODULE_STALE_BLOCK = keccak256("STALE_BLOCK_MODULE");
    uint256 private constant BPS_SCALE = 10_000;

    uint256 public constant TOTAL_SUPPLY_CAP = 2_000_000_000 ether;
    uint256 public constant MINING_ALLOCATION = 700_000_000 ether;
    uint256 public constant STALE_BLOCK_ALLOCATION = 25_000_000 ether;
    uint256 public constant MAX_BONUS_MULTIPLIER = 500;
    uint256 public constant DEFAULT_BONUS_THRESHOLD = 2;
    uint16 public constant DEFAULT_BONUS_MULTIPLIER = 125;

    ITGBT public tgbtToken;
    TokenomicsLib.EpochState internal epochState;
    uint256 public totalMined;
    uint256 public totalStaleRewards;
    uint256 public bonusThreshold;
    uint16 public bonusMultiplier;

    mapping(address => uint256) public lastActivityBlock;
    mapping(address => uint256) public missedContributions;

    event ExceptionalSolution(address indexed miner, uint256 difficulty, uint256 threshold, uint256 multiplier);
    event MissedContributionRecorded(address indexed account, uint256 totalMissedContributions);
    event StaleEntropyRewarded(address indexed recipient, uint256 requestedReward, uint256 actualReward);

    error OnlyMiningModule();
    error ZeroToken();
    error InvalidMultiplier();
    error InvalidThreshold();
    error InitialRewardExceedsAllocation();

    function initialize(
        address coreAddress,
        address tokenAddress,
        uint256 initialReward,
        uint256 blocksPerEpoch,
        uint256 halvingInterval,
        uint256 initialBonusThreshold,
        uint16 initialBonusMultiplier
    ) external {
        __ModuleBase_init(coreAddress);

        if (tokenAddress == address(0)) revert ZeroToken();
        if (initialReward > MINING_ALLOCATION) revert InitialRewardExceedsAllocation();

        tgbtToken = ITGBT(tokenAddress);
        TokenomicsLib.initializeEpochState(epochState, initialReward, blocksPerEpoch, halvingInterval);
        totalMined = 0;
        bonusThreshold = initialBonusThreshold == 0 ? DEFAULT_BONUS_THRESHOLD : initialBonusThreshold;
        bonusMultiplier = initialBonusMultiplier == 0 ? DEFAULT_BONUS_MULTIPLIER : initialBonusMultiplier;

        if (bonusThreshold == 0) revert InvalidThreshold();
        if (bonusMultiplier == 0 || bonusMultiplier > MAX_BONUS_MULTIPLIER) revert InvalidMultiplier();
    }

    function onBlockMined(
        address miner,
        bytes32 output,
        uint8,
        uint256 poolTargetDifficulty,
        uint256 poolTotalMined,
        uint256 poolEmissionBucket
    ) external onlyAuthorizedMiningModule whenSystemActive returns (uint256 reward) {
        epochState.rewardAmount = TokenomicsLib.checkEpochTransition(epochState);
        reward = _calculateReward(output, epochState.rewardAmount, poolTargetDifficulty, poolTotalMined, poolEmissionBucket);

        if (reward > 0) {
            tgbtToken.mint(miner, reward);
            totalMined += reward;
            _updateActivity(miner);
        }
    }

    function recordMissedContribution(address contributor) external onlyCoreOrModule whenSystemActive {
        missedContributions[contributor]++;
        emit MissedContributionRecorded(contributor, missedContributions[contributor]);
    }

    function onStaleBlockReward(address recipient, uint256 requestedReward)
        external
        onlyAuthorizedStaleBlockModule
        whenSystemActive
        returns (uint256 actualReward)
    {
        if (recipient == address(0) || requestedReward == 0) {
            emit StaleEntropyRewarded(recipient, requestedReward, 0);
            return 0;
        }

        uint256 remainingStaleAllocation = STALE_BLOCK_ALLOCATION > totalStaleRewards
            ? STALE_BLOCK_ALLOCATION - totalStaleRewards
            : 0;
        uint256 remainingTotalSupply = TOTAL_SUPPLY_CAP > tgbtToken.totalSupply()
            ? TOTAL_SUPPLY_CAP - tgbtToken.totalSupply()
            : 0;

        actualReward = requestedReward;
        if (actualReward > remainingStaleAllocation) {
            actualReward = remainingStaleAllocation;
        }
        if (actualReward > remainingTotalSupply) {
            actualReward = remainingTotalSupply;
        }

        if (actualReward > 0) {
            tgbtToken.mint(recipient, actualReward);
            totalStaleRewards += actualReward;
            _updateActivity(recipient);
        }

        emit StaleEntropyRewarded(recipient, requestedReward, actualReward);
    }

    function resetMissedContributions(address account) external onlyGovernance {
        missedContributions[account] = 0;
    }

    function getMiningEconomics()
        external
        view
        returns (
            uint256 currentReward,
            uint256 currentEpoch,
            uint256 blocksPerEpoch,
            uint256 halvingInterval,
            uint256 nextHalvingBlock,
            uint256 currentBonusThreshold,
            uint256 currentBonusMultiplier,
            uint256 minedSoFar,
            uint256 remainingAllocation
        )
    {
            (currentReward, currentEpoch, , nextHalvingBlock) = TokenomicsLib.previewEpochState(epochState);

        return (
                currentReward,
                currentEpoch,
            epochState.blocksPerEpoch,
            epochState.halvingInterval,
                nextHalvingBlock,
            bonusThreshold,
            bonusMultiplier,
            totalMined,
            MINING_ALLOCATION > totalMined ? MINING_ALLOCATION - totalMined : 0
        );
    }

    function getTokenomicsInfo()
        external
        view
        returns (
            uint256 cap,
            uint256 miningAlloc,
            uint256 currentBlockReward,
            uint256 epoch,
            uint256 totalMinedToDate,
            uint256 remaining,
            uint256 nextHalvingBlock
        )
    {
        return TokenomicsLib.getTokenomicsInfo(epochState, TOTAL_SUPPLY_CAP, MINING_ALLOCATION, totalMined);
    }

    function getEmissionHealth()
        external
        view
        returns (
            uint256 totalSupplyMinted,
            uint256 capUtilizationBps,
            uint256 miningAllocationUtilizationBps,
            uint256 remainingTotalSupply,
            uint256 remainingMiningAllocation,
            uint256 currentReward,
            uint256 currentEpoch
        )
    {
        (currentReward, currentEpoch, , ) = TokenomicsLib.previewEpochState(epochState);

        totalSupplyMinted = tgbtToken.totalSupply();
        remainingTotalSupply = TOTAL_SUPPLY_CAP > totalSupplyMinted ? TOTAL_SUPPLY_CAP - totalSupplyMinted : 0;
        remainingMiningAllocation = MINING_ALLOCATION > totalMined ? MINING_ALLOCATION - totalMined : 0;
        capUtilizationBps = TOTAL_SUPPLY_CAP == 0 ? 0 : Math.mulDiv(totalSupplyMinted, BPS_SCALE, TOTAL_SUPPLY_CAP);
        miningAllocationUtilizationBps = MINING_ALLOCATION == 0 ? 0 : Math.mulDiv(totalMined, BPS_SCALE, MINING_ALLOCATION);
    }

    function previewBlockReward(
        bytes32 output,
        uint256 poolTargetDifficulty,
        uint256 poolTotalMined,
        uint256 poolEmissionBucket
    )
        external
        view
        returns (
            uint256 currentBaseReward,
            bool bonusEligible,
            uint256 bonusReward,
            uint256 finalReward,
            uint256 remainingMiningAllocation,
            uint256 remainingPoolAllocation
        )
    {
        (currentBaseReward, , , ) = TokenomicsLib.previewEpochState(epochState);

        (
            finalReward,
            ,
            ,
            bonusEligible,
            remainingMiningAllocation,
            remainingPoolAllocation
        ) = _previewReward(output, currentBaseReward, poolTargetDifficulty, poolTotalMined, poolEmissionBucket, totalMined);

        bonusReward = Math.mulDiv(currentBaseReward, bonusMultiplier, 100);
    }

    function getAccountPenaltyState(address account)
        external
        view
        returns (uint256 lastActivity, uint256 missedContributionCount)
    {
        return (lastActivityBlock[account], missedContributions[account]);
    }

    function getStaleRewardHealth()
        external
        view
        returns (
            uint256 rewardedSoFar,
            uint256 remainingAllocation,
            uint256 utilizationBps
        )
    {
        rewardedSoFar = totalStaleRewards;
        remainingAllocation = STALE_BLOCK_ALLOCATION > totalStaleRewards ? STALE_BLOCK_ALLOCATION - totalStaleRewards : 0;
        utilizationBps = STALE_BLOCK_ALLOCATION == 0 ? 0 : Math.mulDiv(totalStaleRewards, BPS_SCALE, STALE_BLOCK_ALLOCATION);
    }

    function onlyMiningModuleAddress() external view returns (address) {
        return _module(MODULE_MINING);
    }

    function onlyBatchMiningModuleAddress() external view returns (address) {
        return _module(MODULE_BATCH_MINING);
    }

    function _calculateReward(
        bytes32 output,
        uint256 baseReward,
        uint256 poolTargetDifficulty,
        uint256 poolTotalMined,
        uint256 poolEmissionBucket
    ) internal returns (uint256 reward) {
        uint256 difficulty;
        uint256 bonusTarget;
        bool bonusEligible;

        (reward, difficulty, bonusTarget, bonusEligible, , ) = _previewReward(
            output,
            baseReward,
            poolTargetDifficulty,
            poolTotalMined,
            poolEmissionBucket,
            totalMined
        );

        if (bonusEligible) {
            emit ExceptionalSolution(msg.sender, difficulty, bonusTarget, bonusMultiplier);
        }
    }

    function _previewReward(
        bytes32 output,
        uint256 baseReward,
        uint256 poolTargetDifficulty,
        uint256 poolTotalMined,
        uint256 poolEmissionBucket,
        uint256 minedSoFar
    )
        internal
        view
        returns (
            uint256 reward,
            uint256 difficulty,
            uint256 bonusTarget,
            bool bonusEligible,
            uint256 remainingMiningAllocation,
            uint256 remainingPoolAllocation
        )
    {
        difficulty = type(uint256).max - uint256(output);
        reward = baseReward;
        bonusTarget = Math.mulDiv(poolTargetDifficulty, bonusThreshold, 1);

        if (difficulty > bonusTarget) {
            bonusEligible = true;
            reward = Math.mulDiv(baseReward, bonusMultiplier, 100);
        }

        remainingMiningAllocation = MINING_ALLOCATION > minedSoFar ? MINING_ALLOCATION - minedSoFar : 0;
        remainingPoolAllocation = poolEmissionBucket > poolTotalMined ? poolEmissionBucket - poolTotalMined : 0;

        if (reward > remainingMiningAllocation) {
            reward = remainingMiningAllocation;
        }

        if (reward > remainingPoolAllocation) {
            reward = remainingPoolAllocation;
        }
    }

    function _updateActivity(address account) internal {
        lastActivityBlock[account] = block.number;
    }

    modifier onlyAuthorizedMiningModule() {
        address miningModule = _module(MODULE_MINING);
        address batchMiningModule = _module(MODULE_BATCH_MINING);
        if (msg.sender != miningModule && msg.sender != batchMiningModule) revert OnlyMiningModule();
        _;
    }

    modifier onlyAuthorizedStaleBlockModule() {
        if (msg.sender != _module(MODULE_STALE_BLOCK)) revert OnlyMiningModule();
        _;
    }
}