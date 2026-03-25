// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

// Legacy reference moved from archive for L3 bridge/export design work.
// Not production-ready in its current form; requires refactor before any Orbit use.

import { Initializable } from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import { UUPSUpgradeable } from "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";
import { Ownable2StepUpgradeable } from "@openzeppelin/contracts-upgradeable/access/Ownable2StepUpgradeable.sol";
import { ReentrancyGuardUpgradeable } from "@openzeppelin/contracts-upgradeable/security/ReentrancyGuardUpgradeable.sol";
import { AccessControlUpgradeable } from "@openzeppelin/contracts-upgradeable/access/AccessControlUpgradeable.sol";
import { PausableUpgradeable } from "@openzeppelin/contracts-upgradeable/security/PausableUpgradeable.sol";
import { IERC20Upgradeable } from "@openzeppelin/contracts-upgradeable/token/ERC20/IERC20Upgradeable.sol";

contract BeaconExporter is
    Initializable,
    UUPSUpgradeable,
    Ownable2StepUpgradeable,
    ReentrancyGuardUpgradeable,
    AccessControlUpgradeable,
    PausableUpgradeable
{
    // Custom errors
    error ZeroAddress();
    error ZeroDestination();
    error BurnFailed();
    error InvalidBridgeCall();
    error ExceedsMaxTransfer();
    error InvalidBurnRate();
    error InvalidTimePeriod();
    error RateLimitExceeded();
    error NotBridge();
    error UpgradeGracePeriodNotOver();
    error InvalidRandomnessSource();
    error InvalidRandomnessAge();
    error InvalidSignature();
    error InvalidChainQuota();
    error NotSubscribed();
    error InsufficientQuota();
    error InvalidFeeConfig();

    // Events
    event RandomnessExported(bytes32 indexed randomness, uint256 destinationChainId, address destinationAddress, uint256 burnAmount);
    event BeaconAddressUpdated(address indexed oldAddress, address indexed newAddress);
    event BridgeAddressUpdated(address indexed oldAddress, address indexed newAddress);
    event TokenAddressUpdated(address indexed oldAddress, address indexed newAddress);
    event BridgeLimitUpdated(uint256 oldLimit, uint256 newLimit);
    event BurnRateUpdated(uint256 oldRate, uint256 newRate);
    event RateLimitUpdated(uint256 oldLimit, uint256 newLimit);
    event ChainQuotaUpdated(uint256 indexed chainId, uint256 oldLimit, uint256 newLimit);
    event NewSubscription(address indexed user);
    event QuotaUpdated(address indexed user, uint256 amount);
    event FeeCollected(address indexed collector, uint256 amount);

    bytes32 public constant UPGRADER_ROLE = keccak256("UPGRADER_ROLE");
    bytes32 public constant EXPORTER_ROLE = keccak256("EXPORTER_ROLE");

    address public beaconAddress;
    address public bridgeAddress;
    address public tgbtTokenAddress;

    // Bridge configuration
    uint256 public maxTransferAmount;
    uint256 public burnRateBps; // Configurable burn rate in basis points
    uint256 public dailyLimit;
    uint256 public usedToday;
    uint256 public lastResetDay;
    uint256 public upgradeGracePeriod;
    uint256 public lastUpgradeTimestamp;
    uint256 public maxRandomnessAge;

    // Verification state
    mapping(bytes32 => bool) public usedRandomness;
    mapping(uint256 => mapping(address => uint256)) public chainQuota;

    // Add multi-bridge support
    mapping(address => bool) public authorizedBridges;
    
    // Add verification methods
    mapping(uint256 => bytes32) public chainEndpoints;

    // Add user subscription state
    mapping(address => bool) public subscribers;
    mapping(address => uint256) public userQuota;
    uint256 public subscriptionFee;

    // Add subscription tiers
    enum SubscriptionTier { None, Basic, Premium, Enterprise }
    
    struct TierConfig {
        uint256 fee;
        uint256 quota;
        uint256 maxDestinations;
        bool customQuota;
    }

    mapping(address => SubscriptionTier) public userTier;
    mapping(SubscriptionTier => TierConfig) public tierConfigs;

    // Add tier benefits
    struct TierBenefits {
        uint256 priceDiscount;    // Basis points (100 = 1%)
        uint256 priorityLevel;     // Higher = faster processing
        bool batchingEnabled;      // Can submit multiple requests
        uint256 destinationChains; // Number of supported chains
    }

    mapping(SubscriptionTier => TierBenefits) public tierBenefits;

    // Constants
    uint256 private constant MAX_BURN_RATE_BPS = 1000; // Max 10%
    uint256 private constant BPS = 10000;
    uint256 private constant MIN_GRACE_PERIOD = 2 days;

    // --- Versioning ---
    uint256 public constant VERSION = 1;

    // --- Storage gap for future upgrades ---
    uint256[43] private __gap;

    // Fee configuration
    address public feeCollector;
    uint256 public protocolFeeBps = 8000; // 80% to protocol
    uint256 public burnFeeBps = 2000;     // 20% burned for tokenomics

    function initialize(
        address _beaconAddress, 
        address _bridgeAddress, 
        address _tgbtTokenAddress
    ) public initializer {
        if (_beaconAddress == address(0) || 
            _bridgeAddress == address(0) || 
            _tgbtTokenAddress == address(0)) revert ZeroAddress();

        __Ownable2Step_init();
        __UUPSUpgradeable_init();
        __ReentrancyGuard_init();
        __AccessControl_init();
        __Pausable_init();

        beaconAddress = _beaconAddress;
        bridgeAddress = _bridgeAddress;
        tgbtTokenAddress = _tgbtTokenAddress;

        _grantRole(DEFAULT_ADMIN_ROLE, msg.sender);
        _grantRole(UPGRADER_ROLE, msg.sender);
        _grantRole(EXPORTER_ROLE, msg.sender);

        initializeTiers();
    }

    function exportRandomness(
        bytes32 randomness, 
        uint256 destinationChainId, 
        address destinationAddress,
        bytes32 sourceBlockHash,
        uint256 timestamp,
        bytes calldata signature
    ) external onlyRole(EXPORTER_ROLE) whenNotPaused nonReentrant {
        // Add multi-bridge support
        if (!authorizedBridges[msg.sender]) revert NotBridge();
        
        // Add endpoint verification
        if (chainEndpoints[destinationChainId] == bytes32(0)) revert("Chain not supported");

        // Verify randomness source
        if (!IBeacon(beaconAddress).verifyRandomness(randomness, sourceBlockHash, timestamp, signature))
            revert InvalidRandomnessSource();
        
        if (block.timestamp - timestamp > maxRandomnessAge)
            revert InvalidRandomnessAge();
            
        if (usedRandomness[randomness])
            revert("Already exported");
        usedRandomness[randomness] = true;

        if (destinationAddress == address(0)) revert ZeroDestination();
        if (msg.sender != bridgeAddress) revert NotBridge();

        // Check rate limits
        uint256 currentDay = block.timestamp / 1 days;
        if (currentDay > lastResetDay) {
            usedToday = 0;
            lastResetDay = currentDay;
        }
        if (usedToday >= dailyLimit) revert RateLimitExceeded();
        usedToday++;

        // Chain-specific quota
        uint256 chainDaily = chainQuota[destinationChainId][destinationAddress];
        if (chainDaily >= maxTransferAmount)
            revert ExceedsMaxTransfer();
        chainQuota[destinationChainId][destinationAddress] = chainDaily + 1;

        uint256 balance = IERC20Upgradeable(tgbtTokenAddress).balanceOf(address(this));
        uint256 burnAmount;

        if (balance > 0) {
            burnAmount = (balance * burnRateBps) / BPS;
            if (burnAmount > 0) {
                bool success = IERC20Upgradeable(tgbtTokenAddress).transfer(address(0xdead), burnAmount);
                if (!success) revert BurnFailed();
            }
        }

        emit RandomnessExported(randomness, destinationChainId, destinationAddress, burnAmount);
    }

    function setBeaconAddress(address newBeaconAddress) external onlyRole(DEFAULT_ADMIN_ROLE) {
        if (newBeaconAddress == address(0)) revert ZeroAddress();
        address oldAddress = beaconAddress;
        beaconAddress = newBeaconAddress;
        emit BeaconAddressUpdated(oldAddress, newBeaconAddress);
    }

    function setBridgeAddress(address newBridgeAddress) external onlyRole(DEFAULT_ADMIN_ROLE) {
        if (newBridgeAddress == address(0)) revert ZeroAddress();
        address oldAddress = bridgeAddress;
        bridgeAddress = newBridgeAddress;
        emit BridgeAddressUpdated(oldAddress, newBridgeAddress);
    }

    function setTGBTTokenAddress(address newTokenAddress) external onlyRole(DEFAULT_ADMIN_ROLE) {
        if (newTokenAddress == address(0)) revert ZeroAddress();
        address oldAddress = tgbtTokenAddress;
        tgbtTokenAddress = newTokenAddress;
        emit TokenAddressUpdated(oldAddress, newTokenAddress);
    }

    function setBurnRate(uint256 newRateBps) external onlyRole(DEFAULT_ADMIN_ROLE) {
        if (newRateBps > MAX_BURN_RATE_BPS) revert InvalidBurnRate();
        uint256 oldRate = burnRateBps;
        burnRateBps = newRateBps;
        emit BurnRateUpdated(oldRate, newRateBps);
    }

    function setDailyLimit(uint256 newLimit) external onlyRole(DEFAULT_ADMIN_ROLE) {
        uint256 oldLimit = dailyLimit;
        dailyLimit = newLimit;
        emit RateLimitUpdated(oldLimit, newLimit);
    }

    function setMaxTransferAmount(uint256 newLimit) external onlyRole(DEFAULT_ADMIN_ROLE) {
        uint256 oldLimit = maxTransferAmount;
        maxTransferAmount = newLimit;
        emit BridgeLimitUpdated(oldLimit, newLimit);
    }

    function setChainQuota(
        uint256 chainId,
        uint256 dailyLimit
    ) external onlyRole(DEFAULT_ADMIN_ROLE) {
        if (dailyLimit > maxTransferAmount) revert InvalidChainQuota();
        emit ChainQuotaUpdated(chainId, chainQuota[chainId][address(0)], dailyLimit);
        chainQuota[chainId][address(0)] = dailyLimit;
    }

    function resetChainQuota(uint256 chainId) external onlyRole(DEFAULT_ADMIN_ROLE) {
        delete chainQuota[chainId][address(0)];
        emit ChainQuotaUpdated(chainId, chainQuota[chainId][address(0)], 0);
    }

    function getChainQuota(uint256 chainId) external view returns (uint256) {
        return chainQuota[chainId][address(0)];
    }

    function pause() external onlyRole(DEFAULT_ADMIN_ROLE) {
        _pause();
    }

    function unpause() external onlyRole(DEFAULT_ADMIN_ROLE) {
        _unpause();
    }

    function getVersion() external pure returns (uint256) {
        return VERSION;
    }

    function _authorizeUpgrade(address newImplementation) internal override onlyRole(UPGRADER_ROLE) {
        if (block.timestamp < lastUpgradeTimestamp + upgradeGracePeriod) {
            revert UpgradeGracePeriodNotOver();
        }
        lastUpgradeTimestamp = block.timestamp;
        emit TimelockActionScheduled(keccak256(abi.encode(newImplementation)), newImplementation);
    }

    function addBridge(address bridge) external onlyRole(DEFAULT_ADMIN_ROLE) {
        authorizedBridges[bridge] = true;
    }
    
    function setChainEndpoint(uint256 chainId, bytes32 endpoint) external onlyRole(DEFAULT_ADMIN_ROLE) {
        chainEndpoints[chainId] = endpoint;
    }

    function subscribe() external payable {
        if(msg.value < subscriptionFee) revert("Insufficient fee");
        
        // Split fees between protocol and burn
        uint256 protocolFee = (msg.value * protocolFeeBps) / BPS;
        uint256 burnAmount = msg.value - protocolFee; // Rest is burned
        
        // Send protocol fee to collector
        (bool success, ) = feeCollector.call{value: protocolFee}("");
        require(success, "Fee transfer failed");
        
        // Burn portion through TGBT
        if(burnAmount > 0) {
            IERC20Upgradeable(tgbtTokenAddress).transfer(address(0xdead), burnAmount);
        }
        
        subscribers[msg.sender] = true;
        userQuota[msg.sender] = 100;
        emit NewSubscription(msg.sender);
        emit FeeCollected(feeCollector, protocolFee);
    }

    function subscribeTier(SubscriptionTier tier) external payable {
        if (msg.value < tierConfigs[tier].fee) revert("Insufficient fee");
        userTier[msg.sender] = tier;
        userQuota[msg.sender] = tierConfigs[tier].quota;
        emit NewSubscription(msg.sender);
    }

    function requestExport(
        uint256 destinationChainId,
        address destinationAddress
    ) external {
        if (!subscribers[msg.sender]) revert NotSubscribed();
        if (userQuota[msg.sender] == 0) revert InsufficientQuota();
        
        userQuota[msg.sender]--;
        // Queue request for EXPORTER_ROLE to process
    }

    function initializeTiers() internal {
        tierBenefits[SubscriptionTier.Basic] = TierBenefits({
            priceDiscount: 0,
            priorityLevel: 1,
            batchingEnabled: false,
            destinationChains: 1
        });
        
        tierBenefits[SubscriptionTier.Premium] = TierBenefits({
            priceDiscount: 1000, // 10% discount
            priorityLevel: 2,
            batchingEnabled: true,
            destinationChains: 3
        });
        
        tierBenefits[SubscriptionTier.Enterprise] = TierBenefits({
            priceDiscount: 2500, // 25% discount
            priorityLevel: 3,
            batchingEnabled: true,
            destinationChains: type(uint256).max // unlimited
        });
    }

    function setFeeCollector(address _collector) external onlyRole(DEFAULT_ADMIN_ROLE) {
        if(_collector == address(0)) revert ZeroAddress();
        feeCollector = _collector;
    }

    function setFeeConfig(uint256 _protocolFeeBps, uint256 _burnFeeBps) external onlyRole(DEFAULT_ADMIN_ROLE) {
        if(_protocolFeeBps + _burnFeeBps > BPS) revert InvalidFeeConfig();
        protocolFeeBps = _protocolFeeBps;
        burnFeeBps = _burnFeeBps;
    }

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor() {
        _disableInitializers();
    }
}
