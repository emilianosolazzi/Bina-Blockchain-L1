// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ITemporalGradientCore } from "../interfaces/ITemporalGradientCore.sol";

abstract contract ModuleBase {
    bytes32 internal constant GOVERNANCE_ROLE = keccak256("GOVERNANCE_ROLE");

    ITemporalGradientCore public core;
    bool private _moduleInitialized;

    error OnlyCore();
    error OnlyCoreOrModule();
    error OnlyGovernance();
    error SystemPaused();
    error AlreadyInitialized();

    function __ModuleBase_init(address coreAddress) internal {
        if (_moduleInitialized) revert AlreadyInitialized();
        _moduleInitialized = true;
        core = ITemporalGradientCore(coreAddress);
    }

    modifier onlyCore() {
        if (msg.sender != address(core)) revert OnlyCore();
        _;
    }

    modifier onlyCoreOrModule() {
        if (msg.sender != address(core) && !core.isModule(msg.sender)) revert OnlyCoreOrModule();
        _;
    }

    modifier onlyGovernance() {
        if (!core.hasRole(GOVERNANCE_ROLE, msg.sender)) revert OnlyGovernance();
        _;
    }

    modifier whenSystemActive() {
        if (core.isPaused()) revert SystemPaused();
        _;
    }

    function _outputHistory() internal view returns (bytes32[32] memory) {
        return core.getOutputHistory();
    }

    function _currentOutputIndex() internal view returns (uint64) {
        return core.getCurrentOutputIndex();
    }

    function _module(bytes32 moduleId) internal view returns (address) {
        return core.moduleAddress(moduleId);
    }
}
