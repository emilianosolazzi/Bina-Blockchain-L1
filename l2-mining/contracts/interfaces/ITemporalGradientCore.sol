// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

interface ITemporalGradientCore {
    function outputHistoryAt(uint256 index) external view returns (bytes32);
    function getOutputHistory() external view returns (bytes32[32] memory history);
    function getCurrentOutputIndex() external view returns (uint64);
    function moduleAddress(bytes32 moduleId) external view returns (address);
    function isModule(address account) external view returns (bool);
    function isPaused() external view returns (bool);
    function hasRole(bytes32 role, address account) external view returns (bool);
    function recordMinedOutput(
        bytes32 newOutput,
        address miner,
        uint8 poolId,
        uint256 reward,
        uint64 nonce
    ) external;
}
