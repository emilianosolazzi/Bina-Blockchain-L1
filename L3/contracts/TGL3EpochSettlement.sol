// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";

/// @title TGL3EpochSettlement
/// @notice Minimal epoch commit/finalize scaffold for Temporal Gradient's first L3 wave.
/// @dev Scaffold only. Intentionally small and Orbit-devnet oriented.
contract TGL3EpochSettlement is Ownable {
    struct Epoch {
        bytes32 merkleRoot;
        uint32 leafCount;
        address operator;
        uint64 sourceChainId;
        uint64 committedAt;
        uint64 finalizedAt;
        bytes32 sourceRef;
        bool finalized;
        string dataUri;
    }

    uint256 public immutable challengeWindowSeconds;
    uint256 public nextEpochId;

    mapping(uint256 => Epoch) private _epochs;
    mapping(address => bool) public operators;

    error NotOperator();
    error InvalidEpoch();
    error InvalidMerkleRoot();
    error InvalidLeafCount();
    error EpochAlreadyFinalized();
    error ChallengeWindowOpen();

    event OperatorUpdated(address indexed operator, bool allowed);
    event EpochCommitted(
        uint256 indexed epochId,
        address indexed operator,
        bytes32 merkleRoot,
        uint32 leafCount,
        uint64 sourceChainId,
        bytes32 sourceRef,
        string dataUri
    );
    event EpochFinalized(uint256 indexed epochId, address indexed operator, uint64 finalizedAt);

    modifier onlyOperator() {
        if (!operators[msg.sender]) revert NotOperator();
        _;
    }

    constructor(address initialOwner, uint256 challengeWindowSecs) Ownable(initialOwner) {
        if (challengeWindowSecs == 0) revert ChallengeWindowOpen();
        challengeWindowSeconds = challengeWindowSecs;
    }

    function setOperator(address operator, bool allowed) external onlyOwner {
        operators[operator] = allowed;
        emit OperatorUpdated(operator, allowed);
    }

    function epochExists(uint256 epochId) external view returns (bool) {
        return _epochs[epochId].operator != address(0);
    }

    function getEpoch(uint256 epochId) external view returns (Epoch memory) {
        Epoch memory epoch = _epochs[epochId];
        if (epoch.operator == address(0)) revert InvalidEpoch();
        return epoch;
    }

    function commitEpoch(
        bytes32 merkleRoot,
        uint32 leafCount,
        string calldata dataUri,
        uint64 sourceChainId,
        bytes32 sourceRef
    ) external onlyOperator returns (uint256 epochId) {
        if (merkleRoot == bytes32(0)) revert InvalidMerkleRoot();
        if (leafCount == 0) revert InvalidLeafCount();

        epochId = nextEpochId++;
        _epochs[epochId] = Epoch({
            merkleRoot: merkleRoot,
            leafCount: leafCount,
            operator: msg.sender,
            sourceChainId: sourceChainId,
            committedAt: uint64(block.timestamp),
            finalizedAt: 0,
            sourceRef: sourceRef,
            finalized: false,
            dataUri: dataUri
        });

        emit EpochCommitted(epochId, msg.sender, merkleRoot, leafCount, sourceChainId, sourceRef, dataUri);
    }

    function finalizeEpoch(uint256 epochId) external {
        Epoch storage epoch = _epochs[epochId];
        if (epoch.operator == address(0)) revert InvalidEpoch();
        if (epoch.finalized) revert EpochAlreadyFinalized();
        if (msg.sender != owner() && msg.sender != epoch.operator) revert NotOperator();
        if (block.timestamp < uint256(epoch.committedAt) + challengeWindowSeconds) revert ChallengeWindowOpen();

        epoch.finalized = true;
        epoch.finalizedAt = uint64(block.timestamp);

        emit EpochFinalized(epochId, epoch.operator, epoch.finalizedAt);
    }
}