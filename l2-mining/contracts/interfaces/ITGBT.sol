// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { IERC20Metadata } from "@openzeppelin/contracts/token/ERC20/extensions/IERC20Metadata.sol";

/**
 * @title  ITGBT — Interface for Temporal Gradient Beacon Token
 * @notice Module-authorized, immutable-cap token with Bitcoin-like
 *         permission ossification.
 */
interface ITGBT is IERC20, IERC20Metadata {

    // ── Constants & State ────────────────────────────────────
    function MAX_SUPPLY() external view returns (uint256);
    function governance() external view returns (address);
    function permissionsLocked() external view returns (bool);
    function isAuthorized(address module) external view returns (bool);
    function authorizedCount() external view returns (uint256);

    // ── Authorization (governance only, before lock) ─────────
    function grantAuthorization(address module) external;
    function revokeAuthorization(address module) external;
    function lockPermissions() external;

    // ── Minting (authorized modules only) ────────────────────
    function mint(address to, uint256 amount) external;
    function availableToMint() external view returns (uint256);

    // ── Stamp System ─────────────────────────────────────────
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

    function stampCount() external view returns (uint256);
    function epochStamp(uint64 epochId) external view returns (uint256);
    function epochStamped(uint64 epochId) external view returns (bool);
    function recordStamp(
        uint64  epochId,
        address miner,
        bytes32 merkleRoot,
        bytes32 bitcoinTxHash,
        uint32  bitcoinVout,
        uint32  bitcoinBlock,
        bytes calldata btcInclusionProof
    ) external returns (uint256 stampId);
    function getEpochStamp(uint64 epochId) external view returns (Stamp memory);

    // ── Events ───────────────────────────────────────────────
    event AuthorizationGranted(address indexed module);
    event AuthorizationRevoked(address indexed module);
    event PermissionsLockedForever(uint256 authorizedModules);
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
}
