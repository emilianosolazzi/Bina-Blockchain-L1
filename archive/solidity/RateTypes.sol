// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title RateTypes
 * @notice Library defining data structures and functions for rate management
 * @dev Provides reusable types for rate limiting, tracking, and analysis
 */
library RateTypes {
    /// @notice Structure for token bucket rate limiter
    struct TokenBucket {
        uint256 tokens;          // Current number of tokens in bucket
        uint256 capacity;        // Maximum token capacity
        uint256 refillRate;      // Tokens refilled per second
        uint256 lastUpdate;      // Last timestamp bucket was updated
    }
    
    /// @notice Structure for sliding window rate tracking
    struct SlidingWindow {
        uint64[] timestamps;     // Array of operation timestamps
        uint16 windowSize;       // Maximum operations to track
        uint16 currentIndex;     // Current position in circular buffer - Fixed from uint8 to uint16 to match windowSize
        uint256 windowDuration;  // Time window duration in seconds
    }
    
    /// @notice Structure for holding rate thresholds
    struct RateThresholds {
        uint256 warningThreshold;    // Warning level threshold
        uint256 criticalThreshold;   // Critical level threshold
        uint256 banThreshold;        // Threshold for auto-banning
        uint256 throttleThreshold;   // Threshold for throttling
        uint256 individualUserLimit; // Per-user rate limit
        uint256 globalLimit;         // Global system rate limit
    }
    
    /// @notice Data structure for storing rate stats
    struct RateStats {
        uint256 currentRate;      // Current rate (ops per period)
        uint256 peakRate;         // Highest observed rate
        uint256 averageRate;      // Average rate over time
        uint256 lastCalculated;   // When rate was last calculated
        uint8 trendIndicator;     // 0=stable, 1=increasing, 2=decreasing
        uint16 rateBps;           // Rate as basis points of capacity
        bool rateExceedsWarning;  // Whether rate exceeds warning threshold
        bool rateExceedsCritical; // Whether rate exceeds critical threshold
    }
    
    /**
     * @notice Initializes a token bucket with specified parameters
     * @param bucket The token bucket to initialize
     * @param _capacity Maximum capacity of the bucket
     * @param _refillRate Number of tokens refilled per second
     * @param initialTokens Starting number of tokens (defaults to full capacity)
     */
    function initTokenBucket(
        TokenBucket storage bucket, 
        uint256 _capacity, 
        uint256 _refillRate,
        uint256 initialTokens
    ) internal {
        require(_capacity > 0, "Capacity must be greater than 0");
        require(_refillRate > 0, "Refill rate must be greater than 0");
        
        bucket.capacity = _capacity;
        bucket.refillRate = _refillRate;
        bucket.tokens = initialTokens > 0 ? Math.min(initialTokens, _capacity) : _capacity;
        bucket.lastUpdate = block.timestamp;
    }
    
    /**
     * @notice Initializes a sliding window rate tracker
     * @param window The sliding window to initialize
     * @param _windowSize Maximum operations to track
     * @param _windowDuration Time window duration in seconds
     */
    function initSlidingWindow(
        SlidingWindow storage window, 
        uint16 _windowSize,
        uint256 _windowDuration
    ) internal {
        require(_windowSize > 0, "Window size must be greater than 0");
        require(_windowDuration > 0, "Window duration must be greater than 0");
        
        // Initialize timestamps array with proper size
        delete window.timestamps;
        window.timestamps = new uint64[](_windowSize);
        
        window.windowSize = _windowSize;
        window.currentIndex = 0;
        window.windowDuration = _windowDuration;
    }
    
    /**
     * @notice Initializes rate thresholds with default or provided values
     * @param thresholds The rate thresholds to initialize
     * @param _warningThreshold Warning level threshold
     * @param _criticalThreshold Critical level threshold
     */
    function initRateThresholds(
        RateThresholds storage thresholds,
        uint256 _warningThreshold,
        uint256 _criticalThreshold
    ) internal {
        require(_warningThreshold < _criticalThreshold, "Warning threshold must be less than critical threshold");
        
        thresholds.warningThreshold = _warningThreshold;
        thresholds.criticalThreshold = _criticalThreshold;
        thresholds.banThreshold = _criticalThreshold * 2; // Default to 2x critical
        thresholds.throttleThreshold = _warningThreshold; // Default to warning level
        thresholds.individualUserLimit = _warningThreshold / 10; // Default to 1/10 of warning
        thresholds.globalLimit = _criticalThreshold; // Default to critical level
    }
    
    /**
     * @notice Updates a token bucket and checks if operation is allowed
     * @param bucket The token bucket to update
     * @param cost Number of tokens required for this operation
     * @return allowed Whether the operation is allowed
     * @return remainingTokens Tokens remaining after operation
     */
    function consumeTokens(TokenBucket storage bucket, uint256 cost) internal returns (bool allowed, uint256 remainingTokens) {
        // Update bucket based on time elapsed
        uint256 timePassed = block.timestamp - bucket.lastUpdate;
        uint256 newTokens = timePassed * bucket.refillRate;
        bucket.tokens = Math.min(bucket.capacity, bucket.tokens + newTokens);
        bucket.lastUpdate = block.timestamp;
        
        // Check if operation is allowed
        if (bucket.tokens >= cost) {
            bucket.tokens -= cost;
            return (true, bucket.tokens);
        } else {
            return (false, bucket.tokens);
        }
    }
    
    /**
     * @notice Records an operation timestamp in a sliding window
     * @dev Maintains a circular buffer of timestamps for rate calculation
     * @param window The sliding window to update
     * @return operationCount Operations in the current window
     */
    function recordOperation(SlidingWindow storage window) internal returns (uint256 operationCount) {
        // Ensure window is initialized
        require(window.timestamps.length > 0, "Window not initialized");
        require(window.timestamps.length == window.windowSize, "Window size mismatch");
        
        // Add current timestamp to the window
        window.timestamps[window.currentIndex] = uint64(block.timestamp);
        
        // Update index for next operation
        window.currentIndex = (window.currentIndex + 1) % window.windowSize;
        
        // Count operations within the window duration
        uint256 windowStart = block.timestamp - window.windowDuration;
        uint256 count = 0;
        
        for (uint16 i = 0; i < window.windowSize; i++) {
            if (window.timestamps[i] > windowStart) {
                count++;
            }
        }
        
        return count;
    }
    
    /**
     * @notice Calculates the current rate based on a sliding window
     * @param window The sliding window to analyze
     * @return currentRate Operations per window duration
     */
    function calculateCurrentRate(SlidingWindow storage window) internal view returns (uint256 currentRate) {
        // Ensure window is initialized
        require(window.timestamps.length > 0, "Window not initialized");
        require(window.timestamps.length == window.windowSize, "Window size mismatch");
        
        uint256 windowStart = block.timestamp - window.windowDuration;
        uint256 count = 0;
        
        for (uint16 i = 0; i < window.windowSize; i++) {
            if (window.timestamps[i] > windowStart) {
                count++;
            }
        }
        
        return count;
    }
    
    /**
     * @notice Updates rate stats based on a new rate measurement
     * @param stats Rate stats to update
     * @param newRate The newly measured rate
     * @param thresholds Thresholds for warning/critical levels
     */
    function updateRateStats(
        RateStats storage stats, 
        uint256 newRate, 
        RateThresholds storage thresholds
    ) internal {
        // Update peak rate if this is a new peak
        if (newRate > stats.peakRate) {
            stats.peakRate = newRate;
        }
        
        // Update trend indicator
        if (newRate > stats.currentRate) {
            stats.trendIndicator = 1; // increasing
        } else if (newRate < stats.currentRate) {
            stats.trendIndicator = 2; // decreasing
        } else {
            stats.trendIndicator = 0; // stable
        }
        
        // Update threshold checks
        stats.rateExceedsWarning = newRate >= thresholds.warningThreshold;
        stats.rateExceedsCritical = newRate >= thresholds.criticalThreshold;
        
        // Calculate rate as basis points of capacity (assuming global limit is capacity)
        if (thresholds.globalLimit > 0) {
            stats.rateBps = uint16((newRate * 10000) / thresholds.globalLimit);
        }
        
        // Update average rate using exponential moving average (EMA)
        // If this is the first update, set average = current
        if (stats.lastCalculated == 0) {
            stats.averageRate = newRate;
        } else {
            // Apply simple EMA with 0.2 weight for new value
            stats.averageRate = (stats.averageRate * 8 + newRate * 2) / 10;
        }
        
        // Update current rate and timestamp
        stats.currentRate = newRate;
        stats.lastCalculated = block.timestamp;
    }
    
    /**
     * @notice Checks if an operation should be throttled based on rate thresholds
     * @param stats Current rate statistics
     * @param thresholds Rate thresholds for decision making
     * @return shouldThrottle Whether the operation should be throttled
     * @return throttleReason Reason code for throttling (0=none, 1=warning, 2=critical)
     */
    function shouldThrottleOperation(
        RateStats storage stats, 
        RateThresholds storage thresholds
    ) internal view returns (bool shouldThrottle, uint8 throttleReason) {
        if (stats.currentRate >= thresholds.banThreshold) {
            return (true, 3); // Ban threshold exceeded
        } else if (stats.currentRate >= thresholds.criticalThreshold) {
            return (true, 2); // Critical threshold exceeded
        } else if (stats.currentRate >= thresholds.throttleThreshold) {
            return (true, 1); // Throttle threshold exceeded
        }
        return (false, 0); // No throttling needed
    }
}

// Helper library for math operations
library Math {
    function min(uint256 a, uint256 b) internal pure returns (uint256) {
        return a < b ? a : b;
    }
    
    function max(uint256 a, uint256 b) internal pure returns (uint256) {
        return a > b ? a : b;
    }
}
