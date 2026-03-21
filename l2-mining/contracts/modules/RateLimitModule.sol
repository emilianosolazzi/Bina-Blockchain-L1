// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ModuleBase } from "./ModuleBase.sol";
import { RateTypes } from "../RateTypes.sol";
import { IRateLimitModule } from "../interfaces/IRateLimitModule.sol";

contract RateLimitModule is ModuleBase, IRateLimitModule {
    using RateTypes for RateTypes.TokenBucket;
    using RateTypes for RateTypes.SlidingWindow;

    uint16 private constant GLOBAL_WINDOW_SIZE = 1000;
    uint256 private constant DEFAULT_WINDOW_DURATION = 3600;

    mapping(address => RateTypes.TokenBucket) private userRateBuckets;
    RateTypes.TokenBucket private globalRateBucket;
    RateTypes.SlidingWindow private globalWindow;
    RateTypes.RateThresholds private rateThresholds;
    RateTypes.RateStats private rateStats;

    event RateConsumed(address indexed user, bytes32 indexed operation, uint256 cost, uint256 currentRate, uint16 rateBps);
    event RateThresholdsConfigured(
        uint256 warningThreshold,
        uint256 criticalThreshold,
        uint256 throttleThreshold,
        uint256 banThreshold,
        uint256 individualUserLimit,
        uint256 globalLimit
    );

    error InvalidThresholds();
    error RateLimitExceededGlobal();
    error RateLimitExceededUser(uint256 currentRate, uint256 limit);
    error RateLimitThrottled(uint8 reason);

    function initialize(address coreAddress) external {
        __ModuleBase_init(coreAddress);

        RateTypes.initTokenBucket(globalRateBucket, 1200, 10, 1200);
        RateTypes.initSlidingWindow(globalWindow, GLOBAL_WINDOW_SIZE, DEFAULT_WINDOW_DURATION);
        RateTypes.initRateThresholds(rateThresholds, 500, 900);
        rateThresholds.banThreshold = 1000;
        rateThresholds.throttleThreshold = 400;
        rateThresholds.individualUserLimit = 60;
        rateThresholds.globalLimit = 1200;
    }

    function consumeOrRevert(address user, uint256 cost, bytes32 operation) external onlyCoreOrModule whenSystemActive {
        (bool globalAllowed, ) = RateTypes.consumeTokens(globalRateBucket, cost);
        if (!globalAllowed) revert RateLimitExceededGlobal();

        RateTypes.TokenBucket storage userBucket = userRateBuckets[user];
        if (userBucket.capacity == 0) {
            RateTypes.initTokenBucket(
                userBucket,
                rateThresholds.individualUserLimit,
                1,
                rateThresholds.individualUserLimit
            );
        }

        (bool userAllowed, uint256 currentTokens) = RateTypes.consumeTokens(userBucket, cost);
        if (!userAllowed) revert RateLimitExceededUser(currentTokens, userBucket.capacity);

        uint256 currentRate = RateTypes.recordOperation(globalWindow);
        RateTypes.updateRateStats(rateStats, currentRate, rateThresholds);
        (bool shouldThrottle, uint8 reason) = RateTypes.shouldThrottleOperation(rateStats, rateThresholds);
        if (shouldThrottle) revert RateLimitThrottled(reason);

        emit RateConsumed(user, operation, cost, currentRate, rateStats.rateBps);
    }

    // configureThresholds removed — rate limits are set once at initialize(), immutable thereafter.
    // To change thresholds, deploy a new RateLimitModule and register it on Core.

    function getUserCapacity(address user) external view returns (uint256 currentTokens, uint256 capacity) {
        RateTypes.TokenBucket storage bucket = userRateBuckets[user];
        if (bucket.capacity == 0) {
            return (rateThresholds.individualUserLimit, rateThresholds.individualUserLimit);
        }

        uint256 timePassed = block.timestamp - bucket.lastUpdate;
        uint256 newTokens = timePassed * bucket.refillRate;
        currentTokens = bucket.tokens + newTokens;
        if (currentTokens > bucket.capacity) {
            currentTokens = bucket.capacity;
        }
        return (currentTokens, bucket.capacity);
    }

    function getRateStatistics()
        external
        view
        returns (
            uint256 currentRate,
            uint256 averageRate,
            uint256 peakRate,
            uint16 rateBps,
            bool isWarning,
            bool isCritical
        )
    {
        return (
            rateStats.currentRate,
            rateStats.averageRate,
            rateStats.peakRate,
            rateStats.rateBps,
            rateStats.rateExceedsWarning,
            rateStats.rateExceedsCritical
        );
    }
}
