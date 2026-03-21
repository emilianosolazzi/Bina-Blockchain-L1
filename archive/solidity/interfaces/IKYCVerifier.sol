// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title IKYCVerifier
 * @notice Interface for KYC verification and management
 */
interface IKYCVerifier {
    // Constants that should be exposed
    uint256 constant MAX_BATCH_SIZE = 50;
    uint256 constant REQUEST_COOLDOWN = 1 days;
    uint256 constant MAX_RISK_LEVEL = 100;
    bytes32 constant ISSUER_ROLE = keccak256("ISSUER_ROLE");
    bytes32 constant EMERGENCY_ROLE = keccak256("EMERGENCY_ROLE");

    // Structs
    struct KYCData {
        uint8 status;      // Using uint8 instead of enum for interface compatibility
        uint256 issuedAt;
        uint256 expiresAt;
        string region;
        string institutionId;
        bytes32 documentHash;
        uint256 riskLevel;
        string[] approvalTypes;
    }

    // Additional institution struct for view functions
    struct InstitutionView {
        bool active;
        uint256 dailyLimit;
        uint256 usedToday;
        uint256 lastResetDay;
    }

    // Errors
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

    // Additional helpful errors
    error InvalidInstitutionId();
    error InvalidDailyLimit();
    error InvalidTimelock();
    error ExpiryTooLong();

    // Events
    event KYCRequested(address indexed user, string institutionId);
    event KYCApproved(address indexed user, string region, uint256 expiresAt, bytes32 documentHash, uint256 riskLevel);
    event KYCRejected(address indexed user);
    event KYCRevoked(address indexed user);
    event InstitutionUpdated(string indexed institutionId, bool active, uint256 dailyLimit);
    event TimelockActionScheduled(bytes32 indexed actionId, address target);

    // Additional helpful events
    event KYCBatchProcessed(uint256 batchSize, uint256 successCount);
    event InstitutionLimitUpdated(string indexed institutionId, uint256 oldLimit, uint256 newLimit);

    // === User Management ===
    // User Functions
    function requestKYC() external;
    
    // === Institution Management ===
    // Issuer Functions
    function approveKYC(address user, string calldata region, uint256 validFor) external;
    function rejectKYC(address user) external;
    function revokeKYC(address user) external;
    
    // === Batch Operations ===
    function batchApproveKYC(
        address[] calldata users,
        string[] calldata regions,
        uint256[] calldata validityPeriods,
        bytes32[] calldata documentHashes,
        uint256[] calldata riskLevels
    ) external;

    // Admin Functions
    function setInstitution(string calldata institutionId, bool active, uint256 dailyLimit) external;
    function pause() external;
    function unpause() external;

    // === Additional View Functions ===
    // View Functions
    function getKYCData(address user) external view returns (KYCData memory);
    function isKYCApproved(address user) external view returns (bool);
    
    /// @notice Get institution details
    function getInstitution(string calldata institutionId) external view returns (InstitutionView memory);
    
    /// @notice Get remaining daily limit for institution
    function getRemainingDailyLimit(string calldata institutionId) external view returns (uint256);
    
    /// @notice Check if user can request KYC (cooldown elapsed)
    function canRequestKYC(address user) external view returns (bool);
    
    /// @notice Get total verified users count
    function getTotalVerifiedUsers() external view returns (uint256);
}
