// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ITokenomicsModule } from "../../contracts/interfaces/ITokenomicsModule.sol";

contract MockTokenomicsModule is ITokenomicsModule {
    uint256 public rewardToReturn = 5 ether;
    uint256 public staleRewardToReturn = 2 ether;
    uint256 public minedCallCount;
    uint256 public staleRewardCallCount;

    address public lastMiner;
    bytes32 public lastOutput;
    uint8 public lastPoolId;
    uint256 public lastPoolTargetDifficulty;
    uint256 public lastPoolTotalMined;
    uint256 public lastPoolEmissionBucket;
    address public lastStaleRecipient;
    uint256 public lastRequestedStaleReward;

    function setReward(uint256 newReward) external {
        rewardToReturn = newReward;
    }

    function setStaleReward(uint256 newReward) external {
        staleRewardToReturn = newReward;
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

    function onStaleBlockReward(address recipient, uint256 requestedReward) external returns (uint256 actualReward) {
        staleRewardCallCount++;
        lastStaleRecipient = recipient;
        lastRequestedStaleReward = requestedReward;
        return staleRewardToReturn > requestedReward ? requestedReward : staleRewardToReturn;
    }
}