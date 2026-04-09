// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ModuleBase } from "./ModuleBase.sol";
import { ITGBT } from "../interfaces/ITGBT.sol";
import { ITokenomicsModule } from "../interfaces/ITokenomicsModule.sol";
import { TokenomicsLib } from "../TokenomicsLib.sol";
import { Math } from "@openzeppelin/contracts/utils/math/Math.sol";

/**
 * @title TokenomicsModuleV2
 * @notice Drop-in replacement for TokenomicsModule with two fixes:
 *
 *   1. Halving interval corrected for L1 block.number on Arbitrum.
 *      Old value: 252,288,000 (assumed L2 0.25s blocks → ~2yr, actual ~96yr on L1).
 *      New value: set via initialize() — caller passes the correct L1-based interval
 *      (e.g. 7,884,000 for ~3yr or 5,256,000 for ~2yr).
 *
 *   2. Network-wide emission rate limiter.
 *      Tracks solutions per rolling window (e.g. 7,200 L1 blocks ≈ 24h).
 *      When solutions exceed targetSolutionsPerWindow, reward scales down linearly:
 *        effectiveReward = baseReward × target / actual
 *      This prevents uncapped drain regardless of miner count.
 *
 * @dev Interface-compatible with ITokenomicsModule — MiningModule, BatchMiningModule,
 *      and StaleBlockOracle require zero changes.
 */
contract TokenomicsModuleV2 is ModuleBase, ITokenomicsModule {
    using TokenomicsLib for TokenomicsLib.EpochState;
    using Math for uint256;

    // ── Module IDs (unchanged) ──────────────────────────
    bytes32 public constant MODULE_MINING = keccak256("MINING_MODULE");
    bytes32 public constant MODULE_BATCH_MINING = keccak256("BATCH_MINING_MODULE");
    bytes32 public constant MODULE_STALE_BLOCK = keccak256("STALE_BLOCK_MODULE");
    uint256 private constant BPS_SCALE = 10_000;

    // ── Supply constants (unchanged) ────────────────────
    uint256 public constant TOTAL_SUPPLY_CAP = 2_000_000_000 ether;
    uint256 public constant MINING_ALLOCATION = 1_900_000_000 ether;
    uint256 public constant STALE_BLOCK_ALLOCATION = 75_000_000 ether;
    uint256 public constant MAX_BONUS_MULTIPLIER = 500;
    uint256 public constant DEFAULT_BONUS_THRESHOLD = 2;
    uint16 public constant DEFAULT_BONUS_MULTIPLIER = 125;

    // ── Rate limiter constants ──────────────────────────
    /// @notice Minimum allowed window size (1 hour at 12s/block)
    uint256 public constant MIN_WINDOW_BLOCKS = 300;
    /// @notice Maximum allowed window size (7 days at 12s/block)
    uint256 public constant MAX_WINDOW_BLOCKS = 50_400;
    /// @notice Minimum target solutions per window
    uint256 public constant MIN_TARGET_SOLUTIONS = 100;
    /// @notice Maximum target solutions per window
    uint256 public constant MAX_TARGET_SOLUTIONS = 1_000_000;
    /// @notice Floor: reward never drops below 1% of base (prevents zero-reward spam)
    uint256 private constant RATE_LIMIT_FLOOR_BPS = 100; // 1%

    // ── Existing storage (layout-compatible) ────────────
    ITGBT public tgbtToken;
    TokenomicsLib.EpochState internal epochState;
    uint256 public totalMined;
    uint256 public totalStaleRewards;
    uint256 public bonusThreshold;
    uint16 public bonusMultiplier;

    mapping(address => uint256) public lastActivityBlock;
    mapping(address => uint256) public missedContributions;

    // ── New storage: rate limiter ────────────────────────
    /// @notice Maximum solutions per window before reward scaling kicks in
    uint256 public targetSolutionsPerWindow;
    /// @notice Window duration in L1 blocks (e.g. 7,200 ≈ 24h)
    uint256 public windowBlocks;
    /// @notice L1 block number when current window started
    uint256 public windowStartBlock;
    /// @notice Solutions counted in current window
    uint256 public windowSolutions;

    // ── Events ──────────────────────────────────────────
    event ExceptionalSolution(address indexed miner, uint256 difficulty, uint256 threshold, uint256 multiplier);
    event MissedContributionRecorded(address indexed account, uint256 totalMissedContributions);
    event StaleEntropyRewarded(address indexed recipient, uint256 requestedReward, uint256 actualReward);
    event RateLimiterApplied(uint256 windowSolutions, uint256 target, uint256 baseReward, uint256 effectiveReward);

    // ── Errors ──────────────────────────────────────────
    error OnlyMiningModule();
    error OnlyStaleBlockModule();
    error ZeroToken();
    error InvalidMultiplier();
    error InvalidThreshold();
    error InitialRewardExceedsAllocation();
    error InvalidSeededTotalMined();
    error InvalidSeededStaleRewards();
    error InvalidWindowBlocks();
    error InvalidTargetSolutions();

    // ── Initializer ─────────────────────────────────────

    /**
     * @notice One-shot initializer. Replaces constructor for proxy/module pattern.
     * @param coreAddress         Core contract address
     * @param tokenAddress        TGBT token address
     * @param initialReward       Base reward per solution (e.g. 10 ether = 10 TGBT)
     * @param blocksPerEpoch      Blocks per epoch (e.g. 345,600)
     * @param halvingInterval     L1 blocks between halvings (e.g. 7,884,000 ≈ 3yr)
     * @param initialBonusThreshold  Bonus difficulty threshold multiplier (0 → default 2)
     * @param initialBonusMultiplier Bonus reward multiplier in % (0 → default 125)
     * @param initialTotalMined      Seed from old module (wei). Read on-chain before deploy.
     * @param initialTotalStaleRewards Seed from old module (wei). Read on-chain before deploy.
     * @param _targetSolutionsPerWindow  Rate limiter target (e.g. 30,000)
     * @param _windowBlocks              Rate limiter window in L1 blocks (e.g. 7,200 ≈ 24h)
     */
    function initialize(
        address coreAddress,
        address tokenAddress,
        uint256 initialReward,
        uint256 blocksPerEpoch,
        uint256 halvingInterval,
        uint256 initialBonusThreshold,
        uint16 initialBonusMultiplier,
        uint256 initialTotalMined,
        uint256 initialTotalStaleRewards,
        uint256 _targetSolutionsPerWindow,
        uint256 _windowBlocks
    ) external {
        __ModuleBase_init(coreAddress);

        if (tokenAddress == address(0)) revert ZeroToken();
        if (initialReward > MINING_ALLOCATION) revert InitialRewardExceedsAllocation();
        if (initialTotalMined > MINING_ALLOCATION) revert InvalidSeededTotalMined();
        if (initialTotalStaleRewards > STALE_BLOCK_ALLOCATION) revert InvalidSeededStaleRewards();
        if (_windowBlocks < MIN_WINDOW_BLOCKS || _windowBlocks > MAX_WINDOW_BLOCKS) revert InvalidWindowBlocks();
        if (_targetSolutionsPerWindow < MIN_TARGET_SOLUTIONS || _targetSolutionsPerWindow > MAX_TARGET_SOLUTIONS) {
            revert InvalidTargetSolutions();
        }

        tgbtToken = ITGBT(tokenAddress);
        TokenomicsLib.initializeEpochState(epochState, initialReward, blocksPerEpoch, halvingInterval);
        totalMined = initialTotalMined;
        totalStaleRewards = initialTotalStaleRewards;
        bonusThreshold = initialBonusThreshold == 0 ? DEFAULT_BONUS_THRESHOLD : initialBonusThreshold;
        bonusMultiplier = initialBonusMultiplier == 0 ? DEFAULT_BONUS_MULTIPLIER : initialBonusMultiplier;

        if (bonusThreshold == 0) revert InvalidThreshold();
        if (bonusMultiplier == 0 || bonusMultiplier > MAX_BONUS_MULTIPLIER) revert InvalidMultiplier();

        // Rate limiter
        targetSolutionsPerWindow = _targetSolutionsPerWindow;
        windowBlocks = _windowBlocks;
        windowStartBlock = block.number;
        windowSolutions = 0;
    }

    // ── ITokenomicsModule: onBlockMined ─────────────────

    function onBlockMined(
        address miner,
        bytes32 output,
        uint8,
        uint256 poolTargetDifficulty,
        uint256 poolTotalMined,
        uint256 poolEmissionBucket
    ) external onlyAuthorizedMiningModule whenSystemActive returns (uint256 reward) {
        // Advance window if elapsed
        _advanceWindow();
        windowSolutions++;

        uint256 currentReward = TokenomicsLib.checkEpochTransition(epochState);

        // Apply rate limiter: scale down reward when network exceeds target
        uint256 effectiveReward = _applyRateLimiter(currentReward);

        reward = _calculateReward(output, effectiveReward, poolTargetDifficulty, poolTotalMined, poolEmissionBucket);

        if (reward > 0) {
            tgbtToken.mint(miner, reward);
            totalMined += reward;
            _updateActivity(miner);
        }
    }

    // ── ITokenomicsModule: onStaleBlockReward (unchanged) ─

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

    // ── View functions (all unchanged signatures) ───────

    function recordMissedContribution(address contributor) external onlyCoreOrModule whenSystemActive {
        missedContributions[contributor]++;
        emit MissedContributionRecorded(contributor, missedContributions[contributor]);
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

        // Preview includes rate limiter effect
        uint256 effectiveBase = _previewRateLimiter(currentBaseReward);

        (
            finalReward,
            ,
            ,
            bonusEligible,
            remainingMiningAllocation,
            remainingPoolAllocation
        ) = _previewReward(output, effectiveBase, poolTargetDifficulty, poolTotalMined, poolEmissionBucket, totalMined);

        bonusReward = Math.mulDiv(effectiveBase, bonusMultiplier, 100);
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

    /// @notice Rate limiter state for dashboards and monitoring
    function getRateLimiterState()
        external
        view
        returns (
            uint256 _targetSolutionsPerWindow,
            uint256 _windowBlocks,
            uint256 _windowStartBlock,
            uint256 _windowSolutions,
            uint256 _blocksRemainingInWindow,
            uint256 _effectiveRewardBps  // 10_000 = full reward, 5_000 = 50%, etc.
        )
    {
        _targetSolutionsPerWindow = targetSolutionsPerWindow;
        _windowBlocks = windowBlocks;
        _windowStartBlock = windowStartBlock;
        _windowSolutions = windowSolutions;

        // Return actual storage values; caller interprets based on blocksRemaining
        if (block.number >= windowStartBlock + windowBlocks) {
            _blocksRemainingInWindow = 0;
            _effectiveRewardBps = BPS_SCALE; // window elapsed, next call resets
        } else {
            _blocksRemainingInWindow = (windowStartBlock + windowBlocks) - block.number;
            if (_windowSolutions <= targetSolutionsPerWindow) {
                _effectiveRewardBps = BPS_SCALE;
            } else {
                _effectiveRewardBps = Math.mulDiv(targetSolutionsPerWindow, BPS_SCALE, _windowSolutions);
                if (_effectiveRewardBps < RATE_LIMIT_FLOOR_BPS) {
                    _effectiveRewardBps = RATE_LIMIT_FLOOR_BPS;
                }
            }
        }
    }

    function onlyMiningModuleAddress() external view returns (address) {
        return _module(MODULE_MINING);
    }

    function onlyBatchMiningModuleAddress() external view returns (address) {
        return _module(MODULE_BATCH_MINING);
    }

    // ── Internal: rate limiter ──────────────────────────

    /// @dev Reset the window counter if the current window has elapsed
    function _advanceWindow() internal {
        if (block.number >= windowStartBlock + windowBlocks) {
            windowStartBlock = block.number;
            windowSolutions = 0;
        }
    }

    /// @dev Scale down reward when solutions exceed target. Floor at 1% of base.
    function _applyRateLimiter(uint256 baseReward) internal returns (uint256 effectiveReward) {
        if (windowSolutions <= targetSolutionsPerWindow) {
            return baseReward;
        }

        // effectiveReward = baseReward × target / actual
        effectiveReward = Math.mulDiv(baseReward, targetSolutionsPerWindow, windowSolutions);

        // Floor: never below 1% of base to prevent zero-reward griefing
        uint256 floor = Math.mulDiv(baseReward, RATE_LIMIT_FLOOR_BPS, BPS_SCALE);
        if (effectiveReward < floor) {
            effectiveReward = floor;
        }

        emit RateLimiterApplied(windowSolutions, targetSolutionsPerWindow, baseReward, effectiveReward);
    }

    /// @dev Pure view version for preview functions
    function _previewRateLimiter(uint256 baseReward) internal view returns (uint256) {
        // If window elapsed, no scaling (would reset on next mutating call)
        if (block.number >= windowStartBlock + windowBlocks) {
            return baseReward;
        }
        if (windowSolutions <= targetSolutionsPerWindow) {
            return baseReward;
        }
        uint256 effective = Math.mulDiv(baseReward, targetSolutionsPerWindow, windowSolutions);
        uint256 floor = Math.mulDiv(baseReward, RATE_LIMIT_FLOOR_BPS, BPS_SCALE);
        return effective < floor ? floor : effective;
    }

    // ── Internal: reward calculation (unchanged) ────────

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

        return reward;
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

    // ── Access control (unchanged) ──────────────────────

    modifier onlyAuthorizedMiningModule() {
        if (msg.sender != _module(MODULE_MINING)) {
            if (msg.sender != _module(MODULE_BATCH_MINING)) revert OnlyMiningModule();
        }
        _;
    }

    modifier onlyAuthorizedStaleBlockModule() {
        if (msg.sender != _module(MODULE_STALE_BLOCK)) revert OnlyStaleBlockModule();
        _;
    }
}
