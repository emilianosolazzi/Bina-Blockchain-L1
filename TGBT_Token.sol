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
 * @notice Upgradeable ERC20 token with capped supply, halving emission, burnability, and role-based minting.
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
    uint256 public constant MAX_SUPPLY = 2_000_000_000 ether;
    bytes32 public constant MINTER_ROLE = keccak256("MINTER_ROLE");

    // --- Emission ---
    uint256 public emissionRate;
    uint256 public halvingInterval;
    uint256 public lastHalvingTimestamp;

    // --- Events ---
    event EmissionHalved(uint256 newRate, uint256 timestamp);
    event MinterAdded(address indexed minter);
    event MinterRemoved(address indexed minter);

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor() {
        _disableInitializers();
    }

    /**
     * @notice Initializes the token.
     * @param _initialEmissionRate Starting reward emission rate
     * @param _halvingInterval Seconds between each emission halving
     * @param _initialMinter First contract granted MINTER_ROLE (e.g., TemporalGradientBeacon)
     */
    function initialize(
        uint256 _initialEmissionRate,
        uint256 _halvingInterval,
        address _initialMinter
    ) public initializer {
        __ERC20_init("Temporal Gradient Beacon Token", "TGBT");
        __ERC20Burnable_init();
        __AccessControl_init();
        __Pausable_init();
        __Ownable2Step_init();
        __UUPSUpgradeable_init();

        require(_initialMinter != address(0), "Zero minter");
        require(_halvingInterval >= 365 days, "Halving too frequent");

        _grantRole(DEFAULT_ADMIN_ROLE, msg.sender);
        _grantRole(MINTER_ROLE, _initialMinter);

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
