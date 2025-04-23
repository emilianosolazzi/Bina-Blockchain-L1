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
import { BytesArrayLib } from "./BytesArrayLib.sol";
import { RandomnessLib } from "./RandomnessLib.sol";
import { BloomFilterLib } from "./BloomFilterLib.sol";
import { MiningLib } from "./MiningLib.sol";
import { TokenomicsLib } from "./TokenomicsLib.sol";
import { StorageLib } from "./StorageLib.sol";
import { GovernanceLib } from "./GovernanceLib.sol";
import { RandomnessHandlerLib } from "./RandomnessHandlerLib.sol";

/**
 * @title EnhancedTemporalGradientBeacon
 * @notice A temporal gradient beacon with randomness, multi-pool support, staking, meta-transactions, and optimized bloom filter integration
 * @dev Deploy using UUPS proxy pattern to handle contract size
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
    using BytesArrayLib for bytes32[];
    using RandomnessLib for RandomnessLib.State;
    using BloomFilterLib for BloomFilterLib.Filter;
    using MiningLib for MiningLib.RevealParams;
    using TokenomicsLib for TokenomicsLib.EpochState;
    using StorageLib for StorageLib.HistoricalStorage;
    using GovernanceLib for GovernanceLib.GovernanceContext;
    using RandomnessHandlerLib for RandomnessHandlerLib.RandomnessContext;

    // Constants
    bytes32 private constant _GOVERNANCE_ROLE_HASH = keccak256("GOVERNANCE_ROLE");
    bytes32 private constant _UPGRADER_ROLE_HASH = keccak256("UPGRADER_ROLE");
    bytes32 private constant _EMERGENCY_ROLE_HASH = keccak256("EMERGENCY_ROLE");
    bytes32 private constant _TOKENOMICS_ROLE_HASH = keccak256("TOKENOMICS_ROLE");
    bytes32 public constant GOVERNANCE_ROLE = _GOVERNANCE_ROLE_HASH;
    bytes32 public constant UPGRADER_ROLE = _UPGRADER_ROLE_HASH;
    bytes32 public constant EMERGENCY_ROLE = _EMERGENCY_ROLE_HASH;
    bytes32 public constant TOKENOMICS_ROLE = _TOKENOMICS_ROLE_HASH;

    // Bloom filter constants
    uint256 public constant MIN_BLOOM_FILTER_SIZE = 128;
    uint256 public constant MAX_BLOOM_FILTER_SIZE = 65536;
    uint256 public constant MAX_BLOOM_FILTER_HASHES = 8;

    // Difficulty and bonus constants
    uint256 public constant MIN_DIFFICULTY = 1000;
    uint256 public constant MAX_DIFFICULTY = 2**220;
    uint256 public constant MAX_BONUS_MULTIPLIER = 500;

    // Token state
    ITGBT public tgbtToken;
    ITGBT public tstakeToken;
    uint256 public rewardAmount;
    uint256 public targetDifficulty;

    // Library storage contexts
    TokenomicsLib.EpochState internal epochState;
    StorageLib.HistoricalStorage internal historicalStorage;
    GovernanceLib.GovernanceContext internal governanceContext;
    RandomnessHandlerLib.RandomnessContext internal randomnessContext;

    // Staking
    uint256 public constant REQUIRED_TSTAKE_AMOUNT = 100 ether;

    // Output history
    uint256 public constant OUTPUT_HISTORY_SIZE = 32;
    bytes32[OUTPUT_HISTORY_SIZE] public outputHistory;
    uint256 public currentOutputIndex;
    uint256 public lastOutputTimestamp;

    // Genesis block
    bytes32 public genesisBlockOutput;
    uint256 public genesisBlockTimestamp;

    // Mining parameters
    uint256 public constant MIN_EXPIRY_BLOCKS = 40000;
    mapping(address => uint256) public lastMinerBlock;

    // Bloom filter
    BloomFilterLib.Filter public bloomFilter;
    mapping(bytes32 => uint256) public usedOutputs;
    uint256 public outputCount;

    // Tokenomics
    uint256 public constant TOTAL_SUPPLY_CAP = 2_000_000_000 ether;
    uint256 public constant MINING_ALLOCATION = 700_000_000 ether;
    uint256 public initialBlockReward;

    // EIP-712
    bytes32 private constant MINING_COMMITMENT_TYPEHASH =
        keccak256("MiningCommitment(address miner,bytes32 commitHash,uint256 poolId,uint256 nonce,uint256 deadline)");
    bytes32 private constant MINING_REVEAL_TYPEHASH =
        keccak256(
            "MiningReveal(address miner,bytes32 previousOutput,bytes temporalSeed,uint64 nonce,bytes signature,bytes32 secretValue,uint256 poolId,uint256 nonce,uint256 deadline)"
        );
    mapping(address => uint256) public nonces;

    // Used anonymous IDs for stealth mining rewards
    mapping(bytes32 => bool) public usedAnonymousIds;

    // Events
    event BeaconBlockMined(address indexed miner, bytes32 hmacOutput, uint256 reward, uint64 nonce, uint256 timestamp, uint256 poolId);
    event CommitmentSubmitted(address indexed miner, bytes32 commitHash, uint256 poolId);
    event CommitmentRevealed(address indexed miner, bytes32 revealedValue, uint256 poolId);
    event OutputHistoryUpdated(bytes32 newOutput, uint256 index);
    event OutputsPruned(uint256 count);
    event BatchProcessed(uint256 startId, uint256 endId, uint256 fulfilledCount);
    event GenesisBlockCreated(bytes32 indexed output, uint256 timestamp);
    event StealthSolutionSubmitted(bytes32 indexed anonymousId, address indexed recipient, uint256 reward);

    // Errors
    error ZeroToken();
    error InsufficientStake();
    error ActiveCommitmentExists();
    error MiningTooFrequently();
    error NoCommitmentFound();
    error CommitmentAlreadyRevealed();
    error CommitmentTooRecent();
    error CommitmentExpired();
    error InvalidCommitment();
    error InvalidPreviousOutput();
    error SizeMustBePowerOf2();
    error InvalidSizeRange();
    error InvalidNumHashes();
    error BloomFilterNotInitialized();
    error OutputAlreadyUsed();
    error DeadlineExpired();
    error InvalidNonce();

    /**
     * @notice Initializes the contract
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

        // Verify RandomnessLib.State compatibility
        RandomnessLib.State storage state = randomnessContext.state;
        state.fee = 100 ether;
        state.maxContributions = 10;
        require(state.fee == 100 ether, "RandomnessLib.State layout mismatch");
        require(state.maxContributions == 10, "RandomnessLib.State layout mismatch");

        // Verify BloomFilterLib.Filter compatibility
        BloomFilterLib.Filter memory filter = BloomFilterLib.createFilter(1024, 3);
        require(filter.size == 1024, "BloomFilterLib.Filter layout mismatch");
        require(filter.numHashes == 3, "BloomFilterLib.Filter layout mismatch");

        if (_tgbtToken == address(0) || _tstakeToken == address(0)) revert ZeroToken();
        if (_difficulty < MIN_DIFFICULTY || _difficulty > MAX_DIFFICULTY) revert InvalidDifficulty();
        if (_blocksPerEpoch == 0 || _halvingInterval == 0) revert InvalidEpochParameters();

        tgbtToken = ITGBT(_tgbtToken);
        tstakeToken = ITGBT(_tstakeToken);
        targetDifficulty = _difficulty;
        outputExpiryBlocks = 50000;

        // Tokenomics
        initialBlockReward = _initialReward;
        rewardAmount = _initialReward;
        blocksPerEpoch = _blocksPerEpoch;
        halvingInterval = _halvingInterval;
        currentEpoch = 0;
        epochStartBlock = block.number;
        lastHalvingBlock = block.number;
        totalMined = 0;

        // Default mining pool
        miningPools[0] = MiningLib.MiningPool({
            targetDifficulty: _difficulty,
            emissionBucket: MINING_ALLOCATION,
            totalMined: 0,
            active: true
        });
        poolCount = 1;

        // Genesis block initialization
        genesisBlockOutput = keccak256(abi.encodePacked("GENESIS_BLOCK", msg.sender, block.timestamp, block.prevrandao));
        genesisBlockTimestamp = block.timestamp;
        outputHistory[0] = genesisBlockOutput;
        usedOutputs[genesisBlockOutput] = block.number;
        currentOutputIndex = 0;
        lastOutputTimestamp = block.timestamp;
        emit GenesisBlockCreated(genesisBlockOutput, block.timestamp);

        // Initialize remaining output history
        for (uint256 i = 1; i < OUTPUT_HISTORY_SIZE; i++) {
            outputHistory[i] = genesisBlockOutput;
        }

        minBlockInterval = 5;
        minSubmissionsPerBlock = 1;
        consensusThreshold = 70;
        minCommitmentAge = 5;
        maxCommitmentAge = 100;

        // Randomness state
        randomnessContext.state.fee = 100 ether;
        randomnessContext.state.expiryBlocks = 50000;
        randomnessContext.state.minContributions = 3;
        randomnessContext.state.maxContributions = 10;
        randomnessContext.state.maxBatchSize = 10;

        // Bloom filter
        bloomFilter = BloomFilterLib.createFilter(1024, 3);
        outputCount = 1; // Genesis block counts as first output

        // Roles
        _grantRole(DEFAULT_ADMIN_ROLE, msg.sender);
        _grantRole(GOVERNANCE_ROLE, msg.sender);
        _grantRole(UPGRADER_ROLE, msg.sender);
        _grantRole(EMERGENCY_ROLE, msg.sender);
        _grantRole(TOKENOMICS_ROLE, msg.sender);

        // Initialize historical storage (disabled by default)
        historicalStorage.enabled = false;
        historicalStorage.maxBlocks = 1000;

        // Add genesis block to historical blocks if enabled
        if (historicalStorage.enabled) {
            // Use the initializer method instead of direct push
            StorageLib.archiveBlock(
                historicalStorage,
                genesisBlockOutput,
                bytes32(0),
                0,
                msg.sender,
                0,
                0,
                block.timestamp,
                0
            );
        }
    }

    /**
     * @notice Retrieves details of the genesis block
     * @return output The genesis block output
     * @return timestamp The timestamp of the genesis block
     * @return index The index in outputHistory (always 0)
     */
    function getGenesisBlock() external view returns (bytes32 output, uint256 timestamp, uint256 index) {
        return (genesisBlockOutput, genesisBlockTimestamp, 0);
    }

    /**
     * @notice Initializes the bloom filter with a specified size and number of hash functions
     * @param newSize Number of buckets (power of 2, 128 to 65536)
     * @param numHashes Number of hash functions (1 to 8)
     */
    function initializeBloomFilter(uint256 newSize, uint256 numHashes) external onlyRole(GOVERNANCE_ROLE) {
        if ((newSize & (newSize - 1)) != 0) revert SizeMustBePowerOf2();
        if (newSize < MIN_BLOOM_FILTER_SIZE || newSize > MAX_BLOOM_FILTER_SIZE) revert InvalidSizeRange();
        if (numHashes < 1 || numHashes > MAX_BLOOM_FILTER_HASHES) revert InvalidNumHashes();

        bloomFilter = BloomFilterLib.createFilter(newSize, numHashes);
        outputCount = 1; // Preserve genesis block
        usedOutputs[genesisBlockOutput] = block.number; // Ensure genesis block remains
        emit GovernanceParameterChanged("bloomFilterSize", newSize);
        emit GovernanceParameterChanged("bloomFilterNumHashes", numHashes);
    }

    /**
     * @notice Resets the bloom filter to an empty state
     */
    function resetBloomFilter() external onlyRole(GOVERNANCE_ROLE) {
        if (bloomFilter.size == 0) revert BloomFilterNotInitialized();
        BloomFilterLib.clearFilter(bloomFilter);
        outputCount = 1; // Preserve genesis block
        usedOutputs[genesisBlockOutput] = block.number; // Ensure genesis block remains
        emit BloomFilterReset(bloomFilter.size, bloomFilter.numHashes);
    }

    /**
     * @notice Estimates the bloom filter's false-positive rate
     * @param numEntries Number of entries (use outputCount for current state)
     * @return rate Approximate false-positive rate (percentage, scaled by 1e18)
     */
    function estimateBloomFilterFalsePositiveRate(uint256 numEntries) external view returns (uint256 rate) {
        if (bloomFilter.size == 0) revert BloomFilterNotInitialized();
        return BloomFilterLib.estimateFalsePositiveRate(bloomFilter, numEntries);
    }

    /**
     * @notice Checks if an output might exist in the bloom filter
     * @param output Output to check
     * @return mightExist True if output might exist (false positives possible)
     */
    function checkBloomFilter(bytes32 output) external view returns (bool mightExist) {
        if (bloomFilter.size == 0) revert BloomFilterNotInitialized();
        return BloomFilterLib.mightContain(bloomFilter, output);
    }

    // Mining Functions
    function submitMiningCommitment(bytes32 commitHash, uint256 poolId) external nonReentrant whenNotPaused {
        _submitMiningCommitment(msg.sender, commitHash, poolId);
    }

    function submitMiningCommitmentMeta(
        address miner,
        bytes32 commitHash,
        uint256 poolId,
        uint256 deadline,
        bytes calldata signature
    ) external nonReentrant whenNotPaused {
        if (block.timestamp > deadline) revert DeadlineExpired();
        uint256 nonce = nonces[miner]++;
        bytes32 structHash = keccak256(
            abi.encode(
                MINING_COMMITMENT_TYPEHASH,
                miner,
                commitHash,
                poolId,
                nonce,
                deadline
            )
        );
        address signer = _hashTypedDataV4(structHash).recover(signature);
        if (signer != miner) revert InvalidSigner();

        _submitMiningCommitment(miner, commitHash, poolId);
    }

    function _submitMiningCommitment(address miner, bytes32 commitHash, uint256 poolId) internal {
        if (tstakeToken.balanceOf(miner) < REQUIRED_TSTAKE_AMOUNT) revert InsufficientStake();
        if (poolId >= poolCount || !miningPools[poolId].active) revert InvalidPoolId();

        MiningLib.Commitment storage commitment = minerCommitments[miner];
        if (
            !(commitment.commitHash == bytes32(0) ||
                commitment.flags.revealed ||
                block.number > commitment.timestamp + maxCommitmentAge)
        ) {
            revert ActiveCommitmentExists();
        }

        if (minBlockInterval > 0 && lastMinerBlock[miner] != 0 && block.number - lastMinerBlock[miner] < minBlockInterval) {
            revert MiningTooFrequently();
        }

        commitment.commitHash = commitHash;
        commitment.timestamp = uint64(block.number);
        commitment.flags.revealed = false;
        commitment.revealedValue = bytes32(0);
        commitment.poolId = poolId;

        emit CommitmentSubmitted(miner, commitHash, poolId);
    }

    function revealMiningCommitment(
        bytes32 previousOutput,
        bytes calldata temporalSeed,
        uint64 nonce,
        bytes calldata signature,
        bytes32 secretValue,
        uint256 poolId
    ) external nonReentrant whenNotPaused {
        _revealMiningCommitment(msg.sender, previousOutput, temporalSeed, nonce, signature, secretValue, poolId);
    }

    function revealMiningCommitmentMeta(
        address miner,
        bytes32 previousOutput,
        bytes calldata temporalSeed,
        uint64 nonce,
        bytes calldata signature,
        bytes32 secretValue,
        uint256 poolId,
        uint256 deadline,
        bytes calldata metaSignature
    ) external nonReentrant whenNotPaused {
        if (block.timestamp > deadline) revert DeadlineExpired();
        uint256 metaNonce = nonces[miner]++;
        bytes32 structHash = keccak256(
            abi.encode(
                MINING_REVEAL_TYPEHASH,
                miner,
                previousOutput,
                keccak256(temporalSeed),
                nonce,
                keccak256(signature),
                secretValue,
                poolId,
                metaNonce,
                deadline
            )
        );
        address signer = _hashTypedDataV4(structHash).recover(metaSignature);
        if (signer != miner) revert InvalidSigner();

        _revealMiningCommitment(miner, previousOutput, temporalSeed, nonce, signature, secretValue, poolId);
    }

    function _finalizeMiningReward(bytes32 hmacOutput, uint256 poolId) internal returns (uint256) {
        uint256 calculatedReward = MiningLib.calculateMiningReward(
            hmacOutput,
            rewardAmount,
            bonusThreshold,
            bonusMultiplier,
            totalMined,
            MINING_ALLOCATION,
            miningPools[poolId]
        );
        
        if (calculatedReward > 0) {
            tgbtToken.mint(msg.sender, calculatedReward);
        }

        totalMined += calculatedReward;
        miningPools[poolId].totalMined += calculatedReward;

        return calculatedReward;
    }

    // Struct to group reveal parameters
    struct RevealParams {
        address miner;
        bytes32 previousOutput;
        bytes temporalSeed;
        uint64 nonce;
        bytes signature;
        bytes32 secretValue;
        uint256 poolId;
    }

    function _checkCommitmentValidity(
        MiningLib.RevealParams memory params,
        MiningLib.Commitment storage commitment
    ) internal view {
        MiningLib.checkCommitmentValidity(params, commitment);
    }

    function _validateMiningCommitment(
        MiningLib.RevealParams memory params
    ) internal view returns (MiningLib.Commitment storage commitment, MiningLib.MiningPool storage pool) {
        if (bloomFilter.size == 0) revert BloomFilterNotInitialized();
        if (totalMined >= MINING_ALLOCATION) revert MiningCapReached();
        if (tstakeToken.balanceOf(params.miner) < REQUIRED_TSTAKE_AMOUNT) revert InsufficientStake();
        if (params.poolId >= poolCount || !miningPools[params.poolId].active) revert InvalidPoolId();

        commitment = minerCommitments[params.miner];
        if (commitment.commitHash == bytes32(0)) revert NoCommitmentFound();
        if (commitment.flags.revealed) revert CommitmentAlreadyRevealed();
        if (block.number < uint256(commitment.timestamp) + minCommitmentAge) revert CommitmentTooRecent();
        if (block.number > uint256(commitment.timestamp) + maxCommitmentAge) revert CommitmentExpired();
        if (commitment.poolId != params.poolId) revert InvalidPoolId();

        _checkCommitmentValidity(params, commitment);

        pool = miningPools[params.poolId]; // Cache pool
    }

    function _processMiningRevealAndFinalize(MiningLib.RevealParams memory params) internal {
        (MiningLib.Commitment storage commitment, MiningLib.MiningPool storage pool) = _validateMiningCommitment(params);

        commitment.revealedValue = _processMiningReveal(
            params.previousOutput,
            params.temporalSeed,
            params.nonce,
            params.signature,
            params.secretValue,
            pool.targetDifficulty
        );
        commitment.flags.revealed = true;

        _updateOutputHistory(commitment.revealedValue);
        usedOutputs[commitment.revealedValue] = block.number;
        lastMinerBlock[params.miner] = block.number;

        BloomFilterLib.updateFilter(bloomFilter, commitment.revealedValue);
        outputCount++;

        entropyAccumulator = uint256(keccak256(abi.encodePacked(
            entropyAccumulator,
            commitment.revealedValue,
            block.timestamp,
            block.prevrandao
        )));

        _checkEpochTransition();

        uint256 calculatedReward = _finalizeMiningReward(commitment.revealedValue, params.poolId);
        
        if (historicalStorage.enabled) {
            _archiveBlock(
                commitment.revealedValue,
                params.previousOutput,
                params.nonce,
                params.miner,
                type(uint256).max - uint256(commitment.revealedValue),
                calculatedReward,
                block.timestamp,
                params.poolId
            );
        }

        _processRandomnessRequests();

        emit CommitmentRevealed(params.miner, commitment.revealedValue, params.poolId);
        emit BeaconBlockMined(params.miner, commitment.revealedValue, calculatedReward, params.nonce, block.timestamp, params.poolId);
    }

    function _revealMiningCommitment(
        address miner,
        bytes32 previousOutput,
        bytes calldata temporalSeed,
        uint64 nonce,
        bytes calldata signature,
        bytes32 secretValue,
        uint256 poolId
    ) internal {
        MiningLib.RevealParams memory params = MiningLib.RevealParams({
            miner: miner,
            previousOutput: previousOutput,
            temporalSeed: temporalSeed,
            nonce: nonce,
            signature: signature,
            secretValue: secretValue,
            poolId: poolId
        });
        _processMiningRevealAndFinalize(params);
    }

    /**
     * @notice Archives a new block in the historical storage
     * @dev Manages the historical blocks array according to configured max size
     */
    function _archiveBlock(
        bytes32 output,
        bytes32 previousOutput,
        uint64 nonce,
        address miner,
        uint256 actualDifficulty,
        uint256 reward,
        uint256 timestamp,
        uint256 poolId
    ) internal {
        StorageLib.archiveBlock(
            historicalStorage,
            output,
            previousOutput,
            nonce,
            miner,
            actualDifficulty,
            reward,
            timestamp,
            poolId
        );
    }

    /**
     * @notice Configures the historical block storage
     * @param enabled Whether to store historical blocks
     * @param maxBlocks Maximum number of historical blocks to store (0 for unlimited)
     */
    function configureHistoricalStorage(bool enabled, uint256 maxBlocks) external onlyRole(GOVERNANCE_ROLE) {
        StorageLib.configureHistoricalStorage(
            historicalStorage,
            enabled,
            maxBlocks,
            genesisBlockOutput,
            genesisBlockTimestamp,
            msg.sender
        );
    }
    
    /**
     * @notice Gets the count of historical blocks stored
     * @return count Number of historical blocks
     */
    function getHistoricalBlockCount() external view returns (uint256 count) {
        return historicalStorage.blocks.length;
    }
    
    /**
     * @notice Gets multiple historical blocks in a range
     * @param startIndex Start index (inclusive)
     * @param endIndex End index (exclusive)
     * @return blocks Array of BeaconBlock structs
     */
    function getHistoricalBlockRange(uint256 startIndex, uint256 endIndex) 
        external 
        view 
        returns (StorageLib.BeaconBlock[] memory blocks) 
    {
        return StorageLib.getHistoricalBlockRange(
            historicalStorage,
            startIndex,
            endIndex
        );
    }

    function _validatePreviousOutput(bytes32 previousOutput) internal view returns (bool isValid) {
        assembly {
            let i := 0
            let size := OUTPUT_HISTORY_SIZE
            let baseSlot := outputHistory.slot
            for { } lt(i, size) { i := add(i, 1) } {
                let slot := add(baseSlot, i)
                if eq(sload(slot), previousOutput) {
                    isValid := 1
                    break
                }
            }
        }
    }

    // Update _processMiningReveal to use MiningLib
    function _processMiningReveal(
        bytes32 previousOutput,
        bytes memory temporalSeed,
        uint64 nonce,
        bytes memory signature,
        bytes32 secretValue,
        uint256 poolDifficulty
    ) internal view returns (bytes32 hmacOutput) {
        return MiningLib.processMiningReveal(
            previousOutput,
            temporalSeed,
            nonce,
            signature,
            secretValue,
            poolDifficulty,
            msg.sender,
            bloomFilter,
            usedOutputs,
            _quantumResistantHash
        );
    }

    // Keep as gateway function to provide to MiningLib
    function _quantumResistantHash(bytes memory input) internal view returns (bytes32) {
        return MiningLib.quantumResistantHash(input);
    }

    function _checkEpochTransition() internal {
        rewardAmount = TokenomicsLib.checkEpochTransition(epochState);
    }

    // Randomness Functions
    function _processRandomnessAndUpdateMerkle() internal {
        bytes32 historicalHash = _getHistoricalOutputsHash();
        randomnessContext.state.processPendingRequests(historicalHash, entropyAccumulator);

        bytes32 newRoot = keccak256(abi.encodePacked(entropyMerkleRoot, entropyAccumulator, historicalHash, block.timestamp, block.prevrandao));
        entropyMerkleRoot = newRoot;
        emit MerkleRootUpdated(newRoot);
    }

    function _processRandomnessRequests() internal {
        _processRandomnessAndUpdateMerkle();
    }

    function updateEntropyMerkleRoot() external {
        _processRandomnessAndUpdateMerkle();
    }

    function requestRandomness(bytes32 userSeed) external nonReentrant whenNotPaused returns (uint256 requestId) {
        if (randomnessContext.state.fee == 0) revert FeeNotSet();
        tgbtToken.burnFrom(msg.sender, randomnessContext.state.fee);
        requestId = randomnessContext.state.createRequest(msg.sender, userSeed);
        emit RandomnessRequested(requestId, msg.sender, userSeed);
        return requestId;
    }

    function contributeEntropy(uint256 requestId, bytes32 entropyContribution, bytes calldata entropySignature)
        external
        nonReentrant
        whenNotPaused
    {
        bytes32 messageHash = keccak256(abi.encodePacked(requestId, entropyContribution, msg.sender));
        address recovered = messageHash.toEthSignedMessageHash().recover(entropySignature);
        if (recovered != msg.sender) revert InvalidSigner();

        bool shouldFulfill = randomnessContext.state.addContribution(requestId, msg.sender, entropyContribution);
        emit EntropyContributed(requestId, msg.sender, entropyContribution);

        if (shouldFulfill) {
            _fulfillRandomness(requestId);
        }
    }

    function _fulfillRandomness(uint256 requestId) internal {
        bytes32 randomValue = randomnessContext.state.fulfillRequest(requestId, _getHistoricalOutputsHash(), entropyAccumulator);
        emit RandomnessFulfilled(requestId, randomValue);
    }

    function getRandomness(uint256 requestId) external view returns (bytes32 randomValue) {
        return randomnessContext.state.getRandomness(requestId);
    }

    function _getHistoricalOutputsHash() internal view returns (bytes32 combinedHash) {
        bytes32[] memory outputs = BytesArrayLib.createBytes32Array(OUTPUT_HISTORY_SIZE);
        for (uint256 i = 0; i < OUTPUT_HISTORY_SIZE; i++) {
            outputs[i] = outputHistory[i];
        }
        return keccak256(abi.encodePacked(outputs));
    }

    function _updateOutputHistory(bytes32 newOutput) internal {
        currentOutputIndex = (currentOutputIndex + 1) % OUTPUT_HISTORY_SIZE;
        outputHistory[currentOutputIndex] = newOutput;
        lastOutputTimestamp = block.timestamp;
        emit OutputHistoryUpdated(newOutput, currentOutputIndex);
    }

    // Utility Functions
    function pruneExpiredOutputs(bytes32[] calldata outputs) external returns (uint256 count) {
        if (bloomFilter.size == 0) revert BloomFilterNotInitialized();
        uint256 pruneCount = 0;
        uint256 outputsLen = outputs.length; // ← use a local variable to avoid length lookup issues
        for (uint256 i = 0; i < outputsLen; i++) {
            bytes32 output = outputs[i];
            if (output == genesisBlockOutput) continue; // Prevent pruning genesis block
            uint256 blockNum = usedOutputs[output];
            if (blockNum > 0 && block.number - blockNum > outputExpiryBlocks) {
                delete usedOutputs[output];
                pruneCount++;
            }
        }
        if (pruneCount > 0) {
            BloomFilterLib.clearFilter(bloomFilter);
            outputCount = 1; // Preserve genesis block
            usedOutputs[genesisBlockOutput] = block.number; // Ensure genesis block remains
            emit OutputsPruned(pruneCount);
            emit BloomFilterReset(bloomFilter.size, bloomFilter.numHashes);
        }
        return pruneCount;
    }

    function getMiningChallenge(uint256 poolId) external view returns (bytes32[] memory outputs, uint256 difficulty) {
        if (poolId >= poolCount || !miningPools[poolId].active) revert InvalidPoolId();
        bytes32[] memory history = BytesArrayLib.createBytes32Array(OUTPUT_HISTORY_SIZE);
        for (uint256 i = 0; i < OUTPUT_HISTORY_SIZE; i++) {
            history[i] = outputHistory[i];
        }
        return (history, miningPools[poolId].targetDifficulty);
    }

    function getEntropyStats() external view returns (uint256 accumulator, bytes32 merkleRoot) {
        return (entropyAccumulator, entropyMerkleRoot);
    }

    // Tokenomics Functions
    function getTokenomicsInfo() external view returns (
        uint256 cap,
        uint256 miningAlloc,
        uint256 currentBlockReward,
        uint256 epoch,
        uint256 totalMinedToDate,
        uint256 remaining,
        uint256 nextHalvingBlock
    ) {
        return (
            TOTAL_SUPPLY_CAP,
            MINING_ALLOCATION,
            rewardAmount,
            currentEpoch,
            totalMined,
            MINING_ALLOCATION - totalMined,
            lastHalvingBlock + halvingInterval
        );
    }

    function setHalvingInterval(uint256 blocks) external onlyRole(TOKENOMICS_ROLE) {
        if (blocks == 0) revert InvalidEpochParameters();
        halvingInterval = blocks;
        emit GovernanceParameterChanged("halvingInterval", blocks);
    }

    function setEpochBlocks(uint256 blocks) external onlyRole(TOKENOMICS_ROLE) {
        if (blocks == 0) revert InvalidEpochParameters();
        blocksPerEpoch = blocks;
        emit GovernanceParameterChanged("blocksPerEpoch", blocks);
    }

    // Multi-Pool Management
    function createMiningPool(uint256 _targetDifficulty, uint256 emissionBucket) external onlyRole(GOVERNANCE_ROLE) {
        if (poolCount >= MAX_POOLS) revert MaxPoolsReached();
        if (_targetDifficulty < MIN_DIFFICULTY || _targetDifficulty > MAX_DIFFICULTY) revert InvalidDifficulty();
        if (emissionBucket == 0 || totalMined + emissionBucket > MINING_ALLOCATION) revert InvalidEpochParameters();

        uint256 poolId = poolCount++;
        miningPools[poolId] = MiningLib.MiningPool({
            targetDifficulty: _targetDifficulty,
            emissionBucket: emissionBucket,
            totalMined: 0,
            active: true
        });

        emit MiningPoolCreated(poolId, _targetDifficulty, emissionBucket);
    }

    function updateMiningPool(uint256 poolId, uint256 _targetDifficulty, uint256 emissionBucket, bool active)
        external
        onlyRole(GOVERNANCE_ROLE)
    {
        if (poolId >= poolCount) revert InvalidPoolId();
        if (_targetDifficulty < MIN_DIFFICULTY || _targetDifficulty > MAX_DIFFICULTY) revert InvalidDifficulty();
        if (emissionBucket == 0 || totalMined + emissionBucket > MINING_ALLOCATION) revert InvalidEpochParameters();

        miningPools[poolId].targetDifficulty = _targetDifficulty;
        miningPools[poolId].emissionBucket = emissionBucket;
        miningPools[poolId].active = active;

        emit MiningPoolUpdated(poolId, _targetDifficulty, emissionBucket);
        if (!active) {
            emit MiningPoolDeactivated(poolId);
        }
    }

    function getPoolInfo(uint256 poolId) external view returns (uint256 difficulty, uint256 emission, uint256 mined, bool active) {
        if (poolId >= poolCount) revert InvalidPoolId();
        MiningLib.MiningPool storage pool = miningPools[poolId];
        return (pool.targetDifficulty, pool.emissionBucket, pool.totalMined, pool.active);
    }

    // Admin/Governance Functions
    function setRewardAmount(uint256 amount) external onlyRole(TOKENOMICS_ROLE) {
        rewardAmount = amount;
        emit GovernanceParameterChanged("rewardAmount", amount);
    }

    function setTargetDifficulty(uint256 poolId, uint256 difficulty) external onlyRole(GOVERNANCE_ROLE) {
        if (poolId >= poolCount) revert InvalidPoolId();
        if (difficulty < MIN_DIFFICULTY || difficulty > MAX_DIFFICULTY) revert InvalidDifficulty();
        miningPools[poolId].targetDifficulty = difficulty;
        emit GovernanceParameterChanged("targetDifficulty", difficulty);
    }

    function setBonusParameters(uint256 multiplier, uint256 threshold) external onlyRole(GOVERNANCE_ROLE) {
        if (multiplier < 100 || multiplier > MAX_BONUS_MULTIPLIER) revert InvalidMultiplier();
        if (threshold <= 1) revert InvalidThresholdValue();
        bonusMultiplier = multiplier;
        bonusThreshold = threshold;
        emit GovernanceParameterChanged("bonusMultiplier", multiplier);
        emit GovernanceParameterChanged("bonusThreshold", threshold);
    }

    function setTGBTToken(address newToken) external onlyRole(GOVERNANCE_ROLE) {
        if (newToken == address(0)) revert ZeroAddress();
        tgbtToken = ITGBT(newToken);
        emit TokenUpdated(newToken);
    }

    function setTStakeToken(address newToken) external onlyRole(GOVERNANCE_ROLE) {
        if (newToken == address(0)) revert ZeroAddress();
        tstakeToken = ITGBT(newToken);
        emit TokenUpdated(newToken);
    }

    function setOutputExpiryBlocks(uint256 blocks) external onlyRole(GOVERNANCE_ROLE) {
        if (blocks < MIN_EXPIRY_BLOCKS) revert ExpiryTooShort();
        outputExpiryBlocks = blocks;
        emit GovernanceParameterChanged("outputExpiryBlocks", blocks);
    }

    function setConsensusParameters(uint256 minSubmissions, uint256 threshold) external onlyRole(GOVERNANCE_ROLE) {
        if (threshold < 51 || threshold > 100) revert InvalidThreshold();
        if (minSubmissions < 1) revert InvalidMinSubmissions();
        minSubmissionsPerBlock = minSubmissions;
        consensusThreshold = threshold;
        emit GovernanceParameterChanged("minSubmissionsPerBlock", minSubmissions);
        emit GovernanceParameterChanged("consensusThreshold", threshold);
    }

    function setCommitRevealParameters(uint256 minAge, uint256 maxAge) external onlyRole(GOVERNANCE_ROLE) {
        if (minAge < 3) revert MinAgeTooLow();
        if (maxAge < minAge * 2) revert MaxAgeTooLow();
        if (maxAge > 1000) revert MaxAgeTooHigh();
        minCommitmentAge = minAge;
        maxCommitmentAge = maxAge;
        emit GovernanceParameterChanged("minCommitmentAge", minAge);
        emit GovernanceParameterChanged("maxCommitmentAge", maxAge);
    }

    function setMinBlockInterval(uint256 blocks) external onlyRole(GOVERNANCE_ROLE) {
        minBlockInterval = blocks;
        emit GovernanceParameterChanged("minBlockInterval", blocks);
    }

    function setRandomnessFee(uint256 fee) external onlyRole(GOVERNANCE_ROLE) {
        uint256 oldFee = randomnessContext.state.fee;
        randomnessContext.state.fee = fee;
        emit RandomnessFeeChanged(oldFee, fee);
    }

    function setContributionParameters(uint256 minContributions, uint256 maxContributions) external onlyRole(GOVERNANCE_ROLE) {
        if (minContributions < 2) revert MinContributionsTooLow();
        if (maxContributions < minContributions) revert MaxLessThanMin();
        if (maxContributions > 50) revert MaxContributionsTooHigh();
        randomnessContext.state.minContributions = minContributions;
        randomnessContext.state.maxContributions = maxContributions;
        emit GovernanceParameterChanged("minContributions", minContributions);
        emit GovernanceParameterChanged("maxContributions", maxContributions);
    }

    // UUPS Upgradeability
    function _authorizeUpgrade(address newImplementation) internal override onlyRole(UPGRADER_ROLE) {}

    // Emergency Functions
    function pause() external onlyRole(EMERGENCY_ROLE) {
        _pause();
    }

    function unpause() external onlyRole(EMERGENCY_ROLE) {
        _unpause();
    }

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor() {
        _disableInitializers();
    }

    /**
     * @notice Submit a solution anonymously for stealth mining rewards.
     * @param anonymousId HMAC of miner's public key (stealth identifier)
     * @param proof Proof data (opaque, to be verified)
     */
    function submitSolution(
        bytes32 anonymousId,
        bytes calldata proof
    ) external {
        if (usedAnonymousIds[anonymousId]) revert OutputAlreadyUsed();
        usedAnonymousIds[anonymousId] = true;

        _verifyProof(anonymousId, proof);

        // Example: reward is fixed or can be parameterized
        uint256 reward = rewardAmount;
        address stealthAddress = computeStealthAddress(anonymousId);
        tgbtToken.mint(stealthAddress, reward);

        emit StealthSolutionSubmitted(anonymousId, stealthAddress, reward);
    }

    /**
     * @dev Internal stub for proof verification. Replace with actual logic.
     */
    function _verifyProof(bytes32 /*anonymousId*/, bytes calldata /*proof*/) internal pure {
        // Implement actual proof verification logic here.
        // For now, this is a stub that always passes.
    }

    /**
     * @dev Computes a stealth address from the anonymousId.
     *      Replace with actual stealth address computation as needed.
     */
    function computeStealthAddress(bytes32 anonymousId) public pure returns (address) {
        // Example: take the lower 20 bytes of the anonymousId as the address
        return address(uint160(uint256(anonymousId)));
    }
}