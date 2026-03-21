// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ERC20 } from "@openzeppelin/contracts/token/ERC20/ERC20.sol";


/**
 * @title TGBT - Temporal Gradient Beacon Token
 * @notice Immutable capped ERC20 with protocol-bound minting and no admin, pause, or slashing controls.
 */
contract TGBT is ERC20 {
    uint256 public constant MAX_SUPPLY = 2_000_000_000 ether;
    address public immutable controller;

    error NotController();

    /**
     * @notice Constructs the TGBT token.
     * @param _controller Immutable protocol controller allowed to mint and record epoch stamps
     */
    constructor(address _controller) ERC20("Temporal Gradient Beacon Token", "TGBT") {
        require(_controller != address(0), "Zero controller address");
        controller = _controller;
    }

    modifier onlyController() {
        if (msg.sender != controller) revert NotController();
        _;
    }

    // --- Minting ---

    /**
     * @notice Mints tokens to an address. Restricted to the immutable protocol controller.
     */
    function mint(address to, uint256 amount) external onlyController {
        require(to != address(0), "Zero address");
        require(totalSupply() + amount <= MAX_SUPPLY, "Cap exceeded");
        _mint(to, amount);
    }

    // --- External Views ---

    /**
     * @return Remaining tokens that can be minted before hitting MAX_SUPPLY
     */
    function availableToMint() external view returns (uint256) {
        return MAX_SUPPLY - totalSupply();
    }

    // ── Temporal Randomness Stamp ───────────────────────────

    struct Stamp {
        uint64  epochId;
        bytes32 merkleRoot;
        bytes32 bitcoinTxHash;
        uint32  bitcoinVout;
        uint32  bitcoinBlock;
        uint64  timestamp;
        address creator;
    }

    uint256 public stampCount;
    mapping(uint256 => Stamp) public stamps;
    mapping(uint64  => uint256) public epochStamp;

    event StampRecorded(uint256 indexed stampId, uint64 indexed epochId, bytes32 merkleRoot, bytes32 bitcoinTxHash, uint32 bitcoinVout, uint32 bitcoinBlock);

    function recordStamp(
        uint64  epochId,
        bytes32 merkleRoot,
        bytes32 bitcoinTxHash,
        uint32  bitcoinVout,
        uint32  bitcoinBlock
    ) external onlyController returns (uint256 stampId) {
        require(merkleRoot    != bytes32(0), "Zero merkle root");
        require(bitcoinTxHash != bytes32(0), "Zero tx hash");
        require(epochStamp[epochId] == 0,    "Epoch already stamped");

        stampId = ++stampCount;
        stamps[stampId] = Stamp(epochId, merkleRoot, bitcoinTxHash, bitcoinVout, bitcoinBlock, uint64(block.timestamp), msg.sender);
        epochStamp[epochId] = stampId;

        emit StampRecorded(stampId, epochId, merkleRoot, bitcoinTxHash, bitcoinVout, bitcoinBlock);
    }

    function getEpochStamp(uint64 epochId) external view returns (Stamp memory) {
        uint256 id = epochStamp[epochId];
        require(id != 0, "Epoch not stamped");
        return stamps[id];
    }

}
