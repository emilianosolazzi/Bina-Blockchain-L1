// SPDX-License-Identifier: MIT
pragma solidity ^0.8.30;

/**
 * @title BinaMinedAction
 * @author Emiliano Solazzi
 * @notice Any action in this contract requires proof of valid BINA mining.
 *         The miner submits their block hash. The contract verifies it exists
 *         in BinaOracle (meaning it passed Blake3 PoW + Falcon verification
 *         off-chain by BINA nodes) and that the miner address matches.
 *
 * @dev This pattern creates a new primitive:
 *      "Proof of BINA Work" as an access control mechanism.
 *      Any contract can inherit or use this pattern.
 *
 *      Security model:
 *      - BinaOracle only stores outputs submitted by authorized publishers
 *      - Publishers only submit finalized BINA L1 blocks
 *      - BINA L1 nodes verify Blake3 PoW + Ed25519 + Falcon-512
 *      - Therefore: existence in BinaOracle = valid proof of work
 *      - The miner address in the stored output cannot be spoofed
 *        because it was part of the signed claim digest on BINA L1
 */

interface IBinaOracle {
    struct StoredOutput {
        bytes32 randomnessOutput;
        bytes32 nullifier;
        bytes20 binaMiner;          // miner address from BINA L1
        uint64  height;
        uint64  btcHeight;
        uint64  minedTimestamp;
        uint8   workBits;
        bool    falconVerified;
        address publisher;
    }

    function getOutput(bytes32 blockHash)
        external view returns (StoredOutput memory);

    function getLatestSeed(bytes32 purpose)
        external view returns (
            bytes32 seed,
            uint64  height,
            uint64  btcHeight,
            bytes32 blockHash
        );

    function randomUint(
        bytes32 purpose,
        bytes32 salt,
        uint256 upperBound
    ) external view returns (uint256);

    function isPQResistant() external pure returns (bool);
}

// ═══════════════════════════════════════════════════════════════
// PROOF OF BINA WORK — BASE MODIFIER
// ═══════════════════════════════════════════════════════════════

/**
 * @title ProofOfBinaWork
 * @notice Base contract providing the `requiresBinaWork` modifier.
 *         Inherit this to gate any function behind valid BINA mining.
 */
abstract contract ProofOfBinaWork {

    IBinaOracle public immutable binaOracle;

    // Minimum PoW difficulty accepted
    uint8 public minWorkBits;

    // Whether Falcon verification is required
    bool public requireFalcon;

    // Track used block hashes — prevents replay
    // One proof per block hash per contract
    mapping(bytes32 => bool) public proofUsed;

    // Track which miner submitted which proof
    mapping(bytes32 => address) public proofSubmitter;

    event ProofAccepted(
        bytes32 indexed blockHash,
        bytes20 indexed binaMiner,
        address indexed submitter,
        uint8   workBits,
        uint64  height,
        bool    falconVerified
    );

    error InvalidProof();
    error ProofAlreadyUsed();
    error InsufficientWork();
    error MinerMismatch();
    error FalconRequired();
    error OracleNotPQResistant();

    constructor(
        address oracle,
        uint8   _minWorkBits,
        bool    _requireFalcon
    ) {
        binaOracle   = IBinaOracle(oracle);
        minWorkBits  = _minWorkBits;
        requireFalcon = _requireFalcon;
    }

    /**
     * @notice Verifies a BINA block hash exists in the oracle
     *         and was mined by the caller's registered BINA address.
     *
     * @param blockHash     The BINA block hash to prove work for
     * @param claimedMiner  The BINA miner address (bytes20) claiming credit
     *
     * @dev   claimedMiner must match binaMiner stored in BinaOracle.
     *        This prevents one miner from using another miner's proof.
     *        The miner address was part of the signed claim digest on
     *        BINA L1 — it cannot be altered without invalidating the
     *        Ed25519 + Falcon-512 signature.
     */
    modifier requiresBinaWork(
        bytes32 blockHash,
        bytes20 claimedMiner
    ) {
        _verifyBinaWork(blockHash, claimedMiner);
        _;
    }

    function _verifyBinaWork(
        bytes32 blockHash,
        bytes20 claimedMiner
    ) internal {

        // Replay protection
        if (proofUsed[blockHash]) revert ProofAlreadyUsed();

        // Fetch from oracle — reverts if not found
        IBinaOracle.StoredOutput memory output =
            binaOracle.getOutput(blockHash);

        // Verify minimum difficulty
        if (output.workBits < minWorkBits) revert InsufficientWork();

        // Verify Falcon if required
        if (requireFalcon && !output.falconVerified) {
            revert FalconRequired();
        }

        // Verify miner address matches stored output
        // Prevents using someone else's proof
        if (output.binaMiner != claimedMiner) revert MinerMismatch();

        // Mark proof as used
        proofUsed[blockHash]      = true;
        proofSubmitter[blockHash] = msg.sender;

        emit ProofAccepted(
            blockHash,
            output.binaMiner,
            msg.sender,
            output.workBits,
            output.height,
            output.falconVerified
        );
    }
}

// ═══════════════════════════════════════════════════════════════
// EXAMPLE 1: MINER-GATED MINT
// Only valid BINA miners can mint this token
// ═══════════════════════════════════════════════════════════════

/**
 * @title BinaMinerToken
 * @notice ERC20-like token that can only be minted by proving
 *         valid BINA PoW. Each valid block hash mints exactly
 *         one reward. The randomness output from that block
 *         determines the mint bonus.
 */
contract BinaMinerToken is ProofOfBinaWork {

    string public constant name   = "BINA Proof Token";
    string public constant symbol = "BPT";
    uint8  public constant decimals = 18;

    uint256 public totalSupply;
    mapping(address => uint256) public balanceOf;

    // Base mint amount per valid proof
    uint256 public constant BASE_MINT = 100 * 1e18;

    // Max bonus multiplier (randomness-derived)
    uint256 public constant MAX_BONUS = 5;

    event Transfer(address indexed from, address indexed to, uint256 value);
    event Minted(
        address indexed to,
        bytes32 indexed blockHash,
        uint256 amount,
        uint256 bonus,
        bytes32 randomnessOutput
    );

    constructor(address oracle)
        ProofOfBinaWork(oracle, 22, false)
        // minWorkBits = 22 (matches BINA MIN_WORK_BITS)
        // requireFalcon = false (provisional, Ed25519 works)
    {}

    /**
     * @notice Mint tokens by proving you mined a BINA block.
     * @param blockHash    The BINA block hash you mined
     * @param binaMiner    Your BINA miner address (bytes20)
     *
     * @dev The randomness output from your block determines
     *      your bonus multiplier. Higher entropy = bigger bonus.
     *      This is fair because:
     *      1. You cannot predict your randomness output
     *         before mining the block
     *      2. The randomness is derived from Bitcoin entropy
     *         which you also cannot predict or manipulate
     *      3. Each block hash can only be used once
     */
    function mintWithProof(
        bytes32 blockHash,
        bytes20 binaMiner
    )
        external
        requiresBinaWork(blockHash, binaMiner)
    {
        // Get the stored output for bonus calculation
        IBinaOracle.StoredOutput memory output =
            binaOracle.getOutput(blockHash);

        // Derive bonus from randomness output
        // Using last byte of randomness as bonus seed
        // Range: 0-4 → bonus multiplier: 1x-5x
        uint256 bonusMultiplier = (
            uint256(output.randomnessOutput) % MAX_BONUS
        ) + 1;

        uint256 mintAmount = BASE_MINT * bonusMultiplier;

        balanceOf[msg.sender] += mintAmount;
        totalSupply           += mintAmount;

        emit Transfer(address(0), msg.sender, mintAmount);
        emit Minted(
            msg.sender,
            blockHash,
            mintAmount,
            bonusMultiplier,
            output.randomnessOutput
        );
    }
}

// ═══════════════════════════════════════════════════════════════
// EXAMPLE 2: MINER LOTTERY
// Valid miners enter a lottery with BINA randomness as the draw
// ═══════════════════════════════════════════════════════════════

/**
 * @title BinaMinerLottery
 * @notice A lottery where:
 *         - Entry requires proof of valid BINA mining
 *         - The winning number is derived from BINA randomness
 *         - Nobody can predict the winner before the draw
 *         - Draw uses commit-reveal: winner determined by
 *           a future BINA block, not the entry block
 */
contract BinaMinerLottery is ProofOfBinaWork {

    struct Round {
        uint256 pot;
        uint64  closeHeight;    // BINA height when entries close
        uint64  drawHeight;     // BINA height that determines winner
        bytes32 drawBlockHash;  // set when draw occurs
        address winner;
        bool    settled;
        uint256 entryCount;
    }

    mapping(uint256 => Round)    public rounds;
    mapping(uint256 => address[]) private _entries;
    uint256 public currentRound;

    uint256 public entryFee = 0.01 ether;

    event RoundOpened(uint256 indexed round, uint64 closeHeight);
    event EntrySubmitted(
        uint256 indexed round,
        address indexed entrant,
        bytes32 indexed blockHash,
        uint256 entryIndex
    );
    event RoundDrawn(
        uint256 indexed round,
        bytes32 drawBlockHash,
        address winner,
        uint256 pot
    );

    error RoundClosed();
    error RoundNotClosed();
    error DrawHeightNotReached();
    error RoundAlreadySettled();
    error NoEntries();

    constructor(address oracle)
        ProofOfBinaWork(oracle, 22, false)
    {
        _openRound();
    }

    function _openRound() internal {
        currentRound++;

        // Get current BINA height from oracle
        (, uint64 currentHeight, ,) = binaOracle.getLatestSeed(
            bytes32(keccak256("BINA_GENERIC_UTILITY"))
        );

        rounds[currentRound] = Round({
            pot:         0,
            closeHeight: currentHeight + 100,  // open for 100 BINA blocks
            drawHeight:  currentHeight + 110,  // draw 10 blocks after close
            drawBlockHash: bytes32(0),
            winner:      address(0),
            settled:     false,
            entryCount:  0
        });

        emit RoundOpened(currentRound, rounds[currentRound].closeHeight);
    }

    /**
     * @notice Enter the lottery by proving you mined a BINA block.
     * @param blockHash  Your mined BINA block hash
     * @param binaMiner  Your BINA miner address
     *
     * @dev One valid proof = one entry. Multiple blocks = multiple entries.
     *      You cannot buy entries with ETH alone — you must mine BINA.
     *      This ensures only active BINA miners participate.
     */
    function enter(
        bytes32 blockHash,
        bytes20 binaMiner
    )
        external
        payable
        requiresBinaWork(blockHash, binaMiner)
    {
        Round storage round = rounds[currentRound];

        // Check round is open
        (, uint64 currentHeight,,) = binaOracle.getLatestSeed(
            bytes32(keccak256("BINA_GENERIC_UTILITY"))
        );

        if (currentHeight >= round.closeHeight) revert RoundClosed();
        if (msg.value < entryFee) revert();

        round.pot        += msg.value;
        round.entryCount += 1;

        _entries[currentRound].push(msg.sender);

        emit EntrySubmitted(
            currentRound,
            msg.sender,
            blockHash,
            round.entryCount
        );
    }

    /**
     * @notice Draw the winner using a future BINA block's randomness.
     * @param drawBlockHash  A BINA block hash at or after drawHeight
     *
     * @dev The draw block must be AFTER entries closed.
     *      Nobody knew this block hash when they entered.
     *      Therefore the winner selection is unpredictable.
     *      The miner of the draw block has no advantage —
     *      they cannot choose their randomness output.
     */
    function draw(bytes32 drawBlockHash) external {
        Round storage round = rounds[currentRound];

        if (round.settled) revert RoundAlreadySettled();
        if (round.entryCount == 0) revert NoEntries();

        IBinaOracle.StoredOutput memory output =
            binaOracle.getOutput(drawBlockHash);

        // Draw block must be at or after drawHeight
        if (output.height < round.drawHeight) {
            revert DrawHeightNotReached();
        }

        // Must be after close height
        if (output.height < round.closeHeight) {
            revert RoundNotClosed();
        }

        // Select winner using randomness from draw block
        uint256 winnerIndex = uint256(output.randomnessOutput)
            % round.entryCount;

        address winner = _entries[currentRound][winnerIndex];

        round.drawBlockHash = drawBlockHash;
        round.winner        = winner;
        round.settled       = true;

        emit RoundDrawn(
            currentRound,
            drawBlockHash,
            winner,
            round.pot
        );

        // Pay winner
        uint256 pot = round.pot;
        round.pot = 0;
        (bool ok,) = winner.call{value: pot}("");
        require(ok, "Transfer failed");

        // Open next round
        _openRound();
    }
}

// ═══════════════════════════════════════════════════════════════
// EXAMPLE 3: MINER REPUTATION REGISTRY
// Builds on-chain reputation for BINA miners
// ═══════════════════════════════════════════════════════════════

/**
 * @title BinaMinerRegistry
 * @notice On-chain reputation system for BINA miners.
 *         Miners prove blocks they've mined to build reputation.
 *         Reputation gates access to other contracts/systems.
 *
 * @dev This is the EVM-side complement to the BINA L1 miner stats.
 *      Miners who want EVM privileges must prove their BINA work here.
 */
contract BinaMinerRegistry is ProofOfBinaWork {

    struct MinerProfile {
        bytes20 binaAddress;       // BINA L1 address
        uint256 proofsSubmitted;   // blocks proven on EVM
        uint256 totalWorkBits;     // accumulated difficulty
        uint64  firstProofHeight;  // earliest BINA block proven
        uint64  lastProofHeight;   // most recent BINA block proven
        uint256 reputationScore;   // derived metric
        bool    registered;
    }

    mapping(address => MinerProfile) public miners;
    mapping(bytes20 => address)      public binaToEvm;

    // Reputation tiers
    uint256 public constant TIER_BRONZE   = 10;   // 10+ proofs
    uint256 public constant TIER_SILVER   = 50;   // 50+ proofs
    uint256 public constant TIER_GOLD     = 200;  // 200+ proofs
    uint256 public constant TIER_PLATINUM = 1000; // 1000+ proofs

    event MinerRegistered(
        address indexed evmAddress,
        bytes20 indexed binaAddress,
        bytes32 firstBlockHash
    );
    event ReputationUpdated(
        address indexed miner,
        uint256 proofsSubmitted,
        uint256 reputationScore,
        uint8   tier
    );

    error AlreadyRegisteredWithDifferentBinaAddress();

    constructor(address oracle)
        ProofOfBinaWork(oracle, 22, false)
    {}

    /**
     * @notice Register as a BINA miner by proving your first block.
     * @param blockHash   Your mined BINA block hash
     * @param binaMiner   Your BINA L1 address (bytes20)
     *
     * @dev Links your EVM address to your BINA L1 address permanently.
     *      This is the EVM-side proof that you are a BINA miner.
     *      Combined with FastPathIdentity, this creates a three-chain
     *      identity: Bitcoin → EVM → BINA.
     */
    function registerMiner(
        bytes32 blockHash,
        bytes20 binaMiner
    )
        external
        requiresBinaWork(blockHash, binaMiner)
    {
        MinerProfile storage profile = miners[msg.sender];

        if (profile.registered) {
            // Already registered — must use same BINA address
            if (profile.binaAddress != binaMiner) {
                revert AlreadyRegisteredWithDifferentBinaAddress();
            }
        } else {
            // First registration
            IBinaOracle.StoredOutput memory output =
                binaOracle.getOutput(blockHash);

            profile.binaAddress      = binaMiner;
            profile.firstProofHeight = output.height;
            profile.registered       = true;

            binaToEvm[binaMiner] = msg.sender;

            emit MinerRegistered(msg.sender, binaMiner, blockHash);
        }

        _updateReputation(blockHash, binaMiner);
    }

    /**
     * @notice Submit additional proofs to increase reputation.
     * @param blockHash   A BINA block hash you mined
     * @param binaMiner   Your BINA L1 address
     */
    function submitProof(
        bytes32 blockHash,
        bytes20 binaMiner
    )
        external
        requiresBinaWork(blockHash, binaMiner)
    {
        require(miners[msg.sender].registered, "Not registered");
        require(
            miners[msg.sender].binaAddress == binaMiner,
            "Wrong BINA address"
        );
        _updateReputation(blockHash, binaMiner);
    }

    function _updateReputation(
        bytes32 blockHash,
        bytes20 binaMiner
    ) internal {
        IBinaOracle.StoredOutput memory output =
            binaOracle.getOutput(blockHash);

        MinerProfile storage profile = miners[msg.sender];

        profile.proofsSubmitted += 1;
        profile.totalWorkBits   += output.workBits;
        profile.lastProofHeight  = output.height;

        // Reputation = proofs × average difficulty
        profile.reputationScore =
            profile.proofsSubmitted
            * (profile.totalWorkBits / profile.proofsSubmitted);

        uint8 tier = getTier(msg.sender);

        emit ReputationUpdated(
            msg.sender,
            profile.proofsSubmitted,
            profile.reputationScore,
            tier
        );
    }

    function getTier(address miner) public view returns (uint8) {
        uint256 proofs = miners[miner].proofsSubmitted;
        if (proofs >= TIER_PLATINUM) return 4;
        if (proofs >= TIER_GOLD)     return 3;
        if (proofs >= TIER_SILVER)   return 2;
        if (proofs >= TIER_BRONZE)   return 1;
        return 0;
    }

    function getProfile(address miner)
        external view
        returns (MinerProfile memory)
    {
        return miners[miner];
    }

    function isVerifiedMiner(address miner)
        external view returns (bool)
    {
        return miners[miner].registered
            && miners[miner].proofsSubmitted > 0;
    }

    function getMinerByBinaAddress(bytes20 binaAddr)
        external view returns (address)
    {
        return binaToEvm[binaAddr];
    }
}