// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ERC20 } from "@openzeppelin/contracts/token/ERC20/ERC20.sol";

/**
 * @title  TGBT — Temporal Gradient Beacon Token
 * @notice Immutable-cap ERC20 with module-based minting authorization
 *         and permanent permission lock (Bitcoin-like ossification).
 *
 *  Authorization model:
 *    - `governance` (immutable) can grant / revoke module authorizations
 *    - Any authorized module (TokenomicsModule, future StaleBlockOracle, …)
 *      can call mint() and recordStamp()
 *    - lockPermissions() freezes all authorizations permanently —
 *      after that, governance has zero power. Irreversible.
 *
 *  No pause.  No admin mint.  No upgrade proxy.  Hard cap enforced.
 *  Bitcoin philosophy: ossify once the module set is stable.
 */
contract TGBT is ERC20 {

    // ── Constants ────────────────────────────────────────────
    uint256 public constant MAX_SUPPLY = 2_000_000_000 ether;

    // ── Authorization ────────────────────────────────────────
    address public immutable governance;
    bool    public permissionsLocked;
    mapping(address => bool) public isAuthorized;
    uint256 public authorizedCount;

    // ── Errors ───────────────────────────────────────────────
    error NotGovernance();
    error NotAuthorized();
    error PermissionsAreLocked();
    error ZeroAddress();
    error CapExceeded();
    error EpochAlreadyStamped();
    error EpochNotStamped();
    error ZeroMerkleRoot();
    error ZeroBitcoinTxHash();
    error NoAuthorizedModules();

    // ── Authorization Events ─────────────────────────────────
    event AuthorizationGranted(address indexed module);
    event AuthorizationRevoked(address indexed module);
    event PermissionsLockedForever(uint256 authorizedModules);

    // ── Temporal Randomness Stamp ────────────────────────────
    //
    //  Storage-packed struct (5 slots vs 5 in old layout, but carries
    //  MORE data: proof digest + real miner identity).
    //
    //  slot 0: bytes32 merkleRoot
    //  slot 1: bytes32 bitcoinTxHash
    //  slot 2: bytes32 proofDigest        ← NEW: keccak256(btcInclusionProof), 0x0 if none
    //  slot 3: address miner (20) + uint64 epochId (8) + uint32 bitcoinBlock (4) = 32 ✓
    //  slot 4: uint64 timestamp (8) + uint32 bitcoinVout (4) = 12
    //
    struct Stamp {
        bytes32 merkleRoot;
        bytes32 bitcoinTxHash;
        bytes32 proofDigest;
        address miner;
        uint64  epochId;
        uint32  bitcoinBlock;
        uint64  timestamp;
        uint32  bitcoinVout;
    }

    uint256 public stampCount;
    mapping(uint256 => Stamp)   public stamps;
    mapping(uint64  => uint256) public epochStamp;
    mapping(uint64  => bool)    public epochStamped;

    event StampRecorded(
        uint256 indexed stampId,
        uint64  indexed epochId,
        address indexed miner,
        bytes32 merkleRoot,
        bytes32 bitcoinTxHash,
        uint32  bitcoinVout,
        uint32  bitcoinBlock,
        bool    hasInclusionProof
    );

    // ── Constructor ──────────────────────────────────────────
    constructor(address _governance) ERC20("Temporal Gradient Beacon Token", "TGBT") {
        if (_governance == address(0)) revert ZeroAddress();
        governance = _governance;
    }

    // ── Modifiers ────────────────────────────────────────────
    modifier onlyGovernance() {
        if (msg.sender != governance) revert NotGovernance();
        _;
    }

    modifier onlyAuthorized() {
        if (!isAuthorized[msg.sender]) revert NotAuthorized();
        _;
    }

    modifier whenNotLocked() {
        if (permissionsLocked) revert PermissionsAreLocked();
        _;
    }

    // ── Authorization Management ─────────────────────────────
    //  Bootstrap phase: governance adds modules (TokenomicsModule,
    //  BatchMiningModule, future StaleBlockOracle, etc.)
    //  Once stable → lockPermissions() → permanent. No going back.

    function grantAuthorization(address module) external onlyGovernance whenNotLocked {
        if (module == address(0)) revert ZeroAddress();
        if (!isAuthorized[module]) {
            isAuthorized[module] = true;
            authorizedCount++;
            emit AuthorizationGranted(module);
        }
    }

    function revokeAuthorization(address module) external onlyGovernance whenNotLocked {
        if (isAuthorized[module]) {
            isAuthorized[module] = false;
            authorizedCount--;
            emit AuthorizationRevoked(module);
        }
    }

    /**
     * @notice Permanently freeze all authorization changes.
     *         After this call governance has zero power. Irreversible.
     *         Bitcoin-like ossification — the module set becomes consensus.
     */
    function lockPermissions() external onlyGovernance whenNotLocked {
        if (authorizedCount == 0) revert NoAuthorizedModules();
        permissionsLocked = true;
        emit PermissionsLockedForever(authorizedCount);
    }

    // ── Minting ──────────────────────────────────────────────
    //  Any authorized module can mint (TokenomicsModule enforces
    //  epoch halving, emission caps, difficulty bonuses).

    function mint(address to, uint256 amount) external onlyAuthorized {
        if (to == address(0)) revert ZeroAddress();
        if (totalSupply() + amount > MAX_SUPPLY) revert CapExceeded();
        _mint(to, amount);
    }

    function availableToMint() external view returns (uint256) {
        return MAX_SUPPLY - totalSupply();
    }

    // ── Temporal Randomness Stamps ───────────────────────────
    //  Records epoch-anchored Bitcoin proofs on-chain.
    //  `miner` is the actual entropy contributor (not msg.sender).
    //  `btcInclusionProof` is optional — stored as keccak256 digest
    //  for future on-chain SPV verification.

    function recordStamp(
        uint64  epochId,
        address miner,
        bytes32 merkleRoot,
        bytes32 bitcoinTxHash,
        uint32  bitcoinVout,
        uint32  bitcoinBlock,
        bytes calldata btcInclusionProof
    ) external onlyAuthorized returns (uint256 stampId) {
        if (merkleRoot    == bytes32(0)) revert ZeroMerkleRoot();
        if (bitcoinTxHash == bytes32(0)) revert ZeroBitcoinTxHash();
        if (epochStamped[epochId])       revert EpochAlreadyStamped();

        stampId = ++stampCount;

        bytes32 proofDigest = btcInclusionProof.length > 0
            ? keccak256(btcInclusionProof)
            : bytes32(0);

        stamps[stampId] = Stamp({
            merkleRoot:    merkleRoot,
            bitcoinTxHash: bitcoinTxHash,
            proofDigest:   proofDigest,
            miner:         miner,
            epochId:       epochId,
            bitcoinBlock:  bitcoinBlock,
            timestamp:     uint64(block.timestamp),
            bitcoinVout:   bitcoinVout
        });

        epochStamp[epochId]   = stampId;
        epochStamped[epochId] = true;

        emit StampRecorded(
            stampId, epochId, miner,
            merkleRoot, bitcoinTxHash,
            bitcoinVout, bitcoinBlock,
            proofDigest != bytes32(0)
        );
    }

    function getEpochStamp(uint64 epochId) external view returns (Stamp memory) {
        if (!epochStamped[epochId]) revert EpochNotStamped();
        return stamps[epochStamp[epochId]];
    }
}
