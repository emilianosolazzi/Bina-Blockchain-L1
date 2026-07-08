// SPDX-License-Identifier: MIT
pragma solidity ^0.8.30;

interface IBinaOracle {
    function isPQResistant() external pure returns (bool);
    function pqSecurityBits() external pure returns (uint8);
    function signingScheme() external pure returns (string memory);
}

/**
 * @title BinaOracle
 * @author Emiliano Solazzi
 * @notice Utility oracle for using finalized BINA L1 randomness on EVM chains.
 * @dev BINA L1 uses BLAKE3 PoW and Ed25519 + Falcon-512 wallet signatures.
 *      Those proofs are verified by BINA nodes off-chain. This contract records
 *      finalized BINA outputs relayed by authorized EVM publishers and exposes
 *      consumer-friendly derivation helpers for DeFi, games, AI, validator
 *      selection, and other utility use cases.
 *
 *      Trust model (read before integrating):
 *      - Publisher set is permissioned by `owner` via `setPublisher` — this
 *        contract does not re-verify BINA's BLAKE3 PoW or Ed25519/Falcon-512
 *        signatures on-chain (that would require an on-chain light client;
 *        Falcon-512 verification in the EVM is currently impractical). It
 *        trusts whichever authorized publisher(s) relay an output.
 *      - `quorumThreshold` (default 1) lets the owner require K independent
 *        publishers to submit byte-identical output+purposes before a
 *        purpose seed advances, reducing exposure to any single compromised
 *        or dishonest relayer. Disagreeing submissions revert rather than
 *        silently overwrite.
 *      - Publisher bonding (`depositBond`/`withdrawBond`) plus
 *        `requestFuturePublication`/`resolveFutureCommitment` give future
 *        (not-yet-mined) randomness requests an economic liveness guarantee:
 *        a bonded publisher is deterministically assigned to a requested
 *        height, and anyone can slash their bond if the deadline passes
 *        without that purpose reaching the target height. This does not
 *        identify *which* publisher specifically withheld a submission if
 *        multiple are bonded — it makes the assigned publisher accountable
 *        for the pool's liveness on that height, which is the honest
 *        alpha-stage tradeoff for keeping the contract simple.
 */
contract BinaOracle is IBinaOracle {
    // ======================== TYPES ========================

    /// @dev Field-mapping notes for whoever writes the BINA L1 -> EVM
    ///      publisher relay (this contract only stores what a publisher
    ///      submits; it does not re-verify BINA's PoW/signatures on-chain,
    ///      see the trust-model note above):
    ///
    ///      - minedTimestamp MUST be Unix **seconds** (checked against
    ///        `block.timestamp`, which is seconds). BINA L1's own node API
    ///        exposes both `timestamp` (consensus clock, Unix
    ///        **milliseconds** — used for BINA's own difficulty
    ///        retargeting) and `mined_timestamp_secs` (the same instant,
    ///        floor-divided to seconds). Use `mined_timestamp_secs`. Relaying
    ///        the raw millisecond value here reverts on every submission.
    ///      - workBits should be the block's *actual* achieved leading-zero
    ///        count (BINA API field `zero_bits`), not the difficulty
    ///        threshold it had to clear (`difficulty_bits`) — the former is
    ///        the real proof-of-work measure and is always >= the latter.
    ///        Note MAX_WORK_BITS=64 below: an extraordinarily lucky BINA
    ///        block (rare but not impossible) could exceed that and fail
    ///        `InvalidWorkBits` — cap workBits at 64 when relaying rather
    ///        than passing zero_bits through unclamped.
    ///      - btcHeight/btcSeed are BINA's *checkpoint-pinned* Bitcoin
    ///        anchor (BINA API fields `btc_height`/`btc_seed` on a
    ///        BlockRecord), reused across ~20 BINA blocks between
    ///        checkpoints — not an independent live read for every block.
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
        bool falconVerified;
    }

    struct StoredOutput {
        bytes32 randomnessOutput;
        bytes32 nullifier;
        bytes20 binaMiner;
        uint64 height;
        uint64 btcHeight;
        uint64 minedTimestamp;
        uint8 workBits;
        bool falconVerified;
        bytes32 proofHash;
        uint32 attestationCount;
        address publisher;
    }

    struct UtilityRequest {
        address requester;
        bytes32 purpose;
        bytes32 salt;
        uint64 minHeight;
        bool fulfilled;
    }

    /// @dev In-flight attestation state for a BINA blockHash that has not
    ///      yet reached `quorumThreshold` independent publisher submissions.
    struct PendingAttestation {
        bytes32 fingerprint; // keccak256(abi.encode(output, purposes)) all attesters must match
        uint32 count;
    }

    /// @dev A liveness commitment: `assignedPublisher` is expected to make
    ///      `purpose` reach `targetHeight` by `deadline`, backed by a locked
    ///      slice of their bond. See `resolveFutureCommitment`.
    struct FutureCommitment {
        bytes32 purpose;
        uint64 targetHeight;
        uint64 deadline;
        address assignedPublisher;
        bool resolved;
    }

    // ======================== EVENTS ========================

    event PublisherUpdated(address indexed publisher, bool authorized);
    event FalconRequirementUpdated(bool required);
    event QuorumThresholdUpdated(uint32 threshold);
    event OutputAttested(bytes32 indexed blockHash, address indexed publisher, uint32 count, uint32 threshold);
    event BinaOutputSubmitted(
        uint64 indexed height,
        bytes32 indexed blockHash,
        bytes32 indexed purpose,
        bytes32 randomnessOutput,
        bytes32 nullifier,
        bytes20 binaMiner,
        address publisher,
        bool falconVerified
    );
    event BinaOutputMetadata(
        bytes32 indexed blockHash,
        uint64 btcHeight,
        uint64 minedTimestamp,
        uint8 workBits,
        bytes32 claimDigest,
        bytes32 electionScore,
        bytes32 proofHash,
        string proofURI
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
    event BondDeposited(address indexed publisher, uint256 amount, uint256 totalBond);
    event BondWithdrawn(address indexed publisher, uint256 amount, uint256 totalBond);
    event FuturePublicationRequested(
        uint256 indexed commitmentId,
        bytes32 indexed purpose,
        uint64 targetHeight,
        uint64 deadline,
        address assignedPublisher
    );
    event FutureCommitmentResolved(
        uint256 indexed commitmentId,
        address indexed assignedPublisher,
        bool slashed,
        uint256 amount,
        address indexed resolver
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
    error FalconNotVerified();
    error StaleHeight();
    error AttestationMismatch();
    error AlreadyAttested();
    error NoActivePublishers();
    error InsufficientFreeBond();
    error InsufficientBond();
    error DeadlineInPast();
    error DeadlineNotReached();
    error TransferFailed();

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

    /// @notice Minimum bonded stake (native value) for a publisher to enter
    ///         the rotation eligible for future-height assignments.
    uint256 public constant MIN_PUBLISHER_BOND = 1 ether;
    /// @notice Bond amount locked (and slashable) per future-publication commitment.
    uint256 public constant COMMITMENT_SLASH_AMOUNT = 0.5 ether;

    /// @notice Post-quantum security level of the BLAKE3 PoW + Bitcoin-anchored randomness source.
    uint8 public constant PQ_SECURITY_BITS_SOURCE = 128;
    /// @notice Post-quantum security level of BINA L1 submission integrity via Falcon-512.
    uint8 public constant PQ_SECURITY_BITS_SUBMISSION = 128;
    /// @notice Post-quantum security level of EVM derivation helpers using keccak256 under Grover.
    uint8 public constant PQ_SECURITY_BITS_DERIVATION = 128;
    /// @notice This oracle accepts only publisher-attested Ed25519 + Falcon-512 BINA outputs.
    bool public constant IS_PQ_RESISTANT = true;

    // ======================== STORAGE ========================

    address public owner;
    mapping(address => bool) public publishers;
    bool public requireFalconVerification;

    /// @notice Number of independent publisher attestations required before
    ///         a BINA output finalizes and its purpose seed(s) update.
    ///         Defaults to 1 (single-publisher, same behavior as no quorum).
    uint32 public quorumThreshold = 1;
    mapping(bytes32 => PendingAttestation) public attestations;
    mapping(bytes32 => mapping(address => bool)) public hasAttested;

    mapping(bytes32 => StoredOutput) public outputsByBlockHash;
    mapping(bytes32 => string) public proofURIByBlockHash;
    mapping(bytes32 => bool) public usedNullifiers;
    mapping(bytes32 => bytes32) public latestSeedByPurpose;
    mapping(bytes32 => bytes32) public latestBlockByPurpose;
    mapping(bytes32 => uint64) public latestHeightByPurpose;
    mapping(bytes32 => uint64) public latestBTCHeightByPurpose;

    bytes32[] public blockHashes;
    mapping(uint256 => UtilityRequest) public requests;
    uint256 public nextRequestId = 1;

    mapping(address => uint256) public publisherBonds;
    mapping(address => uint256) public lockedBonds;
    address[] public activePublishers;
    mapping(address => uint256) private activePublisherIndex1; // 1-based; 0 = not active

    mapping(uint256 => FutureCommitment) public futureCommitments;
    uint256 public nextCommitmentId = 1;

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
        if (!authorized) {
            _removeActivePublisher(publisher);
        }
        emit PublisherUpdated(publisher, authorized);
    }

    function setRequireFalcon(bool required) external onlyOwner {
        requireFalconVerification = required;
        emit FalconRequirementUpdated(required);
    }

    /// @notice Raise or lower how many independent publishers must agree on
    ///         an output before it finalizes. Existing pending (not yet
    ///         finalized) attestations are unaffected until their next call.
    function setQuorumThreshold(uint32 threshold) external onlyOwner {
        if (threshold == 0) revert ZeroValue();
        quorumThreshold = threshold;
        emit QuorumThresholdUpdated(threshold);
    }

    // ======================== PUBLISHER BONDING ========================

    /// @notice Authorized publishers post bond to become eligible for
    ///         future-height assignment (see `requestFuturePublication`).
    function depositBond() external payable onlyPublisher {
        if (msg.value == 0) revert ZeroValue();
        publisherBonds[msg.sender] += msg.value;
        if (publisherBonds[msg.sender] >= MIN_PUBLISHER_BOND && activePublisherIndex1[msg.sender] == 0) {
            activePublishers.push(msg.sender);
            activePublisherIndex1[msg.sender] = activePublishers.length;
        }
        emit BondDeposited(msg.sender, msg.value, publisherBonds[msg.sender]);
    }

    /// @notice Withdraw any bond not currently locked against an
    ///         unresolved future-publication commitment.
    function withdrawBond(uint256 amount) external {
        uint256 free = publisherBonds[msg.sender] - lockedBonds[msg.sender];
        if (amount == 0 || amount > free) revert InsufficientBond();
        publisherBonds[msg.sender] -= amount;
        if (publisherBonds[msg.sender] < MIN_PUBLISHER_BOND) {
            _removeActivePublisher(msg.sender);
        }
        (bool ok, ) = msg.sender.call{value: amount}("");
        if (!ok) revert TransferFailed();
        emit BondWithdrawn(msg.sender, amount, publisherBonds[msg.sender]);
    }

    function activePublisherCount() external view returns (uint256) {
        return activePublishers.length;
    }

    function _removeActivePublisher(address publisher) internal {
        uint256 idx1 = activePublisherIndex1[publisher];
        if (idx1 == 0) return;
        uint256 lastIdx = activePublishers.length - 1;
        address lastPublisher = activePublishers[lastIdx];
        activePublishers[idx1 - 1] = lastPublisher;
        activePublisherIndex1[lastPublisher] = idx1;
        activePublishers.pop();
        delete activePublisherIndex1[publisher];
    }

    // ======================== FUTURE-HEIGHT LIVENESS ========================

    /// @notice Request that `purpose` reach `targetHeight` by `deadline`
    ///         (Unix seconds). Deterministically assigns one currently
    ///         bonded publisher and locks `COMMITMENT_SLASH_AMOUNT` of their
    ///         bond against it. Callers must pick `deadline` themselves
    ///         based on BINA's known block cadence — this contract has no
    ///         way to compute BINA height-to-time mapping on its own since
    ///         BINA's difficulty retargets independently of the EVM chain.
    function requestFuturePublication(
        bytes32 purpose,
        uint64 targetHeight,
        uint64 deadline
    ) external returns (uint256 commitmentId) {
        if (deadline <= block.timestamp) revert DeadlineInPast();
        uint256 n = activePublishers.length;
        if (n == 0) revert NoActivePublishers();

        bytes32 effectivePurpose = purpose == bytes32(0) ? PURPOSE_GENERIC : purpose;
        address assigned = activePublishers[targetHeight % n];
        uint256 free = publisherBonds[assigned] - lockedBonds[assigned];
        if (free < COMMITMENT_SLASH_AMOUNT) revert InsufficientFreeBond();

        lockedBonds[assigned] += COMMITMENT_SLASH_AMOUNT;
        commitmentId = nextCommitmentId++;
        futureCommitments[commitmentId] = FutureCommitment({
            purpose: effectivePurpose,
            targetHeight: targetHeight,
            deadline: deadline,
            assignedPublisher: assigned,
            resolved: false
        });
        emit FuturePublicationRequested(commitmentId, effectivePurpose, targetHeight, deadline, assigned);
    }

    /// @notice Permissionlessly close out a commitment: unlocks the
    ///         assigned publisher's bond if `purpose` reached `targetHeight`
    ///         (no penalty), or — once `deadline` has passed without that —
    ///         slashes the locked bond and pays it to whoever calls this.
    function resolveFutureCommitment(uint256 commitmentId) external {
        FutureCommitment storage c = futureCommitments[commitmentId];
        if (c.deadline == 0) revert RequestNotFound();
        if (c.resolved) revert RequestAlreadyFulfilled();

        bool published = latestHeightByPurpose[c.purpose] >= c.targetHeight;
        if (!published && block.timestamp < c.deadline) revert DeadlineNotReached();

        c.resolved = true;
        uint256 lockAmt = lockedBonds[c.assignedPublisher] < COMMITMENT_SLASH_AMOUNT
            ? lockedBonds[c.assignedPublisher]
            : COMMITMENT_SLASH_AMOUNT;
        lockedBonds[c.assignedPublisher] -= lockAmt;

        if (published) {
            emit FutureCommitmentResolved(commitmentId, c.assignedPublisher, false, 0, msg.sender);
            return;
        }

        publisherBonds[c.assignedPublisher] -= lockAmt;
        if (publisherBonds[c.assignedPublisher] < MIN_PUBLISHER_BOND) {
            _removeActivePublisher(c.assignedPublisher);
        }
        (bool ok, ) = msg.sender.call{value: lockAmt}("");
        if (!ok) revert TransferFailed();
        emit FutureCommitmentResolved(commitmentId, c.assignedPublisher, true, lockAmt, msg.sender);
    }

    // ======================== BINA OUTPUT INGESTION ========================

    /**
     * @notice Relay a finalized BINA L1 output into the EVM oracle.
     * @param output Finalized BINA output from the L1 node/P2P network.
     * @param purpose Utility namespace this output should update.
     * @param proofBundle Opaque BINA proof material for off-chain audit
     *        (for example public key, hybrid signature, serialized header, and
     *        BINA peer/election metadata). EVM storage keeps compact facts only.
     * @param proofURI Optional off-chain pointer (for example an IPFS CID or
     *        HTTPS URL) to the full proof bundle for independent verification.
     */
    function submitOutput(
        BinaOutput calldata output,
        bytes32 purpose,
        bytes calldata proofBundle,
        string calldata proofURI
    ) external onlyPublisher {
        bytes32[] memory purposes = new bytes32[](1);
        purposes[0] = purpose;
        _attestAndMaybeFinalize(output, purposes, proofBundle, proofURI);
    }

    function submitOutputForPurposes(
        BinaOutput calldata output,
        bytes32[] calldata purposes,
        bytes calldata proofBundle,
        string calldata proofURI
    ) external onlyPublisher {
        if (purposes.length == 0) revert ZeroValue();
        _attestAndMaybeFinalize(output, purposes, proofBundle, proofURI);
    }

    /// @dev Records msg.sender's attestation to (output, purposes). Reverts
    ///      if a different publisher already attested a conflicting
    ///      fingerprint for this blockHash, or if msg.sender already
    ///      attested. Once `quorumThreshold` matching attestations are in,
    ///      finalizes: stores the output, marks the nullifier used, and
    ///      advances each named purpose's seed (rejecting any purpose whose
    ///      recorded height would move backward — see `StaleHeight`).
    function _attestAndMaybeFinalize(
        BinaOutput calldata output,
        bytes32[] memory purposes,
        bytes calldata proofBundle,
        string calldata proofURI
    ) internal {
        _validateOutput(output);
        if (outputsByBlockHash[output.blockHash].randomnessOutput != bytes32(0)) {
            revert AlreadySubmitted();
        }
        if (usedNullifiers[output.nullifier]) revert NullifierAlreadyUsed();

        bytes32 blockHash = output.blockHash;
        bytes32 fp = keccak256(abi.encode(output, purposes));
        PendingAttestation storage att = attestations[blockHash];

        if (att.count == 0) {
            att.fingerprint = fp;
        } else if (att.fingerprint != fp) {
            revert AttestationMismatch();
        }
        if (hasAttested[blockHash][msg.sender]) revert AlreadyAttested();
        hasAttested[blockHash][msg.sender] = true;
        att.count += 1;

        emit OutputAttested(blockHash, msg.sender, att.count, quorumThreshold);

        if (att.count < quorumThreshold) {
            return;
        }

        for (uint256 i = 0; i < purposes.length; i++) {
            bytes32 purpose = purposes[i] == bytes32(0) ? PURPOSE_GENERIC : purposes[i];
            if (output.height < latestHeightByPurpose[purpose]) revert StaleHeight();
        }

        bytes32 proofHash = keccak256(proofBundle);
        outputsByBlockHash[blockHash] = StoredOutput({
            randomnessOutput: output.randomnessOutput,
            nullifier: output.nullifier,
            binaMiner: output.binaMiner,
            height: output.height,
            btcHeight: output.btcHeight,
            minedTimestamp: output.minedTimestamp,
            workBits: output.workBits,
            falconVerified: output.falconVerified,
            proofHash: proofHash,
            attestationCount: att.count,
            publisher: msg.sender
        });
        proofURIByBlockHash[blockHash] = proofURI;
        usedNullifiers[output.nullifier] = true;
        blockHashes.push(blockHash);

        for (uint256 i = 0; i < purposes.length; i++) {
            bytes32 purpose = purposes[i] == bytes32(0) ? PURPOSE_GENERIC : purposes[i];
            _updatePurposeSeed(purpose, output.randomnessOutput, output.height, output.btcHeight, blockHash);
            _emitOutputEvents(output, purpose, proofBundle, proofHash, proofURI);
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

    function getAttestationStatus(bytes32 blockHash)
        external
        view
        returns (uint32 count, uint32 threshold, bool finalized)
    {
        count = attestations[blockHash].count;
        threshold = quorumThreshold;
        finalized = outputsByBlockHash[blockHash].randomnessOutput != bytes32(0);
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

    function randomUintFor(
        bytes32 purpose,
        bytes32 salt,
        address consumer,
        uint256 upperBound
    ) external view returns (uint256) {
        if (upperBound == 0) revert InvalidUpperBound();
        return uint256(deriveWord(purpose, salt, consumer)) % upperBound;
    }

    function blockCount() external view returns (uint256) {
        return blockHashes.length;
    }

    function blockHashAt(uint256 index) external view returns (bytes32) {
        return blockHashes[index];
    }

    function isPQResistant() external pure returns (bool) {
        return IS_PQ_RESISTANT;
    }

    function pqSecurityBits() external pure returns (uint8) {
        return PQ_SECURITY_BITS_SOURCE;
    }

    function signingScheme() external pure returns (string memory) {
        return "Ed25519+Falcon512";
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
        if (requireFalconVerification && !output.falconVerified) revert FalconNotVerified();
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
        bytes calldata proofBundle,
        bytes32 proofHash,
        string calldata proofURI
    ) internal {
        emit BinaOutputSubmitted(
            output.height,
            output.blockHash,
            purpose,
            output.randomnessOutput,
            output.nullifier,
            output.binaMiner,
            msg.sender,
            output.falconVerified
        );
        emit BinaOutputMetadata(
            output.blockHash,
            output.btcHeight,
            output.minedTimestamp,
            output.workBits,
            output.claimDigest,
            output.electionScore,
            proofHash,
            proofURI
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
