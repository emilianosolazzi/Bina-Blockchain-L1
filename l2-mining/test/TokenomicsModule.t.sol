// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Test } from "forge-std/Test.sol";
import { ERC1967Proxy } from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import { TemporalGradientCore } from "../contracts/TemporalGradientCore.sol";
import { TokenomicsModule } from "../contracts/modules/TokenomicsModule.sol";
import { MockProtocolToken } from "./mocks/MockProtocolToken.sol";

contract TokenomicsModuleTest is Test {
    bytes32 internal constant MINING_MODULE = keccak256("MINING_MODULE");
    bytes32 internal constant TOKENOMICS_MODULE = keccak256("TOKENOMICS_MODULE");

    TemporalGradientCore internal core;
    TokenomicsModule internal tokenomics;
    MockProtocolToken internal token;

    address internal miner = vm.addr(0xBEEF);
    address internal outsider = vm.addr(0xCAFE);

    function setUp() public {
        core = new TemporalGradientCore(address(this), bytes32(uint256(1)));

        token = new MockProtocolToken("Temporal Gradient Token", "TGBT");

        TokenomicsModule tokenomicsImplementation = new TokenomicsModule();
        ERC1967Proxy tokenomicsProxy = new ERC1967Proxy(
            address(tokenomicsImplementation),
            abi.encodeCall(TokenomicsModule.initialize, (address(core), address(token), 10 ether, 1_000, 10_000, 2, 125))
        );
        tokenomics = TokenomicsModule(address(tokenomicsProxy));

        core.setModule(TOKENOMICS_MODULE, address(tokenomics));
        core.setModule(MINING_MODULE, address(this));
    }

    function testOnBlockMinedMintsBaseReward() public {
        bytes32 output = bytes32(type(uint256).max - 1_500);

        uint256 reward = tokenomics.onBlockMined(
            miner,
            output,
            0,
            1_000,
            0,
            tokenomics.MINING_ALLOCATION()
        );

        assertEq(reward, 10 ether);
        assertEq(token.balanceOf(miner), 10 ether);
        assertEq(tokenomics.totalMined(), 10 ether);

        (
            uint256 currentReward,
            uint256 currentEpoch,
            uint256 blocksPerEpoch,
            uint256 halvingInterval,
            uint256 nextHalvingBlock,
            uint256 currentBonusThreshold,
            uint256 currentBonusMultiplier,
            uint256 minedSoFar,
            uint256 remainingAllocation
        ) = tokenomics.getMiningEconomics();

        assertEq(currentReward, 10 ether);
        assertEq(currentEpoch, 0);
        assertEq(blocksPerEpoch, 1_000);
        assertEq(halvingInterval, 10_000);
        assertEq(nextHalvingBlock, 10_001);
        assertEq(currentBonusThreshold, 2);
        assertEq(currentBonusMultiplier, 125);
        assertEq(minedSoFar, 10 ether);
        assertEq(remainingAllocation, tokenomics.MINING_ALLOCATION() - 10 ether);
    }

    function testOnBlockMinedAppliesBonusAndPoolCap() public {
        bytes32 exceptionalOutput = bytes32(0);

        uint256 reward = tokenomics.onBlockMined(
            miner,
            exceptionalOutput,
            0,
            1_000,
            0,
            11 ether
        );

        assertEq(reward, 11 ether);
        assertEq(token.balanceOf(miner), 11 ether);
        assertEq(tokenomics.totalMined(), 11 ether);
    }

    function testEpochTransitionAppliesHalving() public {
        vm.roll(block.number + 10_000);

        bytes32 output = bytes32(type(uint256).max - 1_500);
        uint256 reward = tokenomics.onBlockMined(
            miner,
            output,
            0,
            1_000,
            0,
            tokenomics.MINING_ALLOCATION()
        );

        assertEq(reward, 6.5 ether);

        (
            uint256 currentReward,
            uint256 currentEpoch,
            uint256 blocksPerEpoch,
            uint256 halvingInterval,
            uint256 nextHalvingBlock,
            uint256 currentBonusThreshold,
            uint256 currentBonusMultiplier,
            uint256 minedSoFar,
            uint256 remainingAllocation
        ) = tokenomics.getMiningEconomics();

        assertEq(currentReward, 6.5 ether);
        assertEq(currentEpoch, 10);
        assertEq(blocksPerEpoch, 1_000);
        assertEq(halvingInterval, 10_000);
        assertEq(currentBonusThreshold, 2);
        assertEq(currentBonusMultiplier, 125);
        assertEq(minedSoFar, 6.5 ether);
        assertEq(remainingAllocation, tokenomics.MINING_ALLOCATION() - 6.5 ether);
        assertEq(nextHalvingBlock, 20_001);
    }

    function testMiningEconomicsPreviewTracksL2BlockScheduleBeforeNextMine() public {
        vm.roll(block.number + 10_000);

        (
            uint256 currentReward,
            uint256 currentEpoch,
            uint256 blocksPerEpoch,
            uint256 halvingInterval,
            uint256 nextHalvingBlock,
            uint256 currentBonusThreshold,
            uint256 currentBonusMultiplier,
            uint256 minedSoFar,
            uint256 remainingAllocation
        ) = tokenomics.getMiningEconomics();

        assertEq(currentReward, 6.5 ether);
        assertEq(currentEpoch, 10);
        assertEq(blocksPerEpoch, 1_000);
        assertEq(halvingInterval, 10_000);
        assertEq(nextHalvingBlock, 20_001);
        assertEq(currentBonusThreshold, 2);
        assertEq(currentBonusMultiplier, 125);
        assertEq(minedSoFar, 0);
        assertEq(remainingAllocation, tokenomics.MINING_ALLOCATION());
    }

    function testEpochAnchoringDoesNotDriftWhenUpdatesOccurLate() public {
        vm.roll(block.number + 1_550);

        tokenomics.onBlockMined(
            miner,
            bytes32(type(uint256).max - 1_500),
            0,
            1_000,
            0,
            tokenomics.MINING_ALLOCATION()
        );

        uint256 currentEpoch;
        uint256 nextHalvingBlock;
        {
            (
                ,
                currentEpoch,
                ,
                ,
                nextHalvingBlock,
                ,
                ,
                ,
                
            ) = tokenomics.getMiningEconomics();
        }

        assertEq(currentEpoch, 1);
        assertEq(nextHalvingBlock, 10_001);

        vm.roll(2_001);

        uint256 previewReward;
        uint256 previewEpoch;
        {
            (
                previewReward,
                previewEpoch,
                ,
                ,
                ,
                ,
                ,
                ,
                
            ) = tokenomics.getMiningEconomics();
        }

        assertEq(previewReward, 10 ether);
        assertEq(previewEpoch, 2);
    }

    function testLegacyEmissionControlSelectorsAreUnavailable() public {
        (bool setTokenOk, ) = address(tokenomics).call(abi.encodeWithSignature("setTGBTToken(address)", address(token)));
        (bool setBonusOk, ) = address(tokenomics).call(abi.encodeWithSignature("setBonusParameters(uint16,uint256)", 150, 2));
        (bool setEpochOk, ) = address(tokenomics).call(abi.encodeWithSignature("setEpochBlocks(uint256)", 2_000));
        (bool setHalvingOk, ) = address(tokenomics).call(abi.encodeWithSignature("setHalvingInterval(uint256)", 20_000));

        assertFalse(setTokenOk);
        assertFalse(setBonusOk);
        assertFalse(setEpochOk);
        assertFalse(setHalvingOk);
    }

    function testRecordMissedContributionTracksReputationOnly() public {
        token.mint(miner, 100 ether);

        tokenomics.recordMissedContribution(miner);
        tokenomics.recordMissedContribution(miner);
        tokenomics.recordMissedContribution(miner);

        assertEq(token.balanceOf(miner), 100 ether);

        (uint256 lastActivity, uint256 missedContributionCount) = tokenomics.getAccountPenaltyState(miner);
        assertEq(lastActivity, 0);
        assertEq(missedContributionCount, 3);
    }

    function testMinedActivityAndMissedContributionsCanCoexist() public {
        bytes32 output = bytes32(type(uint256).max - 1_500);
        tokenomics.onBlockMined(miner, output, 0, 1_000, 0, tokenomics.MINING_ALLOCATION());
        tokenomics.recordMissedContribution(miner);

        (uint256 lastActivity, ) = tokenomics.getAccountPenaltyState(miner);
        assertEq(lastActivity, block.number);
    }

    function testResetMissedContributionsClearsReputationCounter() public {
        tokenomics.recordMissedContribution(miner);
        tokenomics.recordMissedContribution(miner);

        tokenomics.resetMissedContributions(miner);

        (, uint256 missedContributionCount) = tokenomics.getAccountPenaltyState(miner);
        assertEq(missedContributionCount, 0);
    }
}