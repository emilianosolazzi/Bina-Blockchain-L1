// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Test } from "forge-std/Test.sol";
import { TokenomicsModule } from "../../contracts/modules/TokenomicsModule.sol";

contract TokenomicsHandler is Test {
    TokenomicsModule internal immutable tokenomics;
    address[] internal recipients;

    constructor(TokenomicsModule _tokenomics) {
        tokenomics = _tokenomics;

        recipients.push(vm.addr(0x101));
        recipients.push(vm.addr(0x102));
        recipients.push(vm.addr(0x103));
    }

    function mineBlock(
        uint256 recipientSeed,
        uint256 outputSeed,
        uint256 poolDifficulty,
        uint256 poolTotalMined,
        uint256 poolEmissionBucket,
        uint256 blocksForward
    ) external {
        vm.roll(block.number + bound(blocksForward, 0, 2_000));

        address recipient = recipients[bound(recipientSeed, 0, recipients.length - 1)];
        poolDifficulty = bound(poolDifficulty, 1, type(uint128).max);
        poolEmissionBucket = bound(poolEmissionBucket, 0, tokenomics.MINING_ALLOCATION());
        poolTotalMined = bound(poolTotalMined, 0, poolEmissionBucket);

        tokenomics.onBlockMined(
            recipient,
            bytes32(outputSeed),
            0,
            poolDifficulty,
            poolTotalMined,
            poolEmissionBucket
        );
    }

    function rewardStaleEntropy(uint256 recipientSeed, uint256 requestedReward) external {
        address recipient = recipients[bound(recipientSeed, 0, recipients.length - 1)];
        requestedReward = bound(requestedReward, 0, tokenomics.STALE_BLOCK_ALLOCATION() * 2);
        tokenomics.onStaleBlockReward(recipient, requestedReward);
    }

    function recordMiss(uint256 recipientSeed) external {
        address recipient = recipients[bound(recipientSeed, 0, recipients.length - 1)];
        tokenomics.recordMissedContribution(recipient);
    }
}