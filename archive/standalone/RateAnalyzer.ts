import { BigInt, log } from '@graphprotocol/graph-ts'

/**
 * Enum for better type safety when describing rate trends
 */
export enum RateTrendType {
  INSUFFICIENT_DATA = 'INSUFFICIENT_DATA',
  RAPIDLY_INCREASING = 'RAPIDLY_INCREASING',
  RAPIDLY_DECREASING = 'RAPIDLY_DECREASING',
  INCREASING = 'INCREASING',
  DECREASING = 'DECREASING',
  REBOUNDING = 'REBOUNDING',
  PEAKING = 'PEAKING',
  STABLE = 'STABLE',
  FLUCTUATING = 'FLUCTUATING',
  SEASONAL = 'SEASONAL',
  CYCLICAL = 'CYCLICAL'
}

/**
 * Enum for violation severity levels
 */
export enum ViolationSeverity {
  NONE = 'NONE',
  INFO = 'INFO',
  WARNING = 'WARNING',
  CRITICAL = 'CRITICAL'
}

/**
 * Enum for suggested actions when limits are exceeded
 */
export enum RateLimitAction {
  ALLOW = 'ALLOW',
  MONITOR = 'MONITOR',
  THROTTLE = 'THROTTLE',
  BLOCK = 'BLOCK'
}

/**
 * @title RateAnalyzer
 * @notice Provides advanced rate analysis and prediction capabilities for Bloom Filter metrics
 * @dev Supports sliding window analysis, rate trending, and anomaly detection
 */
export class RateAnalyzer {
  /**
   * Analyzes rate data to detect anomalies and patterns
   * @param historical Array of historical rate values
   * @param timestamps Array of timestamps for each value
   * @param currentRate Latest rate reading
   * @param threshold Threshold for anomaly detection (percentage deviation)
   * @returns Object containing analysis results
   */
  static analyzeRateTrend(
    historical: number[],
    timestamps: BigInt[],
    currentRate: number,
    threshold: number = 25
  ): RateAnalysisResult {
    // Validate inputs
    if (historical.length < 2 || timestamps.length < 2) {
      return {
        trend: RateTrendType.INSUFFICIENT_DATA,
        anomaly: false,
        percentChange: 0,
        volatility: 0,
        forecast: currentRate,
        zScore: 0,
        confidence: 0,
        timeWeighted: false
      };
    }
    
    // Ensure arrays are of the same length - Fixed for AssemblyScript compatibility
    let minLength = historical.length;
    if (historical.length !== timestamps.length) {
      log.warning('Historical values and timestamps have different lengths', []);
      // Use the shorter length to avoid out of bounds errors
      minLength = historical.length < timestamps.length ? historical.length : timestamps.length;
      
      // Create new arrays of proper length instead of using slice()
      const newHistorical: number[] = [];
      const newTimestamps: BigInt[] = [];
      
      for (let i = 0; i < minLength; i++) {
        newHistorical.push(historical[i]);
        newTimestamps.push(timestamps[i]);
      }
      
      historical = newHistorical;
      timestamps = newTimestamps;
    }
    
    // Calculate moving average and standard deviation
    let sum = 0;
    let squareSum = 0;
    
    for (let i = 0; i < historical.length; i++) {
      sum += historical[i];
      squareSum += historical[i] * historical[i];
    }
    
    const mean = sum / historical.length;
    // Max with 0 to avoid negative variance due to floating point errors
    const variance = Math.max(0, squareSum / historical.length - mean * mean);
    const stdDev = Math.sqrt(variance);
    
    // Calculate Z-score of latest value
    const zScore = stdDev > 0 ? (currentRate - mean) / stdDev : 0;
    
    // Calculate trend with timestamps for better accuracy
    const trend = this.determineTimeWeightedTrend(historical, timestamps, currentRate);
    
    // Calculate percent change from previous value with safety check
    const previousRate = historical[historical.length - 1];
    const percentChange = Math.abs(previousRate) > 0.00001 ? 
      ((currentRate - previousRate) / Math.abs(previousRate)) * 100 : 
      (currentRate > 0 ? 100 : 0); // Handle near-zero previous rate
    
    // Check for anomaly
    const anomaly = Math.abs(zScore) > threshold / 10;
    
    // Calculate volatility (coefficient of variation) with safety check
    const volatility = mean > 0 ? stdDev / mean * 100 : 0;
    
    // Forecast next value using time-weighted analysis
    const forecast = this.timeWeightedForecast(historical, timestamps);
    
    // Calculate confidence based on data quality
    const confidence = this.calculateForecastConfidence(historical, volatility);
    
    // Check for seasonality
    const seasonal = this.detectSeasonality(historical, timestamps);
    
    // If seasonal pattern detected, override the trend
    const finalTrend = seasonal ? RateTrendType.SEASONAL : trend;
    
    return {
      trend: finalTrend,
      anomaly: anomaly,
      percentChange: percentChange,
      volatility: volatility,
      forecast: forecast,
      zScore: zScore,
      confidence: confidence,
      timeWeighted: true
    };
  }
  
  /**
   * Calculates the exponential moving average (EMA)
   * @param previousEMA Previous EMA value
   * @param currentValue Current data point
   * @param alpha Smoothing factor (0 < alpha < 1)
   * @returns New EMA value
   */
  static calculateEMA(previousEMA: number, currentValue: number, alpha: number): number {
    // Validate alpha is in range [0,1]
    const safeAlpha = Math.max(0, Math.min(1, alpha));
    
    // Check for NaN or Infinity - Fixed for AssemblyScript compatibility
    if (isNaN(previousEMA) || previousEMA == Infinity || previousEMA == -Infinity ||
        isNaN(currentValue) || currentValue == Infinity || currentValue == -Infinity) {
      
      // Handle NaN and Infinity correctly
      if (isNaN(previousEMA) || previousEMA == Infinity || previousEMA == -Infinity) {
        if (isNaN(currentValue) || currentValue == Infinity || currentValue == -Infinity) {
          return 0; // Both values are invalid
        }
        return currentValue; // Only previousEMA is invalid
      }
      return previousEMA; // Only currentValue is invalid
    }
    
    return safeAlpha * currentValue + (1 - safeAlpha) * previousEMA;
  }
  
  /**
   * Determines the trend based on historical data and current rate
   * @param historical Historical rate values
   * @param currentRate Current rate value
   * @returns Trend classification
   */
  static determineTrend(historical: number[], currentRate: number): RateTrendType {
    if (historical.length < 3) return RateTrendType.INSUFFICIENT_DATA;
    
    const last = historical[historical.length - 1];
    const secondLast = historical[historical.length - 2];
    const thirdLast = historical[historical.length - 3];
    
    // Normalize changes relative to the magnitude of values
    const normalizationBase = Math.max(0.1, Math.abs(last)); // Avoid division by zero
    
    // Calculate first and second derivatives (normalized)
    const firstDerivative = (last - secondLast) / normalizationBase;
    const secondDerivative = (firstDerivative - ((secondLast - thirdLast) / normalizationBase));
    const currentDerivative = (currentRate - last) / normalizationBase;
    
    // Adaptive threshold based on data volatility
    let volatilityThreshold = 0.05; // Default 5%
    
    if (historical.length > 5) {
      // Calculate recent volatility
      let sumChanges = 0;
      for (let i = historical.length - 5; i < historical.length; i++) {
        if (i > 0) {
          sumChanges += Math.abs((historical[i] - historical[i-1]) / Math.max(0.1, Math.abs(historical[i-1])));
        }
      }
      volatilityThreshold = Math.max(0.02, Math.min(0.20, sumChanges / 5)); // Min 2%, Max 20%
    }
    
    // Strong acceleration upward
    if (currentDerivative > volatilityThreshold && firstDerivative > volatilityThreshold && secondDerivative > 0) {
      return RateTrendType.RAPIDLY_INCREASING;
    }
    
    // Acceleration downward
    if (currentDerivative < -volatilityThreshold && firstDerivative < -volatilityThreshold && secondDerivative < 0) {
      return RateTrendType.RAPIDLY_DECREASING;
    }
    
    // Steady increase
    if (currentDerivative > volatilityThreshold && firstDerivative > volatilityThreshold) {
      return RateTrendType.INCREASING;
    }
    
    // Steady decrease
    if (currentDerivative < -volatilityThreshold && firstDerivative < -volatilityThreshold) {
      return RateTrendType.DECREASING;
    }
    
    // Inflection point upward
    if (currentDerivative > volatilityThreshold && firstDerivative < -volatilityThreshold) {
      return RateTrendType.REBOUNDING;
    }
    
    // Inflection point downward
    if (currentDerivative < -volatilityThreshold && firstDerivative > volatilityThreshold) {
      return RateTrendType.PEAKING;
    }
    
    // No significant change
    if (Math.abs(currentDerivative) < volatilityThreshold) {
      return RateTrendType.STABLE;
    }
    
    // Default case
    return RateTrendType.FLUCTUATING;
  }
  
  /**
   * Determines trend with time-weighted analysis
   * @param historical Historical rate values
   * @param timestamps Array of timestamps for values
   * @param currentRate Current rate value
   * @returns Trend classification
   */
  static determineTimeWeightedTrend(
    historical: number[], 
    timestamps: BigInt[], 
    currentRate: number
  ): RateTrendType {
    if (historical.length < 3 || timestamps.length < 3) return RateTrendType.INSUFFICIENT_DATA;
    
    // Normalize by time intervals
    const last = historical[historical.length - 1];
    const secondLast = historical[historical.length - 2];
    const thirdLast = historical[historical.length - 3];
    
    const lastTimeDiff = timestamps[timestamps.length - 1].minus(timestamps[timestamps.length - 2]).toI32();
    const secondLastTimeDiff = timestamps[timestamps.length - 2].minus(timestamps[timestamps.length - 3]).toI32();
    
    // Time-weight the derivatives (rate of change per time unit)
    const safeLastTimeDiff = Math.max(1, lastTimeDiff);
    const safeSecondLastTimeDiff = Math.max(1, secondLastTimeDiff);
    
    const firstDerivative = (last - secondLast) / safeLastTimeDiff;
    const secondDerivative = (firstDerivative - ((secondLast - thirdLast) / safeSecondLastTimeDiff));
    
    // Apply standard trend logic to time-weighted derivatives
    const historicalTimeWeighted = [thirdLast, secondLast, last];
    return this.determineTrend(historicalTimeWeighted, currentRate);
  }
  
  /**
   * Detects seasonal patterns in rate data
   * @param historical Historical rate values
   * @param timestamps Array of timestamps
   * @returns Whether seasonality was detected
   */
  static detectSeasonality(historical: number[], timestamps: BigInt[]): boolean {
    // Need substantial data to detect seasonality
    if (historical.length < 24 || timestamps.length < 24) return false;
    
    // Convert timestamps to seconds
    const timeIntervals: number[] = [];
    for (let i = 1; i < timestamps.length; i++) {
      timeIntervals.push(timestamps[i].minus(timestamps[i-1]).toI32());
    }
    
    // Check if time intervals are fairly regular (within 20% of mean)
    // Fixed: replaced array.reduce with explicit loop for AssemblyScript
    let sum = 0;
    for (let i = 0; i < timeIntervals.length; i++) {
      sum += timeIntervals[i];
    }
    const meanInterval = sum / timeIntervals.length;
    
    // Fixed: replaced array.every with explicit loop for AssemblyScript
    let regularIntervals = true;
    for (let i = 0; i < timeIntervals.length; i++) {
      if (Math.abs(timeIntervals[i] - meanInterval) >= meanInterval * 0.2) {
        regularIntervals = false;
        break;
      }
    }
    
    if (!regularIntervals) return false;
    
    // Compute autocorrelation at different lags
    const maxLag = Math.min(historical.length / 3, 24);
    let maxAutocorrelation = 0;
    let bestLag = 0;
    
    for (let lag = 2; lag <= maxLag; lag++) {
      let autocorr = this.calculateAutocorrelation(historical, lag);
      if (autocorr > maxAutocorrelation && autocorr > 0.5) { // Significant correlation threshold
        maxAutocorrelation = autocorr;
        bestLag = lag;
      }
    }
    
    // Return true if we found strong seasonality
    return maxAutocorrelation > 0.5 && bestLag > 0;
  }
  
  /**
   * Calculates autocorrelation at specified lag
   * @param data Data series
   * @param lag Lag value
   * @returns Autocorrelation coefficient
   */
  static calculateAutocorrelation(data: number[], lag: number): number {
    if (data.length <= lag) return 0;
    
    // Calculate mean
    const mean = data.reduce((sum, val) => sum + val, 0) / data.length;
    
    // Calculate variance
    let variance = 0;
    for (let i = 0; i < data.length; i++) {
      variance += Math.pow(data[i] - mean, 2);
    }
    
    if (variance === 0) return 0;
    
    // Calculate autocorrelation
    let autocorr = 0;
    for (let i = 0; i < data.length - lag; i++) {
      autocorr += (data[i] - mean) * (data[i + lag] - mean);
    }
    
    return autocorr / variance;
  }
  
  /**
   * Forecasts next value using time-weighted regression
   * @param historical Historical values
   * @param timestamps Timestamps for values
   * @returns Forecasted next value
   */
  static timeWeightedForecast(historical: number[], timestamps: BigInt[]): number {
    if (historical.length < 2 || timestamps.length < 2) return historical.length > 0 ? historical[historical.length - 1] : 0;
    
    // Simple case - just use last value + momentum
    if (historical.length === 2) {
      const lastValue = historical[1];
      const prevValue = historical[0];
      const timeDiff = timestamps[1].minus(timestamps[0]).toI32();
      // Momentum per second
      const momentum = timeDiff > 0 ? (lastValue - prevValue) / timeDiff : 0;
      // Project forward by estimated time interval
      return lastValue + momentum * timeDiff;
    }
    
    // For more data points, use proper time-weighted regression
    return this.forecastRates(historical, timestamps, 1)[0];
  }
  
  /**
   * Calculates confidence level in forecast
   * @param data Historical data
   * @param volatility Data volatility
   * @returns Confidence percentage (0-100)
   */
  static calculateForecastConfidence(data: number[], volatility: number): number {
    // More data and lower volatility = higher confidence
    const dataPoints = Math.min(20, data.length) / 20; // Scale to 0-1 (max benefit at 20 points)
    const volatilityFactor = Math.max(0, 1 - (volatility / 100)); // Lower volatility = higher confidence
    
    // Combine factors and scale to 0-100
    return Math.min(100, Math.max(0, (dataPoints * 0.6 + volatilityFactor * 0.4) * 100));
  }
  
  /**
   * Predicts future rate values with timestamps
   * @param historical Historical rate values
   * @param timestamps Array of timestamps
   * @param periods Number of periods to forecast
   * @returns Array of predicted values
   */
  static forecastRates(
    historical: number[],
    timestamps: BigInt[],
    periods: number = 5
  ): number[] {
    // Validate inputs
    if (historical.length < 2 || timestamps.length < 2) return [];
    if (periods <= 0 || periods > 100) periods = Math.min(Math.max(1, periods), 100);
    
    // Ensure arrays are of equal length
    const n = Math.min(historical.length, timestamps.length);
    
    // Convert BigInt timestamps to seconds since first timestamp
    const baseTime = timestamps[0].toI64();
    const timeValues: number[] = [];
    for (let i = 0; i < n; i++) {
      timeValues.push(Number(timestamps[i].toI64() - baseTime));
    }
    
    // Calculate weighted means - more recent data gets higher weight
    let sumX = 0, sumY = 0, sumXY = 0, sumX2 = 0, totalWeight = 0;
    for (let i = 0; i < n; i++) {
      // Exponential weighting - newer data gets exponentially more weight
      const weight = Math.exp((i - n + 1) * 0.1); // Simple decay factor
      
      sumX += timeValues[i] * weight;
      sumY += historical[i] * weight;
      sumXY += timeValues[i] * historical[i] * weight;
      sumX2 += timeValues[i] * timeValues[i] * weight;
      totalWeight += weight;
    }
    
    const weightedMeanX = sumX / totalWeight;
    const weightedMeanY = sumY / totalWeight;
    
    // Calculate slope and intercept with safety checks
    let slope = 0, intercept = weightedMeanY;
    const denominator = sumX2 - (sumX * sumX) / totalWeight;
    
    if (Math.abs(denominator) > 1e-10) { // Avoid division by near-zero
      slope = (sumXY - (sumX * sumY) / totalWeight) / denominator;
      intercept = weightedMeanY - slope * weightedMeanX;
    }
    
    // Generate forecast with non-negative check
    const forecast: number[] = [];
    const lastTimestamp = timeValues[timeValues.length - 1];
    const avgInterval = (timeValues[timeValues.length - 1] - timeValues[0]) / (n - 1);
    
    for (let i = 0; i < periods; i++) {
      const forecastTime = lastTimestamp + ((i + 1) * avgInterval);
      const value = Math.max(0, intercept + slope * forecastTime); // Ensure non-negative
      forecast.push(value);
    }
    
    return forecast;
  }
  
  /**
   * Detects rate limit violations with enhanced metrics
   * @param currentRate Current rate value
   * @param rateLimit Maximum allowed rate
   * @param burstLimit Short-term burst allowance
   * @param windowSize Time window in seconds
   * @returns Enhanced violation result with detailed metrics
   */
  static detectRateLimitViolation(
    currentRate: number,
    rateLimit: number,
    burstLimit: number,
    windowSize: number = 60
  ): RateLimitViolationResult {
    // Validate inputs
    if (rateLimit <= 0) rateLimit = 1; // Prevent division by zero
    if (burstLimit <= rateLimit) burstLimit = rateLimit * 1.5; // Ensure burst > base
    if (windowSize <= 0) windowSize = 60; // Default window
    
    currentRate = Math.max(0, currentRate); // Ensure non-negative
    
    // Multiple threshold levels for graduated response
    const warningLevel = rateLimit * 0.8; // 80% of limit
    const exceedsLimit = currentRate > rateLimit;
    const exceedsBurst = currentRate > burstLimit;
    const approachingLimit = currentRate > warningLevel && currentRate <= rateLimit;
    
    // Calculate intensity of violation
    const violationIntensity = exceedsLimit ? 
      Math.min(100, ((currentRate - rateLimit) / rateLimit) * 100) : 
      approachingLimit ?
        Math.min(100, ((currentRate - warningLevel) / (rateLimit - warningLevel)) * 50) : 0;
    
    // Calculate utilization percentage
    const utilizationPct = Math.min(100, (currentRate / rateLimit) * 100);
    
    // Determine appropriate action and severity
    let severity: ViolationSeverity;
    let action: RateLimitAction;
    
    if (exceedsBurst) {
      severity = ViolationSeverity.CRITICAL;
      action = RateLimitAction.BLOCK;
    } else if (exceedsLimit) {
      severity = ViolationSeverity.WARNING;
      action = RateLimitAction.THROTTLE;
    } else if (approachingLimit) {
      severity = ViolationSeverity.INFO;
      action = RateLimitAction.MONITOR;
    } else {
      severity = ViolationSeverity.NONE;
      action = RateLimitAction.ALLOW;
    }
    
    return {
      violated: exceedsLimit,
      violationSeverity: severity,
      violationIntensity: violationIntensity,
      suggestedAction: action,
      utilizationPct: utilizationPct,
      remainingCapacity: Math.max(0, rateLimit - currentRate),
      windowSizeSeconds: windowSize
    };
  }
  
  /**
   * Smooths a rate series to reduce noise
   * @param data Array of rate values
   * @param smoothingFactor Smoothing intensity (0-1)
   * @returns Smoothed data series
   */
  static smoothRates(data: number[], smoothingFactor: number = 0.3): number[] {
    if (data.length <= 2) {
      // Fixed: replaced spread operator with explicit copy for AssemblyScript
      const result: number[] = [];
      for (let i = 0; i < data.length; i++) {
        result.push(data[i]);
      }
      return result;
    }
    
    // Validate smoothing factor
    const alpha = Math.max(0, Math.min(1, smoothingFactor));
    
    // Apply exponential smoothing
    const result: number[] = [];
    result.push(data[0]); // First point unchanged
    
    for (let i = 1; i < data.length; i++) {
      result.push(alpha * data[i] + (1 - alpha) * result[i-1]);
    }
    
    return result;
  }
  
  /**
   * Fix for missing isFinite function in some AssemblyScript environments
   * @param value Value to check
   * @returns Whether the value is finite
   */
  static isFiniteValue(value: number): boolean {
    return !isNaN(value) && value != Infinity && value != -Infinity;
  }
}

/**
 * Enhanced interface for rate analysis results
 */
export interface RateAnalysisResult {
  trend: RateTrendType;      // Rate trend indicator (enum)
  anomaly: boolean;          // Whether an anomaly was detected
  percentChange: number;     // Percent change from previous measurement
  volatility: number;        // Rate volatility (coefficient of variation)
  forecast: number;          // Forecasted next value
  zScore: number;            // Standard deviation distance from mean
  confidence: number;        // Confidence level in forecast (0-100)
  timeWeighted: boolean;     // Whether time weighting was applied
}

/**
 * Enhanced interface for rate limit violation checks
 */
export interface RateLimitViolationResult {
  violated: boolean;             // Whether rate limit was violated
  violationSeverity: ViolationSeverity; // Severity level (enum)
  violationIntensity: number;    // Percent over limit (0-100)
  suggestedAction: RateLimitAction; // Recommended action (enum)
  utilizationPct: number;        // Percentage of limit utilized
  remainingCapacity: number;     // Remaining capacity before limit
  windowSizeSeconds: number;     // Window size in seconds
}
