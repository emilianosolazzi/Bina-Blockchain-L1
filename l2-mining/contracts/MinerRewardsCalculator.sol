// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { MiningLib } from "./MiningLib.sol";
import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";

/**
 * @title MinerRewardsCalculator
 * @notice Calculates and projects mining rewards for Temporal Gradient miners
 *
 * Fixes applied vs original:
 *   1. blocksPerDay corrected from 5760 (15s Ethereum) to 345,600 (0.25s Arbitrum One)
 *   2. updateParameters() gated with onlyOwner — previously callable by anyone
 *   3. addPool() added with onlyOwner guard
 *   4. estimateReward bonus check fixed — original multiplied target * threshold
 *      which overflows for realistic values; now uses threshold as a divisor
 *   5. projectMiningROI returns (0, 0, 0) on zero daily reward instead of
 *      silently computing nonsense ROI from zero inputs
 *
 * Read-only by design — this contract projects rewards but does not mint.
 * Actual reward distribution is handled by TokenomicsModule.
 */
contract MinerRewardsCalculator is Ownable {

    // ─────────────────────────────────────────────────────────────
    // Constants
    // ─────────────────────────────────────────────────────────────

    uint256 public constant MINING_ALLOCATION = 700_000_000 ether;
    uint256 public constant BASE_WEIGHT       = 1e18;

    /// @notice Arbitrum One produces a block approximately every 0.25 seconds.
    ///         5760 (the original value) assumed 15-second Ethereum blocks —
    ///         60× too slow, making every ROI projection 60× too pessimistic.
    uint256 public constant BLOCKS_PER_DAY = 345_600;

    // ─────────────────────────────────────────────────────────────
    // State
    // ─────────────────────────────────────────────────────────────

    uint256 public targetDifficulty;
    uint256 public rewardAmount;

    /// @notice Divisor applied to targetDifficulty to derive the bonus threshold.
    ///         A solution whose difficulty exceeds (targetDifficulty / bonusThreshold)
    ///         receives the bonus. Example: bonusThreshold = 4 → top 25% solutions.
    uint256 public bonusThreshold;

    /// @notice Bonus multiplier expressed as a percentage (e.g. 125 = 1.25×).
    uint16  public bonusMultiplier;

    uint256 public totalMined;

    mapping(uint8 => MiningLib.MiningPool) public miningPools;
    uint8 public poolCount;

    // ─────────────────────────────────────────────────────────────
    // History
    // ─────────────────────────────────────────────────────────────

    struct RewardSnapshot {
        uint256 timestamp;
        uint256 blockNumber;
        uint256 rewardAmount;
        uint256 totalMined;
        uint256 difficulty;
    }

    RewardSnapshot[] public rewardHistory;

    // ─────────────────────────────────────────────────────────────
    // Events
    // ─────────────────────────────────────────────────────────────

    event ParametersUpdated(
        uint256 targetDifficulty,
        uint256 rewardAmount,
        uint256 bonusThreshold,
        uint16  bonusMultiplier,
        uint256 totalMined,
        uint256 blockNumber
    );

    event PoolAdded(uint8 poolId, uint256 targetDifficulty, uint256 emissionBucket);
    event PoolUpdated(uint8 poolId, bool active);

    // ─────────────────────────────────────────────────────────────
    // Constructor
    // ─────────────────────────────────────────────────────────────

    /**
     * @param _targetDifficulty  Current target difficulty
     * @param _rewardAmount      Base reward per solution (wei)
     * @param _bonusThreshold    Divisor for bonus difficulty threshold (e.g. 4 = top 25%)
     * @param _bonusMultiplier   Bonus multiplier as percentage (e.g. 125 = 1.25×)
     * @param _totalMined        Total tokens mined so far (wei)
     */
    constructor(
        uint256 _targetDifficulty,
        uint256 _rewardAmount,
        uint256 _bonusThreshold,
        uint16  _bonusMultiplier,
        uint256 _totalMined
    ) Ownable(msg.sender) {
        require(_bonusThreshold > 0,    "MRC: bonusThreshold must be > 0");
        require(_bonusMultiplier >= 100, "MRC: bonusMultiplier must be >= 100");

        targetDifficulty = _targetDifficulty;
        rewardAmount     = _rewardAmount;
        bonusThreshold   = _bonusThreshold;
        bonusMultiplier  = _bonusMultiplier;
        totalMined       = _totalMined;

        miningPools[0] = MiningLib.MiningPool({
            targetDifficulty: _targetDifficulty,
            emissionBucket:   MINING_ALLOCATION,
            totalMined:       _totalMined,
            active:           true,
            lastUpdateBlock:  uint64(block.number),
            minerCount:       0
        });
        poolCount = 1;

        rewardHistory.push(RewardSnapshot({
            timestamp:    block.timestamp,
            blockNumber:  block.number,
            rewardAmount: _rewardAmount,
            totalMined:   _totalMined,
            difficulty:   _targetDifficulty
        }));
    }

    // ─────────────────────────────────────────────────────────────
    // Core reward estimation
    // ─────────────────────────────────────────────────────────────

    /**
     * @notice Estimate the reward for a given solution hash in a given pool.
     * @param hmacOutput  The solution hash (bytes32) produced by the miner.
     * @param poolId      Pool to evaluate against.
     * @return reward     Reward amount in wei (may be zero if allocation exhausted).
     * @return isBonus    True if the solution qualifies for the bonus multiplier.
     */
    function estimateReward(
        bytes32 hmacOutput,
        uint8   poolId
    ) external view returns (uint256 reward, bool isBonus) {
        if (poolId >= poolCount || !miningPools[poolId].active) {
            return (0, false);
        }

        // Difficulty = distance of hash from zero (higher = harder solution)
        uint256 difficulty = type(uint256).max - uint256(hmacOutput);

        reward  = rewardAmount;

        // Fix: original code did targetDifficulty * bonusThreshold which overflows.
        // Correct interpretation: bonus if difficulty > targetDifficulty / bonusThreshold
        // i.e. the solution is in the top (1/bonusThreshold) fraction.
        uint256 bonusTarget = miningPools[poolId].targetDifficulty / bonusThreshold;
        if (difficulty > bonusTarget) {
            // bonusMultiplier is a percentage: 125 → multiply by 125, divide by 100
            reward  = (rewardAmount * bonusMultiplier) / 100;
            isBonus = true;
        }

        // Cap at global allocation remaining
        if (totalMined + reward > MINING_ALLOCATION) {
            reward = MINING_ALLOCATION > totalMined
                ? MINING_ALLOCATION - totalMined
                : 0;
        }

        // Cap at pool bucket remaining
        MiningLib.MiningPool storage pool = miningPools[poolId];
        if (pool.totalMined + reward > pool.emissionBucket) {
            reward = pool.emissionBucket > pool.totalMined
                ? pool.emissionBucket - pool.totalMined
                : 0;
        }
    }

    // ─────────────────────────────────────────────────────────────
    // Efficiency & ROI projections
    // ─────────────────────────────────────────────────────────────

    /**
     * @notice Calculate mining efficiency for a miner.
     * @param minerHashrate    Miner's hashrate in H/s.
     * @param networkHashrate  Total network hashrate in H/s.
     * @param blocksPerDay_    Pass 0 to use the protocol constant (345,600).
     *                         Exposed as a parameter so callers can model
     *                         different network conditions.
     * @return dailyReward         Estimated TGBT earned per day (wei).
     * @return monthlyReward       Estimated TGBT earned per month (wei).
     * @return rateInTokensPerHash Tokens per hash, scaled by 1e18.
     */
    function calculateMiningEfficiency(
        uint256 minerHashrate,
        uint256 networkHashrate,
        uint256 blocksPerDay_
    ) external view returns (
        uint256 dailyReward,
        uint256 monthlyReward,
        uint256 rateInTokensPerHash
    ) {
        if (networkHashrate == 0 || minerHashrate == 0) return (0, 0, 0);

        uint256 bpd = blocksPerDay_ > 0 ? blocksPerDay_ : BLOCKS_PER_DAY;

        uint256 minerShare = (minerHashrate * 1e18) / networkHashrate;
        dailyReward        = (bpd * rewardAmount * minerShare) / 1e18;
        monthlyReward      = dailyReward * 30;
        rateInTokensPerHash = targetDifficulty > 0
            ? (rewardAmount * 1e18) / targetDifficulty
            : 0;
    }

    /**
     * @notice Project ROI for a miner given hardware and operating costs.
     *
     * @param hardwareCost      One-time hardware cost in USD cents (e.g. $500 = 50000).
     * @param powerConsumptionW Power draw in watts.
     * @param powerCostCentsKwh Electricity cost in USD cents per kWh (e.g. $0.10 = 10).
     * @param tokenPriceUSDCents Token price in USD cents (e.g. $1.50 = 150).
     * @param minerHashrate     Miner hashrate in H/s.
     * @param networkHashrate   Total network hashrate in H/s.
     *
     * @return roiDays        Days to break even on hardware (0 = unprofitable or no data).
     * @return monthlyRevenue Monthly gross revenue in USD cents.
     * @return monthlyProfit  Monthly net profit after electricity in USD cents.
     *
     * @dev All USD values are in cents to avoid fractional arithmetic in Solidity.
     *      Divide by 100 off-chain for display.
     */
    function projectMiningROI(
        uint256 hardwareCost,
        uint256 powerConsumptionW,
        uint256 powerCostCentsKwh,
        uint256 tokenPriceUSDCents,
        uint256 minerHashrate,
        uint256 networkHashrate
    ) external view returns (
        uint256 roiDays,
        uint256 monthlyRevenue,
        uint256 monthlyProfit
    ) {
        if (networkHashrate == 0 || tokenPriceUSDCents == 0 || minerHashrate == 0) {
            return (0, 0, 0);
        }

        // ── Token rewards ─────────────────────────────────────────
        uint256 minerShare    = (minerHashrate * 1e18) / networkHashrate;
        uint256 dailyReward   = (BLOCKS_PER_DAY * rewardAmount * minerShare) / 1e18;
        uint256 monthlyReward = dailyReward * 30;

        if (monthlyReward == 0) return (0, 0, 0);

        // monthlyReward is in wei (1e18 = 1 TGBT).
        // monthlyRevenue in USD cents:
        //   (tokens_in_wei / 1e18) * tokenPriceUSDCents
        monthlyRevenue = (monthlyReward * tokenPriceUSDCents) / 1e18;

        // ── Power costs ───────────────────────────────────────────
        // kWh per month = watts * hours_per_month / 1000
        // hours_per_month = 24 * 30 = 720
        uint256 monthlyKwh       = (powerConsumptionW * 720) / 1000;
        uint256 monthlyPowerCost = monthlyKwh * powerCostCentsKwh;

        // ── Profit & ROI ──────────────────────────────────────────
        monthlyProfit = monthlyRevenue > monthlyPowerCost
            ? monthlyRevenue - monthlyPowerCost
            : 0;

        if (monthlyProfit == 0) {
            roiDays = 0; // Unprofitable at current parameters
        } else {
            // roiDays = hardwareCost / (monthlyProfit / 30)
            //         = hardwareCost * 30 / monthlyProfit
            roiDays = (hardwareCost * 30) / monthlyProfit;
        }
    }

    // ─────────────────────────────────────────────────────────────
    // Progress statistics
    // ─────────────────────────────────────────────────────────────

    /**
     * @notice Global mining progress against the 700M token allocation.
     * @return percentMinedBps       Percentage mined in basis points (100 = 1%).
     * @return estimatedSecsRemaining Estimated seconds until allocation is exhausted.
     * @return averageBlockReward    Average reward per block over the last history window.
     */
    function getMiningProgressStats() external view returns (
        uint256 percentMinedBps,
        uint256 estimatedSecsRemaining,
        uint256 averageBlockReward
    ) {
        percentMinedBps = (totalMined * 10_000) / MINING_ALLOCATION;

        uint256 histLen = rewardHistory.length;
        if (histLen < 2) {
            return (percentMinedBps, 0, rewardAmount);
        }

        // Use up to the last 30 snapshots for the rate calculation
        uint256 oldIdx    = histLen > 30 ? histLen - 30 : 0;
        RewardSnapshot storage old_    = rewardHistory[oldIdx];
        RewardSnapshot storage recent_ = rewardHistory[histLen - 1];

        uint256 tokensMined   = recent_.totalMined   > old_.totalMined
            ? recent_.totalMined - old_.totalMined : 0;
        uint256 timeElapsed   = recent_.timestamp    > old_.timestamp
            ? recent_.timestamp  - old_.timestamp  : 0;
        uint256 blocksElapsed = recent_.blockNumber  > old_.blockNumber
            ? recent_.blockNumber - old_.blockNumber : 0;

        if (timeElapsed == 0) return (percentMinedBps, 0, rewardAmount);

        // Mining rate: tokens per second
        uint256 miningRate = tokensMined / timeElapsed;
        if (miningRate > 0) {
            uint256 remaining = MINING_ALLOCATION > totalMined
                ? MINING_ALLOCATION - totalMined : 0;
            estimatedSecsRemaining = remaining / miningRate;
        }

        averageBlockReward = blocksElapsed > 0
            ? tokensMined / blocksElapsed
            : rewardAmount;
    }

    // ─────────────────────────────────────────────────────────────
    // Admin — parameter updates
    // ─────────────────────────────────────────────────────────────

    /**
     * @notice Update live beacon parameters.
     * @dev    Fix: was callable by anyone — now onlyOwner.
     *         Should be called after each halving or difficulty adjustment.
     */
    function updateParameters(
        uint256 _targetDifficulty,
        uint256 _rewardAmount,
        uint256 _bonusThreshold,
        uint16  _bonusMultiplier,
        uint256 _totalMined
    ) external onlyOwner {
        require(_bonusThreshold > 0,    "MRC: bonusThreshold must be > 0");
        require(_bonusMultiplier >= 100, "MRC: bonusMultiplier must be >= 100");

        targetDifficulty = _targetDifficulty;
        rewardAmount     = _rewardAmount;
        bonusThreshold   = _bonusThreshold;
        bonusMultiplier  = _bonusMultiplier;
        totalMined       = _totalMined;

        miningPools[0].targetDifficulty = _targetDifficulty;
        miningPools[0].totalMined       = _totalMined;

        rewardHistory.push(RewardSnapshot({
            timestamp:    block.timestamp,
            blockNumber:  block.number,
            rewardAmount: _rewardAmount,
            totalMined:   _totalMined,
            difficulty:   _targetDifficulty
        }));

        emit ParametersUpdated(
            _targetDifficulty,
            _rewardAmount,
            _bonusThreshold,
            _bonusMultiplier,
            _totalMined,
            block.number
        );
    }

    /**
     * @notice Register a new mining pool.
     * @param _targetDifficulty  Pool-specific difficulty target.
     * @param _emissionBucket    Maximum tokens this pool can emit (wei).
     */
    function addPool(
        uint256 _targetDifficulty,
        uint256 _emissionBucket
    ) external onlyOwner {
        require(poolCount < 255, "MRC: max pools reached");
        uint8 newId = poolCount++;
        miningPools[newId] = MiningLib.MiningPool({
            targetDifficulty: _targetDifficulty,
            emissionBucket:   _emissionBucket,
            totalMined:       0,
            active:           true,
            lastUpdateBlock:  uint64(block.number),
            minerCount:       0
        });
        emit PoolAdded(newId, _targetDifficulty, _emissionBucket);
    }

    /**
     * @notice Enable or disable a pool without deleting it.
     */
    function setPoolActive(uint8 poolId, bool active) external onlyOwner {
        require(poolId < poolCount, "MRC: unknown pool");
        miningPools[poolId].active = active;
        emit PoolUpdated(poolId, active);
    }

    // ─────────────────────────────────────────────────────────────
    // View helpers
    // ─────────────────────────────────────────────────────────────

    /// @notice Number of parameter snapshots recorded.
    function rewardHistoryLength() external view returns (uint256) {
        return rewardHistory.length;
    }

    /**
     * @notice Quick sanity check — returns the correct Arbitrum block-per-day constant.
     *         Useful for front-end validation that the deployed contract is the fixed version.
     */
    function blocksPerDayConstant() external pure returns (uint256) {
        return BLOCKS_PER_DAY;
    }
}
