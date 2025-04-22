// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { IERC20Upgradeable } from "@openzeppelin/contracts-upgradeable/token/ERC20/IERC20Upgradeable.sol";
import { IERC20MetadataUpgradeable } from "@openzeppelin/contracts-upgradeable/token/ERC20/extensions/IERC20MetadataUpgradeable.sol";
import { IERC20PermitUpgradeable } from "@openzeppelin/contracts-upgradeable/token/ERC20/extensions/IERC20PermitUpgradeable.sol";

/**
 * @title ITGBT - Interface for Temporal Gradient Beacon Token
 * @notice Defines the complete interface for interacting with the TGBT token
 */
interface ITGBT is IERC20Upgradeable, IERC20MetadataUpgradeable, IERC20PermitUpgradeable {
    // --- Constants ---
    
    /**
     * @notice Maximum token supply cap (2 billion TGBT)
     * @return The maximum possible supply of TGBT tokens
     */
    function MAX_SUPPLY() external view returns (uint256);
    
    /**
     * @notice Role identifier for accounts authorized to mint tokens
     * @return The keccak256 hash of "MINTER_ROLE"
     */
    function MINTER_ROLE() external view returns (bytes32);
    
    // --- Emission Parameters ---
    
    /**
     * @notice Current emission rate for token rewards
     * @return The current rate at which new tokens are minted
     */
    function emissionRate() external view returns (uint256);
    
    /**
     * @notice Time interval between emission halvings in seconds
     * @return The number of seconds between each emission rate reduction
     */
    function halvingInterval() external view returns (uint256);
    
    /**
     * @notice Timestamp of the last emission halving event
     * @return The Unix timestamp when the emission rate was last reduced
     */
    function lastHalvingTimestamp() external view returns (uint256);
    
    // --- Minting Functions ---
    
    /**
     * @notice Mints new tokens to the specified address
     * @dev Restricted to accounts with MINTER_ROLE
     * @param to Address receiving the minted tokens
     * @param amount Amount of tokens to mint
     */
    function mint(address to, uint256 amount) external;
    
    // --- Admin Functions ---
    
    /**
     * @notice Adds a new minter to the protocol
     * @dev Restricted to DEFAULT_ADMIN_ROLE
     * @param minter Address to grant minting privileges
     */
    function addMinter(address minter) external;
    
    /**
     * @notice Removes minting privileges from an address
     * @dev Restricted to DEFAULT_ADMIN_ROLE
     * @param minter Address to revoke minting privileges from
     */
    function removeMinter(address minter) external;
    
    /**
     * @notice Updates the emission rate for token rewards
     * @dev Restricted to DEFAULT_ADMIN_ROLE
     * @param rate New emission rate value
     */
    function setEmissionRate(uint256 rate) external;
    
    /**
     * @notice Pauses all token transfers and minting
     * @dev Restricted to DEFAULT_ADMIN_ROLE
     */
    function pause() external;
    
    /**
     * @notice Unpauses token transfers and minting
     * @dev Restricted to DEFAULT_ADMIN_ROLE
     */
    function unpause() external;
    
    // --- Burning Functions ---
    
    /**
     * @notice Burns tokens from the caller's balance
     * @param amount Amount of tokens to burn
     */
    function burn(uint256 amount) external;
    
    /**
     * @notice Burns tokens from a specified account (with allowance)
     * @param account Address to burn tokens from
     * @param amount Amount of tokens to burn
     */
    function burnFrom(address account, uint256 amount) external;
    
    // --- View Functions ---
    
    /**
     * @notice Returns the current emission reward after halving calculations
     * @return Current reward amount for minting
     */
    function getMintableReward() external view returns (uint256);
    
    /**
     * @notice Returns the timestamp when the next emission halving will occur
     * @return Unix timestamp of the next scheduled halving
     */
    function getNextHalvingTime() external view returns (uint256);
    
    /**
     * @notice Returns the number of tokens that can still be minted
     * @return Amount of tokens available to mint before reaching MAX_SUPPLY
     */
    function availableToMint() external view returns (uint256);
    
    /**
     * @notice Checks if an account has the minter role
     * @param account Address to check
     * @return True if the account has minting privileges
     */
    function hasRole(bytes32 role, address account) external view returns (bool);
    
    // --- Events ---
    
    /**
     * @notice Emitted when the emission rate is halved
     * @param newRate The updated emission rate
     * @param timestamp When the halving occurred
     */
    event EmissionHalved(uint256 newRate, uint256 timestamp);
    
    /**
     * @notice Emitted when a new minter is added
     * @param minter Address that received minting privileges
     */
    event MinterAdded(address indexed minter);
    
    /**
     * @notice Emitted when a minter is removed
     * @param minter Address that lost minting privileges
     */
    event MinterRemoved(address indexed minter);
}
