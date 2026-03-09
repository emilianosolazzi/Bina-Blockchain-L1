// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

interface IRandomnessModule {
    function requestRandomness(bytes32 userSeed) external returns (uint256 requestId);
    function contributeEntropy(uint256 requestId, bytes32 entropyContribution) external;
    function getRandomResult(uint256 requestId) external view returns (bytes32);
}
