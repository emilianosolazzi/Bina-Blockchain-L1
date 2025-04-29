import { 
  FilterUpdated,
  FilterCleared,
  FilterReset,
  FilterScaled,
  FalsePositiveDetected,
  AppealResolved,
  ConsortiumMembershipChanged,
  VoteThresholdChanged,
  AppealVoteCast,
  FilterCreated,
  FilterActivated,
  ScalingStarted,
  ScalingBatchCompleted,
  ScalingCompleted,
  ScalingCanceled,
  AppealRegistered,
  OutputPruned,
  ScalingConfigUpdated,
  EmergencyAction,
  FilterPaused,
  FilterUnpaused
} from '../generated/BloomFilterContract/BloomFilterContract'

import {
  Filter,
  Appeal,
  ConsortiumMember,
  Vote,
  FilterMetrics,
  FilterOperation,
  Migration,
  MigrationBatch,
  MigrationError,
  PerformanceMetrics,
  EmergencyAction as EmergencyActionEntity,
  SystemConfig,
  DailyMetrics,
  TimeWeightedMetrics
} from '../generated/schema'

import { BigInt, Bytes, store, ethereum, crypto, log, dataSource, Address, ByteArray } from '@graphprotocol/graph-ts'

// Common constants
const BASIS_POINTS_SCALE = 10000
const SECONDS_PER_DAY = 86400
const DEFAULT_NETWORK_ID = 1 // Default to Ethereum mainnet
const ALPHA_FAST = 0.3  // Fast-responding TWMA (30% weight to new values)
const ALPHA_SLOW = 0.1  // Slow-responding TWMA (10% weight to new values)
const SECONDS_PER_HOUR = 3600
const NULL_ADDRESS = '0x0000000000000000000000000000000000000000'

/**
 * Safely loads a Filter entity with error handling
 * @param id The filter ID to load
 * @returns The Filter entity or null if not found
 */
function safeLoadFilter(id: string): Filter | null {
  try {
    return Filter.load(id);
  } catch (error) {
    log.warning("Failed to load Filter ID {}: {}", [id, error.toString()]);
    return null;
  }
}

/**
 * Safely loads a Migration entity with error handling
 * @param id The migration ID to load
 * @returns The Migration entity or null if not found
 */
function safeLoadMigration(id: string): Migration | null {
  try {
    return Migration.load(id);
  } catch (error) {
    log.warning("Failed to load Migration ID {}: {}", [id, error.toString()]);
    return null;
  }
}

/**
 * Safely loads an Appeal entity with error handling
 * @param id The appeal ID to load
 * @returns The Appeal entity or null if not found
 */
function safeLoadAppeal(id: string): Appeal | null {
  try {
    return Appeal.load(id);
  } catch (error) {
    log.warning("Failed to load Appeal ID {}: {}", [id, error.toString()]);
    return null;
  }
}

/**
 * Initialize or get the system configuration singleton
 */
function getOrCreateSystemConfig(blockTimestamp: BigInt): SystemConfig {
  let config = SystemConfig.load('current')
  if (config == null) {
    config = new SystemConfig('current')
    config.activeFilterId = BigInt.fromI32(0)
    config.batchSize = BigInt.fromI32(500)
    config.targetFPRbps = 50
    config.parallelQueryPeriod = BigInt.fromI32(86400)
    config.autoScaleThresholdBps = 7500
    config.autoScalingEnabled = false
    config.paused = false
    config.lastUpdated = blockTimestamp
    config.updatedBy = Bytes.fromHexString('0x0000000000000000000000000000000000000000')
    config.updateCount = 0
    config.networkId = dataSource.network().split('-')[1] ? parseInt(dataSource.network().split('-')[1]) : DEFAULT_NETWORK_ID
    config.implementation = dataSource.address().toHexString()
    config.deploymentVersion = '1.0.0'
    config.save()
  }
  return config as SystemConfig
}

/**
 * Create or update a filter entity
 */
function getOrCreateFilter(address: Address, blockTimestamp: BigInt, blockNumber: BigInt): Filter {
  let filterId = address.toHexString()
  let filter = Filter.load(filterId)
  
  if (filter == null) {
    filter = new Filter(filterId)
    filter.version = 1
    filter.size = BigInt.fromI32(0)
    filter.numHashes = 0
    filter.salt = BigInt.fromI32(0)
    filter.insertCount = BigInt.fromI32(0)
    filter.bitCount = BigInt.fromI32(0)
    filter.fillRatioBps = 0
    filter.estimatedFPRbps = 0
    filter.minVotesRequired = 1
    filter.totalConsortiumMembers = 0
    filter.totalAppeals = 0
    filter.resolvedAppeals = 0
    filter.acceptedAppeals = 0
    filter.pendingAppeals = 0
    filter.rejectedAppeals = 0
    filter.isActive = false
    filter.isPrevious = false
    filter.status = 'CREATED'
    filter.healthStatus = 'HEALTHY'
    filter.alertLevel = 0
    filter.targetFPRbps = 50
    filter.autoScaleThresholdBps = 7500
    filter.createdAt = blockTimestamp
    filter.updatedAt = blockTimestamp
    filter.createdAtBlock = blockNumber
    filter.networkId = dataSource.network().split('-')[1] ? parseInt(dataSource.network().split('-')[1]) : DEFAULT_NETWORK_ID
    filter.implementation = dataSource.address().toHexString()
    filter.save()
  }
  
  return filter as Filter
}

/**
 * Calculate time-weighted moving average
 * @param currentValue The current value to incorporate
 * @param previousAverage The previous moving average (or same as currentValue if first calculation)
 * @param weightFactor The weight factor alpha (between 0 and 1) - higher means more weight to current
 * @returns The updated moving average
 */
function calculateTWMA(currentValue: number, previousAverage: number, weightFactor: number): number {
  return previousAverage + weightFactor * (currentValue - previousAverage);
}

/**
 * Create or update daily metrics for aggregation
 * @param filterId The filter ID to aggregate metrics for
 * @param timestamp The timestamp of the event
 * @param operation The operation type (INSERT, QUERY, etc.)
 * @param gasUsed The gas used for this operation (if available)
 */
function updateDailyMetrics(
  filterId: string, 
  timestamp: BigInt, 
  operation: string,
  gasUsed: BigInt | null = null
): void {
  // Create a day identifier (YYYY-MM-DD) from the timestamp
  const date = new Date(timestamp.toI64() * 1000);
  const year = date.getUTCFullYear().toString();
  const month = (date.getUTCMonth() + 1).toString().padStart(2, '0');
  const day = date.getUTCDate().toString().padStart(2, '0');
  const dateStr = year + '-' + month + '-' + day;
  
  // Create a unique ID for this filter and day
  const metricId = filterId + '-' + dateStr;
  
  // Load or create the daily metrics entity
  let dailyMetrics = DailyMetrics.load(metricId);
  if (!dailyMetrics) {
    dailyMetrics = new DailyMetrics(metricId);
    dailyMetrics.filter = filterId;
    dailyMetrics.date = dateStr;
    dailyMetrics.dayTimestamp = BigInt.fromI32(Math.floor(date.getTime() / 1000));
    dailyMetrics.insertCount = 0;
    dailyMetrics.queryCount = 0;
    dailyMetrics.appealCount = 0;
    dailyMetrics.voteCount = 0;
    dailyMetrics.totalGasUsed = BigInt.fromI32(0);
    dailyMetrics.insertGasUsed = BigInt.fromI32(0);
    dailyMetrics.queryGasUsed = BigInt.fromI32(0);
    dailyMetrics.uniqueUsers = [];
    dailyMetrics.activeConsortiumMembers = 0;
    dailyMetrics.transactionCount = 0;
  }
  
  // Update counts based on operation type
  if (operation == 'INSERT') {
    dailyMetrics.insertCount += 1;
    if (gasUsed) {
      dailyMetrics.insertGasUsed = dailyMetrics.insertGasUsed.plus(gasUsed);
    }
  } else if (operation == 'QUERY') {
    dailyMetrics.queryCount += 1;
    if (gasUsed) {
      dailyMetrics.queryGasUsed = dailyMetrics.queryGasUsed.plus(gasUsed);
    }
  } else if (operation == 'APPEAL_REGISTER') {
    dailyMetrics.appealCount += 1;
  } else if (operation == 'VOTE_CAST') {
    dailyMetrics.voteCount += 1;
  }
  
  // Update gas metrics if available
  if (gasUsed) {
    dailyMetrics.totalGasUsed = dailyMetrics.totalGasUsed.plus(gasUsed);
  }
  
  // Track unique users
  const caller = ethereum.transaction.from.toHexString();
  if (caller != NULL_ADDRESS) {
    let userExists = false;
    for (let i = 0; i < dailyMetrics.uniqueUsers.length; i++) {
      if (dailyMetrics.uniqueUsers[i] == caller) {
        userExists = true;
        break;
      }
    }
    
    if (!userExists) {
      const newUsers = dailyMetrics.uniqueUsers;
      newUsers.push(caller);
      dailyMetrics.uniqueUsers = newUsers;
    }
  }
  
  // Increment transaction count
  dailyMetrics.transactionCount += 1;
  
  // Save the metrics
  dailyMetrics.save();
}

/**
 * Create or update time-weighted moving average metrics
 * @param filterId The filter ID to track metrics for
 * @param timestamp The timestamp of the event
 * @param metricType The type of metric (GAS, INSERT_TIME, etc.)
 * @param newValue The new value to incorporate into the TWMA
 */
function updateTimeWeightedMetrics(
  filterId: string,
  timestamp: BigInt,
  metricType: string,
  newValue: number
): void {
  // Create a unique ID for this filter and metric type
  const metricId = filterId + '-' + metricType;
  
  // Load or create the TWMA entity
  let twMetrics = TimeWeightedMetrics.load(metricId);
  if (!twMetrics) {
    twMetrics = new TimeWeightedMetrics(metricId);
    twMetrics.filter = filterId;
    twMetrics.metricType = metricType;
    twMetrics.lastUpdated = timestamp;
    twMetrics.fastTWMA = newValue;
    twMetrics.slowTWMA = newValue;
    twMetrics.sampleCount = 1;
    twMetrics.maxValue = newValue;
    twMetrics.minValue = newValue;
    twMetrics.cumulativeValue = newValue;
  } else {
    // Calculate the new TWMAs
    twMetrics.fastTWMA = calculateTWMA(newValue, twMetrics.fastTWMA, ALPHA_FAST);
    twMetrics.slowTWMA = calculateTWMA(newValue, twMetrics.slowTWMA, ALPHA_SLOW);
    
    // Update stats
    twMetrics.lastUpdated = timestamp;
    twMetrics.sampleCount += 1;
    twMetrics.maxValue = Math.max(twMetrics.maxValue, newValue);
    twMetrics.minValue = Math.min(twMetrics.minValue, newValue);
    twMetrics.cumulativeValue += newValue;
  }
  
  // Save the metrics
  twMetrics.save();
}

/**
 * Handles FilterCreated events - now with try/catch for error handling
 */
export function handleFilterCreated(event: FilterCreated): void {
  try {
    let filterId = event.params.filterId.toString()
    let filterAddress = event.address
    
    // Create or update filter
    let filter = getOrCreateFilter(filterAddress, event.block.timestamp, event.block.number)
    
    // Update filter properties
    filter.version = filter.version + 1
    filter.size = event.params.size
    filter.numHashes = event.params.numHashes.toI32()
    filter.salt = crypto.keccak256(ByteArray.fromBigInt(event.block.timestamp)).toBigInt()
    filter.bitCount = filter.size.times(BigInt.fromI32(256))
    filter.status = 'CREATED'
    filter.updatedAt = event.block.timestamp
    filter.save()
    
    // Create operation record
    let operationId = event.transaction.hash.toHexString() + "-" + event.logIndex.toString()
    let operation = new FilterOperation(operationId)
    operation.filter = filterAddress.toHexString()
    operation.operationType = 'CREATE'
    operation.timestamp = event.block.timestamp
    operation.blockNumber = event.block.number
    operation.transactionHash = event.transaction.hash
    operation.caller = event.transaction.from
    operation.details = JSON.stringify({
      'filterId': filterId,
      'size': event.params.size.toString(),
      'numHashes': event.params.numHashes.toString(),
      'timestamp': event.params.timestamp.toString()
    })
    operation.gasUsed = event.receipt ? event.receipt.gasUsed : null
    operation.gasPrice = event.transaction.gasPrice
    operation.save()
    
    // Update system config
    let config = getOrCreateSystemConfig(event.block.timestamp)
    config.lastUpdated = event.block.timestamp
    config.updatedBy = event.transaction.from
    config.updateCount += 1
    config.save()
    
    // Track daily metrics
    updateDailyMetrics(
      filterAddress.toHexString(),
      event.block.timestamp,
      'CREATE',
      event.receipt ? event.receipt.gasUsed : null
    );
  } catch (error) {
    log.error("Error handling FilterCreated: {}", [error.toString()]);
  }
}

/**
 * Handle FilterActivated events
 */
export function handleFilterActivated(event: FilterActivated): void {
  try {
    let filterId = event.params.filterId.toString()
    let filterAddress = event.address
    
    // Create or update filter
    let filter = getOrCreateFilter(filterAddress, event.block.timestamp, event.block.number)
    
    // Update filter status
    filter.isActive = true
    filter.status = 'ACTIVE'
    filter.activatedAt = event.block.timestamp
    filter.updatedAt = event.block.timestamp
    filter.save()
    
    // Update any previous active filter
    let config = getOrCreateSystemConfig(event.block.timestamp)
    if (!config.activeFilterId.isZero() && config.activeFilterId.toString() != filterId) {
      config.previousFilterId = config.activeFilterId
      
      // Update previous filter status
      let prevFilterId = config.previousFilterId.toString()
      let prevFilter = Filter.load(prevFilterId)
      if (prevFilter) {
        prevFilter.isActive = false
        prevFilter.isPrevious = true
        prevFilter.status = 'PREVIOUS'
        prevFilter.updatedAt = event.block.timestamp
        prevFilter.save()
      }
    }
    
    // Update system config
    config.activeFilterId = event.params.filterId
    config.lastUpdated = event.block.timestamp
    config.updatedBy = event.transaction.from
    config.updateCount += 1
    config.save()
    
    // Create operation record
    let operationId = event.transaction.hash.toHexString() + "-" + event.logIndex.toString()
    let operation = new FilterOperation(operationId)
    operation.filter = filterAddress.toHexString()
    operation.operationType = 'ACTIVATE'
    operation.timestamp = event.block.timestamp
    operation.blockNumber = event.block.number
    operation.transactionHash = event.transaction.hash
    operation.caller = event.transaction.from
    operation.details = JSON.stringify({
      'filterId': filterId,
      'timestamp': event.params.timestamp.toString()
    })
    operation.gasUsed = event.receipt ? event.receipt.gasUsed : null
    operation.gasPrice = event.transaction.gasPrice
    operation.save()
    
    // Create metrics snapshot for newly activated filter
    createMetricsSnapshot(filterAddress.toHexString(), event.block.timestamp, event.block.number)
    
    // Track daily metrics
    updateDailyMetrics(
      filterAddress.toHexString(),
      event.block.timestamp,
      'ACTIVATE',
      event.receipt ? event.receipt.gasUsed : null
    );
  } catch (error) {
    log.error("Error handling FilterActivated: {}", [error.toString()]);
  }
}

/**
 * Handle ScalingStarted events
 * Tracks the beginning of filter migration/scaling operations
 */
export function handleScalingStarted(event: ScalingStarted): void {
  try {
    let sourceFilterId = event.params.sourceFilterId.toString()
    let targetFilterId = event.params.targetFilterId.toString()
    let migrationId = sourceFilterId + '-to-' + targetFilterId
    
    // Create migration entity
    let migration = new Migration(migrationId)
    migration.sourceFilter = sourceFilterId
    migration.targetFilter = targetFilterId
    migration.state = 'PREPARING'
    migration.startedAt = event.block.timestamp
    migration.totalBatches = BigInt.fromI32(0)
    migration.batchesCompleted = BigInt.fromI32(0)
    migration.entriesMigrated = BigInt.fromI32(0)
    migration.progress = BigInt.fromI32(0)
    migration.batchSize = 0
    migration.averageBatchGasUsed = BigInt.fromI32(0)
    migration.totalGasUsed = BigInt.fromI32(0)
    migration.estimatedRemainingGas = BigInt.fromI32(0)
    migration.hasErrors = false
    migration.errorCount = 0
    migration.transactionHashes = [event.transaction.hash]
    migration.createdTxHash = event.transaction.hash
    migration.averageBatchTime = BigInt.fromI32(0)
    migration.save()
    
    // Get system config for batch size
    let config = getOrCreateSystemConfig(event.block.timestamp)
    migration.batchSize = config.batchSize.toI32()
    migration.save()
    
    // Update source filter status
    let sourceFilter = Filter.load(sourceFilterId)
    if (sourceFilter) {
      sourceFilter.status = 'MIGRATING'
      sourceFilter.updatedAt = event.block.timestamp
      sourceFilter.save()
      
      // Create operation record
      let operationId = event.transaction.hash.toHexString() + "-" + event.logIndex.toString()
      let operation = new FilterOperation(operationId)
      operation.filter = sourceFilterId
      operation.operationType = 'SCALING_START'
      operation.timestamp = event.block.timestamp
      operation.blockNumber = event.block.number
      operation.transactionHash = event.transaction.hash
      operation.caller = event.transaction.from
      operation.details = JSON.stringify({
        'sourceFilterId': sourceFilterId,
        'targetFilterId': targetFilterId,
        'timestamp': event.params.timestamp.toString()
      })
      operation.gasUsed = event.receipt ? event.receipt.gasUsed : null
      operation.gasPrice = event.transaction.gasPrice
      operation.save()
    }
    
    // Track daily metrics
    updateDailyMetrics(
      sourceFilterId,
      event.block.timestamp,
      'SCALING_START',
      event.receipt ? event.receipt.gasUsed : null
    );
  } catch (error) {
    log.error("Error handling ScalingStarted: {}", [error.toString()]);
  }
}

/**
 * Handle ScalingBatchCompleted events with improved error handling
 */
export function handleScalingBatchCompleted(event: ScalingBatchCompleted): void {
  try {
    // Find the active migration involving this target filter - with error handling
    const targetFilterId = event.params.targetFilterId.toString();
    let migration: Migration | null = null;
    
    try {
      migration = safeLoadMigration(targetFilterId);
      if (!migration) {
        // Try to find migration by scanning all migrations for this target
        const migrationEntities = Migration.find(
          m => m.targetFilter == targetFilterId && m.state != 'COMPLETED',
          [],
          1
        );
        
        if (migrationEntities.length > 0) {
          migration = migrationEntities[0];
        } else {
          log.warning("Unable to find migration for target filter: {}", [targetFilterId]);
          return;
        }
      }
    } catch (error) {
      log.error("Error loading migration for target {}: {}", [targetFilterId, error.toString()]);
      return;
    }
    
    const migrationId = migration.id;
    const batchId = migrationId + '-batch-' + event.params.batchIndex.toString();
    
    // Create batch entity
    let batch = new MigrationBatch(batchId);
    batch.migration = migrationId;
    batch.batchIndex = event.params.batchIndex;
    batch.entriesMigrated = event.params.entriesMigrated;
    batch.timestamp = event.block.timestamp;
    batch.blockNumber = event.block.number;
    batch.transactionHash = event.transaction.hash;
    batch.successful = true;
    
    // Calculate processing time with proper error handling
    let processingTime = BigInt.fromI32(0);
    if (!migration.batchesCompleted.isZero()) {
      try {
        const lastBatchId = migrationId + '-batch-' + migration.batchesCompleted.minus(BigInt.fromI32(1)).toString();
        const lastBatch = MigrationBatch.load(lastBatchId);
        if (lastBatch) {
          processingTime = event.block.timestamp.minus(lastBatch.timestamp);
        }
      } catch (error) {
        log.warning("Error calculating processing time: {}", [error.toString()]);
      }
    }
    
    batch.processingTime = processingTime;
    batch.gasUsed = event.receipt ? event.receipt.gasUsed : BigInt.fromI32(0);
    batch.save();
    
    // Update migration stats
    migration.batchesCompleted = migration.batchesCompleted.plus(BigInt.fromI32(1));
    migration.entriesMigrated = migration.entriesMigrated.plus(event.params.entriesMigrated);
    
    // Calculate progress
    if (!migration.totalBatches.isZero()) {
      migration.progress = migration.batchesCompleted
        .times(BigInt.fromI32(BASIS_POINTS_SCALE))
        .div(migration.totalBatches);
    }
    
    // Update gas and time metrics
    if (event.receipt) {
      migration.totalGasUsed = migration.totalGasUsed.plus(event.receipt.gasUsed);
      
      if (!migration.batchesCompleted.isZero()) {
        migration.averageBatchGasUsed = migration.totalGasUsed.div(migration.batchesCompleted);
      }
      
      let remainingBatches = migration.totalBatches.minus(migration.batchesCompleted);
      migration.estimatedRemainingGas = remainingBatches.times(migration.averageBatchGasUsed);
      
      // Update TWMA for batch gas usage
      updateTimeWeightedMetrics(
        targetFilterId,
        event.block.timestamp,
        'BATCH_GAS_USED', 
        event.receipt.gasUsed.toI32()
      );
    }
    
    // Update average batch time with TWMA
    if (processingTime.gt(BigInt.fromI32(0))) {
      // Update batch time TWMA
      updateTimeWeightedMetrics(
        targetFilterId,
        event.block.timestamp,
        'BATCH_PROCESSING_TIME',
        processingTime.toI32()
      );
      
      // Still maintain the rolling average calculation
      if (migration.averageBatchTime.isZero()) {
        migration.averageBatchTime = processingTime;
      } else {
        migration.averageBatchTime = migration.averageBatchTime
          .plus(processingTime)
          .div(BigInt.fromI32(2)); // Simple rolling average
      }
    }
    
    // If all batches are completed, update state
    if (migration.batchesCompleted.equals(migration.totalBatches)) {
      migration.state = 'FINALIZING';
    }
    
    // Track transaction hash
    try {
      // Add to transaction hashes array if not already there
      let txExists = false;
      for (let i = 0; i < migration.transactionHashes.length; i++) {
        if (migration.transactionHashes[i].equals(event.transaction.hash)) {
          txExists = true;
          break;
        }
      }
      
      if (!txExists) {
        let newHashes = migration.transactionHashes;
        newHashes.push(event.transaction.hash);
        migration.transactionHashes = newHashes;
      }
    } catch (error) {
      log.warning("Error updating transaction hashes: {}", [error.toString()]);
    }
    
    migration.save();
    
    // Track daily metrics
    updateDailyMetrics(
      targetFilterId,
      event.block.timestamp,
      'BATCH_COMPLETED',
      event.receipt ? event.receipt.gasUsed : null
    );
    
  } catch (error) {
    log.error("Error handling ScalingBatchCompleted: {}", [error.toString()]);
  }
}

/**
 * Handle ScalingCompleted events
 * Finalizes a migration when completed
 */
export function handleScalingCompleted(event: ScalingCompleted): void {
  let targetFilterId = event.params.targetFilterId.toString()
  
  // Update target filter
  let targetFilter = Filter.load(targetFilterId)
  if (targetFilter) {
    targetFilter.status = 'ACTIVE'
    targetFilter.updatedAt = event.block.timestamp
    targetFilter.save()
    
    // Create a metrics snapshot for the newly scaled filter
    createMetricsSnapshot(targetFilterId, event.block.timestamp, event.block.number)
  }
  
  // Find the migration
  let migrations = Migration.find(m => m.targetFilter == targetFilterId && m.state != 'COMPLETED')
  if (migrations && migrations.length > 0) {
    let migration = migrations[0]
    migration.state = 'COMPLETED'
    migration.completedAt = event.block.timestamp
    migration.entriesMigrated = event.params.totalEntriesMigrated
    migration.completedTxHash = event.transaction.hash
    
    // Add transaction hash if not already in list
    let txExists = false
    for (let i = 0; i < migration.transactionHashes.length; i++) {
      if (migration.transactionHashes[i].equals(event.transaction.hash)) {
        txExists = true
        break
      }
    }
    if (!txExists) {
      let newHashes = migration.transactionHashes
      newHashes.push(event.transaction.hash)
      migration.transactionHashes = newHashes
    }
    
    migration.save()
  }
  
  // Create operation record
  let operationId = event.transaction.hash.toHexString() + "-" + event.logIndex.toString()
  let operation = new FilterOperation(operationId)
  operation.filter = targetFilterId
  operation.operationType = 'SCALING_COMPLETED'
  operation.timestamp = event.block.timestamp
  operation.blockNumber = event.block.number
  operation.transactionHash = event.transaction.hash
  operation.caller = event.transaction.from
  operation.details = JSON.stringify({
    'targetFilterId': targetFilterId,
    'totalEntriesMigrated': event.params.totalEntriesMigrated.toString(),
    'timestamp': event.params.timestamp.toString()
  })
  operation.gasUsed = event.receipt ? event.receipt.gasUsed : null
  operation.gasPrice = event.transaction.gasPrice
  operation.save()
}

/**
 * Handle ScalingCanceled events
 * Records when a scaling operation is canceled
 */
export function handleScalingCanceled(event: ScalingCanceled): void {
  let sourceFilterId = event.params.sourceFilterId.toString()
  let targetFilterId = event.params.targetFilterId.toString()
  let migrationId = sourceFilterId + '-to-' + targetFilterId
  
  // Update migration
  let migration = Migration.load(migrationId)
  if (migration) {
    migration.state = 'CANCELED'
    migration.completedAt = event.block.timestamp
    
    // Add transaction hash if not already in list
    let txExists = false
    for (let i = 0; i < migration.transactionHashes.length; i++) {
      if (migration.transactionHashes[i].equals(event.transaction.hash)) {
        txExists = true
        break
      }
    }
    if (!txExists) {
      let newHashes = migration.transactionHashes
      newHashes.push(event.transaction.hash)
      migration.transactionHashes = newHashes
    }
    
    migration.save()
  }
  
  // Update source filter
  let sourceFilter = Filter.load(sourceFilterId)
  if (sourceFilter) {
    sourceFilter.status = 'ACTIVE' // Return to active status
    sourceFilter.updatedAt = event.block.timestamp
    sourceFilter.save()
  }
  
  // Create operation record
  let operationId = event.transaction.hash.toHexString() + "-" + event.logIndex.toString()
  let operation = new FilterOperation(operationId)
  operation.filter = sourceFilterId
  operation.operationType = 'SCALING_CANCELED'
  operation.timestamp = event.block.timestamp
  operation.blockNumber = event.block.number
  operation.transactionHash = event.transaction.hash
  operation.caller = event.transaction.from
  operation.details = JSON.stringify({
    'sourceFilterId': sourceFilterId,
    'targetFilterId': targetFilterId,
    'timestamp': event.params.timestamp.toString()
  })
  operation.gasUsed = event.receipt ? event.receipt.gasUsed : null
  operation.gasPrice = event.transaction.gasPrice
  operation.save()
  
  // Create error entity
  let errorId = event.transaction.hash.toHexString() + "-cancel"
  let error = new MigrationError(errorId)
  error.migration = migrationId
  error.errorMessage = "Migration canceled by administrator"
  error.timestamp = event.block.timestamp
  error.transactionHash = event.transaction.hash
  error.severity = "MAJOR"
  error.save()
  
  // Update migration error stats
  if (migration) {
    migration.hasErrors = true
    migration.errorCount += 1
    migration.save()
  }
}

/**
 * Handle FilterUpdated events with TWMA tracking and daily metrics
 */
export function handleFilterUpdated(event: FilterUpdated): void {
  try {
    let filterId = event.address.toHexString();
    
    // Update filter stats with safety checks
    let filter = safeLoadFilter(filterId);
    if (!filter) {
      log.warning("Filter not found in handleFilterUpdated: {}", [filterId]);
      return;
    }
    
    filter.insertCount = filter.insertCount.plus(BigInt.fromI32(1));
    filter.updatedAt = event.block.timestamp;
    
    // Calculate fill ratio based on size and insert count (rough approximation)
    // Actual calculation would ideally come from contract events
    let size = filter.size.times(BigInt.fromI32(256));
    if (!size.isZero()) {
      let fillRatio = filter.insertCount.times(BigInt.fromI32(BASIS_POINTS_SCALE)).div(size);
      filter.fillRatioBps = Math.min(fillRatio.toI32(), BASIS_POINTS_SCALE);
    }
    
    // Estimate FPR based on filter parameters
    // (1-(1-1/m)^(k*n))^k where m=bits, k=hash functions, n=items
    // Simple approximation: k*n/m provides rough FPR for small values
    let k = BigInt.fromI32(filter.numHashes);
    let n = filter.insertCount;
    let m = filter.size.times(BigInt.fromI32(256));
    
    if (!m.isZero()) {
      // Simplified FPR calculation (just an approximation)
      let hashesPerInsert = k.times(n);
      let fprNumerator = hashesPerInsert.times(BigInt.fromI32(BASIS_POINTS_SCALE));
      let fpr = fprNumerator.div(m);
      filter.estimatedFPRbps = Math.min(fpr.toI32(), BASIS_POINTS_SCALE);
    }
    
    // Update health status based on fill ratio
    if (filter.fillRatioBps > 8500) {
      filter.healthStatus = 'CRITICAL';
      filter.alertLevel = 3;
      filter.lastAlertTimestamp = event.block.timestamp;
    } else if (filter.fillRatioBps > 7500) {
      filter.healthStatus = 'DEGRADED';
      filter.alertLevel = 2;
      filter.lastAlertTimestamp = event.block.timestamp;
    } else {
      filter.healthStatus = 'HEALTHY';
      filter.alertLevel = 0;
    }
    
    filter.save();
    
    // Create operation record
    let operationId = event.transaction.hash.toHexString() + "-" + event.logIndex.toString();
    let operation = new FilterOperation(operationId);
    operation.filter = filterId;
    operation.operationType = 'INSERT';
    operation.timestamp = event.block.timestamp;
    operation.blockNumber = event.block.number;
    operation.transactionHash = event.transaction.hash;
    operation.caller = event.transaction.from;
    operation.entryInserted = event.params.entry;
    operation.gasUsed = event.receipt ? event.receipt.gasUsed : null;
    operation.gasPrice = event.transaction.gasPrice;
    operation.effectiveGasPrice = event.receipt ? event.receipt.effectiveGasPrice : null;
    operation.save();
    
    // Track gas usage with TWMA if available
    if (event.receipt && event.receipt.gasUsed) {
      updateTimeWeightedMetrics(
        filterId,
        event.block.timestamp,
        'INSERT_GAS_USED',
        event.receipt.gasUsed.toI32()
      );
      
      // If we have gas price, track cost TWMA
      if (event.transaction.gasPrice) {
        const cost = event.receipt.gasUsed.times(event.transaction.gasPrice).toI32();
        updateTimeWeightedMetrics(
          filterId,
          event.block.timestamp,
          'INSERT_COST',
          cost
        );
      }
    }
    
    // Create metrics snapshot every 1000 inserts
    if (filter && filter.insertCount.mod(BigInt.fromI32(1000)).isZero()) {
      createMetricsSnapshot(filterId, event.block.timestamp, event.block.number);
      
      // Create performance metrics every 5000 inserts
      if (filter.insertCount.mod(BigInt.fromI32(5000)).isZero()) {
        createPerformanceMetrics(filterId, event.block.timestamp);
      }
    }
    
    // Update daily metrics
    updateDailyMetrics(
      filterId,
      event.block.timestamp,
      'INSERT',
      event.receipt ? event.receipt.gasUsed : null
    );
    
  } catch (error) {
    log.error("Error handling FilterUpdated: {}", [error.toString()]);
  }
}

/**
 * Handle AppealRegistered events
 * Creates a new Appeal entity for false positive reports
 */
export function handleAppealRegistered(event: AppealRegistered): void {
  try {
    let appealId = event.params.appealId.toHexString()
    let filterId = event.address.toHexString()
    
    // Create appeal entity
    let appeal = new Appeal(appealId)
    appeal.filter = filterId
    appeal.output = event.params.output
    appeal.reporter = event.params.reporter
    appeal.timestamp = event.block.timestamp
    appeal.resolved = false
    appeal.accepted = false
    appeal.status = 'PENDING'
    appeal.voteCount = 0
    appeal.votesRequired = 0
    appeal.approvalPercentage = 0
    appeal.proof = Bytes.fromHexString('0x') // Default empty proof
    appeal.blockNumber = event.block.number
    appeal.transactionHash = event.transaction.hash
    appeal.createdAtTimestamp = event.block.timestamp
    
    // Set expiry if we have a system config
    let config = SystemConfig.load('current')
    if (config) {
      // Default expiry of 7 days
      appeal.expiryTimestamp = event.block.timestamp.plus(BigInt.fromI32(7 * SECONDS_PER_DAY))
    }
    
    // Get filter info for votesRequired
    let filter = Filter.load(filterId)
    if (filter) {
      appeal.votesRequired = filter.minVotesRequired
      filter.totalAppeals += 1
      filter.pendingAppeals += 1
      filter.updatedAt = event.block.timestamp
      filter.save()
    }
    
    appeal.save()
    
    // Create operation record
    let operationId = event.transaction.hash.toHexString() + "-" + event.logIndex.toString()
    let operation = new FilterOperation(operationId)
    operation.filter = filterId
    operation.operationType = 'APPEAL_REGISTER'
    operation.timestamp = event.block.timestamp
    operation.blockNumber = event.block.number
    operation.transactionHash = event.transaction.hash
    operation.caller = event.transaction.from
    operation.details = JSON.stringify({
      'appealId': appealId,
      'output': event.params.output.toHexString()
    })
    operation.gasUsed = event.receipt ? event.receipt.gasUsed : null
    operation.gasPrice = event.transaction.gasPrice
    operation.save()
    
    // Create metrics snapshot
    createMetricsSnapshot(filterId, event.block.timestamp, event.block.number)
    
    // Track daily metrics
    updateDailyMetrics(
      event.address.toHexString(),
      event.block.timestamp,
      'APPEAL_REGISTER',
      event.receipt ? event.receipt.gasUsed : null
    );
  } catch (error) {
    log.error("Error handling AppealRegistered: {}", [error.toString()]);
  }
}

/**
 * Handle FalsePositiveDetected events
 * Updates Appeal entities with proof data for false positive detection
 */
export function handleFalsePositiveDetected(event: FalsePositiveDetected): void {
  // Create unique ID for this appeal
  const appealId = event.params.appealId.toHexString()
  
  // Check if appeal already exists
  let appeal = Appeal.load(appealId)
  if (appeal == null) {
    appeal = Appeal.load(appealId)
    if (appeal == null) {
      // If appeal doesn't already exist from AppealRegistered, create it
      appeal = new Appeal(appealId)
      appeal.filter = event.address.toHexString()
      appeal.output = event.params.output
      appeal.reporter = event.params.reporter
      appeal.timestamp = event.params.timestamp
      appeal.resolved = false
      appeal.accepted = false
      appeal.status = 'PENDING'
      appeal.voteCount = 0
      appeal.votesRequired = 0
      appeal.approvalPercentage = 0
      appeal.blockNumber = event.block.number
      appeal.transactionHash = event.transaction.hash
      appeal.createdAtTimestamp = event.block.timestamp
      
      // Update filter appeal counts
      let filter = Filter.load(event.address.toHexString())
      if (filter) {
        filter.totalAppeals = filter.totalAppeals + 1
        filter.pendingAppeals = filter.pendingAppeals + 1
        filter.updatedAt = event.block.timestamp
        filter.save()
      }
    }
  }
  
  // Always update the proof data
  appeal.proof = event.params.proof
  appeal.save()
  
  // Create metrics snapshot
  createMetricsSnapshot(event.address.toHexString(), event.block.timestamp, event.block.number)
}

/**
 * Handle AppealVoteCast events
 * Creates or updates Vote entities when consortium members vote on appeals
 */
export function handleAppealVoteCast(event: AppealVoteCast): void {
  try {
    const appealId = event.params.appealId.toHexString()
    const voterId = event.params.voter.toHexString()
    
    // Create vote ID as combination of appeal and voter
    const voteId = appealId + '-' + voterId
    
    // Get consortium member for vote weight with proper error handling
    let member: ConsortiumMember | null = null;
    try {
      member = ConsortiumMember.load(voterId);
    } catch (error) {
      log.warning("Error loading consortium member: {}", [error.toString()]);
    }
    
    let voteWeight = 1
    if (member) {
      member.votesCast = member.votesCast + 1
      member.lastActiveAt = event.block.timestamp
      member.save()
      
      // Get vote weight if available
      voteWeight = member.weight ? member.weight : 1
    }
    
    // Get appeal to check current vote state
    let appeal = Appeal.load(appealId)
    let isDecisive = false
    let appealVoteCount = 0
    
    if (appeal) {
      appeal.voteCount = event.params.newVoteCount.toI32()
      appealVoteCount = appeal.voteCount
      
      // Calculate approval percentage
      let totalVotes = 0
      let approvalVotes = 0
      
      // We need to iterate through existing votes to get totals
      for (let i = 0; i < appeal.votes.length; i++) {
        let existingVote = Vote.load(appeal.votes[i].id)
        if (existingVote) {
          totalVotes += existingVote.weight
          if (existingVote.voteInFavor) {
            approvalVotes += existingVote.weight
          }
        }
      }
      
      // Add current vote
      totalVotes += voteWeight
      if (event.params.voteInFavor) {
        approvalVotes += voteWeight
      }
      
      if (totalVotes > 0) {
        appeal.approvalPercentage = (approvalVotes * BASIS_POINTS_SCALE) / totalVotes
      }
      
      // Check if this vote is decisive
      if (appealVoteCount >= appeal.votesRequired) {
        isDecisive = true
        appeal.resolved = true
        appeal.accepted = appeal.approvalPercentage > (BASIS_POINTS_SCALE / 2)
        appeal.status = appeal.accepted ? 'ACCEPTED' : 'REJECTED'
        appeal.resolutionTime = event.block.timestamp
        appeal.resolutionTxHash = event.transaction.hash
        appeal.pendingDuration = event.block.timestamp.minus(appeal.timestamp)
        
        // Update filter appeal counts
        let filter = Filter.load(appeal.filter)
        if (filter) {
          filter.resolvedAppeals += 1
          filter.pendingAppeals -= 1
          
          if (appeal.accepted) {
            filter.acceptedAppeals += 1
          } else {
            filter.rejectedAppeals += 1
          }
          
          filter.updatedAt = event.block.timestamp
          filter.save()
        }
      }
      
      appeal.save()
    }
    
    // Create the vote entity
    let vote = new Vote(voteId)
    vote.appeal = appealId
    vote.voter = voterId
    vote.voteInFavor = event.params.voteInFavor
    vote.weight = voteWeight
    vote.timestamp = event.block.timestamp
    vote.blockNumber = event.block.number
    vote.transactionHash = event.transaction.hash
    vote.appealVoteCount = appealVoteCount
    vote.isDecisive = isDecisive
    vote.save()
    
    // Create operation record
    let operationId = event.transaction.hash.toHexString() + "-" + event.logIndex.toString()
    let operation = new FilterOperation(operationId)
    operation.filter = appeal ? appeal.filter : event.address.toHexString()
    operation.operationType = 'VOTE_CAST'
    operation.timestamp = event.block.timestamp
    operation.blockNumber = event.block.number
    operation.transactionHash = event.transaction.hash
    operation.caller = event.transaction.from
    operation.details = JSON.stringify({
      'appealId': appealId,
      'voter': voterId,
      'voteInFavor': event.params.voteInFavor,
      'newVoteCount': event.params.newVoteCount.toString()
    })
    operation.gasUsed = event.receipt ? event.receipt.gasUsed : null
    operation.gasPrice = event.transaction.gasPrice
    operation.save()
    
    // If vote is decisive, create a metrics snapshot
    if (isDecisive && appeal) {
      createMetricsSnapshot(appeal.filter, event.block.timestamp, event.block.number)
    }
    
    // Track daily metrics including active consortium members
    const dailyMetricId = appeal ? appeal.filter + '-' + formatDateString(event.block.timestamp) : 
      event.address.toHexString() + '-' + formatDateString(event.block.timestamp);
      
    let dailyMetric = DailyMetrics.load(dailyMetricId);
    if (dailyMetric && member) {
      dailyMetric.activeConsortiumMembers += 1;
      dailyMetric.voteCount += 1;
      dailyMetric.save();
    } else {
      updateDailyMetrics(
        appeal ? appeal.filter : event.address.toHexString(),
        event.block.timestamp,
        'VOTE_CAST',
        event.receipt ? event.receipt.gasUsed : null
      );
    }
  } catch (error) {
    log.error("Error handling AppealVoteCast: {}", [error.toString()]);
  }
}

/**
 * Format a timestamp into a YYYY-MM-DD string for daily metrics
 */
function formatDateString(timestamp: BigInt): string {
  const date = new Date(timestamp.toI64() * 1000);
  const year = date.getUTCFullYear().toString();
  const month = (date.getUTCMonth() + 1).toString().padStart(2, '0');
  const day = date.getUTCDate().toString().padStart(2, '0');
  return year + '-' + month + '-' + day;
}

/**
 * Handle AppealResolved events
 * Updates Appeal entities when appeals are resolved
 */
export function handleAppealResolved(event: AppealResolved): void {
  const appealId = event.params.appealId.toHexString()
  
  let appeal = Appeal.load(appealId)
  if (appeal) {
    appeal.resolved = event.params.resolved
    appeal.accepted = event.params.accepted
    appeal.status = appeal.accepted ? 'ACCEPTED' : 'REJECTED'
    appeal.resolutionTime = event.block.timestamp
    appeal.resolutionTxHash = event.transaction.hash
    appeal.pendingDuration = event.block.timestamp.minus(appeal.timestamp)
    appeal.save()
    
    // Update filter appeal counts
    let filter = Filter.load(appeal.filter)
    if (filter) {
      filter.resolvedAppeals += 1
      filter.pendingAppeals -= 1
      
      if (appeal.accepted) {
        filter.acceptedAppeals += 1
      } else {
        filter.rejectedAppeals += 1
      }
      
      filter.updatedAt = event.block.timestamp
      filter.save()
      
      // Create metrics snapshot
      createMetricsSnapshot(appeal.filter, event.block.timestamp, event.block.number)
    }
    
    // Create operation record
    let operationId = event.transaction.hash.toHexString() + "-" + event.logIndex.toString()
    let operation = new FilterOperation(operationId)
    operation.filter = appeal.filter
    operation.operationType = 'APPEAL_RESOLVED'
    operation.timestamp = event.block.timestamp
    operation.blockNumber = event.block.number
    operation.transactionHash = event.transaction.hash
    operation.caller = event.transaction.from
    operation.details = JSON.stringify({
      'appealId': appealId,
      'accepted': event.params.accepted
    })
    operation.gasUsed = event.receipt ? event.receipt.gasUsed : null
    operation.gasPrice = event.transaction.gasPrice
    operation.save()
  }
}

/**
 * Handle EmergencyAction events
 */
export function handleEmergencyAction(event: EmergencyAction): void {
  const actionId = event.transaction.hash.toHexString()
  const filterId = event.address.toHexString()
  
  // Create emergency action entity
  let action = new EmergencyActionEntity(actionId)
  action.filter = filterId
  action.actionType = event.params.action
  action.actor = event.params.actor
  action.timestamp = event.block.timestamp
  action.blockNumber = event.block.number
  action.transactionHash = event.transaction.hash
  action.reason = event.params.data.toString()
  action.resolved = false
  
  // Try to parse data for related entities
  if (event.params.data.length >= 64) {
    // Check for migration-related data
    const dataStr = event.params.data.toString()
    if (dataStr.includes("MIGRATION")) {
      const migrationParams = dataStr.split(',')
      if (migrationParams.length >= 3) {
        const sourceId = migrationParams[1]
        const targetId = migrationParams[2]
        if (sourceId && targetId) {
          const migrationId = sourceId + '-to-' + targetId
          action.related = migrationId
        }
      }
    }
    
    // Check for appeal-related data
    if (dataStr.includes("APPEAL")) {
      const appealParams = dataStr.split(',')
      if (appealParams.length >= 2) {
        const appealId = appealParams[1]
        if (appealId) {
          action.affectedAppeal = appealId
        }
      }
    }
  }
  
  action.save()
  
  // Update system config
  let config = getOrCreateSystemConfig(event.block.timestamp)
  config.lastUpdated = event.block.timestamp
  config.updatedBy = event.params.actor
  config.updateCount += 1
  config.save()
  
  // Create operation record
  let operationId = event.transaction.hash.toHexString() + "-" + event.logIndex.toString()
  let operation = new FilterOperation(operationId)
  operation.filter = filterId
  operation.operationType = 'EMERGENCY_' + event.params.action
  operation.timestamp = event.block.timestamp
  operation.blockNumber = event.block.number
  operation.transactionHash = event.transaction.hash
  operation.caller = event.params.actor
  operation.details = JSON.stringify({
    'action': event.params.action,
    'data': event.params.data.toString()
  })
  operation.gasUsed = event.receipt ? event.receipt.gasUsed : null
  operation.gasPrice = event.transaction.gasPrice
  operation.save()
}

/**
 * Handle FilterPaused events
 * Updates system config when filter is paused
 */
export function handleFilterPaused(event: FilterPaused): void {
  // Update system config
  let config = getOrCreateSystemConfig(event.block.timestamp)
  config.paused = true
  config.lastPausedAt = event.block.timestamp
  config.lastUpdated = event.block.timestamp
  config.updatedBy = event.params.pauser
  config.updateCount += 1
  config.save()
  
  // Create operation record
  let operationId = event.transaction.hash.toHexString() + "-" + event.logIndex.toString()
  let operation = new FilterOperation(operationId)
  operation.filter = event.address.toHexString()
  operation.operationType = 'PAUSE'
  operation.timestamp = event.block.timestamp
  operation.blockNumber = event.block.number
  operation.transactionHash = event.transaction.hash
  operation.caller = event.params.pauser
  operation.gasUsed = event.receipt ? event.receipt.gasUsed : null
  operation.gasPrice = event.transaction.gasPrice
  operation.save()
}

/**
 * Handle FilterUnpaused events
 * Updates system config when filter is unpaused
 */
export function handleFilterUnpaused(event: FilterUnpaused): void {
  // Update system config
  let config = getOrCreateSystemConfig(event.block.timestamp)
  config.paused = false
  config.lastUnpausedAt = event.block.timestamp
  config.lastUpdated = event.block.timestamp
  config.updatedBy = event.params.unpauser
  config.updateCount += 1
  config.save()
  
  // Create operation record
  let operationId = event.transaction.hash.toHexString() + "-" + event.logIndex.toString()
  let operation = new FilterOperation(operationId)
  operation.filter = event.address.toHexString()
  operation.operationType = 'UNPAUSE'
  operation.timestamp = event.block.timestamp
  operation.blockNumber = event.block.number
  operation.transactionHash = event.transaction.hash
  operation.caller = event.params.unpauser
  operation.gasUsed = event.receipt ? event.receipt.gasUsed : null
  operation.gasPrice = event.transaction.gasPrice
  operation.save()
}

/**
 * Handle ScalingConfigUpdated events
 * Updates system config with new scaling parameters
 */
export function handleScalingConfigUpdated(event: ScalingConfigUpdated): void {
  // Update system config
  let config = getOrCreateSystemConfig(event.block.timestamp)
  config.batchSize = event.params.batchSize
  config.targetFPRbps = event.params.targetFPRbps.toI32()
  config.parallelQueryPeriod = event.params.parallelQueryPeriod
  config.lastUpdated = event.block.timestamp
  config.updatedBy = event.transaction.from
  config.updateCount += 1
  config.save()
  
  // Create operation record
  let operationId = event.transaction.hash.toHexString() + "-" + event.logIndex.toString()
  let operation = new FilterOperation(operationId)
  operation.filter = event.address.toHexString()
  operation.operationType = 'CONFIG_UPDATED'
  operation.timestamp = event.block.timestamp
  operation.blockNumber = event.block.number
  operation.transactionHash = event.transaction.hash
  operation.caller = event.transaction.from
  operation.details = JSON.stringify({
    'batchSize': event.params.batchSize.toString(),
    'targetFPRbps': event.params.targetFPRbps.toString(),
    'parallelQueryPeriod': event.params.parallelQueryPeriod.toString()
  })
  operation.gasUsed = event.receipt ? event.receipt.gasUsed : null
  operation.gasPrice = event.transaction.gasPrice
  operation.save()
}

/**
 * Create periodic metrics snapshots to track filter performance over time
 */
function createMetricsSnapshot(filterId: string, timestamp: BigInt, blockNumber: BigInt): void {
  try {
    const snapshotId = filterId + '-' + timestamp.toString();
    
    let filter = safeLoadFilter(filterId);
    if (!filter) {
      log.warning("Filter not found in createMetricsSnapshot: {}", [filterId]);
      return;
    }
    
    let metrics = new FilterMetrics(snapshotId)
    metrics.filter = filterId
    metrics.timestamp = timestamp
    metrics.blockNumber = blockNumber
    metrics.size = filter.size
    metrics.insertCount = filter.insertCount
    metrics.fillRatioBps = filter.fillRatioBps
    metrics.estimatedFPRbps = filter.estimatedFPRbps
    metrics.appealCount = filter.totalAppeals
    metrics.pendingAppeals = filter.pendingAppeals
    metrics.healthStatus = filter.healthStatus
    
    // Calculate change rates
    let prevMetrics = getLatestMetrics(filterId, timestamp)
    if (prevMetrics) {
      // Calculate insert rate (inserts per day)
      let timeDiff = timestamp.minus(prevMetrics.timestamp).toI32()
      if (timeDiff > 0) {
        let insertDiff = filter.insertCount.minus(prevMetrics.insertCount).toI32()
        let insertsPerSecond = insertDiff / timeDiff
        metrics.insertRate = insertsPerSecond * SECONDS_PER_DAY
        
        // Calculate fill rate change
        metrics.fillRateChange = filter.fillRatioBps - prevMetrics.fillRatioBps
        
        // Forecast days to threshold
        if (metrics.fillRateChange > 0) {
          let config = getOrCreateSystemConfig(timestamp)
          let remainingPoints = config.autoScaleThresholdBps - filter.fillRatioBps
          let pointsPerSecond = metrics.fillRateChange / timeDiff
          let secondsToThreshold = remainingPoints / pointsPerSecond
          metrics.daysToThreshold = secondsToThreshold / SECONDS_PER_DAY
          
          // Project FPR at threshold
          let additionalItems = (remainingPoints * filter.insertCount.toI32()) / filter.fillRatioBps
          metrics.projectedFPRbps = estimateFPR(
            filter.size.toI32(), 
            filter.numHashes, 
            filter.insertCount.toI32() + additionalItems
          )
        }
      }
    } else {
      metrics.insertRate = 0.0
      metrics.fillRateChange = 0.0
    }
    
    metrics.save()
    
    // Calculate TWMA for fill ratio
    updateTimeWeightedMetrics(
      filterId,
      timestamp,
      'FILL_RATIO',
      filter.fillRatioBps
    );
    
    // Calculate TWMA for estimated FPR
    updateTimeWeightedMetrics(
      filterId,
      timestamp,
      'ESTIMATED_FPR',
      filter.estimatedFPRbps
    );
    
  } catch (error) {
    log.error("Error creating metrics snapshot: {}", [error.toString()]);
  }
}

/**
 * Create performance metrics aggregations
 */
function createPerformanceMetrics(filterId: string, timestamp: BigInt): void {
  try {
    // Create hourly, daily and weekly aggregations
    createPeriodPerformanceMetrics(filterId, timestamp, 'HOURLY', 60 * 60); // Last hour
    createPeriodPerformanceMetrics(filterId, timestamp, 'DAILY', 24 * 60 * 60); // Last day
    createPeriodPerformanceMetrics(filterId, timestamp, 'WEEKLY', 7 * 24 * 60 * 60); // Last week
    
  } catch (error) {
    log.error("Error creating performance metrics: {}", [error.toString()]);
  }
}

/**
 * Create performance metrics for a specific time period
 */
function createPeriodPerformanceMetrics(filterId: string, timestamp: BigInt, period: string, timeSeconds: i32): void {
  try {
    const startTime = timestamp.minus(BigInt.fromI32(timeSeconds))
    const metricsId = filterId + '-' + period + '-' + timestamp.toString()
    
    let metrics = new PerformanceMetrics(metricsId)
    metrics.filter = filterId
    metrics.period = period
    metrics.timestamp = timestamp
    
    // Count operations during period
    let operations = FilterOperation.find(
      op => op.filter == filterId && 
      op.timestamp >= startTime && 
      op.timestamp <= timestamp
    )
    
    // Count and total gas for each operation type
    let insertCount = 0
    let queryCount = 0
    let insertGasTotal = BigInt.fromI32(0)
    let queryGasTotal = BigInt.fromI32(0)
    let totalGas = BigInt.fromI32(0)
    let totalCost = BigInt.fromI32(0)
    
    // Timing arrays for percentile calculation
    let insertTimes: i32[] = []
    let queryTimes: i32[] = []
    
    for (let i = 0; i < operations.length; i++) {
      let op = operations[i]
      
      // Track gas
      if (op.gasUsed) {
        totalGas = totalGas.plus(op.gasUsed)
        
        // Calculate cost (gas * gas price)
        if (op.gasPrice) {
          let cost = op.gasUsed.times(op.gasPrice)
          totalCost = totalCost.plus(cost)
        }
      }
      
      // Track specific operation types
      if (op.operationType == 'INSERT') {
        insertCount++
        if (op.gasUsed) {
          insertGasTotal = insertGasTotal.plus(op.gasUsed)
        }
        
        // Estimate timing based on block time differences
        if (i > 0 && operations[i-1].operationType == 'INSERT') {
          let timeDiff = op.timestamp.minus(operations[i-1].timestamp).toI32()
          if (timeDiff > 0 && timeDiff < 60) { // Reasonable time difference < 60s
            insertTimes.push(timeDiff * 1000) // Convert to milliseconds
          }
        }
      }
      
      if (op.operationType == 'QUERY') {
        queryCount++
        if (op.gasUsed) {
          queryGasTotal = queryGasTotal.plus(op.gasUsed)
        }
        
        // Estimate timing based on block time differences
        if (i > 0 && operations[i-1].operationType == 'QUERY') {
          let timeDiff = op.timestamp.minus(operations[i-1].timestamp).toI32()
          if (timeDiff > 0 && timeDiff < 30) { // Reasonable time difference < 30s
            queryTimes.push(timeDiff * 1000) // Convert to milliseconds
          }
        }
      }
    }
    
    // Calculate averages
    metrics.insertCount = insertCount
    metrics.queryCount = queryCount
    metrics.totalGasUsed = totalGas
    metrics.totalCost = totalCost
    
    // Average gas calculations
    if (insertCount > 0) {
      metrics.averageGasPerInsert = insertGasTotal.div(BigInt.fromI32(insertCount))
      metrics.costPerInsert = totalCost.div(BigInt.fromI32(insertCount))
    } else {
      metrics.averageGasPerInsert = BigInt.fromI32(0)
      metrics.costPerInsert = BigInt.fromI32(0)
    }
    
    if (queryCount > 0) {
      metrics.averageGasPerQuery = queryGasTotal.div(BigInt.fromI32(queryCount))
      metrics.costPerQuery = totalCost.div(BigInt.fromI32(queryCount))
    } else {
      metrics.averageGasPerQuery = BigInt.fromI32(0)
      metrics.costPerQuery = BigInt.fromI32(0)
    }
    
    // Average time calculations
    if (insertTimes.length > 0) {
      let sum = 0
      for (let i = 0; i < insertTimes.length; i++) {
        sum += insertTimes[i]
      }
      metrics.averageInsertTime = BigInt.fromI32(sum / insertTimes.length)
      
      // Calculate p95
      insertTimes.sort()
      let p95Index = Math.floor(insertTimes.length * 0.95)
      metrics.p95InsertTime = BigInt.fromI32(insertTimes[p95Index])
      
      // Update TWMA for p95 insert time
      updateTimeWeightedMetrics(
        filterId,
        timestamp,
        'P95_INSERT_TIME',
        insertTimes[p95Index]
      );
      
      // Update TWMA for average insert time
      updateTimeWeightedMetrics(
        filterId,
        timestamp,
        'AVG_INSERT_TIME',
        metrics.averageInsertTime.toI32()
      );
    } else {
      metrics.averageInsertTime = BigInt.fromI32(0)
      metrics.p95InsertTime = BigInt.fromI32(0)
    }
    
    if (queryTimes.length > 0) {
      let sum = 0
      for (let i = 0; i < queryTimes.length; i++) {
        sum += queryTimes[i]
      }
      metrics.averageQueryTime = BigInt.fromI32(sum / queryTimes.length)
      
      // Calculate p95
      queryTimes.sort()
      let p95Index = Math.floor(queryTimes.length * 0.95)
      metrics.p95QueryTime = BigInt.fromI32(queryTimes[p95Index])
      
      // Update TWMA for p95 query time
      updateTimeWeightedMetrics(
        filterId,
        timestamp,
        'P95_QUERY_TIME',
        queryTimes[p95Index]
      );
      
      // Update TWMA for average query time
      updateTimeWeightedMetrics(
        filterId,
        timestamp,
        'AVG_QUERY_TIME',
        metrics.averageQueryTime.toI32()
      );
    } else {
      metrics.averageQueryTime = BigInt.fromI32(0)
      metrics.p95QueryTime = BigInt.fromI32(0)
    }
    
    metrics.save()
    
  } catch (error) {
    log.error("Error creating period performance metrics: {}", [error.toString()]);
  }
}

/**
 * Get the most recent metrics snapshot for a filter
 */
function getLatestMetrics(filterId: string, before: BigInt): FilterMetrics | null {
  let metrics = FilterMetrics.find(
    m => m.filter == filterId && m.timestamp < before,
    [{ field: "timestamp", direction: "desc" }],
    1
  )
  
  return metrics.length > 0 ? metrics[0] : null
}

/**
 * Estimate FPR based on filter parameters
 */
function estimateFPR(size: i32, numHashes: i32, itemCount: i32): i32 {
  // Simple approximation for FPR calculation
  // (1-(1-1/m)^(k*n))^k where m=bits, k=hash functions, n=items
  
  let m = size * 256
  let k = numHashes
  let n = itemCount
  
  // For very small loads or empty filters
  if (n == 0 || m == 0) return 0
  
  // Simplified estimate using k*n/m approximation for small FPRs
  let filling = (k * n * BASIS_POINTS_SCALE) / m
  
  // Apply probability scaling factor (simplified for AssemblyScript)
  let fpr = Math.min(filling, BASIS_POINTS_SCALE)
  
  return fpr
}

// Other event handlers with stub implementations
export function handleFilterCleared(event: FilterCleared): void {
  let filterId = event.address.toHexString()
  
  // Update filter
  let filter = Filter.load(filterId)
  if (filter) {
    filter.insertCount = BigInt.fromI32(0)
    filter.fillRatioBps = 0
    filter.estimatedFPRbps = 0
    filter.updatedAt = event.block.timestamp
    filter.save()
    
    // Create operation record
    let operationId = event.transaction.hash.toHexString() + "-" + event.logIndex.toString()
    let operation = new FilterOperation(operationId)
    operation.filter = filterId
    operation.operationType = 'CLEAR'
    operation.timestamp = event.block.timestamp
    operation.blockNumber = event.block.number
    operation.transactionHash = event.transaction.hash
    operation.caller = event.transaction.from
    operation.details = JSON.stringify({
      'size': event.params.size.toString(),
      'numHashes': event.params.numHashes.toString()
    })
    operation.gasUsed = event.receipt ? event.receipt.gasUsed : null
    operation.gasPrice = event.transaction.gasPrice
    operation.save()
    
    // Create metrics snapshot
    createMetricsSnapshot(filterId, event.block.timestamp, event.block.number)
  }
}

export function handleFilterReset(event: FilterReset): void {
  let filterId = event.address.toHexString()
  
  // Update filter
  let filter = Filter.load(filterId)
  if (filter) {
    filter.size = event.params.newSize
    filter.numHashes = event.params.newNumHashes.toI32()
    filter.salt = event.params.newSalt
    filter.insertCount = BigInt.fromI32(0)
    filter.bitCount = filter.size.times(BigInt.fromI32(256))
    filter.fillRatioBps = 0
    filter.estimatedFPRbps = 0
    filter.updatedAt = event.block.timestamp
    filter.version = filter.version + 1
    filter.save()
    
    // Create operation record
    let operationId = event.transaction.hash.toHexString() + "-" + event.logIndex.toString()
    let operation = new FilterOperation(operationId)
    operation.filter = filterId
    operation.operationType = 'RESET'
    operation.timestamp = event.block.timestamp
    operation.blockNumber = event.block.number
    operation.transactionHash = event.transaction.hash
    operation.caller = event.transaction.from
    operation.details = JSON.stringify({
      'newSize': event.params.newSize.toString(),
      'newNumHashes': event.params.newNumHashes.toString(),
      'newSalt': event.params.newSalt.toString()
    })
    operation.gasUsed = event.receipt ? event.receipt.gasUsed : null
    operation.gasPrice = event.transaction.gasPrice
    operation.save()
    
    // Create metrics snapshot
    createMetricsSnapshot(filterId, event.block.timestamp, event.block.number)
  }
}

export function handleFilterScaled(event: FilterScaled): void {
  let filterId = event.address.toHexString()
  
  // Create operation record
  let operationId = event.transaction.hash.toHexString() + "-" + event.logIndex.toString()
  let operation = new FilterOperation(operationId)
  operation.filter = filterId
  operation.operationType = 'SCALE'
  operation.timestamp = event.block.timestamp
  operation.blockNumber = event.block.number
  operation.transactionHash = event.transaction.hash
  operation.caller = event.transaction.from
  operation.details = JSON.stringify({
    'oldSize': event.params.oldSize.toString(),
    'newSize': event.params.newSize.toString(),
    'itemCount': event.params.itemCount.toString()
  })
  operation.gasUsed = event.receipt ? event.receipt.gasUsed : null
  operation.gasPrice = event.transaction.gasPrice
  operation.save()
  
  // Update filter metrics and create snapshot
  createMetricsSnapshot(filterId, event.block.timestamp, event.block.number)
}
