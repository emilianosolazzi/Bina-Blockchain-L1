/**
 * @title RateMonitor
 * @notice Integration layer between RateAnalyzer and RateVisualizer
 * @dev Coordinates data flow between analysis and visualization components
 *      Optional for miners but provides valuable mining optimization insights
 */
class RateMonitor {
  /**
   * Creates a new RateMonitor instance
   * @param {Object} options Configuration options
   * @param {string} options.containerId DOM element ID for visualization
   * @param {Object} options.analyzerOptions Options for RateAnalyzer
   * @param {Object} options.visualizerOptions Options for RateVisualizer
   */
  constructor(options = {}) {
    // Default options
    this.options = Object.assign({
      containerId: 'rate-chart',
      pollInterval: 5000,
      historyLength: 100,
      alertThreshold: 25, // Default anomaly threshold
      analyzerOptions: {},
      visualizerOptions: {
        rateUnit: 'ops/sec',
        showMovingAverage: true,
        darkMode: false
      }
    }, options);
    
    // Initialize components
    this.visualizer = new RateVisualizer(
      this.options.containerId, 
      this.options.visualizerOptions
    );
    
    // Historical data for analysis
    this.historicalValues = [];
    this.historicalTimestamps = [];
    
    // Rate tracking state
    this.lastRate = 0;
    this.maxRate = 0;
    this.minRate = Infinity;
    this.isPolling = false;
  }
  
  /**
   * Starts monitoring with automatic polling
   * @param {Function} dataFetcher Function that returns latest rate value
   */
  startMonitoring(dataFetcher) {
    if (this.isPolling) return;
    this.isPolling = true;
    this.dataFetcher = dataFetcher;
    
    // Initialize visualizer
    this.visualizer.initialize();
    
    // Start polling
    this._poll();
    this.pollInterval = setInterval(() => this._poll(), this.options.pollInterval);
    
    console.log('Rate monitoring started');
  }
  
  /**
   * Stops the monitoring process
   */
  stopMonitoring() {
    if (this.pollInterval) {
      clearInterval(this.pollInterval);
      this.isPolling = false;
      console.log('Rate monitoring stopped');
    }
    
    // Clean up visualizer to prevent memory leaks
    this.visualizer.destroy();
  }
  
  /**
   * Polls for new data and updates both analyzer and visualizer
   * @private
   */
  async _poll() {
    try {
      // Fetch latest rate data
      const currentRate = await this.dataFetcher();
      const timestamp = new Date();
      
      // Update historical data (limited by historyLength)
      this.historicalValues.push(currentRate);
      this.historicalTimestamps.push(this._convertToGraphQLBigInt(timestamp));
      
      // Limit history length
      if (this.historicalValues.length > this.options.historyLength) {
        this.historicalValues.shift();
        this.historicalTimestamps.shift();
      }
      
      // Update stats
      this.lastRate = currentRate;
      this.maxRate = Math.max(this.maxRate, currentRate);
      this.minRate = Math.min(this.minRate, currentRate);
      
      // Only analyze if we have enough data
      if (this.historicalValues.length >= 2) {
        // Perform rate analysis using RateAnalyzer
        const analysis = RateAnalyzer.analyzeRateTrend(
          this.historicalValues,
          this.historicalTimestamps,
          currentRate,
          this.options.alertThreshold
        );
        
        // Check for violations
        const limitViolation = RateAnalyzer.detectRateLimitViolation(
          currentRate,
          this.options.visualizerOptions.thresholds?.warning || 70,
          this.options.visualizerOptions.thresholds?.critical || 90
        );
        
        // Update threshold values based on violations
        if (limitViolation.violationSeverity === 'CRITICAL') {
          this._triggerAlert('Critical rate violation detected!', 'critical');
        } else if (limitViolation.violationSeverity === 'WARNING') {
          this._triggerAlert('Rate warning threshold exceeded', 'warning');
        }
        
        // Update visualization with new data point
        this.visualizer.update({
          timestamp: timestamp,
          rate: currentRate,
          trend: analysis.trend,
          anomaly: analysis.anomaly
        });
        
        // If anomaly detected, highlight in visualization
        if (analysis.anomaly) {
          this.visualizer.addAnnotation(
            { timestamp, rate: currentRate },
            `Anomaly: ${analysis.percentChange.toFixed(1)}% change`,
            'anomaly'
          );
          this._triggerAlert(`Rate anomaly detected: ${analysis.percentChange.toFixed(1)}% change`, 'info');
        }
        
        // Update forecast visualization
        if (this.historicalValues.length >= 4) {
          const forecast = RateAnalyzer.forecastRates(this.historicalValues, 5);
          this.visualizer.renderForecast(forecast);
        }
        
        // Optional: Log analysis results for debugging
        console.debug('Rate analysis:', analysis);
      }
      
    } catch (error) {
      console.error('Error in rate monitoring:', error);
      this._triggerAlert(`Error monitoring rates: ${error.message}`, 'error');
    }
  }
  
  /**
   * Triggers a UI alert for important events
   * @private
   */
  _triggerAlert(message, level = 'info') {
    // You can implement your own alert system here
    console.log(`[${level.toUpperCase()}] ${message}`);
    
    // If UI alert component available
    if (window.alertSystem) {
      window.alertSystem.showAlert(message, level);
    }
  }
  
  /**
   * Converts a JavaScript Date to a BigInt format compatible with GraphQL
   * @private
   */
  _convertToGraphQLBigInt(date) {
    // Mock the BigInt type used in GraphQL (@graphprotocol/graph-ts)
    return {
      toI64: () => Math.floor(date.getTime() / 1000),
      toI32: () => Math.floor(date.getTime() / 1000),
      toString: () => Math.floor(date.getTime() / 1000).toString()
    };
  }
  
  /**
   * Exports the current data for external use
   */
  exportData() {
    return {
      currentRate: this.lastRate,
      maxRate: this.maxRate,
      minRate: this.minRate,
      historicalValues: [...this.historicalValues],
      timestamps: this.historicalTimestamps.map(t => new Date(t.toI64() * 1000))
    };
  }

  /**
   * Adds mining-specific metrics tracking
   * @param {Object} miningStats Current mining statistics from blockchain
   */
  addMiningMetrics(miningStats) {
    if (!miningStats) return;
    
    // Track mining-specific rates
    this.miningStats = {
      successRate: miningStats.successRate || 0,
      rewardRate: miningStats.rewardRate || 0,
      networkDifficulty: miningStats.difficulty || 0,
      lastReward: miningStats.lastReward || 0,
      gasEfficiency: miningStats.gasEfficiency || 0
    };
    
    // Update visualizer with mining context if available
    if (this.visualizer && this.isPolling) {
      this.visualizer.updateThresholds(
        // Adjust warning threshold based on network difficulty
        this.adjustThresholdForDifficulty(
          this.options.visualizerOptions.thresholds?.warning || 70,
          this.miningStats.networkDifficulty
        ),
        // Keep critical threshold fixed
        this.options.visualizerOptions.thresholds?.critical || 90
      );
    }
  }
  
  /**
   * Adjusts threshold based on current network difficulty
   * Helps miners optimize their submission rate
   * @private
   */
  adjustThresholdForDifficulty(baseThreshold, difficulty) {
    // Higher difficulty = lower optimal submission rate
    const difficultyFactor = difficulty > 0 ? 
      Math.log10(difficulty) / 10 : 1;
    
    // Apply diminishing adjustment as difficulty increases
    return Math.max(30, baseThreshold * (1 - (difficultyFactor * 0.2)));
  }

  /**
   * Calculates optimal mining strategy based on current rates
   * @returns {Object} Optimization recommendations
   */
  calculateMiningStrategy() {
    if (this.historicalValues.length < 10) return null;
    
    // Calculate optimal submission interval based on rate analysis
    const analysis = RateAnalyzer.analyzeRateTrend(
      this.historicalValues,
      this.historicalTimestamps,
      this.lastRate,
      this.options.alertThreshold
    );
    
    return {
      recommendedSubmitInterval: this._calculateOptimalInterval(analysis),
      gasOptimizationTip: this._determineGasStrategy(analysis.trend),
      currentEfficiency: this.miningStats?.gasEfficiency || 0,
      trend: analysis.trend,
      forecast: RateAnalyzer.forecastRates(this.historicalValues, 1)[0]
    };
  }
}

// Example usage:
/*
// Initialize the rate monitor
const monitor = new RateMonitor({
  containerId: 'rate-chart',
  analyzerOptions: {
    // Analyzer specific options
  },
  visualizerOptions: {
    thresholds: {
      warning: 80,
      critical: 95
    },
    darkMode: true
  }
});

// Start monitoring with a data fetch function
monitor.startMonitoring(async () => {
  // This would be replaced with actual API calls to your backend
  // or blockchain data source that provides rate information
  const response = await fetch('/api/current-rate');
  const data = await response.json();
  return data.rate; // Returns current rate value
});

// Stop monitoring when done
// monitor.stopMonitoring();
*/
