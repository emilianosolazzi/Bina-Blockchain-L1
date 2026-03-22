// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Test } from "forge-std/Test.sol";
import { TGBT } from "../contracts/TGBT_Token.sol";

contract TGBTTokenTest is Test {
    TGBT internal token;

    address internal governance = vm.addr(0xA11CE);
    address internal module = vm.addr(0xB0B);
    address internal secondModule = vm.addr(0xCAFE);
    address internal user = vm.addr(0xD00D);

    function setUp() public {
        vm.prank(governance);
        token = new TGBT(governance);
    }

    function testGovernanceManagesAuthorizationAndCanOssify() public {
        vm.prank(governance);
        token.grantAuthorization(module);

        assertTrue(token.isAuthorized(module));
        assertEq(token.authorizedCount(), 1);

        vm.prank(governance);
        token.lockPermissions();

        assertTrue(token.permissionsLocked());

        vm.prank(governance);
        vm.expectRevert(TGBT.PermissionsAreLocked.selector);
        token.grantAuthorization(secondModule);
    }

    function testAuthorizedModuleCanMintWithinCap() public {
        vm.prank(governance);
        token.grantAuthorization(module);

        vm.prank(module);
        token.mint(user, 25 ether);

        assertEq(token.balanceOf(user), 25 ether);
        assertEq(token.totalSupply(), 25 ether);
        assertEq(token.availableToMint(), token.MAX_SUPPLY() - 25 ether);
    }

    function testFuzzAuthorizedMintRespectsCap(uint256 mintAmount) public {
        mintAmount = bound(mintAmount, 0, token.MAX_SUPPLY());

        vm.prank(governance);
        token.grantAuthorization(module);

        vm.prank(module);
        token.mint(user, mintAmount);

        assertEq(token.balanceOf(user), mintAmount);
        assertEq(token.totalSupply(), mintAmount);
    }

    function testMintRevertsWhenCapWouldBeExceeded() public {
        vm.prank(governance);
        token.grantAuthorization(module);

        vm.startPrank(module);
        token.mint(user, token.MAX_SUPPLY());
        vm.expectRevert(TGBT.CapExceeded.selector);
        token.mint(user, 1);
        vm.stopPrank();
    }

    function testUnauthorizedMintReverts() public {
        vm.prank(user);
        vm.expectRevert(TGBT.NotAuthorized.selector);
        token.mint(user, 1 ether);
    }

    function testLockPermissionsRequiresAuthorizedModule() public {
        vm.prank(governance);
        vm.expectRevert(TGBT.NoAuthorizedModules.selector);
        token.lockPermissions();
    }

    function testRecordStampStoresEpochAnchor() public {
        vm.prank(governance);
        token.grantAuthorization(module);

        vm.prank(module);
        uint256 stampId = token.recordStamp(
            7,
            user,
            keccak256("merkle-root"),
            keccak256("bitcoin-tx"),
            2,
            900_000,
            hex"1234"
        );

        assertEq(stampId, 1);
        assertTrue(token.epochStamped(7));

        TGBT.Stamp memory stamp = token.getEpochStamp(7);
        assertEq(stamp.miner, user);
        assertEq(stamp.epochId, 7);
        assertEq(stamp.bitcoinVout, 2);
        assertEq(stamp.bitcoinBlock, 900_000);
        assertEq(stamp.proofDigest, keccak256(hex"1234"));
    }

    function testRecordStampRejectsZeroMiner() public {
        vm.prank(governance);
        token.grantAuthorization(module);

        vm.prank(module);
        vm.expectRevert(TGBT.ZeroMiner.selector);
        token.recordStamp(
            7,
            address(0),
            keccak256("merkle-root"),
            keccak256("bitcoin-tx"),
            2,
            900_000,
            hex"1234"
        );
    }

    function testGetEpochStampRevertsWhenMissing() public {
        vm.expectRevert(TGBT.EpochNotStamped.selector);
        token.getEpochStamp(999);
    }
}