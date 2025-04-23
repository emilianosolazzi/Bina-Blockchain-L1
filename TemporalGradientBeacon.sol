// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Initializable } from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import { UUPSUpgradeable } from "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";
import { Ownable2StepUpgradeable } from "@openzeppelin/contracts-upgradeable/access/Ownable2StepUpgradeable.sol";
import { ReentrancyGuardUpgradeable } from "@openzeppelin/contracts-upgradeable/security/ReentrancyGuardUpgradeable.sol";
import { ECDSAUpgradeable } from "@openzeppelin/contracts-upgradeable/utils/cryptography/ECDSAUpgradeable.sol";
import { ITGBT } from "./interfaces/ITGBT.sol";
import { PausableUpgradeable } from "@openzeppelin/contracts-upgradeable/security/PausableUpgradeable.sol";
import { AccessControlUpgradeable } from "@openzeppelin/contracts-upgradeable/access/AccessControlUpgradeable.sol";
import { EIP712Upgradeable } from "@openzeppelin/contracts-upgradeable/utils/cryptography/EIP712Upgradeable.sol";
import { CoreUtilsLib } from "./CoreUtilsLib.sol";
import { BloomFilterLib } from "./BloomFilterLib.sol";
import { MiningLib } from "./MiningLib.sol";
import { TokenomicsLib } from "./TokenomicsLib.sol";
import { StorageLib } from "./StorageLib.sol";

/**
 * @title EnhancedTemporalGradientBeacon
 * @notice A temporal gradient beacon with mining, randomness generation, and governance features
 * @dev Uses UUPS upgrade pattern with role-based access control
 */
contract EnhancedTemporalGradientBeacon is
    Initializable,
    Ownable2StepUpgradeable,
    ReentrancyGuardUpgradeable,
    PausableUpgradeable,
    UUPSUpgradeable,
    AccessControlUpgradeable,
    EIP712Upgradeable
{
    using ECDSAUpgradeable for bytes32;
    using CoreUtilsLib for bytes32[];
    using CoreUtilsLib for CoreUtilsLib.RandomnessState;
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

    // State variables
    ITGBT public tgbtToken;
    ITGBT public tstakeToken;
    uint256 public targetDifficulty;
    TokenomicsLib.EpochState internal epochState;
    CoreUtilsLib.RandomnessState internal randomnessState;
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
    event RandomnessFulfilled(uint256 indexed requestId, bytes32 result);
    event MiningPoolCreated(uint8 indexed poolId, uint256 targetDifficulty, uint256 emissionBucket);
    event MiningPoolUpdated(uint8 indexed poolId, uint256 targetDifficulty, uint256 emissionBucket);
    event MiningPoolDeactivated(uint8 indexed poolId);
    event TokenUpdated(address newToken);
    event RandomnessFeeChanged(uint256 oldFee, uint256 newFee);
    event TokenomicsUpdate(uint256 indexed epochNumber, uint256 blockReward, uint256 blockNumber, bool isHalving);

    // Errors
    error ZeroToken();
    error DeadlineExpired();
    error InvalidNonce();
    error FeeNotSet();
    error ZeroAddress();
    error MaxPoolsReached();

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
        __Ownable2Step_init();
        __ReentrancyGuard_init();
        __Pausable_init();
        __UUPSUpgradeable_init();
        __AccessControl_init();
        __EIP712_init("EnhancedTemporalGradientBeacon", "1");

        require(_tgbtToken != address(0) && _tstakeToken != address(0), "ZeroToken");
        require(_difficulty >= MIN_DIFFICULTY && _difficulty <= MAX_DIFFICULTY, "InvalidDifficulty");

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
        miningPools[0] = MiningLib.MiningPool({
            targetDifficulty: _difficulty,
            emissionBucket: MINING_ALLOCATION,
            totalMined: 0,
            active: true
        });
        poolCount = 1;

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

        // Set mining parameters
        minBlockInterval = 5;
        minSubmissionsPerBlock = 1;
        consensusThreshold = 70;
        minCommitmentAge = 5;
        maxCommitmentAge = 100;

        // Initialize randomness system
        randomnessState.fee = 100 ether;
        randomnessState.expiryBlocks = 50000;
        randomnessState.minContributions = 3;
        randomnessState.maxContributions = 10;

        // Initialize bloom filter
        bloomFilter = BloomFilterLib.createFilter(1024, 3);
        outputCount = 1;

        // Setup roles
        _grantRole(DEFAULT_ADMIN_ROLE, msg.sender);
        _grantRole(GOVERNANCE_ROLE, msg.sender);
        _grantRole(UPGRADER_ROLE, msg.sender);
        _grantRole(EMERGENCY_ROLE, msg.sender);
        _grantRole(TOKENOMICS_ROLE, msg.sender);
    }

    /* ========== MINING FUNCTIONS ========== */

    /**
     * @notice Submits a mining commitment
     * @param commitHash Hash of the commitment parameters
     * @param poolId ID of the mining pool
     */
    function submitMiningCommitment(bytes32 commitHash, uint8 poolId) external nonReentrant whenNotPaused {
        require(tstakeToken.balanceOf(msg.sender) >= REQUIRED_TSTAKE_AMOUNT, "InsufficientStake");
        require(poolId < poolCount && miningPools[poolId].active, "InvalidPoolId");

        MiningLib.Commitment storage commitment = minerCommitments[msg.sender];
        require(
            commitment.commitHash == bytes32(0) ||
            commitment.flags.revealed || // Changed from commitment.revealed
            block.number > commitment.timestamp + maxCommitmentAge,
            "ActiveCommitmentExists"
        );
        require(
            minBlockInterval == 0 ||
            lastMinerBlock[msg.sender] == 0 ||
            block.number - lastMinerBlock[msg.sender] >= minBlockInterval,
            "MiningTooFrequently"
        );

        commitment.commitHash = commitHash;
        commitment.timestamp = uint64(block.number);
        commitment.flags.revealed = false; // Changed from commitment.revealed
        commitment.revealedValue = bytes32(0);
        commitment.poolId = poolId;

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
        require(commitment.commitHash != bytes32(0), "NoCommitmentFound");
        require(!commitment.flags.revealed, "CommitmentAlreadyRevealed"); // Changed from commitment.revealed
        require(block.number >= commitment.timestamp + minCommitmentAge, "CommitmentTooRecent");
        require(block.number <= commitment.timestamp + maxCommitmentAge, "CommitmentExpired");
        require(commitment.poolId == params.poolId, "InvalidPoolId");

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
        require(computedHash == commitment.commitHash, "InvalidCommitment");

        // Validate previous output exists in history
        require(
            CoreUtilsLib.validatePreviousOutput(params.previousOutput, outputHistory, OUTPUT_HISTORY_SIZE),
            "InvalidPreviousOutput"
        );

        // Process the mining reveal
        bytes32 hmacOutput = MiningLib.processMiningReveal(
            params.previousOutput,
            params.temporalSeed,
            params.nonce,
            params.signature,
            params.secretValue,
            miningPools[params.poolId].targetDifficulty,
            params.miner,
            bloomFilter,
            usedOutputs,
            MiningLib.quantumResistantHash
        );

        // Update commitment state
        commitment.revealedValue = hmacOutput;
        commitment.flags.revealed = true; // Changed from commitment.revealed

        // Update output history
        currentOutputIndex = outputHistory.updateOutputHistory(currentOutputIndex, hmacOutput);
        lastOutputTimestamp = uint64(block.timestamp);
        usedOutputs[hmacOutput] = block.number;
        lastMinerBlock[params.miner] = uint64(block.number);

        // Update bloom filter
        BloomFilterLib.updateFilter(bloomFilter, hmacOutput);
        outputCount++;

        // Update randomness accumulator
        randomnessState.entropyAccumulator = uint256(
            keccak256(abi.encodePacked(
                randomnessState.entropyAccumulator, 
                hmacOutput, 
                block.timestamp, 
                block.prevrandao
            ))
        );

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

    /* ========== POOL MANAGEMENT ========== */

    /**
     * @notice Creates a new mining pool
     * @param _targetDifficulty Initial difficulty target for the pool // Renamed parameter
     * @param emissionBucket Token allocation for this pool
     */
    function createMiningPool(
        uint256 _targetDifficulty, // Renamed parameter
        uint256 emissionBucket
    ) external onlyRole(GOVERNANCE_ROLE) {
        require(poolCount < MAX_POOLS, "MaxPoolsReached");
        require(_targetDifficulty >= MIN_DIFFICULTY && _targetDifficulty <= MAX_DIFFICULTY, "InvalidDifficulty"); // Use renamed parameter
        require(emissionBucket > 0 && totalMined + emissionBucket <= MINING_ALLOCATION, "InvalidEmission");

        uint8 poolId = poolCount++;
        miningPools[poolId] = MiningLib.MiningPool({
            targetDifficulty: _targetDifficulty, // Use renamed parameter
            emissionBucket: emissionBucket,
            totalMined: 0,
            active: true
        });

        emit MiningPoolCreated(poolId, _targetDifficulty, emissionBucket); // Use renamed parameter
    }

    /**
     * @notice Updates mining pool parameters
     * @param poolId ID of the pool to update
     * @param _targetDifficulty New difficulty target // Renamed parameter
     * @param emissionBucket New emission bucket size
     * @param active Whether the pool should be active
     */
    function updateMiningPool(
        uint8 poolId,
        uint256 _targetDifficulty, // Renamed parameter
        uint256 emissionBucket,
        bool active
    ) external onlyRole(GOVERNANCE_ROLE) {
        require(poolId < poolCount, "InvalidPoolId");
        require(_targetDifficulty >= MIN_DIFFICULTY && _targetDifficulty <= MAX_DIFFICULTY, "InvalidDifficulty"); // Use renamed parameter
        require(emissionBucket > 0 && totalMined + emissionBucket <= MINING_ALLOCATION, "InvalidEmission");

        miningPools[poolId].targetDifficulty = _targetDifficulty; // Use renamed parameter
        miningPools[poolId].emissionBucket = emissionBucket;
        miningPools[poolId].active = active;

        emit MiningPoolUpdated(poolId, _targetDifficulty, emissionBucket); // Use renamed parameter
        if (!active) {
            emit MiningPoolDeactivated(poolId);
        }
    }

    /* ========== VIEW FUNCTIONS ========== */

    /**
     * @notice Gets information about a mining pool
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
        require(poolId < poolCount, "InvalidPoolId");
        MiningLib.MiningPool storage pool = miningPools[poolId];
        return (
            pool.targetDifficulty,
            pool.emissionBucket - pool.totalMined,
            pool.totalMined,
            pool.active
        );
    }

    /**
     * @notice Gets the current mining challenge
     * @param poolId ID of the pool
     * @return outputs Array of recent outputs
     * @return difficulty Current difficulty target
     */
    function getMiningChallenge(uint8 poolId) external view returns (
        bytes32[] memory outputs, 
        uint256 difficulty
    ) {
        require(poolId < poolCount && miningPools[poolId].active, "InvalidPoolId");
        bytes32[] memory history = new bytes32[](OUTPUT_HISTORY_SIZE);
        for (uint256 i = 0; i < OUTPUT_HISTORY_SIZE; i++) {
            history[i] = outputHistory[i];
        }
        return (history, miningPools[poolId].targetDifficulty);
    }

    /* ========== GOVERNANCE FUNCTIONS ========== */

    /**
     * @notice Sets bonus parameters for mining rewards
     * @param multiplier Bonus multiplier (percentage)
     * @param threshold Difficulty threshold for bonus
     */
    function setBonusParameters(
        uint16 multiplier, 
        uint256 threshold
    ) external onlyRole(GOVERNANCE_ROLE) {
        require(multiplier >= 100 && multiplier <= MAX_BONUS_MULTIPLIER, "InvalidMultiplier");
        require(threshold > 1, "InvalidThreshold");
        bonusMultiplier = multiplier;
        bonusThreshold = threshold;
    }

    /**
     * @notice Sets the TGBT token contract address
     * @param newToken Address of the new token contract
     */
    function setTGBTToken(address newToken) external onlyRole(GOVERNANCE_ROLE) {
        require(newToken != address(0), "ZeroAddress");
        tgbtToken = ITGBT(newToken);
        emit TokenUpdated(newToken);
    }

    /**
     * @notice Sets the TStake token contract address
     * @param newToken Address of the new token contract
     */
    function setTStakeToken(address newToken) external onlyRole(GOVERNANCE_ROLE) {
        require(newToken != address(0), "ZeroAddress");
        tstakeToken = ITGBT(newToken);
        emit TokenUpdated(newToken);
    }

    /**
     * @notice Sets output expiry blocks
     * @param blocks Number of blocks before outputs expire
     */
    function setOutputExpiryBlocks(uint64 blocks) external onlyRole(GOVERNANCE_ROLE) {
        require(blocks >= MIN_EXPIRY_BLOCKS, "ExpiryTooShort");
        outputExpiryBlocks = blocks;
    }

    /**
     * @notice Sets commit-reveal parameters
     * @param minAge Minimum blocks before reveal
     * @param maxAge Maximum blocks before commitment expires
     */
    function setCommitRevealParameters(
        uint8 minAge, 
        uint16 maxAge
    ) external onlyRole(GOVERNANCE_ROLE) {
        require(minAge >= 3, "MinAgeTooLow");
        require(maxAge >= minAge * 2, "MaxAgeTooLow");
        require(maxAge <= 1000, "MaxAgeTooHigh");
        minCommitmentAge = minAge;
        maxCommitmentAge = maxAge;
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
}