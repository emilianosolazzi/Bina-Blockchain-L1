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

    /**
     * @notice Role identifier for accounts authorized to slash tokens
     * @return The keccak256 hash of "SLASHER_ROLE"
     */
    function SLASHER_ROLE() external view returns (bytes32);

    /**
     * @notice Role identifier for accounts authorized to burn tokens via protocol logic
     * @return The keccak256 hash of "BURNER_ROLE"
     */
    function BURNER_ROLE() external view returns (bytes32);
    
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
     * @notice Adds a new slasher to the protocol
     * @dev Restricted to DEFAULT_ADMIN_ROLE
     * @param slasher Address to grant slashing privileges
     */
    function addSlasher(address slasher) external;

    /**
     * @notice Removes slashing privileges from an address
     * @dev Restricted to DEFAULT_ADMIN_ROLE
     * @param slasher Address to revoke slashing privileges from
     */
    function removeSlasher(address slasher) external;

    /**
     * @notice Adds a new burner (for protocol burns) to the protocol
     * @dev Restricted to DEFAULT_ADMIN_ROLE
     * @param burner Address to grant protocol burning privileges
     */
    function addBurner(address burner) external;

    /**
     * @notice Removes protocol burning privileges from an address
     * @dev Restricted to DEFAULT_ADMIN_ROLE
     * @param burner Address to revoke protocol burning privileges from
     */
    function removeBurner(address burner) external;
    
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

    /**
     * @notice Burns tokens from a specified account as a penalty (slashing).
     * @dev Restricted to SLASHER_ROLE.
     * @param account The address whose tokens will be slashed.
     * @param amount The amount of tokens to slash.
     * @param reason A reason code or identifier for the slash.
     */
    function slash(address account, uint256 amount, bytes32 reason) external;

    /**
     * @notice Burns tokens from a specified account based on protocol rules (e.g., inactivity).
     * @dev Restricted to BURNER_ROLE.
     * @param account The address whose tokens will be burned.
     * @param amount The amount of tokens to burn.
     * @param reason A reason code or identifier for the burn.
     */
    function burnFromBeacon(address account, uint256 amount, bytes32 reason) external;
    
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

    /**
     * @notice Emitted when tokens are slashed from an account
     * @param slasher Address that initiated the slash
     * @param account Address whose tokens were slashed
     * @param amount Amount of tokens slashed
     * @param reason Reason code for the slash
     */
    event TokensSlashed(address indexed slasher, address indexed account, uint256 amount, bytes32 reason);

    /**
     * @notice Emitted when tokens are burned by the protocol (e.g., Beacon)
     * @param burner Address (likely the Beacon contract) that initiated the burn
     * @param account Address whose tokens were burned
     * @param amount Amount of tokens burned
     * @param reason Reason code for the burn
     */
    event TokensBurnedByBeacon(address indexed burner, address indexed account, uint256 amount, bytes32 reason);

    /**
     * @notice Emitted when a new slasher is added
     * @param slasher Address that received slashing privileges
     */
    event SlasherAdded(address indexed slasher);

    /**
     * @notice Emitted when a slasher is removed
     * @param slasher Address that lost slashing privileges
     */
    event SlasherRemoved(address indexed slasher);

    /**
     * @notice Emitted when a new burner (for protocol burns) is added
     * @param burner Address that received protocol burning privileges
     */
    event BurnerAdded(address indexed burner);

    /**
     * @notice Emitted when a burner (for protocol burns) is removed
     * @param burner Address that lost protocol burning privileges
     */
    event BurnerRemoved(address indexed burner);
}
