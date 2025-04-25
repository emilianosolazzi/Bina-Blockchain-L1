// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Initializable } from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import { ERC20Upgradeable } from "@openzeppelin/contracts-upgradeable/token/ERC20/ERC20Upgradeable.sol";
import { ERC20BurnableUpgradeable } from "@openzeppelin/contracts-upgradeable/token/ERC20/extensions/ERC20BurnableUpgradeable.sol";
import { AccessControlUpgradeable } from "@openzeppelin/contracts-upgradeable/access/AccessControlUpgradeable.sol";
import { PausableUpgradeable } from "@openzeppelin/contracts-upgradeable/security/PausableUpgradeable.sol";
import { Ownable2StepUpgradeable } from "@openzeppelin/contracts-upgradeable/access/Ownable2StepUpgradeable.sol";
import { UUPSUpgradeable } from "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";

/**
 * @title TGBT - Temporal Gradient Beacon Token
 * @notice Upgradeable ERC20 token with capped supply, halving emission, burnability, role-based minting, slashing, and controlled burning.
 */
contract TGBT is
    Initializable,
    ERC20Upgradeable,
    ERC20BurnableUpgradeable,
    AccessControlUpgradeable,
    PausableUpgradeable,
    Ownable2StepUpgradeable,
    UUPSUpgradeable
{
    // --- Constants ---
    // Consider reviewing MAX_SUPPLY for healthy tokenomics based on distribution and utility.
    uint256 public constant MAX_SUPPLY = 2_000_000_000 ether;
    bytes32 public constant MINTER_ROLE = keccak256("MINTER_ROLE");
    bytes32 public constant SLASHER_ROLE = keccak256("SLASHER_ROLE"); // Role for slashing tokens
    bytes32 public constant BURNER_ROLE = keccak256("BURNER_ROLE");   // Role for protocol-based burns (e.g., by Beacon)

    // --- Emission ---
    // Consider reviewing initial emissionRate and halvingInterval for sustainability.
    uint256 public emissionRate;
    uint256 public halvingInterval;
    uint256 public lastHalvingTimestamp;

    // --- Events ---
    event EmissionHalved(uint256 newRate, uint256 timestamp);
    event MinterAdded(address indexed minter);
    event MinterRemoved(address indexed minter);
    event TokensSlashed(address indexed slasher, address indexed account, uint256 amount, bytes32 reason); // Slashing event
    event TokensBurnedByBeacon(address indexed burner, address indexed account, uint256 amount, bytes32 reason); // Beacon burn event
    event SlasherAdded(address indexed slasher); // <<< Added event
    event SlasherRemoved(address indexed slasher); // <<< Added event
    event BurnerAdded(address indexed burner); // <<< Added event
    event BurnerRemoved(address indexed burner); // <<< Added event

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor() {
        _disableInitializers();
    }

    /**
     * @notice Initializes the token.
     * @param _initialEmissionRate Starting reward emission rate
     * @param _halvingInterval Seconds between each emission halving
     * @param _initialController Address granted MINTER, SLASHER, and BURNER roles (e.g., TemporalGradientBeacon)
     */
    function initialize(
        uint256 _initialEmissionRate,
        uint256 _halvingInterval,
        address _initialController // Renamed parameter for clarity
    ) public initializer {
        __ERC20_init("Temporal Gradient Beacon Token", "TGBT");
        __ERC20Burnable_init();
        __AccessControl_init();
        __Pausable_init();
        __Ownable2Step_init(); // Keep Ownable for upgrade authorization
        __UUPSUpgradeable_init();

        require(_initialController != address(0), "Zero controller address");
        require(_halvingInterval >= 365 days, "Halving too frequent"); // Example minimum interval

        // Grant admin role to deployer for role management
        _grantRole(DEFAULT_ADMIN_ROLE, msg.sender);

        // Grant operational roles to the controller contract (e.g., Beacon)
        _grantRole(MINTER_ROLE, _initialController);
        _grantRole(SLASHER_ROLE, _initialController);
        _grantRole(BURNER_ROLE, _initialController);

        emissionRate = _initialEmissionRate;
        halvingInterval = _halvingInterval;
        lastHalvingTimestamp = block.timestamp;
    }

    // --- Minting ---

    /**
     * @notice Mints tokens to an address. Restricted to MINTER_ROLE.
     */
    function mint(address to, uint256 amount) external onlyRole(MINTER_ROLE) whenNotPaused {
        _halvingCheck();
        require(totalSupply() + amount <= MAX_SUPPLY, "Cap exceeded");
        _mint(to, amount);
    }

    function _halvingCheck() internal {
        if (block.timestamp >= lastHalvingTimestamp + halvingInterval) {
            emissionRate = (emissionRate * 75) / 100; // Reduce by 25%
            lastHalvingTimestamp = block.timestamp;
            emit EmissionHalved(emissionRate, block.timestamp);
        }
    }

    // --- Burning & Slashing ---

    /**
     * @notice Burns tokens from a specified account as a penalty (slashing).
     * @dev Restricted to SLASHER_ROLE. Typically called by the main protocol contract (e.g., Beacon).
     * @param account The address whose tokens will be slashed.
     * @param amount The amount of tokens to slash.
     * @param reason A reason code or identifier for the slash.
     */
    function slash(address account, uint256 amount, bytes32 reason) external onlyRole(SLASHER_ROLE) whenNotPaused {
        require(account != address(0), "Zero address");
        require(amount > 0, "Amount must be positive");
        // The _burn function handles balance checks internally.
        _burn(account, amount);
        emit TokensSlashed(msg.sender, account, amount, reason);
    }

    /**
     * @notice Burns tokens from a specified account based on protocol rules (e.g., inactivity).
     * @dev Restricted to BURNER_ROLE. Typically called by the main protocol contract (e.g., Beacon).
     * @param account The address whose tokens will be burned.
     * @param amount The amount of tokens to burn.
     * @param reason A reason code or identifier for the burn.
     */
    function burnFromBeacon(address account, uint256 amount, bytes32 reason) external onlyRole(BURNER_ROLE) whenNotPaused {
        require(account != address(0), "Zero address");
        require(amount > 0, "Amount must be positive");
        // The _burn function handles balance checks internally.
        _burn(account, amount);
        emit TokensBurnedByBeacon(msg.sender, account, amount, reason);
    }

    // --- Admin Controls ---

    function addMinter(address minter) external onlyRole(DEFAULT_ADMIN_ROLE) {
        require(minter != address(0), "Zero address");
        _grantRole(MINTER_ROLE, minter);
        emit MinterAdded(minter);
    }

    function removeMinter(address minter) external onlyRole(DEFAULT_ADMIN_ROLE) {
        _revokeRole(MINTER_ROLE, minter);
        emit MinterRemoved(minter);
    }

    // Add functions to manage SLASHER_ROLE and BURNER_ROLE if needed
    function addSlasher(address slasher) external onlyRole(DEFAULT_ADMIN_ROLE) {
        require(slasher != address(0), "Zero address");
        _grantRole(SLASHER_ROLE, slasher);
        emit SlasherAdded(slasher); // <<< Emit event
    }

    function removeSlasher(address slasher) external onlyRole(DEFAULT_ADMIN_ROLE) {
        _revokeRole(SLASHER_ROLE, slasher);
        emit SlasherRemoved(slasher); // <<< Emit event
    }

     function addBurner(address burner) external onlyRole(DEFAULT_ADMIN_ROLE) {
        require(burner != address(0), "Zero address");
        _grantRole(BURNER_ROLE, burner);
        emit BurnerAdded(burner); // <<< Emit event
    }

    function removeBurner(address burner) external onlyRole(DEFAULT_ADMIN_ROLE) {
        _revokeRole(BURNER_ROLE, burner);
        emit BurnerRemoved(burner); // <<< Emit event
    }

    function setEmissionRate(uint256 rate) external onlyRole(DEFAULT_ADMIN_ROLE) {
        emissionRate = rate;
    }

    function pause() external onlyRole(DEFAULT_ADMIN_ROLE) {
        _pause();
    }

    function unpause() external onlyRole(DEFAULT_ADMIN_ROLE) {
        _unpause();
    }

    // --- External Views ---

    /**
     * @return Current emission reward (after halving)
     */
    function getMintableReward() external view returns (uint256) {
        return emissionRate;
    }

    /**
     * @return Timestamp of next emission halving
     */
    function getNextHalvingTime() external view returns (uint256) {
        return lastHalvingTimestamp + halvingInterval;
    }

    /**
     * @return Remaining tokens that can be minted before hitting MAX_SUPPLY
     */
    function availableToMint() external view returns (uint256) {
        return MAX_SUPPLY - totalSupply();
    }

    // --- UUPS Auth ---

    function _authorizeUpgrade(address newImplementation) internal override onlyOwner {}
}
