// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ITokenomicsModule } from "../../contracts/interfaces/ITokenomicsModule.sol";

contract MockTokenomicsModule is ITokenomicsModule {
    uint256 public rewardToReturn = 5 ether;
    uint256 public minedCallCount;

    address public lastMiner;
    bytes32 public lastOutput;
    uint8 public lastPoolId;
    uint256 public lastPoolTargetDifficulty;
    uint256 public lastPoolTotalMined;
    uint256 public lastPoolEmissionBucket;

    function setReward(uint256 newReward) external {
        rewardToReturn = newReward;
    }

    function onBlockMined(
        address miner,
        bytes32 output,
        uint8 poolId,
        uint256 poolTargetDifficulty,
        uint256 poolTotalMined,
        uint256 poolEmissionBucket
    ) external returns (uint256 reward) {
        minedCallCount++;
        lastMiner = miner;
        lastOutput = output;
        lastPoolId = poolId;
        lastPoolTargetDifficulty = poolTargetDifficulty;
        lastPoolTotalMined = poolTotalMined;
        lastPoolEmissionBucket = poolEmissionBucket;
        return rewardToReturn;
    }
}