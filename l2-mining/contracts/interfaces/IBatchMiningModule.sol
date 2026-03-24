// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title IBatchMiningModule
 * @notice Interface for epoch-based batch mining — an operator mines off-chain,
 *         accumulates solutions into Merkle trees, and periodically anchors
 *         an epoch root on-chain.  Gas cost is amortised across all solutions
 *         in the epoch instead of paying per-solution.
 *
 *         Randomness consumers can verify any individual leaf against the
 *         on-chain Merkle root without the miner's involvement.
 */
interface IBatchMiningModule {
    // ── Structs ─────────────────────────────────────────

    struct EpochInfo {
        bytes32   merkleRoot;          // root of all outputs in this epoch
        uint64    startBlock;          // block at which the epoch started
        uint64    endBlock;            // block at which the epoch was finalised
        uint32    leafCount;           // number of mined outputs in the tree
        address   operator;            // miner who submitted the root
        uint8     poolId;              // mining pool used
        bool      finalized;           // whether rewards have been settled
        uint256   totalReward;         // total TGBT awarded for the epoch
        bool      storageAttested;     // whether an archive attestation was anchored on-chain
        bytes32   attestationHash;     // keccak256 hash of the attestation payload
    }

    // ── Events ──────────────────────────────────────────

    event EpochRootCommitted(
        uint256 indexed epochId,
        address indexed operator,
        bytes32 merkleRoot,
        uint32  leafCount,
        uint8   poolId
    );

    event EpochFinalized(
        uint256 indexed epochId,
        uint256 totalReward
    );

    event StorageAttested(
        uint256 indexed epochId,
        bytes32 attestationHash
    );

    /// @dev Emitted by wrapper functions that call verifyRandomnessLeaf
    ///      and then act on the result (not from the view itself).
    event RandomnessLeafVerified(
        uint256 indexed epochId,
        uint256 leafIndex,
        bytes32 outputHash
    );

    // ── Errors ──────────────────────────────────────────

    error EpochAlreadyExists(uint256 epochId);
    error EpochNotFound(uint256 epochId);
    error EpochAlreadyFinalized(uint256 epochId);
    error EpochNotFinalized(uint256 epochId);
    error StorageAttestationAlreadyRecorded(uint256 epochId);
    error InvalidMerkleProof();
    error InvalidLeafCount();
    error NotEpochOperator();
    error CooldownNotElapsed();
    error LeafCountMismatch();

    // ── Operator actions ────────────────────────────────

    /// @notice Commit a Merkle root summarising all mined outputs in one epoch.
    ///         Replaces individual submitMiningCommitment calls.
    /// @param epochId  Sequential epoch counter chosen by the operator.
    /// @param merkleRoot  Root of the Merkle tree whose leaves are the mined
    ///                    output hashes produced off-chain.
    /// @param leafCount   Number of leaves (solutions) in the tree.
    /// @param poolId      Mining pool the solutions were computed against.
    /// @param deadline    EIP-712 deadline timestamp.
    /// @param signature   EIP-712 signature authorising this root submission.
    function commitEpochRoot(
        uint256 epochId,
        bytes32 merkleRoot,
        uint32  leafCount,
        uint8   poolId,
        uint256 deadline,
        bytes calldata signature
    ) external;

    /// @notice Finalise an epoch after the challenge window has passed.
    ///         Mints the accumulated TGBT reward to the operator.
    /// @param epochId  The epoch to finalise.
    function finalizeEpoch(uint256 epochId) external;

    /// @notice Record the storage attestation hash for an already-finalised epoch.
    /// @param epochId The finalised epoch to annotate.
    /// @param attestationHash Keccak256 hash of the attestation payload.
    function recordStorageAttestation(uint256 epochId, bytes32 attestationHash) external;

    // ── Randomness verification (anyone can call) ───────

    /// @notice Verify that a particular output hash belongs to a committed epoch.
    ///         Works immediately after commitEpochRoot — does NOT require finalization.
    ///         The Merkle root is immutable from commit time; the challenge window
    ///         is a dispute mechanism, not a proof-validity gate.
    /// @param epochId     The epoch containing the leaf.
    /// @param leafIndex   Position of the leaf in the Merkle tree.
    /// @param outputHash  The mined output hash (leaf value).
    /// @param proof       Merkle proof siblings.
    /// @return valid  True when the proof is correct against the committed root.
    function verifyRandomnessLeaf(
        uint256 epochId,
        uint256 leafIndex,
        bytes32 outputHash,
        bytes32[] calldata proof
    ) external view returns (bool valid);

    // ── View helpers ────────────────────────────────────

    function getEpochInfo(uint256 epochId) external view returns (EpochInfo memory);
    function currentEpochId() external view returns (uint256);
    function operatorEpochCount(address operator) external view returns (uint256);
}
