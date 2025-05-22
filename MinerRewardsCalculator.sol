// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { MiningLib } from "./MiningLib.sol";
import { TokenomicsLib } from "./TokenomicsLib.sol";
import { ITGBT } from "./interfaces/ITGBT.sol";

/**
 * @title MinerRewardsCalculator
 * @notice Calculates and projects mining rewards for Temporal Gradient miners
 * @dev Uses the same formulas as the core beacon for consistent reward projections
 */
contract MinerRewardsCalculator {
    // Constants from the main beacon
    uint256 public constant MINING_ALLOCATION = 700_000_000 ether;
    uint256 public constant BASE_WEIGHT = 1e18;

    // Current parameters (can be updated)
    uint256 public targetDifficulty;
    uint256 public rewardAmount;
    uint256 public bonusThreshold;
    uint16 public bonusMultiplier;
    uint256 public totalMined;
    
    // Pool parameters map
    mapping(uint8 => MiningLib.MiningPool) public miningPools;
    uint8 public poolCount;
    
    // Historical reward tracking
    struct RewardSnapshot {
        uint256 timestamp;
        uint256 blockNumber;
        uint256 rewardAmount;
        uint256 totalMined;
        uint256 difficulty;
    }
    RewardSnapshot[] public rewardHistory;

    /**
     * @notice Initialize the calculator with current beacon parameters
     * @param _targetDifficulty Current target difficulty
     * @param _rewardAmount Current block reward
     * @param _bonusThreshold Current bonus difficulty threshold
     * @param _bonusMultiplier Current bonus multiplier (percentage)
     * @param _totalMined Total tokens mined so far
     */
    constructor(
        uint256 _targetDifficulty,
        uint256 _rewardAmount,
        uint256 _bonusThreshold,
        uint16 _bonusMultiplier,
        uint256 _totalMined
    ) {
        targetDifficulty = _targetDifficulty;
        rewardAmount = _rewardAmount;
        bonusThreshold = _bonusThreshold;
        bonusMultiplier = _bonusMultiplier;
        totalMined = _totalMined;
        
        // Default pool
        miningPools[0] = MiningLib.MiningPool({
            targetDifficulty: _targetDifficulty,
            emissionBucket: MINING_ALLOCATION,
            totalMined: _totalMined,
            active: true
        });
        poolCount = 1;
        
        // First history entry
        rewardHistory.push(RewardSnapshot({
            timestamp: block.timestamp,
            blockNumber: block.number,
            rewardAmount: _rewardAmount,
            totalMined: _totalMined,
            difficulty: _targetDifficulty
        }));
    }
    
    /**
     * @notice Estimate rewards for a given hash output and pool
     * @param hmacOutput The hash output from mining
     * @param poolId Mining pool ID
     * @return reward Calculated reward amount
     * @return isBonus Whether the reward received a bonus
     */
    function estimateReward(bytes32 hmacOutput, uint8 poolId) external view returns (uint256 reward, bool isBonus) {
        if (poolId >= poolCount || !miningPools[poolId].active) {
            return (0, false);
        }
        
        uint256 difficulty = type(uint256).max - uint256(hmacOutput);
        reward = rewardAmount;
        
        // Check for bonus threshold
        uint256 bonusTarget = miningPools[poolId].targetDifficulty * bonusThreshold;
        if (difficulty > bonusTarget) {
            reward = (rewardAmount * bonusMultiplier) / 100;
            isBonus = true;
        }
        
        // Check emission caps
        if (totalMined + reward > MINING_ALLOCATION) {
            reward = MINING_ALLOCATION > totalMined ? MINING_ALLOCATION - totalMined : 0;
        }
        
        if (miningPools[poolId].totalMined + reward > miningPools[poolId].emissionBucket) {
            reward = miningPools[poolId].emissionBucket > miningPools[poolId].totalMined ? 
                    miningPools[poolId].emissionBucket - miningPools[poolId].totalMined : 0;
        }
        
        return (reward, isBonus);
    }
    
    /**
     * @notice Calculate mining efficiency for a miner
     * @param minerHashrate Estimated hashrate in H/s
     * @param networkHashrate Total network hashrate in H/s
     * @param blocksPerDay Average blocks mined per day
     * @return dailyReward Estimated daily reward
     * @return monthlyReward Estimated monthly reward
     * @return rateInTokensPerHash Tokens per hash calculation
     */
    function calculateMiningEfficiency(
        uint256 minerHashrate,
        uint256 networkHashrate,
        uint256 blocksPerDay
    ) external view returns (uint256 dailyReward, uint256 monthlyReward, uint256 rateInTokensPerHash) {
        if (networkHashrate == 0) return (0, 0, 0);
        
        // Calculate share of network hashrate
        uint256 minerShare = (minerHashrate * 1e18) / networkHashrate;
        
        // Calculate expected rewards per day
        dailyReward = (blocksPerDay * rewardAmount * minerShare) / 1e18;
        monthlyReward = dailyReward * 30;
        
        // Calculate tokens per hash (scaled by 1e18)
        rateInTokensPerHash = (rewardAmount * 1e18) / targetDifficulty;
        
        return (dailyReward, monthlyReward, rateInTokensPerHash);
    }
    
    /**
     * @notice Project mining returns (ROI) based on hardware investment
     * @param hardwareCost Cost of mining hardware in USD
     * @param powerConsumption Power usage in kWh
     * @param powerCost Cost per kWh in USD (scaled by 100, e.g. $0.10 = 10)
     * @param tokenPriceUSD Current token price in USD (scaled by 100, e.g. $1.50 = 150)
     * @param minerHashrate Estimated hashrate in H/s
     * @param networkHashrate Total network hashrate in H/s
     * @return roiDays Days to break even
     * @return monthlyRevenue Monthly revenue in USD
     * @return monthlyProfit Monthly profit after power costs
     */
    function projectMiningROI(
        uint256 hardwareCost,
        uint256 powerConsumption,
        uint256 powerCost,
        uint256 tokenPriceUSD,
        uint256 minerHashrate,
        uint256 networkHashrate
    ) external view returns (
        uint256 roiDays,
        uint256 monthlyRevenue, 
        uint256 monthlyProfit
    ) {
        if (networkHashrate == 0 || tokenPriceUSD == 0) return (0, 0, 0);
        
        // Average 5760 blocks per day (15 second blocks)
        uint256 blocksPerDay = 5760;
        
        // Calculate mining rewards
        uint256 minerShare = (minerHashrate * 1e18) / networkHashrate;
        uint256 dailyReward = (blocksPerDay * rewardAmount * minerShare) / 1e18;
        uint256 monthlyReward = dailyReward * 30;
        
        // Calculate USD values
        monthlyRevenue = (monthlyReward * tokenPriceUSD) / 100;
        
        // Calculate power costs for a month (30 days)
        uint256 monthlyPowerCost = (powerConsumption * 24 * 30 * powerCost) / 100;
        
        // Calculate profit and ROI
        monthlyProfit = monthlyRevenue > monthlyPowerCost ? monthlyRevenue - monthlyPowerCost : 0;
        
        // Calculate days to ROI
        if (monthlyProfit == 0) {
            roiDays = 0; // Impossible ROI
        } else {
            roiDays = (hardwareCost * 30) / monthlyProfit;
        }
        
        return (roiDays, monthlyRevenue, monthlyProfit);
    }
    
    /**
     * @notice Update the calculator with new beacon parameters
     * @dev Should be called periodically to keep estimates accurate
     */
    function updateParameters(
        uint256 _targetDifficulty,
        uint256 _rewardAmount,
        uint256 _bonusThreshold,
        uint16 _bonusMultiplier,
        uint256 _totalMined
    ) external {
        targetDifficulty = _targetDifficulty;
        rewardAmount = _rewardAmount;
        bonusThreshold = _bonusThreshold;
        bonusMultiplier = _bonusMultiplier;
        totalMined = _totalMined;
        
        // Update default pool
        miningPools[0].targetDifficulty = _targetDifficulty;
        miningPools[0].totalMined = _totalMined;
        
        // Add history entry
        rewardHistory.push(RewardSnapshot({
            timestamp: block.timestamp,
            blockNumber: block.number,
            rewardAmount: _rewardAmount,
            totalMined: _totalMined,
            difficulty: _targetDifficulty
        }));
    }

    /**
     * @notice Get mining progress statistics
     * @return percentMined Percentage of total allocation mined
     * @return estimatedTimeRemaining Estimated time until all tokens mined (seconds)
     * @return averageBlockReward Average block reward over the last 30 days
     */
    function getMiningProgressStats() external view returns (
        uint256 percentMined,
        uint256 estimatedTimeRemaining,
        uint256 averageBlockReward
    ) {
        // Calculate percentage mined (in basis points, 1% = 100)
        percentMined = (totalMined * 10000) / MINING_ALLOCATION;
        
        // Calculate rate of mining based on history
        uint256 historyLength = rewardHistory.length;
        if (historyLength < 2) {
            return (percentMined, 0, rewardAmount);
        }
        
        // Use most recent history entries for calculation
        uint256 recentIndex = historyLength > 30 ? historyLength - 30 : 1;
        RewardSnapshot memory oldSnapshot = rewardHistory[recentIndex - 1];
        RewardSnapshot memory recentSnapshot = rewardHistory[historyLength - 1];
        
        // Calculate mining rate (tokens per second)
        uint256 tokensMined = recentSnapshot.totalMined - oldSnapshot.totalMined;
        uint256 timeElapsed = recentSnapshot.timestamp - oldSnapshot.timestamp;
        
        if (timeElapsed == 0) {
            return (percentMined, 0, rewardAmount);
        }
        
        uint256 miningRate = tokensMined / timeElapsed;
        
        // Calculate remaining tokens
        uint256 remainingTokens = MINING_ALLOCATION - totalMined;
        
        // Estimate time remaining based on mining rate
        if (miningRate > 0) {
            estimatedTimeRemaining = remainingTokens / miningRate;
        }
        
        // Calculate average block reward
        uint256 blocksElapsed = recentSnapshot.blockNumber - oldSnapshot.blockNumber;
        if (blocksElapsed > 0) {
            averageBlockReward = tokensMined / blocksElapsed;
        } else {
            averageBlockReward = rewardAmount;
        }
        
        return (percentMined, estimatedTimeRemaining, averageBlockReward);
    }
}

// This component already handles:
// 1. Multiple reward pools (can represent different rights holders)
// 2. Historical reward tracking (parallels royalty payment history)
// 3. Rate calculations (tokens per hash could be adapted to royalties per stream)
// 4. Projection calculations (for forecasting future royalty payments)
