// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Initializable } from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import { UUPSUpgradeable } from "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";
import { OwnableUpgradeable } from "@openzeppelin/contracts-upgradeable/access/OwnableUpgradeable.sol";
import { PausableUpgradeable } from "@openzeppelin/contracts-upgradeable/utils/PausableUpgradeable.sol";
import { AccessControlUpgradeable } from "@openzeppelin/contracts-upgradeable/access/AccessControlUpgradeable.sol";
import { ITemporalGradientCore } from "./interfaces/ITemporalGradientCore.sol";

contract TemporalGradientCore is
    Initializable,
    OwnableUpgradeable,
    PausableUpgradeable,
    UUPSUpgradeable,
    AccessControlUpgradeable,
    ITemporalGradientCore
{
    bytes32 public constant GOVERNANCE_ROLE = keccak256("GOVERNANCE_ROLE");
    bytes32 public constant UPGRADER_ROLE = keccak256("UPGRADER_ROLE");
    uint256 public constant OUTPUT_HISTORY_SIZE = 32;

    bytes32 public constant MINING_MODULE = keccak256("MINING_MODULE");
    bytes32 public constant BATCH_MINING_MODULE = keccak256("BATCH_MINING_MODULE");
    bytes32 public constant RANDOMNESS_MODULE = keccak256("RANDOMNESS_MODULE");
    bytes32 public constant TOKENOMICS_MODULE = keccak256("TOKENOMICS_MODULE");
    bytes32 public constant GOVERNANCE_MODULE = keccak256("GOVERNANCE_MODULE");
    bytes32 public constant RATE_LIMIT_MODULE = keccak256("RATE_LIMIT_MODULE");

    bytes32[OUTPUT_HISTORY_SIZE] public outputHistory;
    uint64 public currentOutputIndex;
    uint64 public lastOutputTimestamp;
    bytes32 public genesisOutput;

    mapping(bytes32 => address) private modules;
    mapping(address => bool) private moduleEnabled;

    event ModuleUpdated(bytes32 indexed moduleId, address indexed previousModule, address indexed newModule);
    event CoreOutputRecorded(bytes32 indexed newOutput, address indexed miner, uint8 indexed poolId, uint256 reward, uint64 nonce);
    event GenesisOutputInitialized(bytes32 indexed genesisOutput, uint64 timestamp);

    error ZeroAddress();
    error InvalidModule();
    error NotModule();
    error ZeroOutput();

    modifier onlyModule() {
        if (!moduleEnabled[msg.sender]) revert NotModule();
        _;
    }

    function initialize(address admin, bytes32 initialGenesisOutput) external initializer {
        if (admin == address(0)) revert ZeroAddress();

        __Ownable_init(admin);
        __Pausable_init();
        __AccessControl_init();

        _grantRole(DEFAULT_ADMIN_ROLE, admin);
        _grantRole(GOVERNANCE_ROLE, admin);
        _grantRole(UPGRADER_ROLE, admin);

        bytes32 genesis = initialGenesisOutput == bytes32(0)
            ? keccak256(abi.encodePacked("TEMPORAL_GRADIENT_CORE", admin, block.timestamp, block.prevrandao))
            : initialGenesisOutput;

        genesisOutput = genesis;
        outputHistory[0] = genesis;
        for (uint256 i = 1; i < OUTPUT_HISTORY_SIZE; i++) {
            outputHistory[i] = genesis;
        }
        lastOutputTimestamp = uint64(block.timestamp);
        emit GenesisOutputInitialized(genesis, uint64(block.timestamp));
    }

    function setModule(bytes32 moduleId, address module) external onlyRole(GOVERNANCE_ROLE) {
        if (module == address(0)) revert ZeroAddress();
        if (
            moduleId != MINING_MODULE &&
            moduleId != BATCH_MINING_MODULE &&
            moduleId != RANDOMNESS_MODULE &&
            moduleId != TOKENOMICS_MODULE &&
            moduleId != GOVERNANCE_MODULE &&
            moduleId != RATE_LIMIT_MODULE
        ) revert InvalidModule();

        address previous = modules[moduleId];
        if (previous != address(0)) {
            moduleEnabled[previous] = false;
        }

        modules[moduleId] = module;
        moduleEnabled[module] = true;
        emit ModuleUpdated(moduleId, previous, module);
    }

    function moduleAddress(bytes32 moduleId) external view returns (address) {
        return modules[moduleId];
    }

    function isModule(address account) external view returns (bool) {
        return moduleEnabled[account];
    }

    function isPaused() external view returns (bool) {
        return paused();
    }

    function hasRole(bytes32 role, address account)
        public
        view
        override(ITemporalGradientCore, AccessControlUpgradeable)
        returns (bool)
    {
        return super.hasRole(role, account);
    }

    function outputHistoryAt(uint256 index) external view returns (bytes32) {
        return outputHistory[index];
    }

    function getOutputHistory() external view returns (bytes32[32] memory history) {
        for (uint256 i = 0; i < OUTPUT_HISTORY_SIZE; i++) {
            history[i] = outputHistory[i];
        }
    }

    function getCurrentOutputIndex() external view returns (uint64) {
        return currentOutputIndex;
    }

    function recordMinedOutput(
        bytes32 newOutput,
        address miner,
        uint8 poolId,
        uint256 reward,
        uint64 nonce
    ) external onlyModule whenNotPaused {
        if (newOutput == bytes32(0)) revert ZeroOutput();

        currentOutputIndex = uint64((currentOutputIndex + 1) % OUTPUT_HISTORY_SIZE);
        outputHistory[currentOutputIndex] = newOutput;
        lastOutputTimestamp = uint64(block.timestamp);

        emit CoreOutputRecorded(newOutput, miner, poolId, reward, nonce);
    }

    function pause() external onlyRole(GOVERNANCE_ROLE) {
        _pause();
    }

    function unpause() external onlyRole(GOVERNANCE_ROLE) {
        _unpause();
    }

    function _authorizeUpgrade(address) internal override onlyRole(UPGRADER_ROLE) {}

    constructor() {
    
    }
}
