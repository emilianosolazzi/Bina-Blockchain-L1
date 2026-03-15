// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

interface IMiningModule {
    function submitMiningCommitment(
        bytes32 commitHash,
        uint8 poolId,
        uint256 nonce,
        uint256 deadline,
        bytes calldata signature
    ) external;

    function revealMiningCommitment(
        bytes32 previousOutput,
        bytes calldata temporalSeed,
        uint64 nonce,
        bytes calldata signature,
        bytes32 secretValue,
        uint8 poolId
    ) external;

    function getMiningChallenge(uint8 poolId) external view returns (bytes32[] memory outputs, uint256 difficulty);

    function nonces(address miner) external view returns (uint256);

    function minCommitmentAge() external view returns (uint8);
}
