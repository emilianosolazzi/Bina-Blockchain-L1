// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ERC20 } from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import { ERC20Burnable } from "@openzeppelin/contracts/token/ERC20/extensions/ERC20Burnable.sol";
import { AccessControl } from "@openzeppelin/contracts/access/AccessControl.sol";
import { Pausable } from "@openzeppelin/contracts/security/Pausable.sol";


/**
 * @title TGBT - Temporal Gradient Beacon Token
 * @notice ERC20 token with capped supply, halving emission, burnability, role-based minting, slashing, and controlled burning.
 */
contract TGBT is
    ERC20,
    ERC20Burnable,
    AccessControl,
    Pausable
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
    event SlasherAdded(address indexed slasher); 
    event SlasherRemoved(address indexed slasher);
    event BurnerAdded(address indexed burner); 
    event BurnerRemoved(address indexed burner); 

    /**
     * @notice Constructs the TGBT token.
     * @param _initialEmissionRate Starting reward emission rate
     * @param _halvingInterval Seconds between each emission halving
     * @param _initialController Address granted MINTER, SLASHER, and BURNER roles (e.g., TemporalGradientBeacon)
     */
    constructor(
        uint256 _initialEmissionRate,
        uint256 _halvingInterval,
        address _initialController
    ) ERC20("Temporal Gradient Beacon Token", "TGBT") {
        require(_initialController != address(0), "Zero controller address");
        require(_halvingInterval >= 365 days, "Halving too frequent");

        // Grant admin role to deployer for role management
        _grantRole(DEFAULT_ADMIN_ROLE, msg.sender);

        // Grant operational roles to the controller contract
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
     * @notice Automatically slashes tokens based on violation metrics
     * @dev Uses predefined rules to determine slash amounts for different violations
     * @param account The address whose tokens will be slashed
     * @param violationType The type of violation (mapped to predefined constants)
     * @param severity Severity level of the violation (1-100)
     * @return amountSlashed The amount that was actually slashed
     */
    function autoSlash(address account, uint8 violationType, uint8 severity) external onlyRole(SLASHER_ROLE) whenNotPaused returns (uint256 amountSlashed) {
        require(account != address(0), "Zero address");
        require(severity > 0 && severity <= 100, "Invalid severity");
        
        // Calculate slash amount based on violation type and severity
        // Different violation types can have different base penalties
        bytes32 reason;
        uint256 baseAmount;
        
        if (violationType == 1) { // Protocol rule violation
            baseAmount = 100 ether; // 100 tokens base penalty
            reason = keccak256("RULE_VIOLATION");
        } else if (violationType == 2) { // Invalid block submission
            baseAmount = 500 ether; // 500 tokens base penalty
            reason = keccak256("INVALID_BLOCK");
        } else if (violationType == 3) { // Malicious behavior
            baseAmount = 1000 ether; // 1000 tokens base penalty
            reason = keccak256("MALICIOUS");
        } else {
            revert("Unknown violation type");
        }
        
        // Scale by severity (1-100%)
        amountSlashed = (baseAmount * severity) / 100;
        
        // Cap at actual balance
        uint256 balance = balanceOf(account);
        if (amountSlashed > balance) {
            amountSlashed = balance;
        }
        
        if (amountSlashed > 0) {
            _burn(account, amountSlashed);
            emit TokensSlashed(msg.sender, account, amountSlashed, reason);
        }
        
        return amountSlashed;
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
    
    /**
     * @notice Automatically burn tokens based on predefined protocol rules
     * @dev Different rule types have different burn formulas
     * @param account The address whose tokens will be burned
     * @param ruleType The type of protocol rule triggering the burn
     * @param parameter Additional parameter specific to the rule type
     * @return amountBurned The amount that was actually burned
     */
    function autoBurn(address account, uint8 ruleType, uint256 parameter) external onlyRole(BURNER_ROLE) whenNotPaused returns (uint256 amountBurned) {
        require(account != address(0), "Zero address");
        
        bytes32 reason;
        uint256 burnAmount;
        
        if (ruleType == 1) { // Inactivity burn
            // Parameter = days of inactivity
            uint256 inactiveDays = parameter;
            require(inactiveDays > 30, "Inactivity period too short");
            
            // 1% burn per 30 days of inactivity beyond the first 30 days
            uint256 burnPercent = (inactiveDays - 30) / 30 + 1;
            if (burnPercent > 10) burnPercent = 10; // Cap at 10%
            
            burnAmount = (balanceOf(account) * burnPercent) / 100;
            reason = keccak256("INACTIVITY");
        } 
        else if (ruleType == 2) { // Missed contributions
            // Parameter = number of missed contributions
            uint256 missed = parameter;
            burnAmount = 5 ether * missed; // 5 tokens per missed contribution
            reason = keccak256("MISSED_CONTRIBUTIONS");
        }
        else {
            revert("Unknown rule type");
        }
        
        // Cap at actual balance
        uint256 balance = balanceOf(account);
        if (burnAmount > balance) {
            burnAmount = balance;
        }
        
        if (burnAmount > 0) {
            _burn(account, burnAmount);
            emit TokensBurnedByBeacon(msg.sender, account, burnAmount, reason);
        }
        
        return burnAmount;
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

}
