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
import { RandomnessLib } from "./RandomnessLib.sol"; // <<< Added import
import { GovernanceLib } from "./GovernanceLib.sol"; // <<< Added import (Needed for setting fee params)
import { IERC20Upgradeable } from "@openzeppelin/contracts-upgradeable/token/ERC20/IERC20Upgradeable.sol"; // <<< Added import

/**
 * @title TemporalGradientBeacon
 * @notice A temporal gradient beacon with mining, randomness generation, and governance features
 * @dev Uses UUPS upgrade pattern with role-based access control
 */
contract TemporalGradientBeacon is
    Initializable,
    Ownable2StepUpgradeable,
    ReentrancyGuardUpgradeable,
    PausableUpgradeable,
    UUPSUpgradeable,
    AccessControlUpgradeable,
    EIP712Upgradeable
{
    using ECDSAUpgradeable for bytes32;
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
    GovernanceLib.GovernanceContext internal governanceContext; // <<< Added Governance Context

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
    event EmergencyFeeParametersChanged(uint256 baseFee, uint256 feePerContributor); // <<< Added event
    event TokenomicsUpdate(uint256 indexed epochNumber, uint256 blockReward, uint256 blockNumber, bool isHalving);

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
        __EIP712_init("TemporalGradientBeacon", "1");

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
        governanceContext.miningPools[0] = MiningLib.MiningPool({
            targetDifficulty: _difficulty,
            emissionBucket: MINING_ALLOCATION,
            totalMined: 0,
            active: true
        });
        governanceContext.poolCount = 1; // Use governance context pool count
        poolCount = 1; // Keep legacy poolCount for compatibility? Re-evaluate if needed.

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
        governanceContext.minBlockInterval = 5;
        governanceContext.minSubmissionsPerBlock = 1;
        governanceContext.consensusThreshold = 70;
        governanceContext.minCommitmentAge = 5;
        governanceContext.maxCommitmentAge = 100;
        // Set legacy vars for compatibility? Re-evaluate if needed.
        minBlockInterval = 5;
        minSubmissionsPerBlock = 1;
        consensusThreshold = 70;
        minCommitmentAge = 5;
        maxCommitmentAge = 100;

        // Initialize randomness system using RandomnessLib.State
        randomnessState.tgbtTokenAddress = _tgbtToken; // <<< Set token address
        randomnessState.baseEmergencyFee = 100 ether; // <<< Set base fee
        randomnessState.feePerContributor = 10 ether; // <<< Set per contributor fee (example value)
        randomnessState.expiryBlocks = 50000;
        randomnessState.minContributions = 3;
        randomnessState.maxContributions = 10;
        randomnessState.maxBatchSize = 20; // <<< Example batch size

        // Initialize bloom filter with size, numHashes, and salt
        bloomFilter = BloomFilterLib.createFilter(1024, 3, block.timestamp); // Added block.timestamp as salt
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
    ) external nonReentrant whenNotPaused {
        require(tstakeToken.balanceOf(msg.sender) >= REQUIRED_TSTAKE_AMOUNT, "InsufficientStake");
        require(poolId < poolCount && miningPools[poolId].active, "InvalidPoolId");
        require(block.timestamp <= deadline, "DeadlineExpired");

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
        address recoveredSigner = ECDSAUpgradeable.recover(digest, signature);
        require(recoveredSigner == msg.sender, "InvalidSignature");
        require(recoveredSigner != address(0), "ZeroAddressSigner"); // Ensure non-zero address recovery

        // Check and increment nonce to prevent signature replay
        require(nonces[msg.sender] == nonce, "InvalidNonce");
        nonces[msg.sender]++; // Increment nonce after successful verification

        MiningLib.Commitment storage commitment = minerCommitments[msg.sender];
        require(
            commitment.commitHash == bytes32(0) ||
            commitment.flags.revealed ||
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
        require(!commitment.flags.revealed, "CommitmentAlreadyRevealed");
        require(block.number >= commitment.timestamp + minCommitmentAge, "CommitmentTooRecent");
        require(block.number <= commitment.timestamp + maxCommitmentAge, "CommitmentExpired");
        require(commitment.poolId == params.poolId, "InvalidPoolId");
        require(miningPools[params.poolId].active, "InvalidPoolId");

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
        require(computedHash == commitment.commitHash, "InvalidCommitment");

        // Validate previous output exists in history
        require(
            CoreUtilsLib.validatePreviousOutput(params.previousOutput, outputHistory, OUTPUT_HISTORY_SIZE),
            "InvalidPreviousOutput"
        );

        // Define a placeholder difficulty weight function (replace with actual logic if needed)
        function(address) view returns (uint256) difficultyWeightFn = _getDifficultyWeight; // Placeholder

        // Process the mining reveal using the library function
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
            MiningLib.quantumResistantHash, // Explicitly pass the hash function
            difficultyWeightFn // Pass the difficulty weight function
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
        return MiningLib.BASE_WEIGHT;
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
    ) external nonReentrant whenNotPaused {
        // Use the existing mapping `usedAnonymousIds`
        if (usedAnonymousIds[anonymousId]) revert DuplicateAnonymousId();
        usedAnonymousIds[anonymousId] = true;

        // Placeholder: Verify the proof associated with the anonymousId.
        // This function needs to implement the logic to validate the proof
        // without requiring the original commitment data directly linked to msg.sender.
        // It might involve checking the proof against contract state or using zero-knowledge techniques.
        _verifyProof(anonymousId, proof);

        // Placeholder: Compute the stealth address where rewards should be sent.
        // This function needs to implement the logic to derive a unique,
        // miner-controlled address from the anonymousId.
        address stealthRecipient = computeStealthAddress(anonymousId);
        require(stealthRecipient != address(0), "ZeroAddress"); // Basic validation

        // Placeholder: Determine the correct reward amount for this stealth submission.
        // This might be a fixed amount, based on the current epoch, or derived from the proof.
        // Using epochState.rewardAmount as a placeholder. Needs refinement.
        uint256 reward = epochState.rewardAmount; // Placeholder reward calculation

        // Ensure reward doesn't exceed allocation (basic check)
        require(totalMined + reward <= MINING_ALLOCATION, "AllocationExceeded");
        totalMined += reward;
        // Note: Pool-specific allocation is not tracked here, might need adjustment.

        // Mint reward to the computed stealth address
        tgbtToken.mint(stealthRecipient, reward);

        emit StealthSolutionSubmitted(anonymousId, stealthRecipient, reward);
    }

    /**
     * @notice Placeholder internal function to verify the anonymous proof.
     * @dev Needs implementation based on the specific proof system used. Placeholder is 'pure', but real implementation likely needs 'view' to read state.
     * @param anonymousId The identifier submitted.
     * @param proof The proof data.
     */
    function _verifyProof(bytes32 anonymousId, bytes calldata proof) internal pure { // Changed to pure for placeholder
        // --- Implementation Required ---
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
    }

    /* ========== RANDOMNESS FUNCTIONS (Using RandomnessLib) ========== */

    /**
     * @notice Requests a new random value.
     * @param userSeed An arbitrary seed provided by the user.
     * @return requestId The ID of the newly created randomness request.
     */
    function requestRandomness(bytes32 userSeed) external nonReentrant whenNotPaused returns (uint256 requestId) {
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
        bool shouldFulfill = RandomnessLib.addContribution(randomnessState, requestId, msg.sender, entropyContribution);

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
            IERC20Upgradeable(randomnessState.tgbtTokenAddress), // Cast token address
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
        require(poolId < poolCount, "InvalidPoolId"); // Use legacy poolCount
        MiningLib.MiningPool storage pool = miningPools[poolId]; // Use legacy mapping
        return (
            pool.targetDifficulty,
            pool.emissionBucket - pool.totalMined,
            pool.totalMined,
            pool.active
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
        require(poolId < poolCount && miningPools[poolId].active, "InvalidPoolId"); // Use legacy mapping/count
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
        require(newToken != address(0), "ZeroAddress");
        tgbtToken = ITGBT(newToken);
        randomnessState.tgbtTokenAddress = newToken; // <<< Also update in randomness state
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
     * @notice Sets output expiry blocks (using GovernanceLib context)
     * @param blocks Number of blocks before outputs expire
     */
    function setOutputExpiryBlocks(uint64 blocks) external onlyRole(GOVERNANCE_ROLE) {
        require(blocks >= MIN_EXPIRY_BLOCKS, "ExpiryTooShort");
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
        // Delegate to GovernanceLib
        GovernanceLib.setCommitRevealParameters(governanceContext, minAge, maxAge);
        // Update legacy vars if needed
        minCommitmentAge = minAge;
        maxCommitmentAge = maxAge;
    }

    /**
     * @notice Sets the parameters for the dynamic emergency randomness fulfillment fee.
     * @param baseFee The base fee in TGBT.
     * @param perContributorFee The additional fee per contributor in TGBT.
     */
    function setEmergencyFeeParams(uint256 baseFee, uint256 perContributorFee) external onlyRole(GOVERNANCE_ROLE) {
        GovernanceLib.setEmergencyFeeParameters(randomnessState, baseFee, perContributorFee);
    }

    /**
     * @notice Sets the contribution parameters for randomness requests.
     * @param minContributions Minimum required contributions.
     * @param maxContributions Maximum allowed contributions.
     */
    function setRandomnessContributionParams(uint256 minContributions, uint256 maxContributions) external onlyRole(GOVERNANCE_ROLE) {
        GovernanceLib.setContributionParameters(randomnessState, minContributions, maxContributions);
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