const axios = require('axios');
const { Logger } = require('../utils/Logger');
const { PrometheusMetrics } = require('../metrics/PrometheusMetrics');

/**
 * Validates KYC status for randomness requests
 * @class KYCValidator
 */
class KYCValidator {
  /**
   * Creates a new KYCValidator instance
   * @param {Object} options Configuration options
   * @param {String} options.apiEndpoint KYC API endpoint
   * @param {String} options.apiKey API key for authentication
   * @param {Number} options.cacheTimeSeconds Time to cache results (seconds)
   * @param {Boolean} options.strictMode Whether to require KYC strictly
   * @param {Set<String>} options.exemptIds Set of IDs exempt from KYC checks
   */
  constructor(options = {}) {
    this.apiEndpoint = options.apiEndpoint || 'https://kyc-api.example.com/v1/verify';
    this.apiKey = options.apiKey || '';
    this.cacheTimeSeconds = options.cacheTimeSeconds || 3600; // 1 hour default
    this.strictMode = options.strictMode !== false; // Default to strict mode
    this.exemptIds = options.exemptIds || new Set();
    
    // Cache of verified KYC status: Map<userId, {verified: boolean, timestamp: number}>
    this.kycCache = new Map();
    
    // Initialize metrics
    this.metrics = new PrometheusMetrics();
    this.initMetrics();
    
    Logger.info('KYCValidator initialized');
  }
  
  /**
   * Initialize Prometheus metrics
   * @private
   */
  initMetrics() {
    this.metrics.registerCounter('kyc_checks_total', 'Total KYC verification checks');
    this.metrics.registerCounter('kyc_approvals_total', 'Total KYC approvals');
    this.metrics.registerCounter('kyc_rejections_total', 'Total KYC rejections');
    this.metrics.registerCounter('kyc_cache_hits_total', 'KYC cache hits');
    this.metrics.registerCounter('kyc_failures_total', 'KYC verification failures');
    this.metrics.registerGauge('kyc_cache_size', 'Number of cached KYC results');
    this.metrics.registerHistogram('kyc_verification_duration_ms', 'KYC verification duration in milliseconds');
  }
  
  /**
   * Check if a user has valid KYC verification
   * @param {String} userId User ID to check
   * @param {Object} options Additional options
   * @param {Boolean} options.bypassCache Whether to bypass the cache
   * @returns {Promise<Boolean>} Whether the user is KYC verified
   */
  async checkKYCStatus(userId, options = {}) {
    const startTime = Date.now();
    this.metrics.incrementCounter('kyc_checks_total');
    
    try {
      // Check for exempt IDs (e.g., known trusted entities)
      if (this.exemptIds.has(userId)) {
        Logger.debug(`User ${userId} is exempt from KYC checks`);
        return true;
      }
      
      // Check cache unless bypassing
      if (!options.bypassCache) {
        const cached = this.kycCache.get(userId);
        if (cached && Date.now() - cached.timestamp < this.cacheTimeSeconds * 1000) {
          this.metrics.incrementCounter('kyc_cache_hits_total');
          return cached.verified;
        }
      }
      
      // Make API call to KYC provider
      const result = await this.callKycApi(userId);
      
      // Cache the result
      this.kycCache.set(userId, {
        verified: result,
        timestamp: Date.now()
      });
      
      // Update cache size metric
      this.metrics.setGauge('kyc_cache_size', this.kycCache.size);
      
      // Record verification time
      this.metrics.observeHistogram(
        'kyc_verification_duration_ms', 
        Date.now() - startTime
      );
      
      // Increment appropriate counter
      if (result) {
        this.metrics.incrementCounter('kyc_approvals_total');
      } else {
        this.metrics.incrementCounter('kyc_rejections_total');
      }
      
      return result;
      
    } catch (error) {
      Logger.error(`KYC verification failed for ${userId}:`, error);
      this.metrics.incrementCounter('kyc_failures_total');
      
      // In strict mode, any failure means rejection
      if (this.strictMode) {
        return false;
      }
      
      // In non-strict mode, reuse cached value if available
      const cached = this.kycCache.get(userId);
      if (cached) {
        Logger.warn(`Using cached KYC result for ${userId} due to API failure`);
        return cached.verified;
      }
      
      // Default to rejection if no cached value
      return false;
    }
  }
  
  /**
   * Call the KYC API to verify a user
   * @private
   * @param {String} userId User ID to verify
   * @returns {Promise<Boolean>} Whether user passed KYC
   */
  async callKycApi(userId) {
    try {
      // For security, avoid logging the full user ID
      const partialId = userId.substring(0, 4) + '...' + 
        (userId.length > 8 ? userId.substring(userId.length - 4) : '');
      
      Logger.debug(`Calling KYC API for user ${partialId}`);
      
      const response = await axios({
        method: 'post',
        url: this.apiEndpoint,
        headers: {
          'Authorization': `Bearer ${this.apiKey}`,
          'Content-Type': 'application/json'
        },
        data: {
          userId,
          timestamp: Date.now(),
          service: 'entropy-randomness'
        },
        timeout: 5000 // 5 second timeout
      });
      
      // Check response
      if (response.status === 200 && response.data) {
        Logger.debug(`KYC result for ${partialId}: ${response.data.verified}`);
        return !!response.data.verified;
      } else {
        Logger.warn(`Unexpected KYC API response for ${partialId}: ${response.status}`);
        return false;
      }
      
    } catch (error) {
      // Specific error handling based on error type
      if (error.response) {
        // API responded with error status
        Logger.error(`KYC API error (${error.response.status}): ${error.response.data}`);
      } else if (error.request) {
        // No response received
        Logger.error('KYC API timeout or network error');
      } else {
        // Other errors
        Logger.error('KYC verification error:', error.message);
      }
      
      throw error;
    }
  }
  
  /**
   * Clear expired cache entries
   * @public
   * @returns {Number} Number of entries cleared
   */
  clearExpiredCache() {
    const now = Date.now();
    const expiredTime = now - (this.cacheTimeSeconds * 1000);
    let cleared = 0;
    
    for (const [userId, data] of this.kycCache.entries()) {
      if (data.timestamp < expiredTime) {
        this.kycCache.delete(userId);
        cleared++;
      }
    }
    
    // Update cache size metric
    this.metrics.setGauge('kyc_cache_size', this.kycCache.size);
    
    return cleared;
  }
  
  /**
   * Add a user ID to the exemption list
   * @param {String} userId User ID to exempt from KYC checks
   */
  addExemption(userId) {
    this.exemptIds.add(userId);
  }
  
  /**
   * Remove a user ID from the exemption list
   * @param {String} userId User ID to remove from exemptions
   * @returns {Boolean} Whether the ID was in the exemption list
   */
  removeExemption(userId) {
    return this.exemptIds.delete(userId);
  }
}

module.exports = { KYCValidator };
