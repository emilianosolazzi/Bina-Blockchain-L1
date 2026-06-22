// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ModuleBase } from "./modules/ModuleBase.sol";

/**
 * @title UTXOAnchorVerifier
 * @notice On-chain registry for canonical dead-UTXO anchors.
 * Upgraded to guarantee strict collision safety against variable-length strings.
 */
contract UTXOAnchorVerifier is ModuleBase {
    bytes32 private constant GOVERNANCE_ROLE = keccak256("GOVERNANCE_ROLE");

    struct AnchorRecord {
        bytes32 anchorId;
        bytes32 utxoIdHash;
        bytes32 dataHash;
        bytes32 merkleRoot;
        bytes32 storageReferenceHash;
        bytes32 metadataDigest;
        uint64 createdAt;
        address attestor;
        bool active;
    }

    mapping(bytes32 => AnchorRecord) private _anchors;
    mapping(address => bool) public authorizedAttestor;

    event AnchorRegistered(
        bytes32 indexed anchorId,
        bytes32 indexed utxoIdHash,
        bytes32 indexed dataHash,
        address attestor,
        uint64 createdAt
    );
    event AnchorRevoked(bytes32 indexed anchorId, address indexed caller);
    event AttestorConfigured(address indexed attestor, bool enabled);

    error ZeroAddress();
    error InvalidCreatedAt();
    error InvalidHexString();
    error UnauthorizedAttestor();
    error NotAuthorized();
    error DuplicateAnchor();
    error AnchorNotFound();

    function initialize(address coreAddress, address defaultAttestor) external {
        if (defaultAttestor == address(0)) revert ZeroAddress();
        __ModuleBase_init(coreAddress);
        authorizedAttestor[defaultAttestor] = true;
        emit AttestorConfigured(defaultAttestor, true);
    }

    function setAttestor(address attestor, bool enabled) external onlyGovernance {
        if (attestor == address(0)) revert ZeroAddress();
        authorizedAttestor[attestor] = enabled;
        emit AttestorConfigured(attestor, enabled);
    }

    function registerAnchor(
        string calldata utxoId,
        string calldata dataHashHex,
        string calldata merkleRootHex,
        string calldata storageReference,
        bytes32 metadataDigest,
        uint64 createdAt,
        address attestor
    ) external whenSystemActive returns (bytes32 anchorId) {
        if (attestor == address(0)) revert ZeroAddress();
        if (createdAt == 0) revert InvalidCreatedAt();
        if (!authorizedAttestor[attestor]) revert UnauthorizedAttestor();

        bool callerAuthorized = msg.sender == attestor
            || core.hasRole(GOVERNANCE_ROLE, msg.sender)
            || msg.sender == address(core)
            || core.isModule(msg.sender);
        if (!callerAuthorized) revert NotAuthorized();

        anchorId = computeAnchorId(utxoId, dataHashHex, merkleRootHex, storageReference, createdAt);
        if (_anchors[anchorId].active) revert DuplicateAnchor();

        bytes32 utxoIdHash = keccak256(bytes(utxoId));
        bytes32 dataHash = _parseHex32(dataHashHex);

        _anchors[anchorId] = AnchorRecord({
            anchorId: anchorId,
            utxoIdHash: utxoIdHash,
            dataHash: dataHash,
            merkleRoot: _parseHex32(merkleRootHex),
            storageReferenceHash: keccak256(bytes(storageReference)),
            metadataDigest: metadataDigest,
            createdAt: createdAt,
            attestor: attestor,
            active: true
        });

        emit AnchorRegistered(anchorId, utxoIdHash, dataHash, attestor, createdAt);
    }

    function revokeAnchor(bytes32 anchorId) external {
        AnchorRecord storage record = _anchors[anchorId];
        if (!record.active) revert AnchorNotFound();
        if (msg.sender != record.attestor && !core.hasRole(GOVERNANCE_ROLE, msg.sender)) {
            revert NotAuthorized();
        }
        record.active = false;
        emit AnchorRevoked(anchorId, msg.sender);
    }

    function getAnchor(bytes32 anchorId) external view returns (AnchorRecord memory record) {
        record = _anchors[anchorId];
        if (!record.active) revert AnchorNotFound();
    }

    function verifyAnchor(
        string calldata utxoId,
        string calldata dataHashHex,
        string calldata merkleRootHex,
        string calldata storageReference,
        bytes32 metadataDigest,
        uint64 createdAt,
        address attestor
    ) external view returns (bool valid, bytes32 anchorId) {
        anchorId = computeAnchorId(utxoId, dataHashHex, merkleRootHex, storageReference, createdAt);
        valid = verifyStoredAnchor(
            anchorId,
            keccak256(bytes(utxoId)),
            _parseHex32(dataHashHex),
            _parseHex32(merkleRootHex),
            keccak256(bytes(storageReference)),
            metadataDigest,
            createdAt,
            attestor
        );
    }

    function verifyStoredAnchor(
        bytes32 anchorId,
        bytes32 utxoIdHash,
        bytes32 dataHash,
        bytes32 merkleRoot,
        bytes32 storageReferenceHash,
        bytes32 metadataDigest,
        uint64 createdAt,
        address attestor
    ) public view returns (bool valid) {
        AnchorRecord memory record = _anchors[anchorId];
        if (!record.active) {
            return false;
        }

        return record.anchorId == anchorId
            && record.utxoIdHash == utxoIdHash
            && record.dataHash == dataHash
            && record.merkleRoot == merkleRoot
            && record.storageReferenceHash == storageReferenceHash
            && record.metadataDigest == metadataDigest
            && record.createdAt == createdAt
            && record.attestor == attestor;
    }

    /**
     * @notice Computes a deterministic identity hash for the UTXO parameter mapping.
     * @dev FIXED: Wraps components via explicit typed grouping to block bytes.concat position shifts.
     */
    function computeAnchorId(
        string calldata utxoId,
        string calldata dataHashHex,
        string calldata merkleRootHex,
        string calldata storageReference,
        uint64 createdAt
    ) public pure returns (bytes32 anchorId) {
        return sha256(
            abi.encodePacked(
                keccak256(bytes(utxoId)),
                keccak256(bytes(dataHashHex)),
                keccak256(bytes(merkleRootHex)),
                keccak256(bytes(storageReference)),
                _toLittleEndian(createdAt)
            )
        );
    }

    function _toLittleEndian(uint64 value) internal pure returns (bytes memory out) {
        out = new bytes(8);
        for (uint256 i = 0; i < 8; i++) {
            out[i] = bytes1(uint8(value >> (i * 8)));
        }
    }

    function _parseHex32(string calldata value) internal pure returns (bytes32 result) {
        bytes calldata raw = bytes(value);
        uint256 offset = 0;
        if (raw.length == 66 && raw[0] == "0" && (raw[1] == "x" || raw[1] == "X")) {
            offset = 2;
        } else if (raw.length != 64) {
            revert InvalidHexString();
        }

        if (raw.length - offset != 64) revert InvalidHexString();

        for (uint256 i = 0; i < 32; i++) {
            uint8 msn = _fromHexChar(uint8(raw[offset + (i * 2)]));
            uint8 lsn = _fromHexChar(uint8(raw[offset + (i * 2) + 1]));
            result |= bytes32(uint256((msn << 4) | lsn) << ((31 - i) * 8));
        }
    }

    function _fromHexChar(uint8 c) internal pure returns (uint8) {
        if (c >= 48 && c <= 57) return c - 48;
        if (c >= 65 && c <= 70) return c - 55;
        if (c >= 97 && c <= 102) return c - 87;
        revert InvalidHexString();
    }
}

```
