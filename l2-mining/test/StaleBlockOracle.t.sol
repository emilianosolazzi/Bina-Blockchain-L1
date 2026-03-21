// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Test } from "forge-std/Test.sol";
import { ERC1967Proxy } from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import { TemporalGradientCore } from "../contracts/TemporalGradientCore.sol";
import { StaleBlockOracle } from "../contracts/StaleBlockOracle.sol";
import { TokenomicsModule } from "../contracts/modules/TokenomicsModule.sol";
import { TGBT } from "../contracts/TGBT_Token.sol";

contract StaleBlockOracleTest is Test {
    bytes32 internal constant TOKENOMICS_MODULE = keccak256("TOKENOMICS_MODULE");
    bytes32 internal constant STALE_BLOCK_MODULE = keccak256("STALE_BLOCK_MODULE");

    TemporalGradientCore internal core;
    TokenomicsModule internal tokenomics;
    TGBT internal token;
    StaleBlockOracle internal oracle;

    address internal governance = address(this);
    address internal submitter = vm.addr(0xBEEF);
    address internal firstSubmitter = vm.addr(0xABCD);

    function setUp() public {
        vm.warp(1_704_067_200 + 10_000);
        vm.roll(100);

        core = new TemporalGradientCore(governance, bytes32(uint256(1)));
        token = new TGBT(governance);

        TokenomicsModule implementation = new TokenomicsModule();
        ERC1967Proxy proxy = new ERC1967Proxy(
            address(implementation),
            abi.encodeCall(TokenomicsModule.initialize, (address(core), address(token), 10 ether, 1_000, 10_000, 2, 125))
        );
        tokenomics = TokenomicsModule(address(proxy));

        oracle = new StaleBlockOracle();
        oracle.initialize(address(core), 0, 10, 7 days, 1 ether);

        core.setModule(TOKENOMICS_MODULE, address(tokenomics));
        core.setModule(STALE_BLOCK_MODULE, address(oracle));

        token.grantAuthorization(address(tokenomics));
    }

    function testSubmitAndClaimMintsThroughTokenomics() public {
        bytes memory header = _buildHeader(uint32(block.timestamp - 60));
        bytes32 blockHash = sha256(abi.encodePacked(sha256(header)));
        bytes32 canonicalHash = keccak256("canonical-winner");

        vm.prank(submitter);
        oracle.submitStaleBlock(header, 900_000, canonicalHash, 1);

        assertTrue(oracle.isSubmitted(blockHash));

        uint256 pendingBefore = oracle.pendingReward(blockHash);
        assertGt(pendingBefore, 0);

        vm.prank(submitter);
        oracle.claimReward(blockHash);

        assertEq(token.balanceOf(submitter), pendingBefore);
        assertEq(tokenomics.totalStaleRewards(), pendingBefore);

        (uint256 rewardedSoFar, uint256 remainingAllocation, uint256 utilizationBps) = tokenomics.getStaleRewardHealth();
        assertEq(rewardedSoFar, pendingBefore);
        assertEq(remainingAllocation, tokenomics.STALE_BLOCK_ALLOCATION() - pendingBefore);
        assertEq(utilizationBps, pendingBefore * 10_000 / tokenomics.STALE_BLOCK_ALLOCATION());
        assertEq(oracle.pendingReward(blockHash), 0);
    }

    function testClaimRewardClipsToStaleAllocation() public {
        oracle.updateConfig(0, 10, 7 days, tokenomics.STALE_BLOCK_ALLOCATION());

        bytes memory firstHeader = _buildHeader(uint32(block.timestamp - 90));
        bytes32 firstBlockHash = sha256(abi.encodePacked(sha256(firstHeader)));
        vm.prank(firstSubmitter);
        oracle.submitStaleBlock(firstHeader, 900_001, keccak256("winner-2"), 5);
        vm.prank(firstSubmitter);
        oracle.claimReward(firstBlockHash);

        bytes memory secondHeader = _buildHeader(uint32(block.timestamp - 120));
        secondHeader[0] = 0x42;
        bytes32 secondBlockHash = sha256(abi.encodePacked(sha256(secondHeader)));
        vm.prank(submitter);
        oracle.submitStaleBlock(secondHeader, 900_002, keccak256("winner-3"), 5);
        vm.prank(submitter);
        oracle.claimReward(secondBlockHash);

        assertEq(token.balanceOf(submitter), 0);
        assertEq(tokenomics.totalStaleRewards(), tokenomics.STALE_BLOCK_ALLOCATION());
    }

    function _buildHeader(uint32 timestamp) internal pure returns (bytes memory header) {
        header = new bytes(80);
        for (uint256 i = 0; i < 80; i++) {
            header[i] = bytes1(uint8((i * 17 + 3) % 251));
        }

        header[68] = bytes1(uint8(timestamp));
        header[69] = bytes1(uint8(timestamp >> 8));
        header[70] = bytes1(uint8(timestamp >> 16));
        header[71] = bytes1(uint8(timestamp >> 24));
    }
}