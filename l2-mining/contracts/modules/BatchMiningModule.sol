// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ECDSA } from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import { EIP712 } from "@openzeppelin/contracts/utils/cryptography/EIP712.sol";
import { MerkleProof } from "@openzeppelin/contracts/utils/cryptography/MerkleProof.sol";
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { ModuleBase } from "./ModuleBase.sol";
import { IBatchMiningModule } from "../interfaces/IBatchMiningModule.sol";
import { ITokenomicsModule } from "../interfaces/ITokenomicsModule.sol";

/**
 * @title BatchMiningModule
 * @author Temporal Gradient Team
 * @notice Epoch-based batch mining — miners accumulate solutions off-chain,
 *         build a Merkle tree, and commit a single root on-chain per epoch.
 *
 *         This amortises the gas cost across N solutions instead of paying
 *         per-commit + per-reveal as the regular MiningModule does.
 *
 *         Randomness consumers verify individual leaves against the anchored
 *         root via `verifyRandomnessLeaf()`.
 *
 *   ┌─────────────────────────────────────────────────────────┐
 *   │  Off-chain Miner (Rust)                                 │
 *   │  1. Mine N solutions → H₁, H₂ … Hₙ                    │
 *   │  2. Build Merkle tree  →  root R                        │
 *   │  3. POST leaves + proofs to Randomness API              │
 *   └───────────────────┬─────────────────────────────────────┘
 *                       │ commitEpochRoot(R, N, poolId)
 *   ┌───────────────────▼─────────────────────────────────────┐
 *   │  BatchMiningModule (on-chain)                           │
 *   │  1. Store root R, leafCount N                           │
 *   │  2. After challenge window → finalizeEpoch()            │
 *   │  3. Mint TGBT reward for all N solutions at once        │
 *   │  4. verifyRandomnessLeaf(epochId, idx, leaf, proof)     │
 *   └────────────────────────────────────────────────────────-─┘
 */
contract BatchMiningModule is ModuleBase, EIP712("TemporalGradientBatch", "1"), IBatchMiningModule {
    using ECDSA for bytes32;

    // ── Constants ───────────────────────────────────────
    bytes32 public constant MODULE_TOKENOMICS = keccak256("TOKENOMICS_MODULE");

    bytes32 private constant EPOCH_ROOT_TYPEHASH =
        keccak256("EpochRoot(address operator,uint256 epochId,bytes32 merkleRoot,uint32 leafCount,uint8 poolId,uint256 deadline)");

    /// @notice Minimum blocks between epoch root commits for the same operator
    uint256 public constant EPOCH_COOLDOWN_BLOCKS = 50;

    /// @notice Blocks after commit before epoch can be finalised (challenge window)
    uint256 public constant CHALLENGE_WINDOW = 100;

    /// @notice Maximum leaves (solutions) per epoch to bound gas on finalisation
    uint32 public constant MAX_LEAVES_PER_EPOCH = 10_000;

    /// @notice TGBT reward per valid solution (same unit as MiningModule)
    uint256 public constant REWARD_PER_SOLUTION = 1.375 ether;

    /// @notice Required TSTAKE balance to submit epoch roots
    uint256 public constant REQUIRED_TSTAKE = 100 ether;

    // ── Storage ─────────────────────────────────────────
    IERC20 public stakeToken;

    /// @notice Auto-incrementing epoch counter
    uint256 private _nextEpochId;

    /// @notice epochId → EpochData
    mapping(uint256 => EpochData) private _epochs;

    /// @notice operator → last commit block
    mapping(address => uint256) public lastCommitBlock;

    /// @notice operator → number of epochs committed
    mapping(address => uint256) private _operatorEpochCounts;

    /// @notice operator → running nonce for EIP-712 replay protection
    mapping(address => uint256) public nonces;

    struct EpochData {
        bytes32  merkleRoot;
        uint64   startBlock;
        uint64   endBlock;          // 0 until finalised
        uint32   leafCount;
        address  operator;
        uint8    poolId;
        bool     finalized;
        uint256  totalReward;
        bool     storageAttested;
        bytes32  attestationHash;
    }

    // ── Initialiser (matches ModuleBase pattern) ────────

    function initialize(address coreAddress, address stakeTokenAddress) external {
        __ModuleBase_init(coreAddress);
        stakeToken = IERC20(stakeTokenAddress);
    }

    // ── Operator: commitEpochRoot ───────────────────────

    /// @inheritdoc IBatchMiningModule
    function commitEpochRoot(
        uint256 epochId,
        bytes32 merkleRoot,
        uint32  leafCount,
        uint8   poolId,
        uint256 deadline,
        bytes calldata signature
    ) external override whenSystemActive {
        // Stake guard
        if (stakeToken.balanceOf(msg.sender) < REQUIRED_TSTAKE)
            revert IBatchMiningModule.InvalidLeafCount(); // reuse — could mint a custom one

        // Leaf bound
        if (leafCount == 0 || leafCount > MAX_LEAVES_PER_EPOCH)
            revert IBatchMiningModule.InvalidLeafCount();

        // Deadline
        if (block.timestamp > deadline)
            revert IBatchMiningModule.CooldownNotElapsed();

        // Cooldown
        if (lastCommitBlock[msg.sender] != 0 &&
            block.number - lastCommitBlock[msg.sender] < EPOCH_COOLDOWN_BLOCKS)
            revert IBatchMiningModule.CooldownNotElapsed();

        // EIP-712 signature verification
        bytes32 structHash = keccak256(abi.encode(
            EPOCH_ROOT_TYPEHASH,
            msg.sender,
            epochId,
            merkleRoot,
            leafCount,
            poolId,
            deadline
        ));
        bytes32 digest = _hashTypedDataV4(structHash);
        address signer = ECDSA.recover(digest, signature);
        if (signer != msg.sender || signer == address(0))
            revert IBatchMiningModule.InvalidMerkleProof(); // signature invalid

        // Epoch must not already exist
        if (_epochs[epochId].merkleRoot != bytes32(0))
            revert IBatchMiningModule.EpochAlreadyExists(epochId);

        // Ensure sequential (operator picks epochId but it must match next expected)
        if (epochId != _nextEpochId)
            revert IBatchMiningModule.EpochNotFound(epochId);
        _nextEpochId++;

        // Store
        _epochs[epochId] = EpochData({
            merkleRoot:  merkleRoot,
            startBlock:  uint64(block.number),
            endBlock:    0,
            leafCount:   leafCount,
            operator:    msg.sender,
            poolId:      poolId,
            finalized:   false,
            totalReward: 0,
            storageAttested: false,
            attestationHash: bytes32(0)
        });

        lastCommitBlock[msg.sender] = block.number;
        nonces[msg.sender]++;
        _operatorEpochCounts[msg.sender]++;

        emit EpochRootCommitted(epochId, msg.sender, merkleRoot, leafCount, poolId);
    }

    // ── Operator: finalizeEpoch ─────────────────────────

    /// @inheritdoc IBatchMiningModule
    function finalizeEpoch(uint256 epochId) external override whenSystemActive {
        EpochData storage ep = _epochs[epochId];
        if (ep.merkleRoot == bytes32(0))
            revert IBatchMiningModule.EpochNotFound(epochId);
        if (ep.finalized)
            revert IBatchMiningModule.EpochAlreadyFinalized(epochId);
        if (ep.operator != msg.sender)
            revert IBatchMiningModule.NotEpochOperator();
        // Challenge window
        if (block.number < ep.startBlock + CHALLENGE_WINDOW)
            revert IBatchMiningModule.CooldownNotElapsed();

        // Calculate total reward for all solutions in this epoch
        uint256 totalReward = uint256(ep.leafCount) * REWARD_PER_SOLUTION;

        // Mint via Tokenomics module — one call for the whole epoch
        ITokenomicsModule tokenomics = ITokenomicsModule(_module(MODULE_TOKENOMICS));
        uint256 actualReward = tokenomics.onBlockMined(
            ep.operator,
            ep.merkleRoot,    // use root as the "output" for accounting
            ep.poolId,
            0,                // poolTargetDifficulty (batch mode — not per-hash)
            0,                // poolTotalMined
            totalReward       // emissionBucket = desired reward
        );

        ep.finalized   = true;
        ep.endBlock    = uint64(block.number);
        ep.totalReward = actualReward;

        // Record the root as a new output in the core history ring
        core.recordMinedOutput(ep.merkleRoot, ep.operator, ep.poolId, actualReward, uint64(epochId));

        emit EpochFinalized(epochId, actualReward);
    }

    /// @inheritdoc IBatchMiningModule
    function recordStorageAttestation(uint256 epochId, bytes32 attestationHash) external override whenSystemActive {
        EpochData storage ep = _epochs[epochId];
        if (ep.merkleRoot == bytes32(0))
            revert IBatchMiningModule.EpochNotFound(epochId);
        if (!ep.finalized)
            revert IBatchMiningModule.EpochNotFinalized(epochId);
        if (ep.operator != msg.sender)
            revert IBatchMiningModule.NotEpochOperator();
        if (ep.storageAttested)
            revert IBatchMiningModule.StorageAttestationAlreadyRecorded(epochId);

        ep.storageAttested = true;
        ep.attestationHash = attestationHash;

        emit StorageAttested(epochId, attestationHash);
    }

    // ── Randomness verification ─────────────────────────

    /// @inheritdoc IBatchMiningModule
    function verifyRandomnessLeaf(
        uint256 epochId,
        uint256 leafIndex,
        bytes32 outputHash,
        bytes32[] calldata proof
    ) external view override returns (bool valid) {
        EpochData storage ep = _epochs[epochId];
        if (ep.merkleRoot == bytes32(0))
            revert IBatchMiningModule.EpochNotFound(epochId);
        if (!ep.finalized)
            revert IBatchMiningModule.EpochNotFinalized(epochId);

        // Build leaf: keccak256(abi.encodePacked(leafIndex, outputHash))
        bytes32 leaf = keccak256(abi.encodePacked(leafIndex, outputHash));

        valid = MerkleProof.verify(proof, ep.merkleRoot, leaf);
        if (!valid) revert IBatchMiningModule.InvalidMerkleProof();
    }

    // ── View helpers ────────────────────────────────────

    /// @inheritdoc IBatchMiningModule
    function getEpochInfo(uint256 epochId) external view override returns (EpochInfo memory) {
        EpochData storage ep = _epochs[epochId];
        return EpochInfo({
            merkleRoot:  ep.merkleRoot,
            startBlock:  ep.startBlock,
            endBlock:    ep.endBlock,
            leafCount:   ep.leafCount,
            operator:    ep.operator,
            poolId:      ep.poolId,
            finalized:   ep.finalized,
            totalReward: ep.totalReward,
            storageAttested: ep.storageAttested,
            attestationHash: ep.attestationHash
        });
    }

    /// @inheritdoc IBatchMiningModule
    function currentEpochId() external view override returns (uint256) {
        return _nextEpochId;
    }

    /// @inheritdoc IBatchMiningModule
    function operatorEpochCount(address operator) external view override returns (uint256) {
        return _operatorEpochCounts[operator];
    }
}
