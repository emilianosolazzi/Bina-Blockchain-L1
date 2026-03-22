// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";
import { Pausable } from "@openzeppelin/contracts/utils/Pausable.sol";
import { AccessControl } from "@openzeppelin/contracts/access/AccessControl.sol";
import { ITemporalGradientCore } from "./interfaces/ITemporalGradientCore.sol";

contract TemporalGradientCore is
    Ownable,
    Pausable,
    AccessControl,
    ITemporalGradientCore
{
    // Governance is role-based bootstrap authority, not a registered module.
    bytes32 public constant GOVERNANCE_ROLE = keccak256("GOVERNANCE_ROLE");
    uint256 public constant OUTPUT_HISTORY_SIZE = 32;

    bytes32 public constant MINING_MODULE = keccak256("MINING_MODULE");
    bytes32 public constant BATCH_MINING_MODULE = keccak256("BATCH_MINING_MODULE");
    bytes32 public constant RANDOMNESS_MODULE = keccak256("RANDOMNESS_MODULE");
    bytes32 public constant TOKENOMICS_MODULE = keccak256("TOKENOMICS_MODULE");
    bytes32 public constant RATE_LIMIT_MODULE = keccak256("RATE_LIMIT_MODULE");
    bytes32 public constant STALE_BLOCK_MODULE = keccak256("STALE_BLOCK_MODULE");

    bytes32[OUTPUT_HISTORY_SIZE] public outputHistory;
    uint64 public currentOutputIndex;
    uint64 public lastOutputTimestamp;
    bytes32 public genesisOutput;
    uint256 public moduleCount;
    uint256 public governanceRoleCount;
    uint256 public defaultAdminRoleCount;
    bool public modulesLocked;
    bool public governanceLocked;
    bool public pausePermanentlyDisabled;

    mapping(bytes32 => address) private modules;
    mapping(address => uint256) private moduleRefCount;

    event ModuleUpdated(bytes32 indexed moduleId, address indexed previousModule, address indexed newModule);
    event ModuleRemoved(bytes32 indexed moduleId, address indexed previousModule);
    event CoreOutputRecorded(bytes32 indexed newOutput, address indexed miner, uint8 indexed poolId, uint256 reward, uint64 nonce);
    event GenesisOutputInitialized(bytes32 indexed genesisOutput, uint64 timestamp);
    event ModuleRegistryLocked(uint256 moduleCount);
    event GovernanceLockedForever();
    event PauseDisabledForever();

    error ZeroAddress();
    error InvalidModule();
    error InvalidModuleId();
    error NotModule();
    error ZeroOutput();
    error ModulesAreLocked();
    error GovernanceIsLocked();
    error PauseDisabled();
    error NoModulesConfigured();
    error UnpauseBeforeFinalizing();
    error UnexpectedGovernanceTopology();

    modifier onlyModule() {
        if (moduleRefCount[msg.sender] == 0) revert NotModule();
        _;
    }

    constructor(address admin, bytes32 initialGenesisOutput) Ownable(admin) {
        if (admin == address(0)) revert ZeroAddress();

        _grantRole(DEFAULT_ADMIN_ROLE, admin);
        _grantRole(GOVERNANCE_ROLE, admin);
        defaultAdminRoleCount = 1;
        governanceRoleCount = 1;

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
        if (modulesLocked) revert ModulesAreLocked();
        if (moduleId == bytes32(0)) revert InvalidModuleId();
        if (module == address(0)) revert ZeroAddress();

        address previous = modules[moduleId];
        if (previous == module) {
            return;
        }

        if (previous != address(0)) {
            unchecked { --moduleRefCount[previous]; }
        } else {
            unchecked { ++moduleCount; }
        }

        modules[moduleId] = module;
        unchecked { ++moduleRefCount[module]; }
        emit ModuleUpdated(moduleId, previous, module);
    }

    function removeModule(bytes32 moduleId) external onlyRole(GOVERNANCE_ROLE) {
        if (modulesLocked) revert ModulesAreLocked();
        if (moduleId == bytes32(0)) revert InvalidModuleId();

        address previous = modules[moduleId];
        if (previous == address(0)) revert InvalidModule();

        delete modules[moduleId];
        unchecked {
            --moduleRefCount[previous];
            --moduleCount;
        }

        emit ModuleRemoved(moduleId, previous);
    }

    function lockModuleRegistry() external onlyRole(GOVERNANCE_ROLE) {
        if (modulesLocked) revert ModulesAreLocked();
        if (moduleCount == 0) revert NoModulesConfigured();

        modulesLocked = true;
        emit ModuleRegistryLocked(moduleCount);
    }

    function disablePauseForever() external onlyRole(GOVERNANCE_ROLE) {
        if (pausePermanentlyDisabled) revert PauseDisabled();
        if (paused()) revert UnpauseBeforeFinalizing();

        pausePermanentlyDisabled = true;
        emit PauseDisabledForever();
    }

    function ossify() external onlyRole(DEFAULT_ADMIN_ROLE) {
        if (moduleCount == 0) revert NoModulesConfigured();
        if (paused()) revert UnpauseBeforeFinalizing();
        if (
            defaultAdminRoleCount != 1 ||
            governanceRoleCount != 1 ||
            !hasRole(DEFAULT_ADMIN_ROLE, msg.sender) ||
            !hasRole(GOVERNANCE_ROLE, msg.sender)
        ) revert UnexpectedGovernanceTopology();

        if (!modulesLocked) {
            modulesLocked = true;
            emit ModuleRegistryLocked(moduleCount);
        }
        if (!pausePermanentlyDisabled) {
            pausePermanentlyDisabled = true;
            emit PauseDisabledForever();
        }
        governanceLocked = true;
        emit GovernanceLockedForever();

        _revokeRole(GOVERNANCE_ROLE, msg.sender);
        _revokeRole(DEFAULT_ADMIN_ROLE, msg.sender);
        governanceRoleCount = 0;
        defaultAdminRoleCount = 0;
        renounceOwnership();
    }

    function moduleAddress(bytes32 moduleId) external view returns (address) {
        return modules[moduleId];
    }

    function isModule(address account) external view returns (bool) {
        return moduleRefCount[account] != 0;
    }

    function isPaused() external view returns (bool) {
        return paused();
    }

    function isOssified() external view returns (bool) {
        return modulesLocked && governanceLocked && pausePermanentlyDisabled;
    }

    function grantRole(bytes32 role, address account)
        public
        override
        onlyRole(getRoleAdmin(role))
    {
        if (governanceLocked && _isGovernanceRole(role)) revert GovernanceIsLocked();

        bool hadRole = hasRole(role, account);
        super.grantRole(role, account);
        if (!hadRole) {
            _trackRoleGrant(role);
        }
    }

    function revokeRole(bytes32 role, address account)
        public
        override
        onlyRole(getRoleAdmin(role))
    {
        if (governanceLocked && _isGovernanceRole(role)) revert GovernanceIsLocked();

        bool hadRole = hasRole(role, account);
        super.revokeRole(role, account);
        if (hadRole) {
            _trackRoleRevoke(role);
        }
    }

    function renounceRole(bytes32 role, address callerConfirmation) public override {
        if (governanceLocked && _isGovernanceRole(role)) revert GovernanceIsLocked();

        bool hadRole = hasRole(role, callerConfirmation);
        super.renounceRole(role, callerConfirmation);
        if (hadRole) {
            _trackRoleRevoke(role);
        }
    }

    function hasRole(bytes32 role, address account)
        public
        view
        override(ITemporalGradientCore, AccessControl)
        returns (bool)
    {
        return super.hasRole(role, account);
    }

    function outputHistoryAt(uint256 index) external view returns (bytes32) {
        return outputHistory[index];
    }

    function getOutputHistory() external view returns (bytes32[32] memory history) {
        for (uint256 i = 0; i < OUTPUT_HISTORY_SIZE;) {
            history[i] = outputHistory[i];
            unchecked { ++i; }
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

        currentOutputIndex = uint64((currentOutputIndex + 1) & 31); // OUTPUT_HISTORY_SIZE=32=2^5
        outputHistory[currentOutputIndex] = newOutput;
        lastOutputTimestamp = uint64(block.timestamp);

        emit CoreOutputRecorded(newOutput, miner, poolId, reward, nonce);
    }

    function pause() external onlyRole(GOVERNANCE_ROLE) {
        if (pausePermanentlyDisabled) revert PauseDisabled();
        _pause();
    }

    function unpause() external onlyRole(GOVERNANCE_ROLE) {
        _unpause();
    }

    function _isGovernanceRole(bytes32 role) private pure returns (bool) {
        return role == DEFAULT_ADMIN_ROLE || role == GOVERNANCE_ROLE;
    }

    function _trackRoleGrant(bytes32 role) private {
        if (role == DEFAULT_ADMIN_ROLE) {
            unchecked { ++defaultAdminRoleCount; }
        } else if (role == GOVERNANCE_ROLE) {
            unchecked { ++governanceRoleCount; }
        }
    }

    function _trackRoleRevoke(bytes32 role) private {
        if (role == DEFAULT_ADMIN_ROLE) {
            unchecked { --defaultAdminRoleCount; }
        } else if (role == GOVERNANCE_ROLE) {
            unchecked { --governanceRoleCount; }
        }
    }
}
