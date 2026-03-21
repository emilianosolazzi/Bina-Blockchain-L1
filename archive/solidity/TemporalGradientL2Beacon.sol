// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Initializable } from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import { UUPSUpgradeable } from "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";
import { OwnableUpgradeable } from "@openzeppelin/contracts-upgradeable/access/OwnableUpgradeable.sol";
import { ReentrancyGuard } from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import { ECDSA } from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import { ITGBT } from "./interfaces/ITGBT.sol";
import { PausableUpgradeable } from "@openzeppelin/contracts-upgradeable/utils/PausableUpgradeable.sol";
import { AccessControlUpgradeable } from "@openzeppelin/contracts-upgradeable/access/AccessControlUpgradeable.sol";
import { EIP712Upgradeable } from "@openzeppelin/contracts-upgradeable/utils/cryptography/EIP712Upgradeable.sol";
import { CoreUtilsLib } from "./CoreUtilsLib.sol";
import { BloomFilterLib } from "./BloomFilterLib.sol";
import { MiningLib } from "./MiningLib.sol";
import { TokenomicsLib } from "./TokenomicsLib.sol";
import { StorageLib } from "./StorageLib.sol";
import { RandomnessLib } from "./RandomnessLib.sol"; // <<< Added import
import { GovernanceLib } from "./GovernanceLib.sol"; // <<< Added import (Needed for setting fee params)
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { Math } from "@openzeppelin/contracts/utils/math/Math.sol";
import { RateTypes } from "./RateTypes.sol"; // <<< Added import for rate limiting

/**
 * @title TemporalGradientL2Beacon
 * @notice A temporal gradient beacon with mining, randomness generation, and governance features
 * @dev Uses UUPS upgrade pattern with role-based access control
 */
contract TemporalGradientL2Beacon is
    Initializable,
    OwnableUpgradeable,
    ReentrancyGuard,
    PausableUpgradeable,
    UUPSUpgradeable,
    AccessControlUpgradeable,
    EIP712Upgradeable
{
    using ECDSA for bytes32;
    using RandomnessLib for RandomnessLib.State; // <<< Added line
    using BloomFilterLib for BloomFilterLib.Filter;
    using MiningLib for MiningLib.RevealParams;
    using TokenomicsLib for TokenomicsLib.EpochState;
    using StorageLib for StorageLib.HistoricalStorage;
    using CoreUtilsLib for bytes32[32];

    // Constants
    bytes32 public constant GOVERNANCE_ROLE = keccak256("GOVERNANCE_ROLE");
    bytes32 public constant UPGRADER_ROLE = keccak256("UPGRADER_ROLE");
    bytes32 public constant EMERGENCY_ROLE = keccak256("EMERGENCY_ROLE");
    bytes32 public constant TOKENOMICS_ROLE = keccak256("TOKENOMICS_ROLE");
    bytes32 public constant SLASHER_ROLE = keccak256("SLASHER_ROLE");
    bytes32 public constant BURNER_ROLE = keccak256("BURNER_ROLE");
    uint256 public constant MIN_BLOOM_FILTER_SIZE = 128;
    uint256 public constant MAX_BLOOM_FILTER_SIZE = 65536;
    uint256 public constant MAX_BLOOM_FILTER_HASHES = 8;
    uint256 public constant MIN_DIFFICULTY = 1000;
    uint256 public constant MAX_DIFFICULTY = 2**220;
    uint256 public constant MAX_BONUS_MULTIPLIER = 500;
    uint256 public constant MAX_POOLS = 5;
    uint256 public constant REQUIRED_TSTAKE_AMOUNT = 100 ether;
    uint256 public constant OUTPUT_HISTORY_SIZE = 32;
    uint256 public constant TOTAL_SUPPLY_CAP = 2_000_000_000 ether;
    uint256 public constant MINING_ALLOCATION = 700_000_000 ether;
    uint256 public constant MIN_EXPIRY_BLOCKS = 40000;
    uint256 public constant DEFAULT_BONUS_THRESHOLD = 2;
    uint16 public constant DEFAULT_BONUS_MULTIPLIER = 125;
    uint256 private constant DEFAULT_DIFFICULTY_WEIGHT = 1e18;
    
    // Auto-slashing constants
    bytes32 public constant RULE_VIOLATION = keccak256("RULE_VIOLATION");
    bytes32 public constant MALICIOUS_BEHAVIOR = keccak256("MALICIOUS");
    bytes32 public constant INACTIVITY = keccak256("INACTIVITY");
    bytes32 public constant MISSED_ENTROPY = keccak256("MISSED_ENTROPY");
    uint8 public constant VIOLATION_TYPE_RULE = 1;
    uint8 public constant VIOLATION_TYPE_MALICIOUS = 2;
    uint8 public constant BURN_TYPE_INACTIVITY = 1;
    uint8 public constant BURN_TYPE_MISSED = 2;

    // State variables
    ITGBT public tgbtToken;
    ITGBT public tstakeToken;
    uint256 public targetDifficulty;
    TokenomicsLib.EpochState internal epochState;
    RandomnessLib.State internal randomnessState; // <<< Changed type
    bytes32[OUTPUT_HISTORY_SIZE] public outputHistory;
    uint64 public currentOutputIndex;
    uint64 public lastOutputTimestamp;
    bytes32 public genesisBlockOutput;
    uint64 public genesisBlockTimestamp;
    mapping(address => uint64) public lastMinerBlock;
    BloomFilterLib.Filter public bloomFilter;
    mapping(bytes32 => uint256) public usedOutputs;
    uint64 public outputCount;
    uint256 public totalMined;
    bytes32 private constant MINING_COMMITMENT_TYPEHASH =
        keccak256("MiningCommitment(address miner,bytes32 commitHash,uint256 poolId,uint256 nonce,uint256 deadline)");
    mapping(address => uint256) public nonces;
    mapping(bytes32 => bool) public usedAnonymousIds;
    uint64 public outputExpiryBlocks;
    uint8 public poolCount;
    uint8 public minBlockInterval;
    uint8 public minSubmissionsPerBlock;
    uint8 public consensusThreshold;
    uint8 public minCommitmentAge;
    uint16 public maxCommitmentAge;
    mapping(address => MiningLib.Commitment) public minerCommitments;
    mapping(uint8 => MiningLib.MiningPool) public miningPools;
    uint256 public bonusThreshold;
    uint16 public bonusMultiplier;
    GovernanceLib.GovernanceContext internal governanceContext; 
    mapping(address => uint256) public lastActivityBlock;  
    mapping(address => uint256) public missedContributions; 

    // L2-specific state variables
    address public l1BeaconAddress;
    bytes32 public latestL1Output;
    bytes32 public l2EntropyAccumulator;

    // Rate limiting state variables
    mapping(address => RateTypes.TokenBucket) private userRateBuckets;
    RateTypes.TokenBucket private globalRateBucket;
    RateTypes.SlidingWindow private globalWindow;
    RateTypes.RateThresholds private rateThresholds;
    RateTypes.RateStats private rateStats;
    
    // Rate limiting constants
    uint16 private constant GLOBAL_WINDOW_SIZE = 1000;
    uint256 private constant DEFAULT_WINDOW_DURATION = 3600; // 1 hour
    uint256 private constant DEFAULT_SUBMISSION_COST = 1;
    uint256 private constant DEFAULT_REVEAL_COST = 2;
    uint256 private constant DEFAULT_COMMIT_RATE = 10; // Commits per user per minute
    uint256 private constant DEFAULT_GLOBAL_RATE = 600; // Global commits per minute

    // Events
    event BeaconBlockMined(address indexed miner, bytes32 hmacOutput, uint256 reward, uint64 nonce, uint64 timestamp, uint8 poolId);
    event CommitmentSubmitted(address indexed miner, bytes32 commitHash, uint8 poolId);
    event CommitmentRevealed(address indexed miner, bytes32 revealedValue, uint8 poolId);
    event OutputHistoryUpdated(bytes32 newOutput, uint64 index);
    event OutputsPruned(uint64 count);
    event GenesisBlockCreated(bytes32 indexed output, uint64 timestamp);
    event StealthSolutionSubmitted(bytes32 indexed anonymousId, address indexed recipient, uint256 reward);
    event GovernanceParameterChanged(string paramName, uint256 newValue);
    event BloomFilterReset(uint256 size, uint256 numHashes);
    event RandomnessRequested(uint256 indexed requestId, address indexed requester, bytes32 userSeed);
    event RandomnessContributionAdded(
        uint256 indexed requestId,
        address indexed contributor,
        bytes32 entropyContribution,
        uint256 contributionCount,
        uint256 minContributions
    );
    event RandomnessFulfilled(uint256 indexed requestId, bytes32 result);
    event MiningPoolCreated(uint8 indexed poolId, uint256 targetDifficulty, uint256 emissionBucket);
    event MiningPoolUpdated(uint8 indexed poolId, uint256 targetDifficulty, uint256 emissionBucket);
    event MiningPoolDeactivated(uint8 indexed poolId);
    event TokenUpdated(address newToken);
    event EmergencyFeeParametersChanged(uint256 baseFee, uint256 feePerContributor); // <<< Added event
    event TokenomicsUpdate(uint256 indexed epochNumber, uint256 blockReward, uint256 blockNumber, bool isHalving);
    event AutoSlashed(address indexed account, uint8 violationType, uint8 severity, uint256 amount);
    event AutoBurned(address indexed account, uint8 burnType, uint256 parameter, uint256 amount);
    event BeaconUpdated(bytes32 newEntropy);
    event RateLimitExceeded(address indexed user, uint8 reason, uint256 currentRate, uint256 threshold);
    event RateLimitThresholdsUpdated(uint256 warningThreshold, uint256 criticalThreshold);

    // Errors
    error ZeroToken();
    error DeadlineExpired();
    error InvalidNonce();
    error FeeNotSet();
    error ZeroAddress();
    error MaxPoolsReached();
    error DuplicateAnonymousId();
    error VerificationFailed();
    error InvalidDifficulty(); // <<< Added missing error from GovernanceLib usage
    error InvalidEmission(); // <<< Added missing error from GovernanceLib usage
    error InvalidMultiplier(); // <<< Added missing error from GovernanceLib usage
    error InvalidThreshold(); // <<< Added missing error from GovernanceLib usage
    error MinAgeTooLow(); // <<< Added missing error from GovernanceLib usage
    error MaxAgeTooLow(); // <<< Added missing error from GovernanceLib usage
    error MaxAgeTooHigh(); // <<< Added missing error from GovernanceLib usage
    error ExpiryTooShort(); // <<< Added missing error from GovernanceLib usage
    error InvalidTGBTAddress(); // <<< Added missing error from RandomnessLib usage
    error TGBTTransferFailed(); // <<< Added missing error from RandomnessLib usage
    error InvalidRequestID(); // <<< Added missing error from RandomnessLib usage
    error RequestFulfilled(); // <<< Added missing error from RandomnessLib usage
    error RequestExpired(); // <<< Added missing error from RandomnessLib usage
    error AlreadyContributed(); // <<< Added missing error from RandomnessLib usage
    error MaxContributionsReached(); // <<< Added missing error from RandomnessLib usage
    error RequestDoesNotExist(); // <<< Added missing error from RandomnessLib usage
    error RequestNotFulfilled(); // <<< Added missing error from RandomnessLib usage
    error InvalidRequest(); // <<< Added missing error from RandomnessLib usage
    error BatchTooLarge(); // <<< Added missing error from RandomnessLib usage
    error ArrayLengthMismatch(); // <<< Added missing error from RandomnessLib usage
    error InvalidSigner(); // <<< Added missing error from RandomnessLib usage
    error InvalidBatchSize(); // <<< Added missing error from RandomnessLib usage
    error MinContributionsTooLow(); // <<< Added missing error from GovernanceLib usage
    error MaxLessThanMin(); // <<< Added missing error from GovernanceLib usage
    error MaxContributionsTooHigh(); // <<< Added missing error from GovernanceLib usage
    error RateLimitExceededGlobal();
    error RateLimitExceededUser(uint256 currentRate, uint256 limit);
    error RateLimitThrottled(uint8 reason);
    error StealthMiningDisabled();
    error InvalidThresholdsConfig();
    error InsufficientStakeBalance();
    error InvalidPoolSelection();
    error InvalidRecoveredSigner();
    error ZeroAddressSigner();
    error ActiveCommitmentExistsError();
    error MiningTooFrequentlyError();
    error CommitmentNotFound();
    error CommitmentAlreadyRevealedError();
    error CommitmentTooRecentError();
    error CommitmentExpiredError();
    error InvalidCommitmentHash();
    error InvalidPreviousOutputRef();
    error InvalidBloomSize();
    error InvalidBloomHashes();
    error InvalidSeverityLevel();
    error InvalidViolationTypeError();
    error InvalidWordCount();
    error EntropyAccumulatorUninitialized();
    error ZeroOutputHash();
    error L1BeaconUnset();
    error InvalidL1OutputProof();

    /**
     * @notice Initializes the contract
     * @param _tgbtToken Address of the TGBT token contract
     * @param _tstakeToken Address of the TStake token contract
     * @param _initialReward Initial mining reward amount
     * @param _difficulty Initial mining difficulty
     * @param _blocksPerEpoch Number of blocks per epoch
     * @param _halvingInterval Blocks between reward halvings
     */
    function initialize(
        address _tgbtToken,
        address _tstakeToken,
        uint256 _initialReward,
        uint256 _difficulty,
        uint256 _blocksPerEpoch,
        uint256 _halvingInterval
    ) public initializer {
        __Ownable_init(msg.sender);
        __Pausable_init();
        __AccessControl_init();
        __EIP712_init("TemporalGradientBeacon", "1");

        if (_tgbtToken == address(0) || _tstakeToken == address(0)) revert ZeroToken();
        if (_difficulty < MIN_DIFFICULTY || _difficulty > MAX_DIFFICULTY) revert InvalidDifficulty();
        
        tgbtToken = ITGBT(_tgbtToken);
        tstakeToken = ITGBT(_tstakeToken);
        targetDifficulty = _difficulty;
        outputExpiryBlocks = 50000;
        
        // Initialize tokenomics
        epochState.currentEpoch = 0;
        epochState.blocksPerEpoch = _blocksPerEpoch;
        epochState.epochStartBlock = uint64(block.number);
        epochState.lastHalvingBlock = uint64(block.number);
        epochState.halvingInterval = _halvingInterval;
        epochState.rewardAmount = _initialReward;
        totalMined = 0;
        
        // Initialize default mining pool
        governanceContext.miningPools[0] = MiningLib.MiningPool({
            targetDifficulty: _difficulty,
            emissionBucket: MINING_ALLOCATION,
            totalMined: 0,
            active: true,
            lastUpdateBlock: uint64(block.number),
            minerCount: 0
        });
        governanceContext.poolCount = 1;
        poolCount = 1;
        miningPools[0] = governanceContext.miningPools[0];

        governanceContext.bonusThreshold = DEFAULT_BONUS_THRESHOLD;
        governanceContext.bonusMultiplier = DEFAULT_BONUS_MULTIPLIER;
        bonusThreshold = DEFAULT_BONUS_THRESHOLD;
        bonusMultiplier = DEFAULT_BONUS_MULTIPLIER;
        
        // Initialize genesis block
        genesisBlockOutput = keccak256(abi.encodePacked("GENESIS_BLOCK", msg.sender, block.timestamp, block.prevrandao));
        genesisBlockTimestamp = uint64(block.timestamp);
        outputHistory[0] = genesisBlockOutput;
        usedOutputs[genesisBlockOutput] = block.number;
        currentOutputIndex = 0;
        lastOutputTimestamp = uint64(block.timestamp);
        emit GenesisBlockCreated(genesisBlockOutput, genesisBlockTimestamp);
        
        // Initialize output history
        for (uint256 i = 1; i < OUTPUT_HISTORY_SIZE; i++) {
            outputHistory[i] = genesisBlockOutput;
        }
        
        // Set mining parameters optimized for 10M+ users on Arbitrum
        governanceContext.minBlockInterval = 1; // Reduced from 5
        governanceContext.minSubmissionsPerBlock = 1;
        governanceContext.consensusThreshold = 70;
        governanceContext.minCommitmentAge = 2; // Reduced from 5
        governanceContext.maxCommitmentAge = 500; // Increased from 100
        
        // Set legacy vars for compatibility
        minBlockInterval = 1;
        minSubmissionsPerBlock = 1;
        consensusThreshold = 70;
        minCommitmentAge = 2;
        maxCommitmentAge = 500;
        
        // Initialize randomness system using RandomnessLib.State
        randomnessState.tgbtTokenAddress = _tgbtToken;
        randomnessState.baseEmergencyFee = 100 ether;
        randomnessState.feePerContributor = 10 ether;
        randomnessState.expiryBlocks = 50000;
        randomnessState.minContributions = 3;
        randomnessState.maxContributions = 10;
        randomnessState.maxBatchSize = 20;
        
        // Initialize bloom filter with parameters optimized for 10M+ users
        // Previous size (1024) was severely undersized, causing false positive errors
        BloomFilterLib.createFilter(bloomFilter, 65536, 4, block.timestamp);
        // With 65536 size and 4 hash functions:
        // - 16.7M bits capacity (~4x more than needed for target FPR)
        // - <0.1% false positive rate at 1M entries
        // - ~0.2% false positive rate at 10M entries
        
        outputCount = 1;
        
        // Setup roles
        _grantRole(DEFAULT_ADMIN_ROLE, msg.sender);
        _grantRole(GOVERNANCE_ROLE, msg.sender);
        _grantRole(UPGRADER_ROLE, msg.sender);
        _grantRole(EMERGENCY_ROLE, msg.sender);
        _grantRole(TOKENOMICS_ROLE, msg.sender);
        _grantRole(SLASHER_ROLE, msg.sender);
        _grantRole(BURNER_ROLE, msg.sender);

        // Initialize L2-specific state
        l2EntropyAccumulator = keccak256(abi.encodePacked(
            "L2_GENESIS",
            block.timestamp,
            block.prevrandao,
            _tgbtToken,
            _tstakeToken
        ));

        // Initialize rate limiting structures
        _initializeRateLimiting();
    }

    /**
     * @notice Initialize rate limiting structures with default values
     * @dev Called during contract initialization
     */
    function _initializeRateLimiting() internal {
        // Initialize global token bucket
        // 600 operations per minute, up to 1200 burst capacity
        RateTypes.initTokenBucket(
            globalRateBucket,
            1200, // capacity
            10,   // refill rate (10 tokens per second = 600 per minute)
            1200  // initial tokens (start full)
        );
        
        // Initialize global sliding window
        RateTypes.initSlidingWindow(
            globalWindow,
            GLOBAL_WINDOW_SIZE,
            DEFAULT_WINDOW_DURATION
        );
        
        // Initialize rate thresholds
        RateTypes.initRateThresholds(
            rateThresholds,
            500,   // warning threshold - operations per window
            900    // critical threshold - operations per window
        );
        
        // Complete the thresholds initialization with custom values
        rateThresholds.banThreshold = 1000;          // Ban at 1000 ops per window
        rateThresholds.throttleThreshold = 400;      // Start throttling at 400 ops
        rateThresholds.individualUserLimit = 60;     // 60 ops per user per window
        rateThresholds.globalLimit = 1200;           // 1200 ops global limit
    }

    /**
     * @notice Update rate limiting thresholds (governance function)
     * @param warningThreshold New warning threshold
     * @param criticalThreshold New critical threshold
     */
    function updateRateLimitThresholds(
        uint256 warningThreshold,
        uint256 criticalThreshold
    ) external onlyRole(GOVERNANCE_ROLE) {
        if (warningThreshold >= criticalThreshold) revert InvalidThresholdsConfig();
        
        rateThresholds.warningThreshold = warningThreshold;
        rateThresholds.criticalThreshold = criticalThreshold;
        rateThresholds.banThreshold = criticalThreshold * 110 / 100;  // 110% of critical
        rateThresholds.throttleThreshold = warningThreshold * 80 / 100; // 80% of warning
        rateThresholds.individualUserLimit = warningThreshold / 10;   // 10% of warning per user
        rateThresholds.globalLimit = criticalThreshold;              // Critical = global limit
        
        emit RateLimitThresholdsUpdated(warningThreshold, criticalThreshold);
    }

    /**
     * @notice Check if a mining operation should be rate limited
     * @param user The address of the user performing the operation
     * @param operationCost The cost of the operation in rate limiting tokens
     * @return shouldLimit Whether the operation should be limited 
     * @return reason The reason code if limited (0=not limited)
     */
    function _checkRateLimit(address user, uint256 operationCost) internal returns (bool shouldLimit, uint8 reason) {
        // Check global rate bucket
        (bool globalAllowed, ) = RateTypes.consumeTokens(globalRateBucket, operationCost);
        if (!globalAllowed) {
            return (true, 4); // Global token bucket exhausted
        }
        
        // Check user's rate bucket
        RateTypes.TokenBucket storage userBucket = userRateBuckets[user];
        
        // Initialize user bucket if needed
        if (userBucket.capacity == 0) {
            RateTypes.initTokenBucket(
                userBucket,
                rateThresholds.individualUserLimit,  // capacity based on individual limit
                1,                                  // refill 1 token per second
                rateThresholds.individualUserLimit   // start full
            );
        }
        
        // Check user bucket
        (bool userAllowed, ) = RateTypes.consumeTokens(userBucket, operationCost);
        if (!userAllowed) {
            return (true, 5); // User token bucket exhausted
        }
        
        // Update global sliding window and stats
        uint256 currentRate = RateTypes.recordOperation(globalWindow);
        RateTypes.updateRateStats(rateStats, currentRate, rateThresholds);
        
        // Check for throttling based on current rate
        return RateTypes.shouldThrottleOperation(rateStats, rateThresholds);
    }

    /* ========== MINING FUNCTIONS ========== */

    /**
     * @notice Submits a mining commitment using an EIP-712 signature
     * @param commitHash Hash of the commitment parameters (previousOutput, temporalSeed, nonce, signature, secretValue, miner)
     * @param poolId ID of the mining pool
     * @param nonce Unique nonce for this commitment signature, obtained from the contract's `nonces` mapping for the miner
     * @param deadline Unix timestamp after which the signature is invalid
     * @param signature EIP-712 signature of the MiningCommitment struct
     */
    function submitMiningCommitment(
        bytes32 commitHash,
        uint8 poolId,
        uint256 nonce,
        uint256 deadline,
        bytes calldata signature
    ) public nonReentrant whenNotPaused {
        // Apply rate limiting
        (bool shouldThrottle, uint8 reason) = _checkRateLimit(msg.sender, DEFAULT_SUBMISSION_COST);
        if (shouldThrottle) {
            emit RateLimitExceeded(msg.sender, reason, rateStats.currentRate, rateThresholds.throttleThreshold);
            revert RateLimitThrottled(reason);
        }

        if (tstakeToken.balanceOf(msg.sender) < REQUIRED_TSTAKE_AMOUNT) revert InsufficientStakeBalance();
        if (poolId >= poolCount || !miningPools[poolId].active) revert InvalidPoolSelection();
        if (block.timestamp > deadline) revert DeadlineExpired();

        // Verify EIP-712 signature
        bytes32 digest = _hashTypedDataV4(
            keccak256(
                abi.encode(
                    MINING_COMMITMENT_TYPEHASH,
                    msg.sender,
                    commitHash,
                    poolId,
                    nonce,
                    deadline
                )
            )
        );
        address recoveredSigner = ECDSA.recover(digest, signature);
        if (recoveredSigner != msg.sender) revert InvalidRecoveredSigner();
        if (recoveredSigner == address(0)) revert ZeroAddressSigner();

        // Check and increment nonce to prevent signature replay
        if (nonces[msg.sender] != nonce) revert InvalidNonce();
        nonces[msg.sender]++; // Increment nonce after successful verification

        MiningLib.Commitment storage commitment = minerCommitments[msg.sender];
        if (
            !(commitment.commitHash == bytes32(0) ||
            commitment.flags.revealed ||
            block.number > commitment.timestamp + maxCommitmentAge)
        ) revert ActiveCommitmentExistsError();
        if (
            !(minBlockInterval == 0 ||
            lastMinerBlock[msg.sender] == 0 ||
            block.number - lastMinerBlock[msg.sender] >= minBlockInterval)
        ) revert MiningTooFrequentlyError();

        commitment.commitHash = commitHash;
        commitment.timestamp = uint64(block.number);
        commitment.flags.revealed = false;
        commitment.revealedValue = bytes32(0);
        commitment.poolId = poolId;
        commitment.deadline = deadline; // Store deadline

        emit CommitmentSubmitted(msg.sender, commitHash, poolId);
    }

    /**
     * @notice Reveals a mining commitment and mints rewards if successful
     * @param previousOutput Previous output in the chain
     * @param temporalSeed Temporal seed value
     * @param nonce Miner's nonce
     * @param signature Signature proving miner's identity
     * @param secretValue Secret value used in commitment
     * @param poolId ID of the mining pool
     */
    function revealMiningCommitment(
        bytes32 previousOutput,
        bytes calldata temporalSeed,
        uint64 nonce,
        bytes calldata signature,
        bytes32 secretValue,
        uint8 poolId
    ) external nonReentrant whenNotPaused {
        // Apply rate limiting with higher cost for reveal operations
        (bool shouldThrottle, uint8 reason) = _checkRateLimit(msg.sender, DEFAULT_REVEAL_COST);
        if (shouldThrottle) {
            emit RateLimitExceeded(msg.sender, reason, rateStats.currentRate, rateThresholds.throttleThreshold);
            revert RateLimitThrottled(reason);
        }

        _updateActivity(msg.sender);
        
        MiningLib.RevealParams memory params = MiningLib.RevealParams({
            miner: msg.sender,
            previousOutput: previousOutput,
            temporalSeed: temporalSeed,
            nonce: nonce,
            signature: signature,
            secretValue: secretValue,
            poolId: poolId
        });

        _processMiningReveal(params);
    }

    /**
     * @notice Internal processing of mining reveal
     * @param params Reveal parameters struct
     */
    function _processMiningReveal(MiningLib.RevealParams memory params) internal {
        // Validate commitment exists and is valid
        MiningLib.Commitment storage commitment = minerCommitments[params.miner];
        if (commitment.commitHash == bytes32(0)) revert CommitmentNotFound();
        if (commitment.flags.revealed) revert CommitmentAlreadyRevealedError();
        if (block.number < commitment.timestamp + minCommitmentAge) revert CommitmentTooRecentError();
        if (block.number > commitment.timestamp + maxCommitmentAge) revert CommitmentExpiredError();
        if (commitment.poolId != params.poolId) revert InvalidPoolSelection();
        if (!miningPools[params.poolId].active) revert InvalidPoolSelection();

        // --- Potential Rate Limiting Enhancement ---
        // Consider adding mechanisms here to mitigate DoS from excessive failed reveals, if needed:
        // 1. Track failed reveal attempts per miner per time window.
        // 2. Introduce a small TGBT cost for failed reveals (revert if balance insufficient).
        // 3. Adjust min/maxCommitmentAge via governance based on observed behavior.
        // Current checks (commit age, revealed status) provide baseline protection.
        // ---

        // Verify commitment hash matches
        bytes32 computedHash = keccak256(
            abi.encodePacked(
                params.previousOutput,
                params.temporalSeed,
                params.nonce,
                params.signature,
                params.secretValue,
                params.miner
            )
        );
        if (computedHash != commitment.commitHash) revert InvalidCommitmentHash();

        // Validate previous output exists in history
        if (!CoreUtilsLib.validatePreviousOutput(params.previousOutput, outputHistory, OUTPUT_HISTORY_SIZE)) {
            revert InvalidPreviousOutputRef();
        }

        // Define a placeholder difficulty weight function (replace with actual logic if needed)
        function(address) view returns (uint256) difficultyWeightFn = _getDifficultyWeight; // Placeholder

        // Unique hybrid: Combines temporal (block-based) with spatial (HMAC-based) verification
        bytes32 hmacOutput = MiningLib.processMiningReveal(
            params,
            miningPools[params.poolId].targetDifficulty,
            bloomFilter,
            usedOutputs,
            MiningLib.quantumResistantHash,
            difficultyWeightFn
        );

        // Update commitment state
        commitment.revealedValue = hmacOutput;
        commitment.flags.revealed = true; // Changed from commitment.revealed

        // --- 1 Million Users Scalability Concerns ---
        // With 1M users, the following issues need addressing:
        // 1. The bloomFilter would reach capacity quickly, increasing false positives
        // 2. Gas costs for verifying commitments would spike during high activity
        // 3. Output history contention would increase, potentially causing chain reorganizations
        // 4. Reward distribution would become highly competitive, potentially centralizing to higher-compute miners
        // ---

        // Update output history
        currentOutputIndex = outputHistory.updateOutputHistory(currentOutputIndex, hmacOutput);
        lastOutputTimestamp = uint64(block.timestamp);
        usedOutputs[hmacOutput] = block.number;
        lastMinerBlock[params.miner] = uint64(block.number);

        // Update bloom filter
        BloomFilterLib.updateFilter(bloomFilter, hmacOutput);
        outputCount++;

        // Check for epoch transition
        epochState.rewardAmount = TokenomicsLib.checkEpochTransition(epochState);

        // Calculate and distribute reward
        uint256 calculatedReward = MiningLib.calculateMiningReward(
            hmacOutput,
            epochState.rewardAmount,
            bonusThreshold,
            bonusMultiplier,
            totalMined,
            MINING_ALLOCATION,
            miningPools[params.poolId]
        );

        if (calculatedReward > 0) {
            tgbtToken.mint(params.miner, calculatedReward);
            totalMined += calculatedReward;
            miningPools[params.poolId].totalMined += calculatedReward;
        }

        emit CommitmentRevealed(params.miner, hmacOutput, params.poolId);
        emit BeaconBlockMined(
            params.miner,
            hmacOutput,
            calculatedReward,
            params.nonce,
            uint64(block.timestamp),
            params.poolId
        );
        emit OutputHistoryUpdated(hmacOutput, currentOutputIndex);
    }

    // Placeholder function for difficulty weight - replace with actual logic
    function _getDifficultyWeight(address /* miner */) internal pure returns (uint256) { // Changed to pure
        // Example: Return base weight for now. Implement logic based on stake, reputation, etc.
        return DEFAULT_DIFFICULTY_WEIGHT;
    }

    /**
     * @notice Submits a solution anonymously using a pre-derived ID and proof.
     * @dev This allows miners to claim rewards without revealing their primary address directly on-chain during submission.
     * @param anonymousId A unique identifier derived from the miner's identity (e.g., HMAC of public key).
     * @param proof Data required to verify the solution's validity without revealing the original commitment details directly.
     */
    function submitSolution(
        bytes32 anonymousId,
        bytes calldata proof
    ) external pure {
        anonymousId;
        proof;
        revert StealthMiningDisabled();
    }

    /**
     * @notice Placeholder internal function to verify the anonymous proof.
     * @dev Needs implementation based on the specific proof system used. Placeholder is 'pure', but real implementation likely needs 'view' to read state.
     * @param anonymousId The identifier submitted.
     * @param proof The proof data.
     */
    function _verifyProof(bytes32 anonymousId, bytes calldata proof) internal pure { // Changed to pure for placeholder
        // --- Implementation Required ----
        // Example: Decode proof, check against contract state, validate cryptographic elements.
        // If verification fails, revert.
        // require(isValidProof(anonymousId, proof), "VerificationFailed");
        // --- Placeholder ---
        // Silence unused parameter warnings until implemented
        anonymousId;
        proof;
        // Revert("VerificationFailed"); // Uncomment and implement actual logic
    }

    /**
     * @notice Placeholder internal function to compute the stealth recipient address.
     * @dev Needs implementation based on the chosen stealth address scheme (e.g., using ECC).
     * @param anonymousId The identifier submitted.
     * @return The derived stealth address.
     */
    function computeStealthAddress(bytes32 anonymousId) internal pure returns (address) {
        // --- Implementation Required ---
        // Example: Use elliptic curve math (e.g., base_point * hash(anonymousId) + miner_pubkey)
        // --- Placeholder ---
        // Simple, insecure placeholder: Hash the ID to get an address-like value.
        return address(uint160(uint256(keccak256(abi.encodePacked("STEALTH_", anonymousId)))));
        // Replace with a proper stealth address generation mechanism.
    }

    /**
     * @notice Submits multiple mining commitments in a single transaction
     * @param commitHashes Array of commitment hashes
     * @param poolIds Array of pool IDs
     * @param deadlines Array of deadlines
     * @param signatures Array of EIP-712 signatures
     */
    function batchSubmitCommitments(
        bytes32[] calldata commitHashes,
        uint8[] calldata poolIds,
        uint256[] calldata deadlines,
        bytes[] calldata signatures
    ) external nonReentrant whenNotPaused {
        if (commitHashes.length > 20) revert BatchTooLarge();
        if (commitHashes.length != poolIds.length || 
            commitHashes.length != deadlines.length ||
            commitHashes.length != signatures.length) revert ArrayLengthMismatch();
            
        // Apply rate limiting with cost scaled by batch size
        (bool shouldThrottle, uint8 reason) = _checkRateLimit(msg.sender, DEFAULT_SUBMISSION_COST * commitHashes.length);
        if (shouldThrottle) {
            emit RateLimitExceeded(msg.sender, reason, rateStats.currentRate, rateThresholds.throttleThreshold);
            revert RateLimitThrottled(reason);
        }

        for (uint256 i = 0; i < commitHashes.length; i++) {
            submitMiningCommitment(
                commitHashes[i],
                poolIds[i],
                nonces[msg.sender] + i,
                deadlines[i],
                signatures[i]
            );
        }
    }

    /**
     * @notice Gets all active mining pools
     * @return activePools Array of active pool IDs
     * @return difficulties Array of pool difficulties
     * @return emissions Array of remaining emissions
     */
    function getActivePools() external view returns (
        uint8[] memory activePools,
        uint256[] memory difficulties,
        uint256[] memory emissions
    ) {
        uint8[] memory _activePools = new uint8[](poolCount);
        uint256[] memory _difficulties = new uint256[](poolCount);
        uint256[] memory _emissions = new uint256[](poolCount);
        uint8 activeCount = 0;

        for (uint8 i = 0; i < poolCount; i++) {
            if (miningPools[i].active) {
                _activePools[activeCount] = i;
                _difficulties[activeCount] = miningPools[i].targetDifficulty;
                _emissions[activeCount] = miningPools[i].emissionBucket - miningPools[i].totalMined;
                activeCount++;
            }
        }

        // Resize arrays to actual count
        assembly {
            mstore(_activePools, activeCount)
            mstore(_difficulties, activeCount) 
            mstore(_emissions, activeCount)
        }

        return (_activePools, _difficulties, _emissions);
    }

    /* ========== POOL MANAGEMENT (Using GovernanceLib) ========== */

    /**
     * @notice Creates a new mining pool using GovernanceLib
     * @param _targetDifficulty Initial difficulty target for the pool
     * @param emissionBucket Token allocation for this pool
     */
    function createMiningPool(
        uint256 _targetDifficulty,
        uint256 emissionBucket
    ) external onlyRole(GOVERNANCE_ROLE) {
        // Delegate to GovernanceLib, passing necessary context and constants
        uint256 poolId = GovernanceLib.createMiningPool(
            governanceContext,
            _targetDifficulty,
            emissionBucket,
            MAX_POOLS,
            MIN_DIFFICULTY,
            MAX_DIFFICULTY,
            MINING_ALLOCATION
        );
        // Optionally update legacy poolCount if needed for compatibility
        poolCount = uint8(governanceContext.poolCount);
        // Copy pool data to legacy mapping if needed for compatibility
        miningPools[uint8(poolId)] = governanceContext.miningPools[poolId];
        emit MiningPoolCreated(uint8(poolId), _targetDifficulty, emissionBucket);
    }

    /**
     * @notice Updates mining pool parameters using GovernanceLib
     * @param poolId ID of the pool to update
     * @param _targetDifficulty New difficulty target
     * @param emissionBucket New emission bucket size
     * @param active Whether the pool should be active
     */
    function updateMiningPool(
        uint8 poolId, // Keep as uint8 for legacy compatibility?
        uint256 _targetDifficulty,
        uint256 emissionBucket,
        bool active
    ) external onlyRole(GOVERNANCE_ROLE) {
        // Delegate to GovernanceLib
        GovernanceLib.updateMiningPool(
            governanceContext,
            poolId, // Pass uint8, GovernanceLib handles comparison correctly
            _targetDifficulty,
            emissionBucket,
            active,
            MIN_DIFFICULTY,
            MAX_DIFFICULTY,
            MINING_ALLOCATION
        );
        // Update legacy mapping if needed
        miningPools[poolId] = governanceContext.miningPools[poolId];
        if (active) {
            emit MiningPoolUpdated(poolId, _targetDifficulty, emissionBucket);
        } else {
            emit MiningPoolDeactivated(poolId);
        }
    }

    /* ========== RANDOMNESS FUNCTIONS (Using RandomnessLib) ========== */

    /**
     * @notice Requests a new random value.
     * @param userSeed An arbitrary seed provided by the user.
     * @return requestId The ID of the newly created randomness request.
     */
    function requestRandomness(bytes32 userSeed) external nonReentrant whenNotPaused returns (uint256 requestId) {
        _updateActivity(msg.sender);
        requestId = RandomnessLib.createRequest(randomnessState, msg.sender, userSeed);
        emit RandomnessRequested(requestId, msg.sender, userSeed);
        return requestId;
    }

    /**
     * @notice Adds an entropy contribution to a pending randomness request.
     * @param requestId The ID of the request to contribute to.
     * @param entropyContribution The contributor's entropy value.
     */
    function contributeEntropy(uint256 requestId, bytes32 entropyContribution) external nonReentrant whenNotPaused {
        _updateActivity(msg.sender);
        bool shouldFulfill = RandomnessLib.addContribution(randomnessState, requestId, msg.sender, entropyContribution);
        uint256 contributionCount = randomnessState.contributions[requestId].contributors.length;

        emit RandomnessContributionAdded(
            requestId,
            msg.sender,
            entropyContribution,
            contributionCount,
            randomnessState.minContributions
        );

        // If enough contributions are met, automatically try to fulfill
        if (shouldFulfill) {
            bytes32 historicalHash = CoreUtilsLib.getHistoricalOutputsHash(outputHistory); // Use CoreUtilsLib directly
            // Note: entropyAccumulator is now managed within RandomnessLib state/functions
            // We might need to pass it explicitly if fulfillRequest requires it, or adjust RandomnessLib
            // Assuming fulfillRequest uses internal state for now.
            bytes32 result = RandomnessLib.fulfillRequest(randomnessState, requestId, historicalHash, 0); // Pass 0 for accumulator for now
            emit RandomnessFulfilled(requestId, result);
        }
    }

     /**
     * @notice Retrieves the result for a fulfilled randomness request.
     * @param requestId The ID of the request.
     * @return The generated random value. Reverts if the request is not found or not fulfilled.
     */
    function getRandomResult(uint256 requestId) external view returns (bytes32) {
        return RandomnessLib.getRandomness(randomnessState, requestId);
    }

    /**
     * @notice Allows emergency fulfillment of a request by paying a TGBT fee.
     * @param requestId The ID of the request to fulfill.
     * @param entropyMerkleRoot A merkle root representing entropy state (if applicable).
     */
    function emergencyRandomnessFulfill(uint256 requestId, bytes32 entropyMerkleRoot) external nonReentrant onlyRole(EMERGENCY_ROLE) {
        bytes32 historicalHash = CoreUtilsLib.getHistoricalOutputsHash(outputHistory);
        // Note: entropyAccumulator is managed within RandomnessLib state/functions
        // Pass 0 for accumulator for now, adjust if RandomnessLib requires external value
        bytes32 result = RandomnessLib.emergencyFulfill(
            randomnessState,
            requestId,
            historicalHash,
            0, // Pass 0 for accumulator
            entropyMerkleRoot,
            address(this),
            IERC20(randomnessState.tgbtTokenAddress), // Cast token address
            msg.sender // Caller pays the fee
        );
        emit RandomnessFulfilled(requestId, result);
    }

    /* ========== VIEW FUNCTIONS ========== */

    /**
     * @notice Gets information about a mining pool (using legacy mapping for now)
     * @param poolId ID of the pool to query
     * @return difficulty Current difficulty target
     * @return emission Remaining emission allocation
     * @return mined Total tokens mined from this pool
     * @return active Whether the pool is active
     */
    function getPoolInfo(uint8 poolId) external view returns (
        uint256 difficulty, 
        uint256 emission, 
        uint256 mined, 
        bool active
    ) {
        if (poolId >= poolCount) revert InvalidPoolSelection();
        MiningLib.MiningPool storage pool = miningPools[poolId]; // Use legacy mapping
        return (
            pool.targetDifficulty,
            pool.emissionBucket > pool.totalMined ? pool.emissionBucket - pool.totalMined : 0,
            pool.totalMined,
            pool.active
        );
    }

    /**
     * @notice Returns the active L2 mining economics in a single call.
     */
    function getMiningEconomics()
        external
        view
        returns (
            uint256 currentReward,
            uint256 currentEpoch,
            uint256 blocksPerEpoch,
            uint256 halvingInterval,
            uint256 nextHalvingBlock,
            uint256 currentBonusThreshold,
            uint256 currentBonusMultiplier,
            uint256 minedSoFar,
            uint256 remainingAllocation
        )
    {
        return (
            epochState.rewardAmount,
            epochState.currentEpoch,
            epochState.blocksPerEpoch,
            epochState.halvingInterval,
            epochState.lastHalvingBlock + epochState.halvingInterval,
            bonusThreshold,
            bonusMultiplier,
            totalMined,
            MINING_ALLOCATION > totalMined ? MINING_ALLOCATION - totalMined : 0
        );
    }

    /**
     * @notice Gets the current mining challenge (using legacy mapping for now)
     * @param poolId ID of the pool
     * @return outputs Array of recent outputs
     * @return difficulty Current difficulty target
     */
    function getMiningChallenge(uint8 poolId) external view returns (
        bytes32[] memory outputs, 
        uint256 difficulty
    ) {
        if (poolId >= poolCount || !miningPools[poolId].active) revert InvalidPoolSelection();
        bytes32[] memory history = new bytes32[](OUTPUT_HISTORY_SIZE);
        for (uint256 i = 0; i < OUTPUT_HISTORY_SIZE; i++) {
            history[i] = outputHistory[i];
        }
        return (history, miningPools[poolId].targetDifficulty); // Use legacy mapping
    }

    /**
     * @notice Gets the state of a randomness request.
     * @param requestId The ID of the request.
     * @return requester The address that initiated the request.
     * @return timestamp The block timestamp when the request was made.
     * @return fulfilled Whether the request has been fulfilled.
     * @return contributionsCount The number of contributions received so far.
     */
    function getRandomRequestState(uint256 requestId)
        external
        view
        returns (address requester, uint256 timestamp, bool fulfilled, uint256 contributionsCount)
    {
        return RandomnessLib.getRequestState(randomnessState, requestId);
    }

    /**
     * @notice Returns the current randomness configuration values.
     */
    function getRandomnessConfig()
        external
        view
        returns (
            uint256 minContributions,
            uint256 maxContributions,
            uint256 expiryBlocks,
            uint256 baseEmergencyFee,
            uint256 feePerContributor,
            uint256 maxBatchSize
        )
    {
        return (
            randomnessState.minContributions,
            randomnessState.maxContributions,
            randomnessState.expiryBlocks,
            randomnessState.baseEmergencyFee,
            randomnessState.feePerContributor,
            randomnessState.maxBatchSize
        );
    }

    /**
     * @notice Returns a wallet-friendly receipt for a randomness request.
     */
    function getRandomnessReceipt(uint256 requestId)
        external
        view
        returns (
            address,
            uint256,
            bool,
            bytes32,
            bytes32,
            uint256,
            uint256,
            uint256,
            uint256,
            uint256
        )
    {
        return _buildRandomnessReceipt(requestId);
    }

    /**
     * @dev Internal helper to avoid stack-too-deep in getRandomnessReceipt.
     *      Reads storage fields directly instead of calling the 9-return-value
     *      library function, keeping the stack under the 16-slot limit.
     */
    function _buildRandomnessReceipt(uint256 requestId)
        internal
        view
        returns (
            address,
            uint256,
            bool,
            bytes32,
            bytes32,
            uint256,
            uint256,
            uint256,
            uint256,
            uint256
        )
    {
        RandomnessLib.RandomnessRequest storage req = randomnessState.requests[requestId];
        if (req.requester == address(0)) revert RequestDoesNotExist();

        uint256 contribCount = randomnessState.contributions[requestId].contributors.length;
        uint256 minC = randomnessState.minContributions;

        return (
            req.requester,
            req.timestamp,
            req.fulfilled,
            req.userSeed,
            req.result,
            contribCount,
            minC,
            contribCount >= minC ? 0 : minC - contribCount,
            randomnessState.maxContributions,
            randomnessState.baseEmergencyFee + (randomnessState.feePerContributor * contribCount)
        );
    }

    /**
     * @notice Returns the contributor addresses and entropy inputs for a request.
     */
    function getRandomnessContributionDetails(uint256 requestId)
        external
        view
        returns (address[] memory contributors, bytes32[] memory contributions)
    {
        return RandomnessLib.getContributionDetails(randomnessState, requestId);
    }

    /* ========== GOVERNANCE FUNCTIONS (Using GovernanceLib where applicable) ========== */

    /**
     * @notice Sets bonus parameters for mining rewards (using GovernanceLib context)
     * @param multiplier Bonus multiplier (percentage)
     * @param threshold Difficulty threshold for bonus
     */
    function setBonusParameters(
        uint16 multiplier,
        uint256 threshold
    ) external onlyRole(GOVERNANCE_ROLE) {
        // Delegate to GovernanceLib
        GovernanceLib.setBonusParameters(governanceContext, multiplier, threshold, MAX_BONUS_MULTIPLIER);
        // Update legacy vars if needed
        bonusMultiplier = multiplier;
        bonusThreshold = threshold;
    }

    /**
     * @notice Sets the TGBT token contract address
     * @param newToken Address of the new token contract
     */
    function setTGBTToken(address newToken) external onlyRole(GOVERNANCE_ROLE) {
        if (newToken == address(0)) revert ZeroAddress();
        tgbtToken = ITGBT(newToken);
        randomnessState.tgbtTokenAddress = newToken; // <<< Also update in randomness state
        emit TokenUpdated(newToken);
    }

    /**
     * @notice Sets the TStake token contract address
     * @param newToken Address of the new token contract
     */
    function setTStakeToken(address newToken) external onlyRole(GOVERNANCE_ROLE) {
        if (newToken == address(0)) revert ZeroAddress();
        tstakeToken = ITGBT(newToken);
        emit TokenUpdated(newToken);
    }

    /**
     * @notice Sets output expiry blocks (using GovernanceLib context)
     * @param blocks Number of blocks before outputs expire
     */
    function setOutputExpiryBlocks(uint64 blocks) external onlyRole(GOVERNANCE_ROLE) {
        if (blocks < MIN_EXPIRY_BLOCKS) revert ExpiryTooShort();
        governanceContext.outputExpiryBlocks = blocks; // Use governance context
        outputExpiryBlocks = blocks; // Update legacy var if needed
    }

    /**
     * @notice Sets commit-reveal parameters (using GovernanceLib context)
     * @param minAge Minimum blocks before reveal
     * @param maxAge Maximum blocks before commitment expires
     */
    function setCommitRevealParameters(
        uint8 minAge, 
        uint16 maxAge
    ) external onlyRole(GOVERNANCE_ROLE) {
        // For 10M+ users, optimal settings would be:
        // - minAge: 1-2 blocks (allows faster processing)
        // - maxAge: 500+ blocks (larger window for submissions)
        GovernanceLib.setCommitRevealParameters(governanceContext, minAge, maxAge);
        // Update legacy vars if needed
        minCommitmentAge = minAge;
        maxCommitmentAge = maxAge;
    }

    /**
     * @notice Updates the number of blocks in each mining epoch.
     */
    function setEpochBlocks(uint256 newBlocksPerEpoch) external onlyRole(GOVERNANCE_ROLE) {
        TokenomicsLib.setEpochBlocks(epochState, newBlocksPerEpoch);
    }

    /**
     * @notice Updates the mining reward halving interval.
     */
    function setHalvingInterval(uint256 newHalvingInterval) external onlyRole(GOVERNANCE_ROLE) {
        TokenomicsLib.setHalvingInterval(epochState, newHalvingInterval);
    }

    /**
     * @notice Sets the parameters for the dynamic emergency randomness fulfillment fee.
     * @param baseFee The base fee in TGBT.
     * @param perContributorFee The additional fee per contributor in TGBT.
     */
    function setEmergencyFeeParams(uint256 baseFee, uint256 perContributorFee) external onlyRole(GOVERNANCE_ROLE) {
        // For scaling to 10M+ users, consider implementing a dynamic fee structure
        // that adjusts based on current network load and user demand
        randomnessState.baseEmergencyFee = baseFee;
        randomnessState.feePerContributor = perContributorFee;
        emit EmergencyFeeParametersChanged(baseFee, perContributorFee);
    }

    /**
     * @notice Sets the contribution parameters for randomness requests.
     * @param minContributions Minimum required contributions.
     * @param maxContributions Maximum allowed contributions.
     */
    function setRandomnessContributionParams(uint256 minContributions, uint256 maxContributions) external onlyRole(GOVERNANCE_ROLE) {
        // Validate parameters
        if (minContributions < 2) revert MinContributionsTooLow();
        if (maxContributions <= minContributions) revert MaxLessThanMin();
        if (maxContributions > 50) revert MaxContributionsTooHigh(); // Arbitrary upper limit for gas considerations
        
        // Update state
        randomnessState.minContributions = minContributions;
        randomnessState.maxContributions = maxContributions;
    }

    /**
     * @dev Addresses 1 million miners' scalability challenges
     * @notice Dynamically resets bloom filter based on network load
     * @param newSize New filter size optimized for current capacity
     * @param numHashes Number of hash functions for Bloom filter
     * @param resetSalt New salt value for filter
     */
    function dynamicallyResizeBloomFilter(
        uint256 newSize,
        uint256 numHashes,
        uint256 resetSalt
    ) external onlyRole(GOVERNANCE_ROLE) {
        // Dynamic scaling for 1M+ users
        if (newSize < MIN_BLOOM_FILTER_SIZE || newSize > MAX_BLOOM_FILTER_SIZE) revert InvalidBloomSize();
        if (numHashes == 0 || numHashes > MAX_BLOOM_FILTER_HASHES) revert InvalidBloomHashes();
        
        // Reset bloom filter with new parameters optimized for 1M users
        BloomFilterLib.resetFilter(bloomFilter, newSize, numHashes, resetSalt);
        
        emit BloomFilterReset(newSize, numHashes);
    }

    /* ========== AUTO SLASHING & BURNING ========== */

    /**
     * @notice Automatically slashes a miner's tokens based on violation metrics
     * @param account The address to slash
     * @param violationType The type of violation (mapped to constants)
     * @param severity How severe the violation is (1-100)
     * @return amount The amount slashed
     */
    function autoSlash(
        address account, 
        uint8 violationType, 
        uint8 severity
    ) external onlyRole(SLASHER_ROLE) whenNotPaused returns (uint256) {
        if (severity == 0 || severity > 100) revert InvalidSeverityLevel();
        
        bytes32 reason;
        uint256 baseAmount;
        
        // Determine base penalty amount based on violation type
        if (violationType == VIOLATION_TYPE_RULE) {
            baseAmount = 100 ether; // 100 tokens base for rule violation
            reason = RULE_VIOLATION;
        } else if (violationType == VIOLATION_TYPE_MALICIOUS) {
            baseAmount = 1000 ether; // 1000 tokens base for malicious behavior  
            reason = MALICIOUS_BEHAVIOR;
        } else {
            revert InvalidViolationTypeError();
        }
        
        // Calculate actual slash amount based on severity
        uint256 amountToSlash = (baseAmount * severity) / 100;
        
        // Slash tokens (will be capped at account balance by _burn)
        uint256 balance = tgbtToken.balanceOf(account);
        uint256 actualAmount = amountToSlash > balance ? balance : amountToSlash;
        
        if (actualAmount > 0) {
            tgbtToken.slash(account, actualAmount, reason);
            emit AutoSlashed(account, violationType, severity, actualAmount);
        }
           
        return actualAmount;
    }
    
    /**
     * @notice Updates last activity timestamp for an account
     * @dev Call this whenever an account interacts with the protocol
     */
    function _updateActivity(address account) internal {
        lastActivityBlock[account] = block.number;
    }
    
    /**
     * @notice Check and potentially burn tokens due to inactivity
     * @param account The address to check
     */
    function checkInactivity(address account) external whenNotPaused {
        uint256 inactiveBlocks = block.number - lastActivityBlock[account];
        uint256 inactiveDays = inactiveBlocks * 15 / 86400; // Approximate days (15s blocks)
        
        // Only burn if inactive for more than 30 days
        if (inactiveDays <= 30) return;
        
        // Calculate 1% burn per 30 days of inactivity beyond the first 30
        uint256 burnPercent = ((inactiveDays - 30) / 30) + 1;
        if (burnPercent > 10) burnPercent = 10; // Cap at 10%
        
        uint256 balance = tgbtToken.balanceOf(account);
        uint256 burnAmount = (balance * burnPercent) / 100;
        
        if (burnAmount > 0) {
            tgbtToken.burnFromBeacon(account, burnAmount, INACTIVITY);
            emit AutoBurned(account, BURN_TYPE_INACTIVITY, inactiveDays, burnAmount);
        }
        
        // Reset activity counter
        _updateActivity(account);
    }
    
    /**
     * @notice Record a missed entropy contribution and potentially burn tokens
     * @param contributor Address that missed a contribution
     */
    function recordMissedContribution(address contributor) external onlyRole(BURNER_ROLE) whenNotPaused {
        missedContributions[contributor]++;
        
        // Burn tokens if missed more than 3 contributions
        if (missedContributions[contributor] >= 3) {
            uint256 missedCount = missedContributions[contributor];
            uint256 burnAmount = 5 ether * missedCount; // 5 tokens per miss
            
            uint256 balance = tgbtToken.balanceOf(contributor);
            uint256 actualBurn = burnAmount > balance ? balance : burnAmount;
            
            if (actualBurn > 0) {
                tgbtToken.burnFromBeacon(contributor, actualBurn, MISSED_ENTROPY);
                emit AutoBurned(contributor, BURN_TYPE_MISSED, missedCount, actualBurn);
            }
            
            // Reset missed counter after burning
            missedContributions[contributor] = 0;
        }
    }
    
    /**
     * @notice Reset missed contributions counter (governance function)
     * @param account Address to reset counter for
     */
    function resetMissedContributions(address account) external onlyRole(GOVERNANCE_ROLE) {
        missedContributions[account] = 0;
    }

    /* ========== UTILITY FUNCTIONS ========== */

    /**
     * @notice Authorizes contract upgrades
     * @param newImplementation Address of the new implementation
     */
    function _authorizeUpgrade(address newImplementation) internal override onlyRole(UPGRADER_ROLE) {}

    /**
     * @notice Pauses the contract
     */
    function pause() external onlyRole(EMERGENCY_ROLE) {
        _pause();
    }

    /**
     * @notice Unpauses the contract
     */
    function unpause() external onlyRole(EMERGENCY_ROLE) {
        _unpause();
    }

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor() {
        _disableInitializers();
    }

    // Get randomness using the L2 beacon
    function getRandomness(uint256 numWords) external view returns (uint256[] memory) {
        if (numWords == 0) revert InvalidWordCount();
        if (l2EntropyAccumulator == bytes32(0)) revert EntropyAccumulatorUninitialized();

        uint256[] memory randomWords = new uint256[](numWords);
        bytes32 seed = l2EntropyAccumulator;

        for (uint256 i = 0; i < numWords; i++) {
            seed = keccak256(abi.encodePacked(seed, block.timestamp, block.prevrandao, i, msg.sender));
            randomWords[i] = uint256(seed);
        }

        return randomWords;
    }
    
    // Placeholder for L1 output verification
    function verifyL1Output(bytes32 /* newOutput */, bytes calldata /* proof */) internal pure returns (bool) {
        // Implement L1 proof verification logic here
        return true; // Placeholder
    }

    /**
     * @notice Updates L2 entropy from an L1 beacon output
     * @param newOutput New output hash from L1 beacon
     * @param proof Proof data to verify the L1 source (e.g., merkle proof)
     */
    function updateFromL1(bytes32 newOutput, bytes calldata proof) external whenNotPaused {
        if (newOutput == bytes32(0)) revert ZeroOutputHash();
        if (l1BeaconAddress == address(0)) revert L1BeaconUnset();
        if (!verifyL1Output(newOutput, proof)) revert InvalidL1OutputProof();
        
        // Update our L1 reference
        latestL1Output = newOutput;
        
        // Mix with L2-specific entropy
        l2EntropyAccumulator = keccak256(
            abi.encodePacked(
                l2EntropyAccumulator,
                latestL1Output,
                blockhash(block.number - 1),
                block.prevrandao
            )
        );
        
        emit BeaconUpdated(l2EntropyAccumulator);
    }

    /**
     * @notice Sets the L1 beacon reference address
     * @param newL1 Address of the L1 beacon
     */
    function setL1BeaconAddress(address newL1) external onlyRole(GOVERNANCE_ROLE) {
        if (newL1 == address(0)) revert ZeroAddress();
        l1BeaconAddress = newL1;
    }

    /**
     * @notice Get current rate statistics
     * @return currentRate Current operations per window
     * @return averageRate Average operations per window
     * @return peakRate Peak operations ever recorded
     * @return rateBps Current rate as basis points of capacity
     * @return isWarning Whether the system is in warning state
     * @return isCritical Whether the system is in critical state
     */
    function getRateStatistics() external view returns (
        uint256 currentRate,
        uint256 averageRate,
        uint256 peakRate,
        uint16 rateBps,
        bool isWarning,
        bool isCritical
    ) {
        return (
            rateStats.currentRate,
            rateStats.averageRate,
            rateStats.peakRate,
            rateStats.rateBps,
            rateStats.rateExceedsWarning,
            rateStats.rateExceedsCritical
        );
    }
    
    /**
     * @notice Get user rate limit status
     * @param user Address to check
     * @return tokens Current token count
     * @return capacity Maximum token capacity
     * @return refillRate Tokens refilled per second
     * @return isLimited Whether the user is currently rate limited
     */
    function getUserRateLimitStatus(address user) external view returns (
        uint256 tokens,
        uint256 capacity,
        uint256 refillRate,
        bool isLimited
    ) {
        RateTypes.TokenBucket storage bucket = userRateBuckets[user];
        
        // If bucket not initialized, return defaults
        if (bucket.capacity == 0) {
            return (
                rateThresholds.individualUserLimit,
                rateThresholds.individualUserLimit,
                1,
                false
            );
        }
        
        // Calculate current tokens with time refill
        uint256 timePassed = block.timestamp - bucket.lastUpdate;
        uint256 newTokens = timePassed * bucket.refillRate;
        uint256 currentTokens = Math.min(bucket.capacity, bucket.tokens + newTokens);
        
        return (
            currentTokens,
            bucket.capacity,
            bucket.refillRate,
            currentTokens < DEFAULT_SUBMISSION_COST
        );
    }
}