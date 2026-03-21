// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

interface ITokenomicsModule {
    function onBlockMined(
        address miner,
        bytes32 output,
        uint8 poolId,
        uint256 poolTargetDifficulty,
        uint256 poolTotalMined,
        uint256 poolEmissionBucket
    ) external returns (uint256 reward);

    function onStaleBlockReward(address recipient, uint256 requestedReward) external returns (uint256 actualReward);
}
