// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

interface IBeaconExporter {
    // Core types
    enum SubscriptionTier { None, Basic, Premium, Enterprise }

    struct TierBenefits {
        uint256 priceDiscount;
        uint256 priorityLevel;
        bool batchingEnabled;
        uint256 destinationChains;
    }

    // Custom errors
    error NotSubscribed();
    error InsufficientQuota();
    error InvalidTier();
    error QuotaExceeded();
    error UnsupportedChain();
    error InvalidDestination();

    struct ExportRequest {
        uint256 chainId;
        address destination;
        bytes32 userSeed;
        bool emergency;
    }

    // Main functions
    function subscribe() external payable;
    function subscribeTier(SubscriptionTier tier) external payable;
    function requestExport(uint256 destinationChainId, address destinationAddress) external;
    
    // Enhanced functions
    function getSubscriptionStatus(address user) external view returns (
        bool active,
        uint256 remainingQuota,
        uint256 expiryTime,
        SubscriptionTier tier
    );

    function isChainSupported(uint256 chainId) external view returns (bool);
    
    function batchExport(ExportRequest[] calldata requests) external returns (bool[] memory success);
    
    function emergencyExport(
        uint256 chainId, 
        address destination, 
        bytes32 userSeed
    ) external payable returns (bytes32 requestId);

    function remainingQuota(address user) external view returns (uint256);

    // View functions
    function getChainQuota(uint256 chainId) external view returns (uint256);
    function userTier(address user) external view returns (SubscriptionTier);
    function tierBenefits(SubscriptionTier tier) external view returns (TierBenefits memory);
    function subscriptionFee() external view returns (uint256);

    // Events
    event RandomnessExported(bytes32 indexed randomness, uint256 destinationChainId, address destinationAddress, uint256 burnAmount);
    event NewSubscription(address indexed user);
    event QuotaUpdated(address indexed user, uint256 newQuota);
    event EmergencyExport(bytes32 indexed requestId, address indexed user);
    event ChainSupported(uint256 indexed chainId, bool supported);
}
