// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title IStaleBlockOracle
 * @notice Interface for the Stale Block Oracle module — harvests entropy
 *         from orphaned Bitcoin blocks on the loser chain.
 */
interface IStaleBlockOracle {
    /// @notice A verified stale block proof stored on-chain.
    struct StaleProof {
        bytes32 blockHash;          // Double-SHA256 of the 80-byte header
        bytes32 canonicalHash;      // Canonical (winning) block hash at same height
        bytes32 entropyDigest;      // Domain-tagged entropy extracted from the header
        uint64  height;             // Bitcoin block height
        uint32  reorgDepth;         // How deep the reorg was (1 = single stale)
        uint32  leadingZeros;       // Leading zero bits in blockHash (PoW measure)
        uint32  qualityScore;       // Entropy quality 0-100
        uint64  submittedAt;        // Timestamp of on-chain submission
        address submitter;          // Who submitted this proof
        bool    rewarded;           // Whether the reward has been claimed
    }

    /// @notice Emitted when a new stale block proof is accepted.
    event StaleBlockSubmitted(
        bytes32 indexed blockHash,
        bytes32 indexed canonicalHash,
        address indexed submitter,
        uint64  height,
        uint32  reorgDepth,
        uint32  qualityScore,
        bytes32 entropyDigest
    );

    /// @notice Emitted when a submitter claims their reward.
    event StaleRewardClaimed(
        bytes32 indexed blockHash,
        address indexed submitter,
        uint256 reward
    );

    /// @notice Emitted when a chain fork event is recorded.
    event ForkEventRecorded(
        uint64  indexed forkHeight,
        bytes32 indexed winnerHash,
        uint32  reorgDepth,
        uint256 loserCount,
        bytes32 forkEntropy
    );

    /// @notice Emitted when configuration is updated.
    event ConfigUpdated(
        uint32  minLeadingZeros,
        uint32  maxReorgDepth,
        uint64  maxStaleAgeSecs,
        uint256 baseReward
    );

    // ── Errors ──────────────────────────────────────────────────
    error InvalidHeaderLength(uint256 actual, uint256 expected);
    error InsufficientPoW(uint32 actual, uint32 required);
    error BlockNotStale();
    error StaleBlockTooOld(uint64 ageSecs, uint64 maxSecs);
    error AlreadySubmitted(bytes32 blockHash);
    error ReorgTooDeep(uint32 depth, uint32 maxDepth);
    error HashMismatch(bytes32 computed, bytes32 claimed);
    error AlreadyRewarded(bytes32 blockHash);
    error NotProofSubmitter(bytes32 blockHash);
    error NoRewardAvailable();
    error InvalidLoserCount();

    // ── Write functions ─────────────────────────────────────────

    /// @notice Submit an 80-byte Bitcoin block header that is stale.
    /// @param rawHeader     The raw 80-byte serialised block header.
    /// @param height        The Bitcoin height this block was mined at.
    /// @param canonicalHash The block hash of the canonical (winning) block at `height`.
    /// @param reorgDepth    Depth of the reorganisation (1 = single block stale).
    function submitStaleBlock(
        bytes calldata rawHeader,
        uint64 height,
        bytes32 canonicalHash,
        uint32 reorgDepth
    ) external;

    /// @notice Claim the entropy reward for a previously submitted stale block.
    /// @param blockHash The stale block hash to claim for.
    function claimReward(bytes32 blockHash) external;

    /// @notice Record a chain fork event (batch of losers at same height).
    /// @param forkHeight  Height of the fork.
    /// @param winnerHash  Hash of the winning block.
    /// @param loserHashes Array of losing block hashes.
    /// @param reorgDepth  Depth of the reorganisation.
    function recordForkEvent(
        uint64 forkHeight,
        bytes32 winnerHash,
        bytes32[] calldata loserHashes,
        uint32 reorgDepth
    ) external;

    // ── View functions ──────────────────────────────────────────

    /// @notice Get a stored stale proof by its block hash.
    function getStaleProof(bytes32 blockHash) external view returns (StaleProof memory);

    /// @notice Check if a block hash has already been submitted.
    function isSubmitted(bytes32 blockHash) external view returns (bool);

    /// @notice Get the current cumulative entropy from all harvested stale blocks.
    function cumulativeEntropy() external view returns (bytes32);

    /// @notice Total stale blocks submitted.
    function totalStaleSubmissions() external view returns (uint256);

    /// @notice Total fork events recorded.
    function totalForkEvents() external view returns (uint256);

    /// @notice Get the pending reward for a stale block (0 if already claimed).
    function pendingReward(bytes32 blockHash) external view returns (uint256);
}
