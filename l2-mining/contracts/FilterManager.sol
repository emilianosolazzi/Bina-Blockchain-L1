// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { BloomFilterLib } from "./BloomFilterLib.sol";
import { OwnableUpgradeable } from "@openzeppelin/contracts-upgradeable/access/OwnableUpgradeable.sol";
import { AccessControlUpgradeable } from "@openzeppelin/contracts-upgradeable/access/AccessControlUpgradeable.sol";
import { PausableUpgradeable } from "@openzeppelin/contracts-upgradeable/utils/PausableUpgradeable.sol";

/**
 * @title FilterManager
 * @notice Manages active Bloom filters with zero-downtime scaling, migration, and versioning
 * @dev Enhanced with advanced governance, security, and seamless scaling capabilities
 */
contract FilterManager is 
    OwnableUpgradeable, 
    AccessControlUpgradeable, 
    PausableUpgradeable 
{
    using BloomFilterLib for BloomFilterLib.Filter;

    uint256 private constant MAX_HASH_FUNCTIONS = 8;
    uint256 private constant MIN_FILTER_BUCKETS = 128;
    uint256 private constant MAX_FILTER_BUCKETS = 65536;
    uint256 private constant HASH_COUNT_SCALE = 1000;
    uint256 private constant HASH_COUNT_NUMERATOR = 693;
    uint256 private constant NOT_ENTERED = 1;
    uint256 private constant ENTERED = 2;

    // Role definitions for fine-grained access control
    bytes32 public constant SCALING_MANAGER_ROLE = keccak256("SCALING_MANAGER_ROLE");
    bytes32 public constant APPEAL_MANAGER_ROLE = keccak256("APPEAL_MANAGER_ROLE");
    bytes32 public constant CONSORTIUM_MEMBER_ROLE = keccak256("CONSORTIUM_MEMBER_ROLE");
    bytes32 public constant PAUSER_ROLE = keccak256("PAUSER_ROLE");
    bytes32 public constant EMERGENCY_ROLE = keccak256("EMERGENCY_ROLE");
    bytes32 public constant INSERTER_ROLE = keccak256("INSERTER_ROLE");

    // Migration states to track scaling operations
    enum MigrationState {
        Inactive,
        Preparing,
        Migrating,
        Finalizing,
        Completed,
        Canceled
    }

    struct FilterInfo {
        BloomFilterLib.Filter filter;
        uint256 createdAt;
        bool active;
        uint256 batchesMigrated; // For tracking migration progress
        uint256 totalEntries;    // Total number of entries in this filter
    }

    struct MigrationInfo {
        uint256 sourceFilterId;
        uint256 targetFilterId;
        MigrationState state;
        uint256 startedAt;
        uint256 completedAt;
        uint256 totalBatches;
        uint256 batchesCompleted;
        uint256 entriesMigrated;
    }

    // Scaling/Migration settings
    struct ScalingConfig {
        uint256 batchSize;           // Number of items to migrate per batch
        uint256 targetFPRbps;        // Target FPR in basis points (e.g., 50 = 0.5%)
        uint256 parallelQueryPeriod; // Time period to query both filters (seconds)
        uint256 autoScaleThresholdBps; // Auto-scale when fill ratio exceeds this (basis points)
        bool autoScalingEnabled;     // Whether to enable automatic scaling
    }

    struct OutputEntry {
        bytes32 hash;
        uint256 timestamp;
    }

    // Active filter ID (version)
    uint256 public activeFilterId;
    
    // Previous filter ID (for zero-downtime transition)
    uint256 public previousFilterId;
    
    // Transition end timestamp
    uint256 public transitionEndTime;

    // Mapping filter IDs to FilterInfo
    mapping(uint256 => FilterInfo) private _filters;

    // Incremental filter ID tracker
    uint256 private _nextFilterId;
    
    // Active migration info
    MigrationInfo private _activeMigration;
    
    // Scaling configuration
    ScalingConfig public scalingConfig;

    uint256 private _reentrancyStatus;
    
    // Track outputs marked as appealed
    mapping(bytes32 => bool) private _appealedOutputs;
    
    // Output Tracker - for indexing and pruning
    mapping(uint256 => OutputEntry) private _outputsIndex;
    uint256 private _nextOutputIndex;
    uint256 private _firstOutputIndex;

    // Events
    event FilterCreated(uint256 indexed filterId, uint256 size, uint256 numHashes, uint256 timestamp);
    event FilterActivated(uint256 indexed filterId, uint256 timestamp);
    event ScalingStarted(uint256 indexed sourceFilterId, uint256 indexed targetFilterId, uint256 timestamp);
    event ScalingBatchCompleted(uint256 indexed targetFilterId, uint256 batchIndex, uint256 entriesMigrated);
    event ScalingCompleted(uint256 indexed targetFilterId, uint256 totalEntriesMigrated, uint256 timestamp);
    event ScalingCanceled(uint256 indexed sourceFilterId, uint256 indexed targetFilterId, uint256 timestamp);
    event AppealRegistered(bytes32 indexed output, address indexed reporter, bytes32 indexed appealId);
    event AppealResolved(bytes32 indexed appealId, bool accepted);
    event OutputPruned(uint256 indexed count, uint256 gasRefunded);
    event ScalingConfigUpdated(uint256 batchSize, uint256 targetFPRbps, uint256 parallelQueryPeriod);
    event EmergencyAction(string indexed action, address indexed actor, bytes data);
    event FilterPaused(address indexed pauser);
    event FilterUnpaused(address indexed unpauser);

    modifier nonReentrant() {
        require(_reentrancyStatus != ENTERED, "FilterManager: Reentrant call");
        _reentrancyStatus = ENTERED;
        _;
        _reentrancyStatus = NOT_ENTERED;
    }

    /**
     * @notice Initializes the FilterManager with governance controls
     * @param admin Address that will have admin rights
     * @param scalingManagers Addresses that can initiate scaling operations
     */
    function initialize(
        address admin,
        address[] memory scalingManagers
    ) external initializer {
        __Ownable_init(admin);
        __AccessControl_init();
        __Pausable_init();

        _reentrancyStatus = NOT_ENTERED;
        
        _nextFilterId = 1;
        _nextOutputIndex = 1;
        _firstOutputIndex = 1;
        
        // Setup roles
        _grantRole(DEFAULT_ADMIN_ROLE, admin);
        _grantRole(SCALING_MANAGER_ROLE, admin);
        _grantRole(APPEAL_MANAGER_ROLE, admin);
        _grantRole(PAUSER_ROLE, admin);
        _grantRole(EMERGENCY_ROLE, admin);
        _grantRole(INSERTER_ROLE, admin);
        
        // Grant scaling manager role to additional addresses if provided
        for (uint i = 0; i < scalingManagers.length; i++) {
            _grantRole(SCALING_MANAGER_ROLE, scalingManagers[i]);
        }
        
        // Initialize default scaling config
        scalingConfig = ScalingConfig({
            batchSize: 500,            // Process 500 entries per batch
            targetFPRbps: 50,          // Target 0.5% false positive rate
            parallelQueryPeriod: 1 days, // Query both filters for 1 day after migration
            autoScaleThresholdBps: 7500, // Auto-scale at 75% fill ratio
            autoScalingEnabled: false   // Disabled by default
        });
    }

    /**
     * @notice Pause the contract in case of emergency
     * @dev Can only be called by accounts with PAUSER_ROLE
     */
    function pause() external onlyRole(PAUSER_ROLE) {
        _pause();
        emit FilterPaused(msg.sender);
    }

    /**
     * @notice Unpause the contract after emergency is resolved
     * @dev Can only be called by accounts with PAUSER_ROLE
     */
    function unpause() external onlyRole(PAUSER_ROLE) {
        _unpause();
        emit FilterUnpaused(msg.sender);
    }

    /**
     * @notice Creates a new filter but does not activate it yet
     * @param size Number of buckets (must be a power of 2)
     * @param numHashes Number of hash functions
     * @param salt Randomization salt
     * @return filterId ID of the newly created filter
     */
    function createFilter(
        uint256 size, 
        uint256 numHashes, 
        uint256 salt
    ) external onlyRole(SCALING_MANAGER_ROLE) whenNotPaused returns (uint256 filterId) {
        filterId = _nextFilterId++;

        _initializeFilter(filterId, size, numHashes, salt);

        emit FilterCreated(filterId, size, numHashes, block.timestamp);
    }

    /**
     * @notice Activate a created filter with zero-downtime transition
     * @param filterId The ID of the filter to activate
     * @param transitionPeriod Period during which both filters will be active (in seconds)
     */
    function activateFilter(
        uint256 filterId, 
        uint256 transitionPeriod
    ) external onlyRole(SCALING_MANAGER_ROLE) whenNotPaused nonReentrant {
        require(_filters[filterId].createdAt != 0, "FilterManager: Filter does not exist");
        require(!_filters[filterId].active, "FilterManager: Already active");

        // If there's an active filter, set it as previous
        if (activeFilterId != 0) {
            previousFilterId = activeFilterId;
            transitionEndTime = block.timestamp + transitionPeriod;
        }

        _filters[filterId].active = true;
        activeFilterId = filterId;

        emit FilterActivated(filterId, block.timestamp);
    }

    /**
     * @notice Insert an entry into the active filter and track in output index
     * @param entry Entry to insert
     */
    function insert(bytes32 entry) external onlyRole(INSERTER_ROLE) whenNotPaused nonReentrant {
        require(activeFilterId != 0, "FilterManager: No active filter");
        
        // Insert into active filter
        _filters[activeFilterId].filter.updateFilter(entry);
        _filters[activeFilterId].totalEntries++;
        
        // Track output for potential pruning
        _outputsIndex[_nextOutputIndex++] = OutputEntry({
            hash: entry,
            timestamp: block.timestamp
        });
        
        // Check if auto-scaling should be triggered
        if (scalingConfig.autoScalingEnabled) {
            _checkAndTriggerAutoScaling();
        }
    }

    /**
     * @notice Query if an entry might exist with zero-downtime transition support
     * @dev During transition, checks both active and previous filters
     * @param entry Entry to check
     * @return exists True if the entry might exist
     */
    function mightContain(bytes32 entry) external view returns (bool exists) {
        if (activeFilterId == 0) return false;
        
        // Always check active filter
        exists = _filters[activeFilterId].filter.mightContain(entry);
        
        // During transition period, also check previous filter
        if (!exists && 
            previousFilterId != 0 && 
            block.timestamp <= transitionEndTime) {
            exists = _filters[previousFilterId].filter.mightContain(entry);
        }
        
        // Check if this output has an active appeal
        if (exists && _appealedOutputs[entry]) {
            // If appealed, consider it not in the filter
            return false;
        }
        
        return exists;
    }

    /**
     * @notice Full query: Checks across all filters (active and inactive)
     * @param entry Entry to check
     * @return exists True if entry exists in any filter
     */
    function mightContainAny(bytes32 entry) external view returns (bool exists) {
        for (uint256 i = 1; i < _nextFilterId; i++) {
            if (_filters[i].createdAt != 0 && _filters[i].filter.mightContain(entry)) {
                // Check if this output has an active appeal
                if (_appealedOutputs[entry]) {
                    continue; // Skip if appealed
                }
                return true;
            }
        }
        return false;
    }

    /**
     * @notice Prepare filter scaling with optimal parameters for the target FPR
     * @dev Calculates optimal size and hash count for the target false positive rate
     * @param expectedItems Number of items expected to be stored
     * @return newFilterId ID of the newly created filter optimized for scaling
     */
    function prepareScaledFilter(
        uint256 expectedItems
    ) external onlyRole(SCALING_MANAGER_ROLE) whenNotPaused returns (uint256 newFilterId) {
        require(activeFilterId != 0, "FilterManager: No active filter");
        require(_activeMigration.state == MigrationState.Inactive, "FilterManager: Migration already in progress");
        
        // Get current filter stats
        FilterInfo storage currentFilter = _filters[activeFilterId];
        
        // Calculate optimal new filter parameters based on expected items
        (uint256 newSize, uint256 newNumHashes) = _calculateOptimalFilterParams(
            expectedItems,
            scalingConfig.targetFPRbps
        );
        
        // Create salt by combining existing salt with current timestamp for uniqueness
        uint256 newSalt = uint256(keccak256(abi.encodePacked(
            block.timestamp, 
            block.prevrandao, 
            currentFilter.filter.salt
        )));
        
        // Create new optimally sized filter
        newFilterId = _nextFilterId++;

        _initializeFilter(newFilterId, newSize, newNumHashes, newSalt);
        
        emit FilterCreated(newFilterId, newSize, newNumHashes, block.timestamp);
        
        // Setup migration state
        _activeMigration = MigrationInfo({
            sourceFilterId: activeFilterId,
            targetFilterId: newFilterId,
            state: MigrationState.Preparing,
            startedAt: block.timestamp,
            completedAt: 0,
            totalBatches: _calculateTotalBatches(currentFilter.totalEntries),
            batchesCompleted: 0,
            entriesMigrated: 0
        });
        
        emit ScalingStarted(activeFilterId, newFilterId, block.timestamp);
        
        return newFilterId;
    }
    
    /**
     * @notice Start the actual migration process after preparation
     * @dev Sets the migration state to Migrating, allowing batch processing
     */
    function startMigration() external onlyRole(SCALING_MANAGER_ROLE) whenNotPaused nonReentrant {
        require(_activeMigration.state == MigrationState.Preparing, "FilterManager: Not in preparing state");
        
        _activeMigration.state = MigrationState.Migrating;
    }

    /**
     * @notice Process a batch of entries during migration for gas efficiency
     * @param maxGas Maximum gas to use in this batch operation (0 for unlimited)
     * @return completed Whether the entire migration is now complete
     * @return batchesProcessed Number of batches processed in this call
     */
    function processMigrationBatch(
        uint256 maxGas
    ) external onlyRole(SCALING_MANAGER_ROLE) whenNotPaused nonReentrant returns (bool completed, uint256 batchesProcessed) {
        require(_activeMigration.state == MigrationState.Migrating, "FilterManager: Not in migration state");
        
        uint256 sourceId = _activeMigration.sourceFilterId;
        uint256 targetId = _activeMigration.targetFilterId;
        uint256 gasStart = gasleft();
        batchesProcessed = 0;
        
        // Process batches until max gas used or migration complete
        while (
            (maxGas == 0 || gasStart - gasleft() < maxGas) && 
            _activeMigration.batchesCompleted < _activeMigration.totalBatches
        ) {
            uint256 batchStart = _firstOutputIndex + _filters[sourceId].batchesMigrated;
            uint256 batchEnd = batchStart + scalingConfig.batchSize;
            uint256 entriesMigrated = 0;
            
            // Migrate this batch of entries from outputs index
            for (uint256 i = batchStart; i < batchEnd && i < _nextOutputIndex; i++) {
                OutputEntry storage indexedOutput = _outputsIndex[i];
                bytes32 entry = indexedOutput.hash;
                
                // Only migrate if entry exists and is not appealed
                if (entry != bytes32(0) && !_appealedOutputs[entry]) {
                    if (_filters[sourceId].filter.mightContain(entry)) {
                        _filters[targetId].filter.updateFilter(entry);
                        entriesMigrated++;
                    }
                }
            }
            
            _filters[sourceId].batchesMigrated += scalingConfig.batchSize;
            _activeMigration.batchesCompleted++;
            _activeMigration.entriesMigrated += entriesMigrated;
            _filters[targetId].totalEntries += entriesMigrated;
            
            emit ScalingBatchCompleted(targetId, _activeMigration.batchesCompleted, entriesMigrated);
            batchesProcessed++;
            
            // Check if we've completed all batches
            if (_activeMigration.batchesCompleted >= _activeMigration.totalBatches) {
                _activeMigration.state = MigrationState.Finalizing;
                break;
            }
        }
        
        // If finished, update state
        if (_activeMigration.batchesCompleted >= _activeMigration.totalBatches) {
            completed = true;
        }
        
        return (completed, batchesProcessed);
    }
    
    /**
     * @notice Finalize migration by activating the target filter
     * @param transitionPeriod Period during which both filters will be active (in seconds)
     */
    function finalizeMigration(
        uint256 transitionPeriod
    ) external onlyRole(SCALING_MANAGER_ROLE) whenNotPaused nonReentrant {
        require(_activeMigration.state == MigrationState.Finalizing, "FilterManager: Not ready to finalize");
        
        uint256 targetId = _activeMigration.targetFilterId;
        
        // Activate the new filter with zero-downtime transition
        previousFilterId = activeFilterId;
        activeFilterId = targetId;
        transitionEndTime = block.timestamp + transitionPeriod;
        
        _filters[targetId].active = true;
        
        // Update migration state
        _activeMigration.state = MigrationState.Completed;
        _activeMigration.completedAt = block.timestamp;
        
        emit ScalingCompleted(targetId, _activeMigration.entriesMigrated, block.timestamp);
        emit FilterActivated(targetId, block.timestamp);
    }
    
    /**
     * @notice Cancel an in-progress migration
     * @dev Can only be called before finalization
     */
    function cancelMigration() external onlyRole(SCALING_MANAGER_ROLE) whenNotPaused nonReentrant {
        require(
            _activeMigration.state == MigrationState.Preparing || 
            _activeMigration.state == MigrationState.Migrating,
            "FilterManager: Can't cancel at current state"
        );
        
        uint256 sourceId = _activeMigration.sourceFilterId;
        uint256 targetId = _activeMigration.targetFilterId;
        
        // Reset source filter's migration counter
        _filters[sourceId].batchesMigrated = 0;
        
        // Mark migration as canceled
        _activeMigration.state = MigrationState.Canceled;
        
        emit ScalingCanceled(sourceId, targetId, block.timestamp);
    }

    /**
     * @notice Report a false positive and register an appeal
     * @param entry Entry incorrectly identified as present
     * @return appealId Unique identifier for this appeal
     */
    function reportFalsePositive(
        bytes32 entry,
        bytes calldata /* proof */
    ) external whenNotPaused nonReentrant returns (bytes32 appealId) {
        require(activeFilterId != 0, "FilterManager: No active filter");
        require(this.mightContain(entry), "FilterManager: Not a false positive");
        require(!_appealedOutputs[entry], "FilterManager: Already appealed");
        
        // Mark as appealed
        _appealedOutputs[entry] = true;
        
        // Generate appeal ID
        appealId = keccak256(abi.encodePacked(entry, msg.sender, block.number, block.timestamp));
        
        emit AppealRegistered(entry, msg.sender, appealId);
        
        return appealId;
    }
    
    /**
     * @notice Resolve a false positive appeal
     * @param entry The entry that was appealed
     * @param appealId The ID of the appeal to resolve
     * @param accepted Whether to accept the appeal
     */
    function resolveAppeal(
        bytes32 entry,
        bytes32 appealId,
        bool accepted
    ) external onlyRole(APPEAL_MANAGER_ROLE) whenNotPaused nonReentrant {
        require(_appealedOutputs[entry], "FilterManager: No active appeal");
        
        if (!accepted) {
            // If rejected, remove appeal mark
            delete _appealedOutputs[entry];
        }
        // If accepted, keep it marked so it won't be considered "contained"
        
        emit AppealResolved(appealId, accepted);
    }
    
    /**
     * @notice Prune old outputs to recover gas and optimize storage
     * @param maxOutputs Maximum number of outputs to prune
     * @param minAge Minimum age in seconds for outputs to be pruned
     * @return pruned Number of outputs pruned
     * @return gasRefunded Estimate of gas refunded (approximate)
     */
    function pruneOutputs(
        uint256 maxOutputs,
        uint256 minAge
    ) external onlyRole(SCALING_MANAGER_ROLE) whenNotPaused nonReentrant returns (uint256 pruned, uint256 gasRefunded) {
        uint256 minTimestamp = block.timestamp - minAge;
        uint256 endIndex = _firstOutputIndex + maxOutputs;
        
        if (endIndex > _nextOutputIndex) {
            endIndex = _nextOutputIndex;
        }
        
        for (uint256 i = _firstOutputIndex; i < endIndex; i++) {
            OutputEntry storage indexedOutput = _outputsIndex[i];
            bytes32 entry = indexedOutput.hash;
            
            // Skip already pruned or appealed entries
            if (entry == bytes32(0) || _appealedOutputs[entry]) {
                continue;
            }
            
            uint256 entryTimestamp = indexedOutput.timestamp;
            
            // Only prune entries older than minAge and not appealed
            if (entryTimestamp < minTimestamp) {
                delete _outputsIndex[i];
                pruned++;
            }
        }
        
        // Update first output index
        _firstOutputIndex = endIndex;
        
        // Calculate approximate gas refund (each storage slot cleared refunds ~15K gas)
        gasRefunded = pruned * 15000;
        
        emit OutputPruned(pruned, gasRefunded);
        
        return (pruned, gasRefunded);
    }

    /**
     * @notice Emergency function to force cancel a migration
     * @dev Bypasses normal state constraints for emergency use only
     * @param reason Description of the emergency situation
     */
    function emergencyCancel(string calldata reason) external onlyRole(EMERGENCY_ROLE) nonReentrant {
        uint256 sourceId = _activeMigration.sourceFilterId;
        uint256 targetId = _activeMigration.targetFilterId;
        
        // Reset migration state regardless of current state
        _activeMigration.state = MigrationState.Canceled;
        _filters[sourceId].batchesMigrated = 0;
        
        emit ScalingCanceled(sourceId, targetId, block.timestamp);
        emit EmergencyAction("CANCEL_MIGRATION", msg.sender, abi.encode(reason));
    }
    
    /**
     * @notice Emergency function to immediately finalize a migration
     * @dev Bypasses batch processing for emergency situations
     * @param transitionPeriod Period during which both filters will be active
     * @param reason Description of the emergency situation
     */
    function emergencyFinalize(
        uint256 transitionPeriod,
        string calldata reason
    ) external onlyRole(EMERGENCY_ROLE) nonReentrant {
        // Skip normal state requirements - force finalization
        uint256 targetId = _activeMigration.targetFilterId;
        
        // Activate the target filter
        previousFilterId = activeFilterId;
        activeFilterId = targetId;
        transitionEndTime = block.timestamp + transitionPeriod;
        
        _filters[targetId].active = true;
        
        // Update migration state
        _activeMigration.state = MigrationState.Completed;
        _activeMigration.completedAt = block.timestamp;
        _activeMigration.batchesCompleted = _activeMigration.totalBatches;
        
        emit ScalingCompleted(targetId, _activeMigration.entriesMigrated, block.timestamp);
        emit FilterActivated(targetId, block.timestamp);
        emit EmergencyAction("EMERGENCY_FINALIZE", msg.sender, abi.encode(reason));
    }
    
    /**
     * @notice Force reset the active migration state
     * @dev For use when migration is stuck or corrupt
     * @param reason Description of the emergency situation
     */
    function resetMigrationState(string calldata reason) external onlyRole(EMERGENCY_ROLE) nonReentrant {
        // Record data for the event
        uint256 sourceId = _activeMigration.sourceFilterId;
        uint256 targetId = _activeMigration.targetFilterId;
        MigrationState state = _activeMigration.state;
        
        // Reset to inactive state
        _activeMigration = MigrationInfo({
            sourceFilterId: 0,
            targetFilterId: 0,
            state: MigrationState.Inactive,
            startedAt: 0,
            completedAt: 0,
            totalBatches: 0,
            batchesCompleted: 0,
            entriesMigrated: 0
        });
        
        emit EmergencyAction("RESET_MIGRATION", msg.sender, 
            abi.encode(reason, sourceId, targetId, uint8(state)));
    }
    
    /**
     * @notice Emergency removal of a false positive appeal
     * @dev For cases where appeal system is being abused
     * @param entry The entry to remove from appealed outputs
     * @param reason Description of why this emergency action is needed
     */
    function emergencyRemoveAppeal(
        bytes32 entry,
        string calldata reason
    ) external onlyRole(EMERGENCY_ROLE) nonReentrant {
        require(_appealedOutputs[entry], "FilterManager: Entry not appealed");
        
        delete _appealedOutputs[entry];
        
        emit EmergencyAction("REMOVE_APPEAL", msg.sender, 
            abi.encode(reason, entry));
    }

    /**
     * @notice Update scaling configuration parameters
     * @param batchSize New batch size for migrations
     * @param targetFPRbps New target false positive rate in basis points
     * @param parallelQueryPeriod New parallel query period in seconds
     * @param autoScaleThresholdBps New auto-scale threshold in basis points
     * @param autoScalingEnabled Whether to enable automatic scaling
     */
    function updateScalingConfig(
        uint256 batchSize,
        uint256 targetFPRbps,
        uint256 parallelQueryPeriod,
        uint256 autoScaleThresholdBps,
        bool autoScalingEnabled
    ) external onlyRole(DEFAULT_ADMIN_ROLE) {
        require(batchSize > 0, "FilterManager: Batch size must be positive");
        require(targetFPRbps > 0 && targetFPRbps <= 10000, "FilterManager: FPR must be between 0-100%");
        
        scalingConfig.batchSize = batchSize;
        scalingConfig.targetFPRbps = targetFPRbps;
        scalingConfig.parallelQueryPeriod = parallelQueryPeriod;
        scalingConfig.autoScaleThresholdBps = autoScaleThresholdBps;
        scalingConfig.autoScalingEnabled = autoScalingEnabled;
        
        emit ScalingConfigUpdated(batchSize, targetFPRbps, parallelQueryPeriod);
    }

    /**
     * @notice Get detailed info about a filter
     * @param filterId ID of the filter
     * @return size Number of buckets
     * @return numHashes Number of hash functions
     * @return insertCount Number of insertions
     * @return createdAt Creation timestamp
     * @return active Is filter active
     * @return fillRatioBps Fill ratio in basis points (100 = 1%)
     * @return estimatedFPRbps Estimated false positive rate in basis points
     */
    function getFilterInfo(uint256 filterId) external view returns (
        uint256 size,
        uint256 numHashes,
        uint256 insertCount,
        uint256 createdAt,
        bool active,
        uint256 fillRatioBps,
        uint256 estimatedFPRbps
    ) {
        require(_filters[filterId].createdAt != 0, "FilterManager: Filter does not exist");
        BloomFilterLib.Filter storage f = _filters[filterId].filter;
        
        // Get basic filter parameters
        size = f.size;
        numHashes = f.numHashes;
        insertCount = f.insertCount;
        createdAt = _filters[filterId].createdAt;
        active = _filters[filterId].active;
        
        // Get advanced filter metrics
        (,,, fillRatioBps, estimatedFPRbps) = f.getFilterMetrics();
        
        return (size, numHashes, insertCount, createdAt, active, fillRatioBps, estimatedFPRbps);
    }
    
    /**
     * @notice Get migration status
     * @return state Current migration state (0=Inactive)
     * @return sourceId Source filter ID
     * @return targetId Target filter ID
     * @return progress Progress percentage in basis points (100 = 1%)
     * @return entriesMigrated Number of entries migrated so far
     */
    function getMigrationStatus() external view returns (
        uint8 state,
        uint256 sourceId,
        uint256 targetId,
        uint256 progress,
        uint256 entriesMigrated
    ) {
        state = uint8(_activeMigration.state);
        sourceId = _activeMigration.sourceFilterId;
        targetId = _activeMigration.targetFilterId;
        entriesMigrated = _activeMigration.entriesMigrated;
        
        if (_activeMigration.totalBatches > 0) {
            progress = (_activeMigration.batchesCompleted * 10000) / _activeMigration.totalBatches;
        }
        
        return (state, sourceId, targetId, progress, entriesMigrated);
    }

    /**
     * @notice Internal helper to calculate optimal filter parameters
     * @param expectedItems Expected number of items to store
     * @param targetFPRbps Target false positive rate in basis points
     * @return size Optimal number of buckets (power of 2)
     * @return numHashes Optimal number of hash functions
     */
    function _calculateOptimalFilterParams(
        uint256 expectedItems, 
        uint256 targetFPRbps
    ) internal pure returns (uint256 size, uint256 numHashes) {
        require(expectedItems > 0, "FilterManager: Expected items must be positive");

        // m = -n * ln(p) / (ln(2)^2)
        // We implement this with a conservative lookup table of bits-per-item
        // multipliers scaled by 1000 for the target false positive rate.
        uint256 minBits = (expectedItems * _bitsPerItemMultiplier(targetFPRbps) + 999) / 1000;

        // Convert to buckets (each bucket = 256 bits)
        uint256 minBuckets = (minBits + 255) / 256;
        
        // Round up to next power of 2
        size = MIN_FILTER_BUCKETS;
        while (size < minBuckets && size < MAX_FILTER_BUCKETS) {
            size *= 2;
        }
        if (size > MAX_FILTER_BUCKETS) {
            size = MAX_FILTER_BUCKETS;
        }
        
        // Calculate optimal number of hash functions
        // k = (m / n) * ln(2)
        uint256 bitsPerItem = (size * 256) / expectedItems;
        numHashes = (bitsPerItem * HASH_COUNT_NUMERATOR + (HASH_COUNT_SCALE - 1)) / HASH_COUNT_SCALE;
        
        // Bounds checking
        if (numHashes < 1) numHashes = 1;
        if (numHashes > MAX_HASH_FUNCTIONS) numHashes = MAX_HASH_FUNCTIONS;
        
        return (size, numHashes);
    }
    
    /**
     * @notice Internal helper to calculate total batches needed for migration
     * @param totalEntries Total number of entries to migrate
     * @return batches Number of batches needed
     */
    function _calculateTotalBatches(uint256 totalEntries) internal view returns (uint256 batches) {
        if (totalEntries == 0) return 1;
        
        uint256 entriesPerBatch = scalingConfig.batchSize;
        if (entriesPerBatch == 0) entriesPerBatch = 1;
        
        return (totalEntries + entriesPerBatch - 1) / entriesPerBatch;
    }
    
    /**
     * @notice Check if auto-scaling should be triggered and prepare scaling if needed
     * @dev Called after inserts if auto-scaling is enabled
     */
    function _checkAndTriggerAutoScaling() internal {
        // Only check if no migration is active
        if (_activeMigration.state != MigrationState.Inactive) return;
        
        BloomFilterLib.Filter storage currentFilter = _filters[activeFilterId].filter;
        
        // Get fill ratio
        (,,, uint256 fillRatioBps,) = currentFilter.getFilterMetrics();
        
        // If above threshold, prepare for scaling
        if (fillRatioBps > scalingConfig.autoScaleThresholdBps) {
            // Calculate expected items as 150% of current to allow growth
            uint256 expectedItems = (currentFilter.insertCount * 3) / 2;
            
            // Use internal function to avoid requiring the role check
            uint256 newFilterId = _nextFilterId++;
            
            // Calculate optimal parameters
            (uint256 newSize, uint256 newNumHashes) = _calculateOptimalFilterParams(
                expectedItems,
                scalingConfig.targetFPRbps
            );
            
            // Create new salt
            uint256 newSalt = uint256(keccak256(abi.encodePacked(
                block.timestamp, 
                block.prevrandao, 
                currentFilter.salt
            )));
            
            // Create new filter
            _initializeFilter(newFilterId, newSize, newNumHashes, newSalt);
            
            emit FilterCreated(newFilterId, newSize, newNumHashes, block.timestamp);
            
            // Setup migration
            _activeMigration = MigrationInfo({
                sourceFilterId: activeFilterId,
                targetFilterId: newFilterId,
                state: MigrationState.Preparing,
                startedAt: block.timestamp,
                completedAt: 0,
                totalBatches: _calculateTotalBatches(_filters[activeFilterId].totalEntries),
                batchesCompleted: 0,
                entriesMigrated: 0
            });
            
            emit ScalingStarted(activeFilterId, newFilterId, block.timestamp);
        }
    }

    function _initializeFilter(
        uint256 filterId,
        uint256 size,
        uint256 numHashes,
        uint256 salt
    ) internal {
        FilterInfo storage info = _filters[filterId];
        BloomFilterLib.createFilter(info.filter, size, numHashes, salt);
        info.createdAt = block.timestamp;
        info.active = false;
        info.batchesMigrated = 0;
        info.totalEntries = 0;
    }

    function _bitsPerItemMultiplier(uint256 targetFPRbps) internal pure returns (uint256) {
        require(targetFPRbps > 0 && targetFPRbps < 10000, "FilterManager: Invalid target FPR");

        if (targetFPRbps <= 1) return 19171;   // 0.01%
        if (targetFPRbps <= 5) return 15819;   // 0.05%
        if (targetFPRbps <= 10) return 14378;  // 0.10%
        if (targetFPRbps <= 25) return 12936;  // 0.25%
        if (targetFPRbps <= 50) return 11028;  // 0.50%
        if (targetFPRbps <= 100) return 9586;  // 1.00%
        if (targetFPRbps <= 250) return 7683;  // 2.50%
        if (targetFPRbps <= 500) return 6235;  // 5.00%
        return 4793;                           // 10.00% and looser
    }
}
