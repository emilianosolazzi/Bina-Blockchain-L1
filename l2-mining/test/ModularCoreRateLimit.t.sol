// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Test } from "forge-std/Test.sol";
import { TemporalGradientCore } from "../contracts/TemporalGradientCore.sol";
import { RateLimitModule } from "../contracts/modules/RateLimitModule.sol";

contract ModularCoreRateLimitTest is Test {
    bytes32 internal constant RATE_LIMIT_MODULE_ID = keccak256("RATE_LIMIT_MODULE");

    TemporalGradientCore internal core;
    RateLimitModule internal rateLimit;
    address internal admin = address(0xA11CE);
    address internal miner = address(0xB0B);
    address internal miningModule = address(0xCAFE);

    function setUp() public {
        vm.startPrank(admin);
        core = new TemporalGradientCore(admin, bytes32(0));

        rateLimit = new RateLimitModule();
        rateLimit.initialize(address(core));

        core.setModule(RATE_LIMIT_MODULE_ID, address(rateLimit));
        core.setModule(keccak256("MINING_MODULE"), miningModule);
        vm.stopPrank();
    }

    function testCoreRegistersModuleAndRecordsOutput() public {
        vm.prank(miningModule);
        core.recordMinedOutput(bytes32(uint256(123)), miner, 0, 5 ether, 77);

        assertEq(core.outputHistoryAt(1), bytes32(uint256(123)));
        assertEq(core.getCurrentOutputIndex(), 1);
        assertTrue(core.isModule(miningModule));
    }

    function testRateLimitConsumesThroughCoreCaller() public {
        vm.prank(address(core));
        rateLimit.consumeOrRevert(miner, 1, keccak256("SUBMIT"));

        (uint256 currentTokens, uint256 capacity) = rateLimit.getUserCapacity(miner);
        assertEq(capacity, 60);
        assertLt(currentTokens, capacity);
    }

    function testCoreAllowsFutureModuleIdsBeforeLock() public {
        bytes32 futureModuleId = keccak256("ANALYTICS_MODULE_V2");
        address futureModule = address(0xDEAD);

        vm.prank(admin);
        core.setModule(futureModuleId, futureModule);

        assertEq(core.moduleAddress(futureModuleId), futureModule);
        assertTrue(core.isModule(futureModule));
        assertEq(core.moduleCount(), 3);
    }

    function testCoreCanRemoveModuleBeforeLock() public {
        vm.prank(admin);
        core.removeModule(RATE_LIMIT_MODULE_ID);

        assertEq(core.moduleAddress(RATE_LIMIT_MODULE_ID), address(0));
        assertFalse(core.isModule(address(rateLimit)));
        assertEq(core.moduleCount(), 1);
    }

    function testCoreCanOssifyAfterBootstrap() public {
        vm.prank(admin);
        core.ossify();

        assertTrue(core.modulesLocked());
        assertTrue(core.pausePermanentlyDisabled());
        assertTrue(core.governanceLocked());
        assertTrue(core.isOssified());
        assertEq(core.owner(), address(0));
        assertEq(core.governanceRoleCount(), 0);
        assertEq(core.defaultAdminRoleCount(), 0);

        vm.prank(admin);
        vm.expectRevert();
        core.setModule(keccak256("POST_LOCK_MODULE"), address(0xBEEF));
    }

    function testDisablePauseForeverBlocksPause() public {
        vm.prank(admin);
        core.disablePauseForever();

        assertTrue(core.pausePermanentlyDisabled());

        vm.prank(admin);
        vm.expectRevert(TemporalGradientCore.PauseDisabled.selector);
        core.pause();
    }
}
