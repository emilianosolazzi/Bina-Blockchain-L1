// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Test } from "forge-std/Test.sol";
import { ERC1967Proxy } from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import { TemporalGradientCore } from "../contracts/TemporalGradientCore.sol";
import { RandomnessLib } from "../contracts/RandomnessLib.sol";
import { RandomnessModule } from "../contracts/modules/RandomnessModule.sol";
import { MockProtocolToken } from "./mocks/MockProtocolToken.sol";
import { RandomnessModuleHarness } from "./mocks/RandomnessModuleHarness.sol";

contract RandomnessModuleTest is Test {
    TemporalGradientCore internal core;
    RandomnessModuleHarness internal randomness;
    MockProtocolToken internal token;

    address internal requester = vm.addr(0xAAA1);
    address internal contributorA = vm.addr(0xAAA2);
    address internal contributorB = vm.addr(0xAAA3);
    address internal contributorC = vm.addr(0xAAA4);
    address internal emergencyOperator = vm.addr(0xAAA5);

    function setUp() public {
        vm.warp(1_704_067_200 + 1_000);
        vm.roll(100);

        core = new TemporalGradientCore(address(this), bytes32(uint256(1)));

        token = new MockProtocolToken("Temporal Gradient Token", "TGBT");

        RandomnessModuleHarness randomnessImplementation = new RandomnessModuleHarness();
        ERC1967Proxy randomnessProxy = new ERC1967Proxy(
            address(randomnessImplementation),
            abi.encodeCall(RandomnessModule.initialize, (address(core), address(token)))
        );
        randomness = RandomnessModuleHarness(address(randomnessProxy));

        core.grantRole(randomness.EMERGENCY_ROLE(), emergencyOperator);
    }

    function testRequestRandomnessStoresReceiptAndDefaults() public {
        bytes32 userSeed = keccak256("request-seed");

        vm.prank(requester);
        uint256 requestId = randomness.requestRandomness(userSeed);

        assertEq(requestId, 0);

        (address storedRequester, uint256 timestamp, bool fulfilled, uint256 contributionCount) = randomness.getRandomRequestState(requestId);
        assertEq(storedRequester, requester);
        assertEq(timestamp, block.timestamp);
        assertFalse(fulfilled);
        assertEq(contributionCount, 0);

        (
            address receiptRequester,
            uint256 requestedAt,
            bool receiptFulfilled,
            bytes32 storedSeed,
            bytes32 result,
            uint256 contributionsCount,
            uint256 minContributions,
            uint256 contributionsRemaining,
            uint256 maxContributions,
            uint256 emergencyFeeQuote
        ) = randomness.getRandomnessReceipt(requestId);

        assertEq(receiptRequester, requester);
        assertEq(requestedAt, block.timestamp);
        assertFalse(receiptFulfilled);
        assertEq(storedSeed, userSeed);
        assertEq(result, bytes32(0));
        assertEq(contributionsCount, 0);
        assertEq(minContributions, 3);
        assertEq(contributionsRemaining, 3);
        assertEq(maxContributions, 10);
        assertEq(emergencyFeeQuote, 100 ether);
    }

    function testContributeEntropyTracksContributors() public {
        uint256 requestId = _requestDefault();

        vm.prank(contributorA);
        randomness.contributeEntropy(requestId, keccak256("entropy-a"));

        vm.prank(contributorB);
        randomness.contributeEntropy(requestId, keccak256("entropy-b"));

        (address[] memory contributors, bytes32[] memory contributions) = randomness.getRandomnessContributionDetails(requestId);
        assertEq(contributors.length, 2);
        assertEq(contributions.length, 2);
        assertEq(contributors[0], contributorA);
        assertEq(contributors[1], contributorB);
        assertEq(contributions[0], keccak256("entropy-a"));
        assertEq(contributions[1], keccak256("entropy-b"));

        (, , bool fulfilled, uint256 contributionCount) = randomness.getRandomRequestState(requestId);
        assertFalse(fulfilled);
        assertEq(contributionCount, 2);
    }

    function testAutomaticFulfillmentAfterMinimumContributions() public {
        uint256 requestId = _requestDefault();

        vm.prank(contributorA);
        randomness.contributeEntropy(requestId, keccak256("entropy-1"));
        vm.prank(contributorB);
        randomness.contributeEntropy(requestId, keccak256("entropy-2"));
        vm.prank(contributorC);
        randomness.contributeEntropy(requestId, keccak256("entropy-3"));

        (, , bool fulfilled, uint256 contributionCount) = randomness.getRandomRequestState(requestId);
        assertTrue(fulfilled);
        assertEq(contributionCount, 3);

        bytes32 result = randomness.getRandomResult(requestId);
        assertTrue(result != bytes32(0));

        (
            ,
            ,
            bool receiptFulfilled,
            ,
            bytes32 receiptResult,
            uint256 contributionsCount,
            uint256 minContributions,
            uint256 contributionsRemaining,
            ,
            uint256 emergencyFeeQuote
        ) = randomness.getRandomnessReceipt(requestId);

        assertTrue(receiptFulfilled);
        assertEq(receiptResult, result);
        assertEq(contributionsCount, 3);
        assertEq(minContributions, 3);
        assertEq(contributionsRemaining, 0);
        assertEq(emergencyFeeQuote, 130 ether);
    }

    function testEmergencyFulfillCollectsFeeAndFinalizesRequest() public {
        uint256 requestId = _requestDefault();

        vm.prank(contributorA);
        randomness.contributeEntropyNoAutoFulfill(requestId, keccak256("entropy-emergency-a"));
        vm.prank(contributorB);
        randomness.contributeEntropyNoAutoFulfill(requestId, keccak256("entropy-emergency-b"));
        vm.prank(contributorC);
        randomness.contributeEntropyNoAutoFulfill(requestId, keccak256("entropy-emergency-c"));

        uint256 expectedFee = 130 ether;
        token.mint(emergencyOperator, expectedFee);
        vm.prank(emergencyOperator);
        token.approve(address(randomness), expectedFee);

        (, uint256 requestedAt, , ) = randomness.getRandomRequestState(requestId);
        (, , uint256 expiryBlocks, , , ) = randomness.getRandomnessConfig();
        vm.warp(requestedAt + expiryBlocks + 1);

        vm.prank(emergencyOperator);
        randomness.emergencyRandomnessFulfill(requestId, keccak256("emergency-root"));

        assertEq(token.balanceOf(address(randomness)), expectedFee);

        (, , bool fulfilled, uint256 contributionCount) = randomness.getRandomRequestState(requestId);
        assertTrue(fulfilled);
        assertEq(contributionCount, 3);
        assertTrue(randomness.getRandomResult(requestId) != bytes32(0));
    }

    function testEmergencyFulfillIgnoresProvidedEntropyRoot() public {
        uint256 firstRequestId = _requestDefault();
        uint256 secondRequestId = _requestDefault();

        _contributeEmergencySet(firstRequestId, "same-a", "same-b", "same-c");
        _contributeEmergencySet(secondRequestId, "same-a", "same-b", "same-c");

        uint256 totalFee = 260 ether;
        token.mint(emergencyOperator, totalFee);
        vm.prank(emergencyOperator);
        token.approve(address(randomness), totalFee);

        (, uint256 requestedAt, , ) = randomness.getRandomRequestState(firstRequestId);
        (, , uint256 expiryBlocks, , , ) = randomness.getRandomnessConfig();
        vm.warp(requestedAt + expiryBlocks + 1);

        bytes32 previewFirst = randomness.previewEmergencyFulfillResult(firstRequestId);
        bytes32 previewSecond = randomness.previewEmergencyFulfillResult(secondRequestId);

        vm.prank(emergencyOperator);
        randomness.emergencyRandomnessFulfill(firstRequestId, keccak256("root-a"));

        vm.prank(emergencyOperator);
        randomness.emergencyRandomnessFulfill(secondRequestId, keccak256("root-b"));

        assertEq(randomness.getRandomResult(firstRequestId), previewFirst);
        assertEq(randomness.getRandomResult(secondRequestId), previewSecond);
        assertEq(randomness.getRandomResult(firstRequestId), randomness.getRandomResult(secondRequestId));
    }

    function testEmergencyFulfillRejectsBeforeExpiry() public {
        uint256 requestId = _requestDefault();

        _contributeEmergencySet(requestId, "early-a", "early-b", "early-c");

        uint256 expectedFee = 130 ether;
        token.mint(emergencyOperator, expectedFee);
        vm.prank(emergencyOperator);
        token.approve(address(randomness), expectedFee);

        vm.prank(emergencyOperator);
        vm.expectRevert(RandomnessLib.RequestNotExpired.selector);
        randomness.emergencyRandomnessFulfill(requestId, keccak256("too-early-root"));
    }

    function testExpiryHandlingRejectsLateContribution() public {
        uint256 requestId = _requestDefault();

        (, , uint256 expiryBlocks, , , ) = randomness.getRandomnessConfig();

        (, uint256 requestedAt, , ) = randomness.getRandomRequestState(requestId);
        vm.warp(requestedAt + expiryBlocks + 1);

        vm.prank(contributorA);
        vm.expectRevert(RandomnessLib.RequestExpired.selector);
        randomness.contributeEntropy(requestId, keccak256("too-late"));
    }

    function testContributionBeforeExpiryStillSucceeds() public {
        uint256 requestId = _requestDefault();

        (, , uint256 expiryBlocks, , , ) = randomness.getRandomnessConfig();
        (, uint256 requestedAt, , ) = randomness.getRandomRequestState(requestId);
        vm.warp(requestedAt + expiryBlocks);

        vm.prank(contributorA);
        randomness.contributeEntropy(requestId, keccak256("on-time"));

        (, , bool fulfilled, uint256 contributionCount) = randomness.getRandomRequestState(requestId);
        assertFalse(fulfilled);
        assertEq(contributionCount, 1);
    }

    function _requestDefault() internal returns (uint256 requestId) {
        vm.prank(requester);
        requestId = randomness.requestRandomness(keccak256("default-seed"));
    }

    function _contributeEmergencySet(uint256 requestId, string memory a, string memory b, string memory c) internal {
        vm.prank(contributorA);
        randomness.contributeEntropyNoAutoFulfill(requestId, keccak256(bytes(a)));
        vm.prank(contributorB);
        randomness.contributeEntropyNoAutoFulfill(requestId, keccak256(bytes(b)));
        vm.prank(contributorC);
        randomness.contributeEntropyNoAutoFulfill(requestId, keccak256(bytes(c)));
    }
}