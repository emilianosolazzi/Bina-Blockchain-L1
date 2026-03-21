// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ModuleBase } from "./modules/ModuleBase.sol";
import { IStaleBlockOracle } from "./interfaces/IStaleBlockOracle.sol";
import { ITokenomicsModule } from "./interfaces/ITokenomicsModule.sol";
import { CoreUtilsLib } from "./CoreUtilsLib.sol";

/**
 * @title  StaleBlockOracle
 * @notice Harvests entropy from Bitcoin's orphaned "loser-chain" blocks.
 *
 * @dev    When two Bitcoin miners find a block at the same height almost
 *         simultaneously, one block wins and the other becomes *stale*
 *         (sometimes called an orphan). The stale block:
 *
 *           • Contains valid Proof of Work — real computation happened.
 *           • Has a unique block hash that nobody else will ever reproduce.
 *           • Is unpredictable — the outcome of which chain wins is a
 *             propagation race that depends on network topology.
 *           • Is rare (~1-2/day on mainnet), making it high-quality entropy.
 *
 *         This contract:
 *           1. Accepts raw 80-byte Bitcoin block headers claimed to be stale.
 *           2. Verifies PoW (leading zero bits in double-SHA256).
 *           3. Extracts domain-tagged entropy from the header.
 *           4. Records fork events as a secondary entropy signal.
 *           5. Rewards submitters for contributing stale-block entropy.
 *           6. Feeds cumulative loser-chain entropy into the TGBT beacon.
 *
 *         On-chain PoW verification uses pure Solidity double-SHA256 on the
 *         80-byte header, counting leading zero bits of the result.  This is
 *         sufficient for PoW validation — no SPV relay or oracle needed.
 */
contract StaleBlockOracle is ModuleBase, IStaleBlockOracle {
    bytes32 public constant MODULE_TOKENOMICS = keccak256("TOKENOMICS_MODULE");

    // ── Constants ────────────────────────────────────────────────
    uint256 private constant HEADER_SIZE = 80;
    uint256 private constant MAX_LOSERS_PER_EVENT = 8;

    /// @dev Domain tags for entropy extraction — ensures different contexts
    ///      never produce the same digest even with identical header bytes.
    bytes32 private constant ENTROPY_DOMAIN_TAG =
        keccak256("TGBT-STALE-ENTROPY-v1");
    bytes32 private constant FORK_DOMAIN_TAG =
        keccak256("TGBT-FORK-DIVERGENCE-v1");

    // ── Configuration ────────────────────────────────────────────
    uint32  public minLeadingZeros;     // Min PoW difficulty for acceptance
    uint32  public maxReorgDepth;       // Deepest reorg we trust
    uint64  public maxStaleAgeSecs;     // Oldest stale block we accept
    uint256 public baseReward;          // Base TGBT reward per accepted stale block

    // ── State ────────────────────────────────────────────────────

    /// @dev blockHash → StaleProof
    mapping(bytes32 => StaleProof) private _proofs;

    /// @dev Running XOR of all harvested stale-block entropy digests.
    bytes32 private _cumulativeEntropy;

    /// @dev Total number of accepted stale block submissions.
    uint256 private _totalSubmissions;

    /// @dev Total number of recorded fork events.
    uint256 private _totalForkEvents;

    /// @dev height → list of stale block hashes at that height.
    mapping(uint64 => bytes32[]) private _stalesByHeight;

    /// @dev Fork event counter per height (prevent replay of identical forks).
    mapping(uint64 => uint256) private _forkEventCount;

    // ── Initialiser ──────────────────────────────────────────────

    /**
     * @notice One-shot initialiser (ModuleBase pattern).
     * @param coreAddress           Address of TemporalGradientCore.
     * @param _minLeadingZeros      Min leading zeros in stale block hash (32 for mainnet).
     * @param _maxReorgDepth        Maximum reorg depth we consider valid (100).
     * @param _maxStaleAgeSecs      Maximum age of a stale block in seconds (604800 = 1 week).
     * @param _baseReward           Base TGBT reward for a valid stale-block submission.
     */
    function initialize(
        address coreAddress,
        uint32  _minLeadingZeros,
        uint32  _maxReorgDepth,
        uint64  _maxStaleAgeSecs,
        uint256 _baseReward
    ) external {
        __ModuleBase_init(coreAddress);
        minLeadingZeros  = _minLeadingZeros;
        maxReorgDepth    = _maxReorgDepth;
        maxStaleAgeSecs  = _maxStaleAgeSecs;
        baseReward       = _baseReward;
    }

    // ── Configuration Updates (Governance) ───────────────────────

    function updateConfig(
        uint32  _minLeadingZeros,
        uint32  _maxReorgDepth,
        uint64  _maxStaleAgeSecs,
        uint256 _baseReward
    ) external onlyGovernance {
        minLeadingZeros  = _minLeadingZeros;
        maxReorgDepth    = _maxReorgDepth;
        maxStaleAgeSecs  = _maxStaleAgeSecs;
        baseReward       = _baseReward;

        emit ConfigUpdated(_minLeadingZeros, _maxReorgDepth, _maxStaleAgeSecs, _baseReward);
    }

    // ══════════════════════════════════════════════════════════════
    //  CORE: Submit a stale block header
    // ══════════════════════════════════════════════════════════════

    /// @inheritdoc IStaleBlockOracle
    function submitStaleBlock(
        bytes calldata rawHeader,
        uint64 height,
        bytes32 canonicalHash,
        uint32 reorgDepth
    ) external override whenSystemActive {
        // ── 1. Basic validation ──────────────────────────────────
        if (rawHeader.length != HEADER_SIZE)
            revert InvalidHeaderLength(rawHeader.length, HEADER_SIZE);

        if (reorgDepth == 0 || reorgDepth > maxReorgDepth)
            revert ReorgTooDeep(reorgDepth, maxReorgDepth);

        // ── 2. Compute double-SHA256 of the raw header ───────────
        bytes32 blockHash = _doubleSha256(rawHeader);

        // ── 3. Reject if it matches canonical (not stale) ────────
        if (blockHash == canonicalHash)
            revert BlockNotStale();

        // ── 4. Reject duplicate submissions ──────────────────────
        if (_proofs[blockHash].submittedAt != 0)
            revert AlreadySubmitted(blockHash);

        // ── 5. Verify PoW — count leading zero bits ──────────────
        uint32 lz = _countLeadingZeroBits(blockHash);
        if (lz < minLeadingZeros)
            revert InsufficientPoW(lz, minLeadingZeros);

        // ── 6. Verify freshness — extract timestamp from header ──
        uint32 blockTimestamp = _extractTimestamp(rawHeader);
        uint64 age = uint64(block.timestamp) > uint64(blockTimestamp)
            ? uint64(block.timestamp) - uint64(blockTimestamp)
            : 0;
        if (age > maxStaleAgeSecs)
            revert StaleBlockTooOld(age, maxStaleAgeSecs);

        // ── 7. Extract entropy (domain-tagged hash of full header)
        bytes32 entropyDigest = _extractEntropy(rawHeader, blockHash, height);

        // ── 8. Compute quality score ─────────────────────────────
        uint32 qualityScore = _computeQualityScore(lz, reorgDepth, blockTimestamp);

        // ── 9. Store proof ───────────────────────────────────────
        _proofs[blockHash] = StaleProof({
            blockHash:      blockHash,
            canonicalHash:  canonicalHash,
            entropyDigest:  entropyDigest,
            height:         height,
            reorgDepth:     reorgDepth,
            leadingZeros:   lz,
            qualityScore:   qualityScore,
            submittedAt:    uint64(block.timestamp),
            submitter:      msg.sender,
            rewarded:       false
        });

        // ── 10. Update cumulative entropy (XOR accumulate) ───────
        _cumulativeEntropy ^= entropyDigest;

        // ── 11. Track by height ──────────────────────────────────
        _stalesByHeight[height].push(blockHash);
        _totalSubmissions++;

        emit StaleBlockSubmitted(
            blockHash,
            canonicalHash,
            msg.sender,
            height,
            reorgDepth,
            qualityScore,
            entropyDigest
        );
    }

    // ══════════════════════════════════════════════════════════════
    //  REWARDS
    // ══════════════════════════════════════════════════════════════

    /// @inheritdoc IStaleBlockOracle
    function claimReward(bytes32 blockHash) external override {
        StaleProof storage proof = _proofs[blockHash];
        if (proof.submittedAt == 0)
            revert NoRewardAvailable();
        if (proof.submitter != msg.sender)
            revert NotProofSubmitter(blockHash);
        if (proof.rewarded)
            revert AlreadyRewarded(blockHash);

        proof.rewarded = true;
        uint256 requestedReward = _calculateReward(proof.qualityScore, proof.reorgDepth);
        uint256 reward = _tokenomics().onStaleBlockReward(msg.sender, requestedReward);

        emit StaleRewardClaimed(blockHash, msg.sender, reward);
    }

    /// @inheritdoc IStaleBlockOracle
    function pendingReward(bytes32 blockHash) external view override returns (uint256) {
        StaleProof storage proof = _proofs[blockHash];
        if (proof.submittedAt == 0 || proof.rewarded) return 0;
        return _calculateReward(proof.qualityScore, proof.reorgDepth);
    }

    // ══════════════════════════════════════════════════════════════
    //  FORK EVENTS — batch recording of multi-loser forks
    // ══════════════════════════════════════════════════════════════

    /// @inheritdoc IStaleBlockOracle
    function recordForkEvent(
        uint64 forkHeight,
        bytes32 winnerHash,
        bytes32[] calldata loserHashes,
        uint32 reorgDepth
    ) external override whenSystemActive {
        require(loserHashes.length > 0 && loserHashes.length <= MAX_LOSERS_PER_EVENT, "invalid loser count");

        if (reorgDepth == 0 || reorgDepth > maxReorgDepth)
            revert ReorgTooDeep(reorgDepth, maxReorgDepth);

        // Combine entropy from all losers
        bytes32 forkEntropy = keccak256(
            abi.encodePacked(FORK_DOMAIN_TAG, winnerHash, forkHeight)
        );
        for (uint256 i = 0; i < loserHashes.length; i++) {
            forkEntropy ^= loserHashes[i];
        }
        forkEntropy = keccak256(abi.encodePacked(forkEntropy));

        // Accumulate into cumulative entropy
        _cumulativeEntropy ^= forkEntropy;

        _forkEventCount[forkHeight]++;
        _totalForkEvents++;

        emit ForkEventRecorded(
            forkHeight,
            winnerHash,
            reorgDepth,
            loserHashes.length,
            forkEntropy
        );
    }

    // ══════════════════════════════════════════════════════════════
    //  VIEW FUNCTIONS
    // ══════════════════════════════════════════════════════════════

    /// @inheritdoc IStaleBlockOracle
    function getStaleProof(bytes32 blockHash) external view override returns (StaleProof memory) {
        return _proofs[blockHash];
    }

    /// @inheritdoc IStaleBlockOracle
    function isSubmitted(bytes32 blockHash) external view override returns (bool) {
        return _proofs[blockHash].submittedAt != 0;
    }

    /// @inheritdoc IStaleBlockOracle
    function cumulativeEntropy() external view override returns (bytes32) {
        return _cumulativeEntropy;
    }

    /// @inheritdoc IStaleBlockOracle
    function totalStaleSubmissions() external view override returns (uint256) {
        return _totalSubmissions;
    }

    /// @inheritdoc IStaleBlockOracle
    function totalForkEvents() external view override returns (uint256) {
        return _totalForkEvents;
    }

    /// @notice Get all stale block hashes submitted at a given Bitcoin height.
    function staleBlocksAtHeight(uint64 height) external view returns (bytes32[] memory) {
        return _stalesByHeight[height];
    }

    /// @notice Get the number of fork events recorded at a given height.
    function forkEventsAtHeight(uint64 height) external view returns (uint256) {
        return _forkEventCount[height];
    }

    // ══════════════════════════════════════════════════════════════
    //  INTERNAL: Bitcoin-compatible cryptographic primitives
    // ══════════════════════════════════════════════════════════════

    /**
     * @dev Bitcoin double-SHA256: SHA256(SHA256(data)).
     *      This is the standard block hash computation used since genesis.
     */
    function _doubleSha256(bytes calldata data) internal pure returns (bytes32) {
        return sha256(abi.encodePacked(sha256(data)));
    }

    /**
     * @dev Count leading zero bits of a bytes32 value.
     *      Bitcoin convention: the "leading zeros" of a block hash refer to
     *      the number of zero bits from the most significant bit downward.
     *      In Solidity, bytes32 is big-endian, so we scan from byte 0.
     */
    function _countLeadingZeroBits(bytes32 hash) internal pure returns (uint32) {
        uint32 count = 0;
        for (uint256 i = 0; i < 32; i++) {
            uint8 b = uint8(hash[i]);
            if (b == 0) {
                count += 8;
            } else {
                // Count leading zeros in this byte
                if (b < 0x80) { count++; b <<= 1; } else { return count; }
                if (b < 0x80) { count++; b <<= 1; } else { return count; }
                if (b < 0x80) { count++; b <<= 1; } else { return count; }
                if (b < 0x80) { count++; b <<= 1; } else { return count; }
                if (b < 0x80) { count++; b <<= 1; } else { return count; }
                if (b < 0x80) { count++; b <<= 1; } else { return count; }
                if (b < 0x80) { count++; } // 7th zero → b was 0x01
                return count;
            }
        }
        return count; // All zeros → 256
    }

    /**
     * @dev Extract the 4-byte timestamp from a raw Bitcoin block header.
     *      Timestamp is at bytes [68..72), little-endian uint32.
     */
    function _extractTimestamp(bytes calldata rawHeader) internal pure returns (uint32) {
        return uint32(uint8(rawHeader[68]))
             | (uint32(uint8(rawHeader[69])) << 8)
             | (uint32(uint8(rawHeader[70])) << 16)
             | (uint32(uint8(rawHeader[71])) << 24);
    }

    /**
     * @dev Extract domain-tagged entropy from a stale block header.
     *
     *      Mixes every divergent field through keccak256 with the domain tag.
     *      This ensures the entropy is:
     *        - Deterministic (same header → same digest)
     *        - Domain-separated (cannot collide with other entropy sources)
     *        - High quality (incorporates nonce, merkle root, timestamp, PoW hash)
     */
    function _extractEntropy(
        bytes calldata rawHeader,
        bytes32 blockHash,
        uint64 height
    ) internal pure returns (bytes32) {
        // Fields: version[0:4] | prevHash[4:36] | merkleRoot[36:68] | ts[68:72] | bits[72:76] | nonce[76:80]
        return keccak256(
            abi.encodePacked(
                ENTROPY_DOMAIN_TAG,
                blockHash,
                rawHeader[36:68],  // merkle root
                rawHeader[76:80],  // nonce
                rawHeader[68:72],  // timestamp
                rawHeader[72:76],  // bits (difficulty)
                rawHeader[0:4],    // version
                rawHeader[4:36],   // prev block hash
                height
            )
        );
    }

    /**
     * @dev Compute a quality score (0–100) for a stale block.
     *
     *      Components:
     *        PoW difficulty  → 0-30  (more leading zeros = harder = better entropy)
     *        Reorg depth     → 0-25  (deeper reorgs are rarer = more valuable)
     *        Freshness       → 0-20  (newer stale blocks = more useful)
     *        Divergence      → 0-25  (how "random" the timestamp looks)
     */
    function _computeQualityScore(
        uint32 lz,
        uint32 reorgDepth,
        uint32 blockTimestamp
    ) internal view returns (uint32) {
        // PoW difficulty score (0-30)
        uint32 powScore;
        if (lz >= 72) powScore = 30;          // Mainnet-level
        else if (lz >= 56) powScore = 25;
        else if (lz >= 40) powScore = 20;
        else if (lz >= 32) powScore = 15;
        else powScore = lz / 3;

        // Reorg depth score (0-25)
        uint32 reorgScore;
        if (reorgDepth >= 6) reorgScore = 25;       // Very rare
        else if (reorgDepth >= 3) reorgScore = 20;   // Rare
        else if (reorgDepth == 2) reorgScore = 15;    // Uncommon
        else reorgScore = 10;                          // Common single-block

        // Freshness score (0-20)
        uint64 age = uint64(block.timestamp) > uint64(blockTimestamp)
            ? uint64(block.timestamp) - uint64(blockTimestamp)
            : 0;
        uint32 freshScore;
        if (age < 600) freshScore = 20;          // < 10 min
        else if (age < 3600) freshScore = 15;    // < 1 hour
        else if (age < 86400) freshScore = 10;   // < 1 day
        else freshScore = 5;

        // Divergence score (0-25): how far timestamp is from a 10-min boundary
        uint32 tsMod = blockTimestamp % 600;
        uint32 distance = tsMod > 300 ? 600 - tsMod : tsMod;
        uint32 divScore = (distance * 25) / 300;

        uint32 total = powScore + reorgScore + freshScore + divScore;
        return total > 100 ? 100 : total;
    }

    /**
     * @dev Calculate the reward for a stale block proof.
     *      Higher quality and deeper reorgs earn more.
     */
    function _calculateReward(
        uint32 qualityScore,
        uint32 reorgDepth
    ) internal view returns (uint256) {
        // Base × (quality / 100) × depth multiplier
        uint256 qualityMultiplier = uint256(qualityScore);
        uint256 depthMultiplier = uint256(reorgDepth) + 1; // depth 1 → 2x, depth 2 → 3x, etc.
        return (baseReward * qualityMultiplier * depthMultiplier) / 100;
    }

    function _tokenomics() internal view returns (ITokenomicsModule) {
        return ITokenomicsModule(_module(MODULE_TOKENOMICS));
    }
}
