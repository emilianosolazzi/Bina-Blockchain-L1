// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ERC721 } from "@openzeppelin/contracts/token/ERC721/ERC721.sol";
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { ReentrancyGuard } from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import { SafeERC20 } from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import { ECDSA } from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import { ModuleBase } from "./modules/ModuleBase.sol";
import { ITGBT } from "./interfaces/ITGBT.sol";

/**
 * @title UTXOCertificateRegistry
 * @notice ERC-721 certificate layer for Rust-backed dead-UTXO anchors.
 * Upgraded to align with the core anchor registry attestor context.
 */
contract UTXOCertificateRegistry is ERC721, ModuleBase, ReentrancyGuard {
    using SafeERC20 for IERC20;

    uint256 private constant BPS_SCALE = 10_000;
    uint16 public constant MAX_PROTOCOL_FEE_BPS = 5_000;

    enum CertificateType {
        DocumentNotarisation,
        SupplyChain,
        LegalEvidence,
        CarbonCredit,
        AcademicPriority,
        SoftwareBuild,
        FinancialAudit,
        Custom
    }

    struct Certificate {
        bytes32 documentHash;
        bytes32 anchorId;
        bytes32 utxoIdHash;
        bytes32 dataHash;
        bytes32 merkleRoot;
        bytes32 storageReferenceHash;
        bytes32 metadataDigest;
        uint64 anchorCreatedAt;
        uint64 issuedAt;
        address issuedTo;
        address attestor;
        CertificateType certType;
        bool revoked;
    }

    ITGBT public tgbtToken;
    address public protocolTreasury;
    address public anchorVerifier;
    uint16 public protocolFeeBps;
    uint256 public nextTokenId;

    mapping(uint256 => Certificate) private _certificates;
    mapping(uint256 => string) private _tokenMetadataUris;
    mapping(uint256 => bytes) private _attestationSignatures;
    mapping(bytes32 => uint256) public certificateByAttestation;
    mapping(bytes32 => uint256[]) private _documentCertificates;
    mapping(bytes32 => uint256) public latestCertificateByDocument;
    mapping(CertificateType => uint256) public certificateFee;
    mapping(CertificateType => bool) public certificateEnabled;
    mapping(address => bool) public authorizedAttestor;
    mapping(address => address) public attestorPayoutRecipient;

    event CertificateMinted(
        uint256 indexed tokenId,
        bytes32 indexed documentHash,
        bytes32 indexed anchorId,
        CertificateType certType,
        address attestor,
        address recipient,
        uint256 feePaid
    );
    event CertificateRevoked(uint256 indexed tokenId, address indexed caller);
    event AttestorConfigured(address indexed attestor, bool enabled, address payoutRecipient);
    event CertificateFeeUpdated(CertificateType indexed certType, uint256 oldFee, uint256 newFee);
    event CertificateTypeEnabled(CertificateType indexed certType, bool enabled);
    event ProtocolFeeUpdated(uint16 oldFeeBps, uint16 newFeeBps);
    event ProtocolTreasuryUpdated(address indexed oldTreasury, address indexed newTreasury);
    event AnchorVerifierUpdated(address indexed oldVerifier, address indexed newVerifier);

    error ZeroAddress();
    error ZeroDocumentHash();
    error EmptyMetadataURI();
    error CertificateTypeDisabled();
    error InvalidProtocolFee();
    error InvalidCertificateFee();
    error InvalidCreatedAt();
    error InsufficientAllowance();
    error UnauthorizedAttestor();
    error InvalidAnchor();
    error DuplicateAttestation();
    error CertificateNotFound();
    error NotAuthorized();
    error InvalidHexString();

    constructor() ERC721("Temporal Gradient UTXO Certificate", "TGUTC") {
        nextTokenId = 1;
    }

    function initialize(
        address coreAddress,
        address tgbtTokenAddress,
        address treasuryAddress,
        address verifierAddress,
        address defaultAttestor,
        address defaultAttestorPayout
    ) external {
        if (tgbtTokenAddress == address(0)) revert ZeroAddress();
        if (treasuryAddress == address(0)) revert ZeroAddress();
        if (verifierAddress == address(0)) revert ZeroAddress();
        if (defaultAttestor == address(0)) revert ZeroAddress();
        if (defaultAttestorPayout == address(0)) revert ZeroAddress();

        __ModuleBase_init(coreAddress);

        tgbtToken = ITGBT(tgbtTokenAddress);
        protocolTreasury = treasuryAddress;
        anchorVerifier = verifierAddress;
        protocolFeeBps = 3_000;

        _configureAttestor(defaultAttestor, true, defaultAttestorPayout);

        certificateFee[CertificateType.DocumentNotarisation] = 100 ether;
        certificateFee[CertificateType.SupplyChain] = 250 ether;
        certificateFee[CertificateType.LegalEvidence] = 500 ether;
        certificateFee[CertificateType.CarbonCredit] = 300 ether;
        certificateFee[CertificateType.AcademicPriority] = 150 ether;
        certificateFee[CertificateType.SoftwareBuild] = 200 ether;
        certificateFee[CertificateType.FinancialAudit] = 400 ether;
        certificateFee[CertificateType.Custom] = 250 ether;

        for (uint256 i = 0; i <= uint256(CertificateType.Custom); i++) {
            certificateEnabled[CertificateType(i)] = true;
            emit CertificateTypeEnabled(CertificateType(i), true);
        }
    }

    function mintCertificate(
        address recipient,
        bytes32 documentHash,
        string calldata utxoId,
        string calldata dataHashHex,
        string calldata merkleRootHex,
        string calldata storageReference,
        bytes32 metadataDigest,
        uint64 anchorCreatedAt,
        CertificateType certType,
        string calldata metadataURI,
        bytes calldata attestationSignature
    ) external nonReentrant whenSystemActive returns (uint256 tokenId) {
        if (recipient == address(0)) revert ZeroAddress();
        if (documentHash == bytes32(0)) revert ZeroDocumentHash();
        if (bytes(metadataURI).length == 0) revert EmptyMetadataURI();
        if (anchorCreatedAt == 0) revert InvalidCreatedAt();
        if (!certificateEnabled[certType]) revert CertificateTypeDisabled();

        uint256 fee = certificateFee[certType];
        if (fee == 0) revert InvalidCertificateFee();
        if (IERC20(address(tgbtToken)).allowance(msg.sender, address(this)) < fee) revert InsufficientAllowance();

        bytes32 utxoIdHash = keccak256(bytes(utxoId));
        bytes32 dataHash = _parseHex32(dataHashHex);
        bytes32 merkleRoot = _parseHex32(merkleRootHex);
        bytes32 storageReferenceHash = keccak256(bytes(storageReference));
        bytes32 anchorId = IUTXOAnchorVerifier(anchorVerifier).computeAnchorId(
            utxoId,
            dataHashHex,
            merkleRootHex,
            storageReference,
            anchorCreatedAt
        );

        bytes32 attestationDigest = _attestationDigest(
            recipient,
            documentHash,
            anchorId,
            utxoIdHash,
            dataHash,
            merkleRoot,
            storageReferenceHash,
            metadataDigest,
            anchorCreatedAt,
            certType,
            metadataURI
        );
        address attestor = _recoverAttestor(attestationDigest, attestationSignature);
        if (!authorizedAttestor[attestor]) revert UnauthorizedAttestor();

        bytes32 attestationKey = keccak256(abi.encode(attestor, attestationDigest));
        if (certificateByAttestation[attestationKey] != 0) revert DuplicateAttestation();

        // FIXED: Query the exact recorded attestor address stored inside the UTXOAnchorVerifier
        // structure instead of guessing alignment configurations blindly.
        IUTXOAnchorVerifier.AnchorRecord memory verifierRecord = IUTXOAnchorVerifier(anchorVerifier).getAnchor(anchorId);

        bool validAnchor = IUTXOAnchorVerifier(anchorVerifier).verifyStoredAnchor(
            anchorId,
            utxoIdHash,
            dataHash,
            merkleRoot,
            storageReferenceHash,
            metadataDigest,
            anchorCreatedAt,
            verifierRecord.attestor
        );
        if (!validAnchor) revert InvalidAnchor();

        IERC20(address(tgbtToken)).safeTransferFrom(msg.sender, address(this), fee);
        _splitFee(attestor, fee);

        tokenId = nextTokenId++;
        _safeMint(recipient, tokenId);

        _certificates[tokenId] = Certificate({
            documentHash: documentHash,
            anchorId: anchorId,
            utxoIdHash: utxoIdHash,
            dataHash: dataHash,
            merkleRoot: merkleRoot,
            storageReferenceHash: storageReferenceHash,
            metadataDigest: metadataDigest,
            anchorCreatedAt: anchorCreatedAt,
            issuedAt: uint64(block.timestamp),
            issuedTo: recipient,
            attestor: attestor,
            certType: certType,
            revoked: false
        });
        _tokenMetadataUris[tokenId] = metadataURI;
        _attestationSignatures[tokenId] = attestationSignature;
        _documentCertificates[documentHash].push(tokenId);
        latestCertificateByDocument[documentHash] = tokenId;
        certificateByAttestation[attestationKey] = tokenId;

        emit CertificateMinted(tokenId, documentHash, anchorId, certType, attestor, recipient, fee);
    }

    function revokeCertificate(uint256 tokenId) external {
        Certificate storage cert = _requireCertificate(tokenId);
        if (msg.sender != cert.attestor && !core.hasRole(GOVERNANCE_ROLE, msg.sender)) {
            revert NotAuthorized();
        }
        cert.revoked = true;
        emit CertificateRevoked(tokenId, msg.sender);
    }

    function verifyCertificate(uint256 tokenId)
        external
        view
        returns (bool valid, Certificate memory certificate)
    {
        certificate = _certificates[tokenId];
        if (certificate.documentHash == bytes32(0) || certificate.revoked) {
            return (false, certificate);
        }

        // FIXED: Re-verify dynamic routing alignment
        IUTXOAnchorVerifier.AnchorRecord memory verifierRecord = IUTXOAnchorVerifier(anchorVerifier).getAnchor(certificate.anchorId);

        valid = IUTXOAnchorVerifier(anchorVerifier).verifyStoredAnchor(
            certificate.anchorId,
            certificate.utxoIdHash,
            certificate.dataHash,
            certificate.merkleRoot,
            certificate.storageReferenceHash,
            certificate.metadataDigest,
            certificate.anchorCreatedAt,
            verifierRecord.attestor
        );
    }

    function getCertificate(uint256 tokenId)
        external
        view
        returns (Certificate memory certificate, string memory metadataURI, bytes memory attestationSignature)
    {
        certificate = _requireCertificate(tokenId);
        metadataURI = _tokenMetadataUris[tokenId];
        attestationSignature = _attestationSignatures[tokenId];
    }

    function getDocumentCertificates(bytes32 documentHash) external view returns (uint256[] memory tokenIds) {
        return _documentCertificates[documentHash];
    }

    function tokenURI(uint256 tokenId) public view override returns (string memory) {
        _requireCertificate(tokenId);
        return _tokenMetadataUris[tokenId];
    }

    function attestationSignatureOf(uint256 tokenId) external view returns (bytes memory) {
        _requireCertificate(tokenId);
        return _attestationSignatures[tokenId];
    }

    function setAttestor(address attestor, bool enabled, address payoutRecipient) external onlyGovernance {
        _configureAttestor(attestor, enabled, payoutRecipient);
    }

    function setCertificateFee(CertificateType certType, uint256 newFee) external onlyGovernance {
        if (newFee == 0) revert InvalidCertificateFee();
        uint256 oldFee = certificateFee[certType];
        certificateFee[certType] = newFee;
        emit CertificateFeeUpdated(certType, oldFee, newFee);
    }

    function setCertificateEnabled(CertificateType certType, bool enabled) external onlyGovernance {
        certificateEnabled[certType] = enabled;
        emit CertificateTypeEnabled(certType, enabled);
    }

    function setProtocolFeeBps(uint16 newFeeBps) external onlyGovernance {
        if (newFeeBps > MAX_PROTOCOL_FEE_BPS) revert InvalidProtocolFee();
        uint16 oldFeeBps = protocolFeeBps;
        protocolFeeBps = newFeeBps;
        emit ProtocolFeeUpdated(oldFeeBps, newFeeBps);
    }

    function setProtocolTreasury(address newTreasury) external onlyGovernance {
        if (newTreasury == address(0)) revert ZeroAddress();
        address oldTreasury = protocolTreasury;
        protocolTreasury = newTreasury;
        emit ProtocolTreasuryUpdated(oldTreasury, newTreasury);
    }

    function setAnchorVerifier(address newVerifier) external onlyGovernance {
        if (newVerifier == address(0)) revert ZeroAddress();
        address oldVerifier = anchorVerifier;
        anchorVerifier = newVerifier;
        emit AnchorVerifierUpdated(oldVerifier, newVerifier);
    }

    function _splitFee(address attestor, uint256 fee) internal {
        uint256 protocolShare = (fee * protocolFeeBps) / BPS_SCALE;
        uint256 attestorShare = fee - protocolShare;
        address payoutRecipient = attestorPayoutRecipient[attestor];
        if (payoutRecipient == address(0)) {
            payoutRecipient = attestor;
        }

        if (protocolShare > 0) {
            IERC20(address(tgbtToken)).safeTransfer(protocolTreasury, protocolShare);
        }
        if (attestorShare > 0) {
            IERC20(address(tgbtToken)).safeTransfer(payoutRecipient, attestorShare);
        }
    }

    function _configureAttestor(address attestor, bool enabled, address payoutRecipient) internal {
        if (attestor == address(0)) revert ZeroAddress();
        if (payoutRecipient == address(0)) revert ZeroAddress();
        authorizedAttestor[attestor] = enabled;
        attestorPayoutRecipient[attestor] = payoutRecipient;
        emit AttestorConfigured(attestor, enabled, payoutRecipient);
    }

    function _attestationDigest(
        address recipient,
        bytes32 documentHash,
        bytes32 anchorId,
        bytes32 utxoIdHash,
        bytes32 dataHash,
        bytes32 merkleRoot,
        bytes32 storageReferenceHash,
        bytes32 metadataDigest,
        uint64 anchorCreatedAt,
        CertificateType certType,
        string calldata metadataURI
    ) internal view returns (bytes32) {
        return keccak256(
            abi.encode(
                address(this),
                block.chainid,
                recipient,
                documentHash,
                anchorId,
                utxoIdHash,
                dataHash,
                merkleRoot,
                storageReferenceHash,
                metadataDigest,
                anchorCreatedAt,
                certType,
                keccak256(bytes(metadataURI))
            )
        );
    }

    function _recoverAttestor(bytes32 digest, bytes memory signature) internal pure returns (address) {
        bytes32 ethSignedDigest = keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", digest));
        return ECDSA.recover(ethSignedDigest, signature);
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

    function _requireCertificate(uint256 tokenId) internal view returns (Certificate storage cert) {
        // FIXED: Safe local lookup to ensure data verification can run independently of v5 ownership changes
        cert = _certificates[tokenId];
        if (cert.documentHash == bytes32(0)) revert CertificateNotFound();
    }
}

interface IUTXOAnchorVerifier {
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

    function getAnchor(bytes32 anchorId) external view returns (AnchorRecord memory record);

    function computeAnchorId(
        string calldata utxoId,
        string calldata dataHashHex,
        string calldata merkleRootHex,
        string calldata storageReference,
        uint64 createdAt
    ) external pure returns (bytes32 anchorId);

    function verifyStoredAnchor(
        bytes32 anchorId,
        bytes32 utxoIdHash,
        bytes32 dataHash,
        bytes32 merkleRoot,
        bytes32 storageReferenceHash,
        bytes32 metadataDigest,
        uint64 createdAt,
        address attestor
    ) external view returns (bool valid);
}
