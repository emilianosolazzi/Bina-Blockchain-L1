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

    function setUp() public {
        TemporalGradientCore coreImplementation = new TemporalGradientCore();
        ERC1967Proxy coreProxy = new ERC1967Proxy(
            address(coreImplementation),
            abi.encodeCall(TemporalGradientCore.initialize, (address(this), bytes32(uint256(1))))
        );
        core = TemporalGradientCore(address(coreProxy));

        token = new MockProtocolToken("Temporal Gradient Token", "TGBT");

        TokenomicsModule tokenomicsImplementation = new TokenomicsModule();
        ERC1967Proxy tokenomicsProxy = new ERC1967Proxy(
            address(tokenomicsImplementation),
            abi.encodeCall(TokenomicsModule.initialize, (address(core), address(token), 10 ether, 1_000, 10_000, 2, 125))
        );
        tokenomics = TokenomicsModule(address(tokenomicsProxy));

        core.setModule(TOKENOMICS_MODULE, address(tokenomics));
        core.setModule(MINING_MODULE, address(this));
        core.grantRole(tokenomics.SLASHER_ROLE(), address(this));
        core.grantRole(tokenomics.BURNER_ROLE(), address(this));
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

    function testManualSlashDelegatesToTokenSlash() public {
        token.mint(miner, 25 ether);

        vm.prank(address(core));
        tokenomics.onManualSlash(miner, 10 ether, keccak256("MANUAL_TEST"));

        assertEq(token.balanceOf(miner), 15 ether);
    }

    function testPenaltyHooksSlashAndBurn() public {
        token.mint(miner, 100 ether);

        uint256 slashed = tokenomics.autoSlash(miner, tokenomics.VIOLATION_TYPE_RULE(), 50);
        assertEq(slashed, 50 ether);
        assertEq(token.balanceOf(miner), 50 ether);

        tokenomics.recordMissedContribution(miner);
        tokenomics.recordMissedContribution(miner);
        tokenomics.recordMissedContribution(miner);

        assertEq(token.balanceOf(miner), 35 ether);

        (uint256 lastActivity, uint256 missedContributionCount) = tokenomics.getAccountPenaltyState(miner);
        assertEq(lastActivity, block.number);
        assertEq(missedContributionCount, 0);
    }

    function testInactivityBurnAppliesAfterThirtyDays() public {
        token.mint(miner, 100 ether);

        vm.roll(block.number + 180_000);
        tokenomics.checkInactivity(miner);

        assertEq(token.balanceOf(miner), 99 ether);

        (uint256 lastActivity, ) = tokenomics.getAccountPenaltyState(miner);
        assertEq(lastActivity, block.number);
    }
}