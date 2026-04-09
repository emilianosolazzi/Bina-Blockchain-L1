// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";
import { TGL3EpochSettlement } from "./TGL3EpochSettlement.sol";
import { TGL3Treasury } from "./TGL3Treasury.sol";

/// @title TGL3CertificateRegistry
/// @notice Minimal certificate issuance settlement scaffold for the first L3 devnet.
/// @dev Scaffold only. Stores issuance records without introducing NFT complexity yet.
contract TGL3CertificateRegistry is Ownable {
    struct CertificateRecord {
        address recipient;
        address issuer;
        uint256 epochId;
        bytes32 documentHash;
        bytes32 anchorId;
        uint256 feePaid;
        uint64 issuedAt;
        string metadataUri;
    }

    uint256 public nextCertificateId;
    uint256 public issuanceFee;

    TGL3EpochSettlement public immutable epochSettlement;
    TGL3Treasury public immutable treasury;

    mapping(address => bool) public approvedIssuers;
    mapping(uint256 => CertificateRecord) public certificates;

    error UnknownEpoch();
    error InvalidDocumentHash();
    error NotApprovedIssuer();
    error ZeroRecipient();

    event IssuerUpdated(address indexed issuer, bool allowed);
    event IssuanceFeeUpdated(uint256 fee);
    event CertificateIssued(
        uint256 indexed certificateId,
        uint256 indexed epochId,
        address indexed recipient,
        address issuer,
        bytes32 documentHash,
        bytes32 anchorId,
        uint256 feePaid,
        string metadataUri
    );

    constructor(
        address initialOwner,
        address epochSettlementAddress,
        address treasuryAddress,
        uint256 issuanceFeeAmount
    ) Ownable(initialOwner) {
        epochSettlement = TGL3EpochSettlement(epochSettlementAddress);
        treasury = TGL3Treasury(treasuryAddress);
        issuanceFee = issuanceFeeAmount;
    }

    function setIssuer(address issuer, bool allowed) external onlyOwner {
        approvedIssuers[issuer] = allowed;
        emit IssuerUpdated(issuer, allowed);
    }

    function setIssuanceFee(uint256 fee) external onlyOwner {
        issuanceFee = fee;
        emit IssuanceFeeUpdated(fee);
    }

    function issueCertificate(
        address recipient,
        uint256 epochId,
        bytes32 documentHash,
        bytes32 anchorId,
        string calldata metadataUri
    ) external returns (uint256 certificateId) {
        if (!approvedIssuers[msg.sender]) revert NotApprovedIssuer();
        if (!epochSettlement.epochExists(epochId)) revert UnknownEpoch();
        if (recipient == address(0)) revert ZeroRecipient();
        if (documentHash == bytes32(0)) revert InvalidDocumentHash();

        certificateId = nextCertificateId++;
        bytes32 paymentReference = keccak256(
            abi.encodePacked("certificate", certificateId, epochId, msg.sender, recipient, documentHash)
        );
        treasury.collectPaymentFrom(msg.sender, issuanceFee, paymentReference);

        certificates[certificateId] = CertificateRecord({
            recipient: recipient,
            issuer: msg.sender,
            epochId: epochId,
            documentHash: documentHash,
            anchorId: anchorId,
            feePaid: issuanceFee,
            issuedAt: uint64(block.timestamp),
            metadataUri: metadataUri
        });

        emit CertificateIssued(
            certificateId,
            epochId,
            recipient,
            msg.sender,
            documentHash,
            anchorId,
            issuanceFee,
            metadataUri
        );
    }
}