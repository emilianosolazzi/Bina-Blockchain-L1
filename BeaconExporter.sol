// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

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
    bytes32 public constant UPGRADER_ROLE = keccak256("UPGRADER_ROLE");
    bytes32 public constant EXPORTER_ROLE = keccak256("EXPORTER_ROLE");

    address public beaconAddress;
    address public bridgeAddress;
    address public tgbtTokenAddress;

    // --- Versioning ---
    uint256 public constant VERSION = 1;

    // --- Storage gap for future upgrades ---
    uint256[49] private __gap;

    function initialize(address _beaconAddress, address _bridgeAddress, address _tgbtTokenAddress) public initializer {
        __Ownable2Step_init();
        __UUPSUpgradeable_init();
        __ReentrancyGuard_init();
        __AccessControl_init();
        __Pausable_init();

        require(_beaconAddress != address(0), "Zero beacon address");
        require(_bridgeAddress != address(0), "Zero bridge address");
        require(_tgbtTokenAddress != address(0), "Zero token address");

        beaconAddress = _beaconAddress;
        bridgeAddress = _bridgeAddress;
        tgbtTokenAddress = _tgbtTokenAddress;

        _grantRole(DEFAULT_ADMIN_ROLE, msg.sender);
        _grantRole(UPGRADER_ROLE, msg.sender);
        _grantRole(EXPORTER_ROLE, msg.sender);
    }

    function exportRandomness(bytes32 randomness, uint256 destinationChainId, address destinationAddress) external onlyRole(EXPORTER_ROLE) whenNotPaused nonReentrant {
        require(destinationAddress != address(0), "Zero destination address");

        // Burn 1% of total TGBT balance of this contract during bridge
        uint256 balance = IERC20Upgradeable(tgbtTokenAddress).balanceOf(address(this));
        if (balance > 0) {
            uint256 burnAmount = balance / 100; // 1% burn
            if (burnAmount > 0) {
                IERC20Upgradeable(tgbtTokenAddress).transfer(address(0xdead), burnAmount); // Burn by sending to dead address
            }
        }

        // Implement bridge logic here (emit an event or interact with bridge contract)
    }

    function setBeaconAddress(address newBeaconAddress) external onlyRole(DEFAULT_ADMIN_ROLE) {
        require(newBeaconAddress != address(0), "Zero address");
        beaconAddress = newBeaconAddress;
    }

    function setBridgeAddress(address newBridgeAddress) external onlyRole(DEFAULT_ADMIN_ROLE) {
        require(newBridgeAddress != address(0), "Zero address");
        bridgeAddress = newBridgeAddress;
    }

    function setTGBTTokenAddress(address newTokenAddress) external onlyRole(DEFAULT_ADMIN_ROLE) {
        require(newTokenAddress != address(0), "Zero address");
        tgbtTokenAddress = newTokenAddress;
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

    function _authorizeUpgrade(address newImplementation) internal override onlyRole(UPGRADER_ROLE) {}

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor() {
        _disableInitializers();
    }
}
