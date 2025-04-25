// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { AccessControlUpgradeable } from "@openzeppelin/contracts-upgradeable/access/AccessControlUpgradeable.sol";
import { Initializable } from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import { UUPSUpgradeable } from "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";

/**
 * @title KYCVerifier
 * @notice Contract to manage DID-based KYC Verification for compliance.
 */
contract KYCVerifier is Initializable, AccessControlUpgradeable, UUPSUpgradeable {
    bytes32 public constant ISSUER_ROLE = keccak256("ISSUER_ROLE");

    enum KYCStatus { Unknown, Pending, Approved, Rejected }

    struct KYCData {
        KYCStatus status;
        uint256 issuedAt;
        uint256 expiresAt;
        string region; // Region or jurisdiction code (e.g., US, EU)
    }

    mapping(address => KYCData) private kycRecords;

    event KYCRequested(address indexed user);
    event KYCApproved(address indexed user, string region, uint256 expiresAt);
    event KYCRejected(address indexed user);
    event KYCRevoked(address indexed user);

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor() {
        _disableInitializers();
    }

    function initialize(address admin) public initializer {
        __AccessControl_init();
        __UUPSUpgradeable_init();

        _grantRole(DEFAULT_ADMIN_ROLE, admin);
    }

    /// --- User Actions ---

    function requestKYC() external {
        require(kycRecords[msg.sender].status == KYCStatus.Unknown, "Already requested or verified");
        kycRecords[msg.sender] = KYCData({
            status: KYCStatus.Pending,
            issuedAt: block.timestamp,
            expiresAt: 0,
            region: ""
        });
        emit KYCRequested(msg.sender);
    }

    /// --- Issuer Actions ---

    function approveKYC(address user, string calldata region, uint256 validFor) external onlyRole(ISSUER_ROLE) {
        require(kycRecords[user].status == KYCStatus.Pending, "Not pending");
        kycRecords[user] = KYCData({
            status: KYCStatus.Approved,
            issuedAt: block.timestamp,
            expiresAt: block.timestamp + validFor,
            region: region
        });
        emit KYCApproved(user, region, block.timestamp + validFor);
    }

    function rejectKYC(address user) external onlyRole(ISSUER_ROLE) {
        require(kycRecords[user].status == KYCStatus.Pending, "Not pending");
        kycRecords[user].status = KYCStatus.Rejected;
        emit KYCRejected(user);
    }

    function revokeKYC(address user) external onlyRole(ISSUER_ROLE) {
        require(kycRecords[user].status == KYCStatus.Approved, "Not approved");
        kycRecords[user].status = KYCStatus.Rejected;
        emit KYCRevoked(user);
    }

    /// --- External Views ---

    function getKYCData(address user) external view returns (KYCData memory) {
        return kycRecords[user];
    }

    function isKYCApproved(address user) external view returns (bool) {
        KYCData memory data = kycRecords[user];
        return data.status == KYCStatus.Approved && block.timestamp <= data.expiresAt;
    }

    /// --- UUPS Authorization ---

    function _authorizeUpgrade(address newImplementation) internal override onlyRole(DEFAULT_ADMIN_ROLE) {}
}
