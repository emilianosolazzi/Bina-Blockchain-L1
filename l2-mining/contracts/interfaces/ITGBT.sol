// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { IERC20Metadata } from "@openzeppelin/contracts/token/ERC20/extensions/IERC20Metadata.sol";

/**
 * @title ITGBT - Interface for Temporal Gradient Beacon Token
 * @notice Defines the immutable protocol token interface.
 */
interface ITGBT is IERC20, IERC20Metadata {
    function MAX_SUPPLY() external view returns (uint256);
    function controller() external view returns (address);
    function mint(address to, uint256 amount) external;
    function availableToMint() external view returns (uint256);

    struct Stamp {
        uint64 epochId;
        bytes32 merkleRoot;
        bytes32 bitcoinTxHash;
        uint32 bitcoinVout;
        uint32 bitcoinBlock;
        uint64 timestamp;
        address creator;
    }

    function stampCount() external view returns (uint256);
    function epochStamp(uint64 epochId) external view returns (uint256);
    function recordStamp(
        uint64 epochId,
        bytes32 merkleRoot,
        bytes32 bitcoinTxHash,
        uint32 bitcoinVout,
        uint32 bitcoinBlock
    ) external returns (uint256 stampId);
    function getEpochStamp(uint64 epochId) external view returns (Stamp memory);

    event StampRecorded(uint256 indexed stampId, uint64 indexed epochId, bytes32 merkleRoot, bytes32 bitcoinTxHash, uint32 bitcoinVout, uint32 bitcoinBlock);
}
