const { EventEmitter } = require('events');
const { PrometheusMetrics } = require('../metrics/PrometheusMetrics');
const { Logger } = require('../utils/Logger');

/**
 * Manages entropy randomness service sharding with backpressure handling and failover
 * @class ShardManager
 */
class ShardManager extends EventEmitter {
  /**
   * Creates a new ShardManager instance
   * @param {Object} options Configuration options
   * @param {Array<String>} options.shardIds List of available shard IDs
   * @param {Number} options.maxShardLoad Maximum load factor before triggering backpressure (0-1)
   * @param {Number} options.failoverThreshold Load threshold that triggers failover (0-1)
   * @param {Number} options.statusCheckIntervalMs Interval for status checking in ms
   * @param {Number} options.recoveryTimeMs Time before allowing a shard to recover after overload
   */
  constructor(options = {}) {
    super();
    
    // Configuration
    this.shardIds = options.shardIds || ['shard-1', 'shard-2', 'shard-3'];
    this.maxShardLoad = options.maxShardLoad || 0.8;    // 80% max load
    this.failoverThreshold = options.failoverThreshold || 0.95; // 95% triggers failover
    this.statusCheckIntervalMs = options.statusCheckIntervalMs || 5000; // Check every 5 seconds
    this.recoveryTimeMs = options.recoveryTimeMs || 60000; // 1 minute recovery time
    
    // State tracking
    this.shardStatus = new Map(); // Maps shard ID to status object
    this.overloadedShards = new Set();  // Set of currently overloaded shards
    this.recoveringShards = new Map();  // Maps shard ID to recovery timeout 
    this.primaryShard = this.shardIds[0]; // Default primary shard
    this.currentShardIndex = 0; // Current shard for round-robin
    
    // Initialize shard status
    this.shardIds.forEach(shardId => {
      this.shardStatus.set(shardId, {
        id: shardId,
        load: 0,
        requestsPerSecond: 0,
        pendingRequests: 0,
        lastUpdated: Date.now(),
        healthy: true,
        active: true,
        metrics: {
          totalRequests: 0,
          successfulRequests: 0,
          failedRequests: 0,
          averageResponseTimeMs: 0
        }
      });
    });
    
    // Initialize metrics
    this.metrics = new PrometheusMetrics();
    this.initMetrics();
    
    // Start status monitoring
    this.statusCheckInterval = setInterval(() => this.checkShardsStatus(), this.statusCheckIntervalMs);
    
    Logger.info(`ShardManager initialized with ${this.shardIds.length} shards`);
  }
  
  /**
   * Initialize Prometheus metrics
   */
  initMetrics() {
    this.metrics.registerGauge('shard_load', 'Current load factor of a shard', ['shard_id']);
    this.metrics.registerGauge('shard_requests_per_second', 'Requests per second for a shard', ['shard_id']);
    this.metrics.registerGauge('shard_pending_requests', 'Pending requests on a shard', ['shard_id']);
    this.metrics.registerGauge('shard_health_status', 'Health status of a shard (1=healthy, 0=unhealthy)', ['shard_id']);
    this.metrics.registerCounter('shard_failovers_total', 'Total number of shard failovers');
    this.metrics.registerCounter('shard_recoveries_total', 'Total number of shard recoveries');
  }
  
  /**
   * Reports the current status of a shard
   * @param {String} shardId The shard ID
   * @param {Object} status Current shard status
   * @param {Number} status.load Current load factor (0-1)
   * @param {Number} status.requestsPerSecond Current RPS
   * @param {Number} status.pendingRequests Current pending requests
   * @param {Boolean} status.healthy Whether the shard is healthy
   */
  reportShardStatus(shardId, status) {
    if (!this.shardStatus.has(shardId)) {
      Logger.warn(`Unknown shard ID: ${shardId}`);
      return;
    }
    
    const currentStatus = this.shardStatus.get(shardId);
    const updatedStatus = {
      ...currentStatus,
      ...status,
      lastUpdated: Date.now()
    };
    
    this.shardStatus.set(shardId, updatedStatus);
    
    // Update metrics
    this.metrics.setGauge('shard_load', updatedStatus.load, { shard_id: shardId });
    this.metrics.setGauge('shard_requests_per_second', updatedStatus.requestsPerSecond, { shard_id: shardId });
    this.metrics.setGauge('shard_pending_requests', updatedStatus.pendingRequests, { shard_id: shardId });
    this.metrics.setGauge('shard_health_status', updatedStatus.healthy ? 1 : 0, { shard_id: shardId });
    
    // Check if shard is overloaded
    this.checkShardLoad(shardId, updatedStatus);
  }
  
  /**
   * Checks if a shard is overloaded and handles backpressure
   * @private
   * @param {String} shardId The shard ID
   * @param {Object} status Current shard status
   */
  checkShardLoad(shardId, status) {
    // Check for overload
    if (status.load >= this.failoverThreshold) {
      // Severe overload - trigger failover
      if (!this.overloadedShards.has(shardId)) {
        Logger.warn(`Shard ${shardId} critically overloaded (${status.load.toFixed(2)}), triggering failover`);
        this.overloadedShards.add(shardId);
        this.triggerFailover(shardId);
        this.emit('shard:failover', { shardId, load: status.load });
      }
    }
    else if (status.load >= this.maxShardLoad) {
      // Moderate overload - apply backpressure
      if (!this.overloadedShards.has(shardId)) {
        Logger.info(`Shard ${shardId} overloaded (${status.load.toFixed(2)}), applying backpressure`);
        this.overloadedShards.add(shardId);
        this.emit('shard:backpressure', { shardId, load: status.load });
      }
    } 
    else if (this.overloadedShards.has(shardId)) {
      // No longer overloaded, but needs recovery time
      Logger.info(`Shard ${shardId} load normalized (${status.load.toFixed(2)}), starting recovery period`);
      this.overloadedShards.delete(shardId);
      
      // Set recovery timeout
      if (!this.recoveringShards.has(shardId)) {
        const timeout = setTimeout(() => {
          Logger.info(`Shard ${shardId} recovery period completed`);
          this.recoveringShards.delete(shardId);
          this.emit('shard:recovered', { shardId });
          this.metrics.incrementCounter('shard_recoveries_total');
        }, this.recoveryTimeMs);
        
        this.recoveringShards.set(shardId, timeout);
      }
    }
  }
  
  /**
   * Initiates failover from one shard to another
   * @private
   * @param {String} overloadedShardId The overloaded shard ID to failover from
   */
  triggerFailover(overloadedShardId) {
    // Find the least loaded healthy shard
    const targetShard = this.findLeastLoadedShard(overloadedShardId);
    
    if (!targetShard) {
      Logger.error('No available target shard for failover');
      return;
    }
    
    Logger.info(`Failing over from ${overloadedShardId} to ${targetShard.id} (load: ${targetShard.load.toFixed(2)})`);
    
    // If the overloaded shard is the primary, reassign primary
    if (this.primaryShard === overloadedShardId) {
      this.primaryShard = targetShard.id;
      Logger.info(`New primary shard: ${this.primaryShard}`);
    }
    
    // Increment failover counter
    this.metrics.incrementCounter('shard_failovers_total');
    
    // Notify subscribers
    this.emit('shard:failover:complete', {
      from: overloadedShardId,
      to: targetShard.id,
      timestamp: Date.now()
    });
  }
  
  /**
   * Finds the least loaded healthy shard that isn't the specified shard
   * @private
   * @param {String} excludeShardId Shard ID to exclude
   * @returns {Object|null} Least loaded shard status or null if none available
   */
  findLeastLoadedShard(excludeShardId) {
    let leastLoadedShard = null;
    let minLoad = 1.1; // Start above the maximum possible load
    
    for (const [shardId, status] of this.shardStatus) {
      // Skip excluded shard, unhealthy shards, inactive shards and recovering shards
      if (shardId === excludeShardId || 
          !status.healthy || 
          !status.active || 
          this.recoveringShards.has(shardId) ||
          this.overloadedShards.has(shardId)) {
        continue;
      }
      
      if (status.load < minLoad) {
        minLoad = status.load;
        leastLoadedShard = status;
      }
    }
    
    return leastLoadedShard;
  }
  
  /**
   * Periodically checks status of all shards
   * @private
   */
  checkShardsStatus() {
    const now = Date.now();
    
    for (const [shardId, status] of this.shardStatus) {
      // Check if shard hasn't reported for too long
      const lastUpdateAge = now - status.lastUpdated;
      if (lastUpdateAge > 3 * this.statusCheckIntervalMs) {
        // Mark as unhealthy if no updates received
        if (status.healthy) {
          Logger.warn(`Shard ${shardId} marked unhealthy - no status updates for ${lastUpdateAge}ms`);
          status.healthy = false;
          this.shardStatus.set(shardId, status);
          this.metrics.setGauge('shard_health_status', 0, { shard_id: shardId });
          this.emit('shard:unhealthy', { shardId, reason: 'No status updates' });
          
          // If this was the primary shard, choose a new one
          if (this.primaryShard === shardId) {
            const newPrimary = this.findLeastLoadedShard(shardId);
            if (newPrimary) {
              this.primaryShard = newPrimary.id;
              Logger.info(`New primary shard due to unhealthy primary: ${this.primaryShard}`);
            }
          }
        }
      }
    }
  }
  
  /**
   * Gets the current appropriate shard for a new request, applying backpressure
   * logic if necessary
   * @param {Object} request The request to be processed
   * @param {Object} options Request options
   * @param {Boolean} options.highPriority Whether this is a high priority request
   * @returns {String} The selected shard ID
   * @throws {Error} If no shards are available or all are overloaded
   */
  getShardForRequest(request, options = {}) {
    const isHighPriority = options.highPriority === true;
    
    // For high priority requests, try to use primary if it's healthy
    if (isHighPriority) {
      const primaryStatus = this.shardStatus.get(this.primaryShard);
      if (primaryStatus && primaryStatus.healthy && 
          !this.overloadedShards.has(this.primaryShard)) {
        return this.primaryShard;
      }
    }
    
    // Try a round robin selection of healthy, non-overloaded shards
    const startIndex = this.currentShardIndex;
    let selectedShard = null;
    
    for (let i = 0; i < this.shardIds.length; i++) {
      const index = (startIndex + i) % this.shardIds.length;
      const shardId = this.shardIds[index];
      const status = this.shardStatus.get(shardId);
      
      if (status.healthy && status.active && 
          !this.overloadedShards.has(shardId) && 
          !this.recoveringShards.has(shardId)) {
        selectedShard = shardId;
        this.currentShardIndex = (index + 1) % this.shardIds.length;
        break;
      }
    }
    
    // If we found a shard, return it
    if (selectedShard) {
      return selectedShard;
    }
    
    // If high priority and all normal shards are busy, check if any overloaded (but not recovering) shard is available
    if (isHighPriority) {
      for (const shardId of this.shardIds) {
        const status = this.shardStatus.get(shardId);
        if (status.healthy && status.active && !this.recoveringShards.has(shardId)) {
          return shardId; // Use an overloaded shard for high priority requests
        }
      }
    }
    
    // No available shards - apply backpressure to client
    throw new Error('All shards overloaded, please retry later');
  }
  
  /**
   * Gets the load factor of a specific shard
   * @param {String} shardId The shard ID
   * @returns {Number} Load factor (0-1) or -1 if shard unknown
   */
  getShardLoad(shardId) {
    const status = this.shardStatus.get(shardId);
    return status ? status.load : -1;
  }
  
  /**
   * Gets the current primary shard
   * @returns {String} Primary shard ID
   */
  getPrimaryShard() {
    return this.primaryShard;
  }
  
  /**
   * Gets overall cluster health status
   * @returns {Object} Health status object
   */
  getClusterHealth() {
    const healthyShardsCount = Array.from(this.shardStatus.values())
      .filter(s => s.healthy && s.active).length;
    
    const totalLoad = Array.from(this.shardStatus.values())
      .reduce((sum, shard) => sum + shard.load, 0) / this.shardStatus.size;
    
    return {
      totalShards: this.shardIds.length,
      healthyShards: healthyShardsCount,
      overloadedShards: this.overloadedShards.size,
      recoveringShards: this.recoveringShards.size,
      averageLoad: totalLoad,
      primaryShard: this.primaryShard,
      status: healthyShardsCount > 0 ? 'healthy' : 'critical'
    };
  }
  
  /**
   * Clean up resources
   */
  shutdown() {
    clearInterval(this.statusCheckInterval);
    
    // Clear any recovery timeouts
    for (const timeout of this.recoveringShards.values()) {
      clearTimeout(timeout);
    }
    
    Logger.info('ShardManager shutdown');
  }
}

module.exports = { ShardManager };
