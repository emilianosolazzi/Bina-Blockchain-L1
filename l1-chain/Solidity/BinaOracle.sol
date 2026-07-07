// SPDX-License-Identifier: MIT
pragma solidity ^0.8.30;

/**
 * @title BinaOracle
 * @author Emiliano Solazzi
 * @notice Utility oracle for using finalized BINA L1 randomness on EVM chains.
 * @dev BINA L1 uses BLAKE3 PoW and Ed25519 + Falcon-512 wallet signatures.
 *      Those proofs are verified by BINA nodes off-chain. This contract records
 *      finalized BINA outputs relayed by authorized EVM publishers and exposes
 *      consumer-friendly derivation helpers for DeFi, games, AI, validator
 *      selection, and other utility use cases.
 */
contract BinaOracle {
    // ======================== TYPES ========================

    struct BinaOutput {
        uint64 height;
        bytes32 blockHash;
        bytes32 randomnessOutput;
        bytes32 nullifier;
        bytes20 binaMiner;
        uint64 btcHeight;
        bytes32 btcSeed;
        uint64 minedTimestamp;
        uint8 workBits;
        bytes32 claimDigest;
        bytes32 electionScore;
    }

    struct StoredOutput {
        bytes32 randomnessOutput;
        bytes32 nullifier;
        bytes20 binaMiner;
        uint64 height;
        uint64 btcHeight;
        uint64 minedTimestamp;
        uint8 workBits;
        address publisher;
    }

    struct UtilityRequest {
        address requester;
        bytes32 purpose;
        bytes32 salt;
        uint64 minHeight;
        bool fulfilled;
    }

    // ======================== EVENTS ========================

    event PublisherUpdated(address indexed publisher, bool authorized);
    event BinaOutputSubmitted(
        uint64 indexed height,
        bytes32 indexed blockHash,
        bytes32 indexed purpose,
        bytes32 randomnessOutput,
        bytes32 nullifier,
        bytes20 binaMiner,
        address publisher
    );
    event BinaOutputMetadata(
        bytes32 indexed blockHash,
        uint64 btcHeight,
        uint64 minedTimestamp,
        uint8 workBits,
        bytes32 claimDigest,
        bytes32 electionScore,
        bytes32 proofHash
    );
    event BinaProofBundle(
        bytes32 indexed blockHash,
        bytes proofBundle
    );
    event PurposeSeedUpdated(bytes32 indexed purpose, bytes32 seed, uint64 height, bytes32 blockHash);
    event UtilityRequested(
        uint256 indexed requestId,
        address indexed requester,
        bytes32 indexed purpose,
        bytes32 salt,
        uint64 minHeight
    );
    event UtilityFulfilled(
        uint256 indexed requestId,
        bytes32 indexed purpose,
        bytes32 seed,
        bytes32 utilityWord,
        uint64 height
    );

    // ======================== ERRORS ========================

    error NotOwner();
    error NotPublisher();
    error ZeroAddress();
    error ZeroValue();
    error InvalidTimestamp();
    error InvalidWorkBits();
    error AlreadySubmitted();
    error NullifierAlreadyUsed();
    error OutputNotFound();
    error PurposeNotReady();
    error RequestNotFound();
    error RequestAlreadyFulfilled();
    error InvalidUpperBound();

    // ======================== CONSTANTS ========================

    bytes32 public constant PURPOSE_GENERIC = keccak256("BINA_GENERIC_UTILITY");
    bytes32 public constant PURPOSE_VALIDATOR_SELECTION = keccak256("BINA_VALIDATOR_SELECTION");
    bytes32 public constant PURPOSE_BATCH_SEED = keccak256("BINA_BATCH_SEED");
    bytes32 public constant PURPOSE_DEFI = keccak256("BINA_DEFI");
    bytes32 public constant PURPOSE_GAMING = keccak256("BINA_GAMING");
    bytes32 public constant PURPOSE_AI = keccak256("BINA_AI");

    uint8 public constant MIN_WORK_BITS = 22;
    uint8 public constant MAX_WORK_BITS = 64;
    uint256 public constant MAX_TIMESTAMP_DRIFT = 1 hours;
    uint256 public constant MAX_TIMESTAMP_AGE = 7 days;

    // ======================== STORAGE ========================

    address public owner;
    mapping(address => bool) public publishers;

    mapping(bytes32 => StoredOutput) public outputsByBlockHash;
    mapping(bytes32 => bool) public usedNullifiers;
    mapping(bytes32 => bytes32) public latestSeedByPurpose;
    mapping(bytes32 => bytes32) public latestBlockByPurpose;
    mapping(bytes32 => uint64) public latestHeightByPurpose;
    mapping(bytes32 => uint64) public latestBTCHeightByPurpose;

    bytes32[] public blockHashes;
    mapping(uint256 => UtilityRequest) public requests;
    uint256 public nextRequestId = 1;

    // ======================== MODIFIERS ========================

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    modifier onlyPublisher() {
        if (!publishers[msg.sender]) revert NotPublisher();
        _;
    }

    // ======================== CONSTRUCTOR ========================

    constructor(address initialPublisher) {
        owner = msg.sender;
        publishers[msg.sender] = true;
        emit PublisherUpdated(msg.sender, true);

        if (initialPublisher != address(0) && initialPublisher != msg.sender) {
            publishers[initialPublisher] = true;
            emit PublisherUpdated(initialPublisher, true);
        }
    }

    // ======================== ADMIN ========================

    function transferOwnership(address newOwner) external onlyOwner {
        if (newOwner == address(0)) revert ZeroAddress();
        owner = newOwner;
    }

    function setPublisher(address publisher, bool authorized) external onlyOwner {
        if (publisher == address(0)) revert ZeroAddress();
        publishers[publisher] = authorized;
        emit PublisherUpdated(publisher, authorized);
    }

    // ======================== BINA OUTPUT INGESTION ========================

    /**
     * @notice Relay a finalized BINA L1 output into the EVM oracle.
     * @param output Finalized BINA output from the L1 node/P2P network.
     * @param purpose Utility namespace this output should update.
     * @param proofBundle Opaque BINA proof material for off-chain audit
     *        (for example public key, hybrid signature, serialized header, and
     *        BINA peer/election metadata). EVM storage keeps compact facts only.
     */
    function submitOutput(
        BinaOutput calldata output,
        bytes32 purpose,
        bytes calldata proofBundle
    ) external onlyPublisher {
        _validateOutput(output);

        if (outputsByBlockHash[output.blockHash].randomnessOutput != bytes32(0)) {
            revert AlreadySubmitted();
        }
        if (usedNullifiers[output.nullifier]) revert NullifierAlreadyUsed();

        outputsByBlockHash[output.blockHash] = StoredOutput({
            randomnessOutput: output.randomnessOutput,
            nullifier: output.nullifier,
            binaMiner: output.binaMiner,
            height: output.height,
            btcHeight: output.btcHeight,
            minedTimestamp: output.minedTimestamp,
            workBits: output.workBits,
            publisher: msg.sender
        });
        usedNullifiers[output.nullifier] = true;
        blockHashes.push(output.blockHash);

        bytes32 effectivePurpose = purpose == bytes32(0) ? PURPOSE_GENERIC : purpose;
        _updatePurposeSeed(effectivePurpose, output.randomnessOutput, output.height, output.btcHeight, output.blockHash);

        _emitOutputEvents(output, effectivePurpose, proofBundle);
    }

    function submitOutputForPurposes(
        BinaOutput calldata output,
        bytes32[] calldata purposes,
        bytes calldata proofBundle
    ) external onlyPublisher {
        _validateOutput(output);

        if (outputsByBlockHash[output.blockHash].randomnessOutput != bytes32(0)) {
            revert AlreadySubmitted();
        }
        if (usedNullifiers[output.nullifier]) revert NullifierAlreadyUsed();
        if (purposes.length == 0) revert ZeroValue();

        outputsByBlockHash[output.blockHash] = StoredOutput({
            randomnessOutput: output.randomnessOutput,
            nullifier: output.nullifier,
            binaMiner: output.binaMiner,
            height: output.height,
            btcHeight: output.btcHeight,
            minedTimestamp: output.minedTimestamp,
            workBits: output.workBits,
            publisher: msg.sender
        });
        usedNullifiers[output.nullifier] = true;
        blockHashes.push(output.blockHash);

        for (uint256 i = 0; i < purposes.length; i++) {
            bytes32 purpose = purposes[i] == bytes32(0) ? PURPOSE_GENERIC : purposes[i];
            _updatePurposeSeed(purpose, output.randomnessOutput, output.height, output.btcHeight, output.blockHash);
            _emitOutputEvents(output, purpose, proofBundle);
        }
    }

    // ======================== UTILITY REQUESTS ========================

    function requestUtility(bytes32 purpose, bytes32 salt, uint64 minHeight) external returns (uint256 requestId) {
        requestId = nextRequestId++;
        bytes32 effectivePurpose = purpose == bytes32(0) ? PURPOSE_GENERIC : purpose;
        requests[requestId] = UtilityRequest({
            requester: msg.sender,
            purpose: effectivePurpose,
            salt: salt,
            minHeight: minHeight,
            fulfilled: false
        });
        emit UtilityRequested(requestId, msg.sender, effectivePurpose, salt, minHeight);
    }

    function fulfillUtility(uint256 requestId) external returns (bytes32 utilityWord) {
        UtilityRequest storage request = requests[requestId];
        if (request.requester == address(0)) revert RequestNotFound();
        if (request.fulfilled) revert RequestAlreadyFulfilled();

        bytes32 seed = latestSeedByPurpose[request.purpose];
        if (seed == bytes32(0) || latestHeightByPurpose[request.purpose] < request.minHeight) {
            revert PurposeNotReady();
        }

        request.fulfilled = true;
        utilityWord = _derive(seed, request.purpose, request.salt, request.requester, requestId);
        emit UtilityFulfilled(requestId, request.purpose, seed, utilityWord, latestHeightByPurpose[request.purpose]);
    }

    // ======================== VIEW HELPERS ========================

    function getOutput(bytes32 blockHash) external view returns (StoredOutput memory output) {
        output = outputsByBlockHash[blockHash];
        if (output.randomnessOutput == bytes32(0)) revert OutputNotFound();
    }

    function getLatestSeed(bytes32 purpose) public view returns (bytes32 seed, uint64 height, uint64 btcHeight, bytes32 blockHash) {
        bytes32 effectivePurpose = purpose == bytes32(0) ? PURPOSE_GENERIC : purpose;
        seed = latestSeedByPurpose[effectivePurpose];
        if (seed == bytes32(0)) revert PurposeNotReady();
        height = latestHeightByPurpose[effectivePurpose];
        btcHeight = latestBTCHeightByPurpose[effectivePurpose];
        blockHash = latestBlockByPurpose[effectivePurpose];
    }

    function deriveWord(bytes32 purpose, bytes32 salt, address consumer) public view returns (bytes32) {
        (bytes32 seed,,,) = getLatestSeed(purpose);
        bytes32 effectivePurpose = purpose == bytes32(0) ? PURPOSE_GENERIC : purpose;
        return _derive(seed, effectivePurpose, salt, consumer, 0);
    }

    function randomUint(bytes32 purpose, bytes32 salt, uint256 upperBound) external view returns (uint256) {
        if (upperBound == 0) revert InvalidUpperBound();
        return uint256(deriveWord(purpose, salt, msg.sender)) % upperBound;
    }

    function blockCount() external view returns (uint256) {
        return blockHashes.length;
    }

    function blockHashAt(uint256 index) external view returns (bytes32) {
        return blockHashes[index];
    }

    // ======================== PURE UTILITY HELPERS ========================

    function shuffleValidators(bytes32 seed, address[] memory validators) external pure returns (address[] memory) {
        uint256 n = validators.length;
        address[] memory shuffled = new address[](n);
        for (uint256 i = 0; i < n; i++) {
            shuffled[i] = validators[i];
        }
        if (n < 2) return shuffled;

        for (uint256 i = n - 1; i > 0; i--) {
            uint256 j = uint256(keccak256(abi.encodePacked("BINA_SHUFFLE", seed, i))) % (i + 1);
            (shuffled[i], shuffled[j]) = (shuffled[j], shuffled[i]);
        }
        return shuffled;
    }

    function generateBatchId(bytes32 seed, bytes32 txMerkleRoot) external pure returns (bytes32) {
        return keccak256(abi.encodePacked("BINA_BATCH", seed, txMerkleRoot));
    }

    function generateValidatorSetId(bytes32 seed, uint256 validatorCount) external pure returns (bytes32) {
        return keccak256(abi.encodePacked("BINA_VALIDATOR_SET", seed, validatorCount));
    }

    // ======================== INTERNALS ========================

    function _validateOutput(BinaOutput calldata output) internal view {
        if (output.height == 0) revert ZeroValue();
        if (output.blockHash == bytes32(0)) revert ZeroValue();
        if (output.randomnessOutput == bytes32(0)) revert ZeroValue();
        if (output.nullifier == bytes32(0)) revert ZeroValue();
        if (output.binaMiner == bytes20(0)) revert ZeroValue();
        if (output.btcSeed == bytes32(0)) revert ZeroValue();
        if (output.claimDigest == bytes32(0)) revert ZeroValue();
        if (output.workBits < MIN_WORK_BITS || output.workBits > MAX_WORK_BITS) revert InvalidWorkBits();
        if (output.minedTimestamp > block.timestamp + MAX_TIMESTAMP_DRIFT) revert InvalidTimestamp();
        if (output.minedTimestamp + MAX_TIMESTAMP_AGE < block.timestamp) revert InvalidTimestamp();
    }

    function _updatePurposeSeed(
        bytes32 purpose,
        bytes32 seed,
        uint64 height,
        uint64 btcHeight,
        bytes32 blockHash
    ) internal {
        latestSeedByPurpose[purpose] = seed;
        latestHeightByPurpose[purpose] = height;
        latestBTCHeightByPurpose[purpose] = btcHeight;
        latestBlockByPurpose[purpose] = blockHash;
        emit PurposeSeedUpdated(purpose, seed, height, blockHash);
    }

    function _emitOutputEvents(
        BinaOutput calldata output,
        bytes32 purpose,
        bytes calldata proofBundle
    ) internal {
        bytes32 proofHash = keccak256(proofBundle);
        emit BinaOutputSubmitted(
            output.height,
            output.blockHash,
            purpose,
            output.randomnessOutput,
            output.nullifier,
            output.binaMiner,
            msg.sender
        );
        emit BinaOutputMetadata(
            output.blockHash,
            output.btcHeight,
            output.minedTimestamp,
            output.workBits,
            output.claimDigest,
            output.electionScore,
            proofHash
        );
        emit BinaProofBundle(output.blockHash, proofBundle);
    }

    function _derive(
        bytes32 seed,
        bytes32 purpose,
        bytes32 salt,
        address consumer,
        uint256 requestId
    ) internal view returns (bytes32) {
        return keccak256(
            abi.encodePacked(
                "BINA_EVM_UTILITY_V1",
                block.chainid,
                address(this),
                seed,
                purpose,
                salt,
                consumer,
                requestId
            )
        );
    }
}
