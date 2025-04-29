// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { AccessControlUpgradeable } from "@openzeppelin/contracts-upgradeable/access/AccessControlUpgradeable.sol";
import { Initializable } from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import { UUPSUpgradeable } from "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";
import { PausableUpgradeable } from "@openzeppelin/contracts-upgradeable/security/PausableUpgradeable.sol";
import { TimelockControllerUpgradeable } from "@openzeppelin/contracts-upgradeable/governance/TimelockControllerUpgradeable.sol";
import { IKYCVerifier } from "./interfaces/IKYCVerifier.sol";

/**
 * @title KYCVerifier
 * @notice Contract to manage DID-based KYC Verification for compliance.
 */
contract KYCVerifier is 
    IKYCVerifier,
    Initializable, 
    AccessControlUpgradeable, 
    PausableUpgradeable,
    TimelockControllerUpgradeable,
    UUPSUpgradeable 
{
    // Roles
    bytes32 public constant ISSUER_ROLE = keccak256("ISSUER_ROLE");
    bytes32 public constant EMERGENCY_ROLE = keccak256("EMERGENCY_ROLE");
    
    // Custom errors
    error AlreadyVerified();
    error NotPending();
    error NotApproved();
    error InvalidValidity();
    error BatchTooLarge();
    error EmptyBatch();
    error ArrayLengthMismatch();
    error InvalidInstitution();
    error RateLimitExceeded();
    error InvalidRiskLevel();

    enum KYCStatus { Unknown, Pending, Approved, Rejected }
    
    struct KYCData {
        KYCStatus status;
        uint256 issuedAt;
        uint256 expiresAt;
        string region;
        string institutionId;    // Added: Issuing institution ID
        bytes32 documentHash;    // Added: Hash of KYC documents
        uint256 riskLevel;      // Added: Risk assessment level (1-100)
        string[] approvalTypes;  // Added: Types of verification passed
    }

    struct Institution {
        bool active;
        uint256 dailyLimit;
        uint256 usedToday;
        uint256 lastResetDay;
    }

    mapping(address => KYCData) private kycRecords;
    mapping(string => Institution) private institutions;
    mapping(address => uint256) private lastRequestTime;
    uint256 private constant MAX_BATCH_SIZE = 50;
    uint256 private constant REQUEST_COOLDOWN = 1 days;
    uint256 private constant MAX_RISK_LEVEL = 100;
    
    // Events with more data
    event KYCRequested(address indexed user, string institutionId);
    event KYCApproved(
        address indexed user, 
        string region, 
        uint256 expiresAt, 
        bytes32 documentHash,
        uint256 riskLevel
    );
    event KYCRejected(address indexed user);
    event KYCRevoked(address indexed user);
    event InstitutionUpdated(string indexed institutionId, bool active, uint256 dailyLimit);
    event TimelockActionScheduled(bytes32 indexed actionId, address target);

    // Storage gap for upgrades
    uint256[50] private __gap;

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor() {
        _disableInitializers();
    }

    function initialize(
        address admin,
        uint256 minDelay,
        address[] memory proposers,
        address[] memory executors
    ) public initializer {
        __AccessControl_init();
        __Pausable_init();
        __TimelockController_init(minDelay, proposers, executors);
        __UUPSUpgradeable_init();

        _grantRole(DEFAULT_ADMIN_ROLE, admin);
    }

    /// --- User Actions ---

    function requestKYC() external {
        // Rate limit check
        if(block.timestamp - lastRequestTime[msg.sender] < REQUEST_COOLDOWN) 
            revert RateLimitExceeded();
        lastRequestTime[msg.sender] = block.timestamp;

        require(kycRecords[msg.sender].status == KYCStatus.Unknown, "Already requested or verified");
        kycRecords[msg.sender] = KYCData({
            status: KYCStatus.Pending,
            issuedAt: block.timestamp,
            expiresAt: 0,
            region: "",
            institutionId: "",
            documentHash: "",
            riskLevel: 0,
            approvalTypes: new string[](0)
        });
        emit KYCRequested(msg.sender, "");
    }

    /// --- Issuer Actions ---

    function approveKYC(address user, string calldata region, uint256 validFor) external onlyRole(ISSUER_ROLE) whenNotPaused {
        _approveKYC(user, region, validFor, "", 0);
    }

    function _approveKYC(
        address user,
        string calldata region,
        uint256 validFor,
        bytes32 documentHash,
        uint256 riskLevel
    ) internal {
        if(kycRecords[user].status != KYCStatus.Pending) revert NotPending();
        if(validFor == 0) revert InvalidValidity();
        if(riskLevel > MAX_RISK_LEVEL) revert InvalidRiskLevel();
        if(documentHash == bytes32(0)) revert("Invalid document hash");

        string memory instId = "INST-001"; // Get from mapping instead
        Institution storage inst = institutions[instId];
        if(!inst.active) revert InvalidInstitution();

        // Reset daily limit if new day
        if(block.timestamp / 1 days > inst.lastResetDay) {
            inst.usedToday = 0;
            inst.lastResetDay = block.timestamp / 1 days;
        }

        // Check daily limit
        if(inst.usedToday >= inst.dailyLimit) revert RateLimitExceeded();
        inst.usedToday++;

        kycRecords[user] = KYCData({
            status: KYCStatus.Approved,
            issuedAt: block.timestamp,
            expiresAt: block.timestamp + validFor,
            region: region,
            institutionId: "INST-001", // Should be set based on msg.sender
            documentHash: documentHash,
            riskLevel: riskLevel,
            approvalTypes: new string[](0)
        });

        emit KYCApproved(user, region, block.timestamp + validFor, documentHash, riskLevel);
    }

    function rejectKYC(address user) external onlyRole(ISSUER_ROLE) whenNotPaused {
        require(kycRecords[user].status == KYCStatus.Pending, "Not pending");
        kycRecords[user].status = KYCStatus.Rejected;
        emit KYCRejected(user);
    }

    function revokeKYC(address user) external onlyRole(ISSUER_ROLE) whenNotPaused {
        require(kycRecords[user].status == KYCStatus.Approved, "Not approved");
        kycRecords[user].status = KYCStatus.Rejected;
        emit KYCRevoked(user);
    }

    // Batch approve function for institutions
    function batchApproveKYC(
        address[] calldata users,
        string[] calldata regions,
        uint256[] calldata validityPeriods,
        bytes32[] calldata documentHashes,
        uint256[] calldata riskLevels
    ) external onlyRole(ISSUER_ROLE) whenNotPaused {
        if(users.length > MAX_BATCH_SIZE) revert BatchTooLarge();
        if(users.length == 0) revert EmptyBatch();
        if(
            regions.length != users.length ||
            validityPeriods.length != users.length ||
            documentHashes.length != users.length ||
            riskLevels.length != users.length
        ) revert ArrayLengthMismatch();
        
        for(uint256 i = 0; i < users.length; i++) {
            _approveKYC(
                users[i],
                regions[i],
                validityPeriods[i],
                documentHashes[i],
                riskLevels[i]
            );
        }
    }

    function setInstitution(
        string calldata institutionId,
        bool active,
        uint256 dailyLimit
    ) external onlyRole(DEFAULT_ADMIN_ROLE) {
        institutions[institutionId] = Institution({
            active: active,
            dailyLimit: dailyLimit,
            usedToday: 0,
            lastResetDay: block.timestamp / 1 days
        });
        emit InstitutionUpdated(institutionId, active, dailyLimit);
    }

    // Emergency pause
    function pause() external onlyRole(EMERGENCY_ROLE) {
        _pause();
    }

    function unpause() external onlyRole(EMERGENCY_ROLE) {
        _unpause();
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

    function _authorizeUpgrade(address newImplementation) internal override onlyRole(DEFAULT_ADMIN_ROLE) {
        require(msg.sender == address(this), "Only through timelock");
        emit TimelockActionScheduled(keccak256(abi.encode(newImplementation)), newImplementation);
    }
}
