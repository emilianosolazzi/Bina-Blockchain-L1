// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Math } from "@openzeppelin/contracts/utils/math/Math.sol";

/**
 * @title RateTypes
 * @notice Gas-optimised library for rate management on L2 (Arbitrum).
 * @dev    Replaces the 1000-slot sliding-window with a lightweight
 *         epoch-counter that costs O(1) storage reads/writes.
 */
library RateTypes {
    // ── Token bucket ─────────────────────────────────────────
    struct TokenBucket {
        uint256 tokens;
        uint256 capacity;
        uint256 refillRate;     // tokens per second
        uint256 lastUpdate;
    }

    // ── Epoch-counter sliding window (O(1) gas) ──────────────
    //    Replaces the old uint64[] timestamps array that cost
    //    2M+ gas to iterate.
    struct SlidingWindow {
        uint256 operationCount;   // ops in current window
        uint256 windowStart;      // timestamp of current window start
        uint16  windowSize;       // kept for ABI compat (not iterated)
        uint256 windowDuration;   // seconds
    }

    struct RateThresholds {
        uint256 warningThreshold;
        uint256 criticalThreshold;
        uint256 banThreshold;
        uint256 throttleThreshold;
        uint256 individualUserLimit;
        uint256 globalLimit;
    }

    struct RateStats {
        uint256 currentRate;
        uint256 peakRate;
        uint256 averageRate;
        uint256 lastCalculated;
        uint8   trendIndicator;   // 0=stable 1=up 2=down
        uint16  rateBps;
        bool    rateExceedsWarning;
        bool    rateExceedsCritical;
    }

    // ── Errors ───────────────────────────────────────────────
    error ZeroCapacity();
    error ZeroRefillRate();
    error ZeroWindowSize();
    error ZeroWindowDuration();
    error WindowNotInitialized();

    // ── Token bucket ─────────────────────────────────────────
    function initTokenBucket(
        TokenBucket storage bucket,
        uint256 _capacity,
        uint256 _refillRate,
        uint256 initialTokens
    ) internal {
        if (_capacity == 0) revert ZeroCapacity();
        if (_refillRate == 0) revert ZeroRefillRate();

        bucket.capacity   = _capacity;
        bucket.refillRate  = _refillRate;
        bucket.tokens      = initialTokens > 0 ? Math.min(initialTokens, _capacity) : _capacity;
        bucket.lastUpdate  = block.timestamp;
    }

    // ── Sliding window (epoch-counter) ───────────────────────
    function initSlidingWindow(
        SlidingWindow storage window,
        uint16 _windowSize,
        uint256 _windowDuration
    ) internal {
        if (_windowSize == 0) revert ZeroWindowSize();
        if (_windowDuration == 0) revert ZeroWindowDuration();

        window.windowSize      = _windowSize;
        window.windowDuration  = _windowDuration;
        window.operationCount  = 0;
        window.windowStart     = block.timestamp;
    }

    function initRateThresholds(
        RateThresholds storage thresholds,
        uint256 _warningThreshold,
        uint256 _criticalThreshold
    ) internal {
        thresholds.warningThreshold    = _warningThreshold;
        thresholds.criticalThreshold   = _criticalThreshold;
        thresholds.banThreshold        = _criticalThreshold * 2;
        thresholds.throttleThreshold   = _warningThreshold;
        thresholds.individualUserLimit = _warningThreshold / 10;
        thresholds.globalLimit         = _criticalThreshold;
    }

    // ── Token consumption ────────────────────────────────────
    function consumeTokens(
        TokenBucket storage bucket,
        uint256 cost
    ) internal returns (bool allowed, uint256 remainingTokens) {
        uint256 timePassed;
        uint256 newTokens;
        uint256 current;
        unchecked {
            timePassed = block.timestamp - bucket.lastUpdate;
            newTokens  = timePassed * bucket.refillRate;
        }
        current = bucket.tokens + newTokens;
        if (current > bucket.capacity) current = bucket.capacity;

        bucket.tokens     = current;
        bucket.lastUpdate = block.timestamp;

        if (current >= cost) {
            unchecked { bucket.tokens = current - cost; }
            return (true, bucket.tokens);
        }
        return (false, current);
    }

    // ── Record operation (O(1)) ──────────────────────────────
    function recordOperation(
        SlidingWindow storage window
    ) internal returns (uint256 operationCount) {
        uint256 elapsed;
        unchecked { elapsed = block.timestamp - window.windowStart; }

        if (elapsed >= window.windowDuration) {
            // New window — reset counter
            window.operationCount = 1;
            window.windowStart    = block.timestamp;
            return 1;
        }
        unchecked { window.operationCount++; }
        return window.operationCount;
    }

    function calculateCurrentRate(
        SlidingWindow storage window
    ) internal view returns (uint256) {
        uint256 elapsed;
        unchecked { elapsed = block.timestamp - window.windowStart; }
        if (elapsed >= window.windowDuration) return 0;
        return window.operationCount;
    }

    // ── Rate stats ───────────────────────────────────────────
    function updateRateStats(
        RateStats storage stats,
        uint256 newRate,
        RateThresholds storage thresholds
    ) internal {
        uint256 _currentRate = stats.currentRate;

        if (newRate > stats.peakRate)   stats.peakRate = newRate;
        stats.trendIndicator  = newRate > _currentRate ? 1 : (newRate < _currentRate ? 2 : 0);
        stats.rateExceedsWarning  = newRate >= thresholds.warningThreshold;
        stats.rateExceedsCritical = newRate >= thresholds.criticalThreshold;

        if (thresholds.globalLimit > 0) {
            stats.rateBps = uint16(Math.mulDiv(newRate, 10_000, thresholds.globalLimit));
        }

        stats.averageRate = stats.lastCalculated == 0
            ? newRate
            : (stats.averageRate * 8 + newRate * 2) / 10;

        stats.currentRate    = newRate;
        stats.lastCalculated = block.timestamp;
    }

    function shouldThrottleOperation(
        RateStats storage stats,
        RateThresholds storage thresholds
    ) internal view returns (bool shouldThrottle, uint8 throttleReason) {
        uint256 _rate = stats.currentRate;
        if (_rate >= thresholds.banThreshold)      return (true, 3);
        if (_rate >= thresholds.criticalThreshold) return (true, 2);
        if (_rate >= thresholds.throttleThreshold) return (true, 1);
        return (false, 0);
    }
}
