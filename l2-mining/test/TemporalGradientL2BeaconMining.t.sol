// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Test } from "forge-std/Test.sol";
import { ERC1967Proxy } from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import { TemporalGradientL2Beacon } from "../contracts/TemporalGradientL2Beacon.sol";
import { TemporalGradientL2BeaconMiningHarness } from "./mocks/TemporalGradientL2BeaconMiningHarness.sol";
import { MockProtocolToken } from "./mocks/MockProtocolToken.sol";

contract TemporalGradientL2BeaconMiningTest is Test {
    bytes32 internal constant DOMAIN_TYPEHASH =
        keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)");
    bytes32 internal constant MINING_COMMITMENT_TYPEHASH =
        keccak256("MiningCommitment(address miner,bytes32 commitHash,uint256 poolId,uint256 nonce,uint256 deadline)");
    bytes32 internal constant NAME_HASH = keccak256(bytes("TemporalGradientBeacon"));
    bytes32 internal constant VERSION_HASH = keccak256(bytes("1"));

    uint256 internal adminPk = 0xA11CE;
    uint256 internal minerPk = 0xB0B;

    address internal admin;
    address internal miner;

    MockProtocolToken internal rewardToken;
    MockProtocolToken internal stakeToken;
    TemporalGradientL2BeaconMiningHarness internal beacon;

    function setUp() public {
        admin = vm.addr(adminPk);
        miner = vm.addr(minerPk);

        vm.warp(1_704_067_200 + 1_000);
        vm.roll(10);

        vm.deal(admin, 100 ether);
        vm.deal(miner, 100 ether);

        vm.startPrank(admin);
        rewardToken = new MockProtocolToken("Reward Token", "RWD");
        stakeToken = new MockProtocolToken("Stake Token", "STK");

        TemporalGradientL2BeaconMiningHarness implementation = new TemporalGradientL2BeaconMiningHarness();
        ERC1967Proxy proxy = new ERC1967Proxy(
            address(implementation),
            abi.encodeCall(
                TemporalGradientL2Beacon.initialize,
                (address(rewardToken), address(stakeToken), 10 ether, 1000, 100, 1000)
            )
        );

        beacon = TemporalGradientL2BeaconMiningHarness(address(proxy));
        vm.stopPrank();

        stakeToken.mint(miner, beacon.REQUIRED_TSTAKE_AMOUNT());
    }

    function testCommitRevealMintsReward() public {
        bytes32 previousOutput = beacon.outputHistory(0);
        uint8 poolId = 0;
        uint64 miningNonce = 42;
        uint256 signatureNonce = beacon.nonces(miner);
        uint64 revealTimestamp = uint64(block.timestamp + 600);
        bytes32 revealPrevrandao = keccak256("integration-test-randao");
        bytes memory temporalSeed = _encodeTemporalSeed(revealTimestamp);
        bytes32 secretValue = keccak256("integration-test-secret");
        uint256 deadline = revealTimestamp + 3600;

        vm.warp(revealTimestamp);
        vm.prevrandao(revealPrevrandao);

        bytes memory revealSignature = _buildRevealSignature(previousOutput, temporalSeed, miningNonce, secretValue);

        bytes32 commitHash = keccak256(
            abi.encodePacked(previousOutput, temporalSeed, miningNonce, revealSignature, secretValue, miner)
        );

        bytes memory commitmentSignature = _buildCommitmentSignature(commitHash, poolId, signatureNonce, deadline);

        vm.prank(miner);
        beacon.submitMiningCommitment(commitHash, poolId, signatureNonce, deadline, commitmentSignature);

        vm.roll(block.number + beacon.minCommitmentAge());
        vm.prevrandao(revealPrevrandao);

        vm.prank(miner);
        beacon.revealMiningCommitmentHarness(previousOutput, temporalSeed, miningNonce, revealSignature, secretValue, poolId);

        assertEq(rewardToken.balanceOf(miner), 12.5 ether);
        assertEq(beacon.totalMined(), 12.5 ether);
        assertEq(beacon.lastMinerBlock(miner), uint64(block.number));

        (, , uint256 poolMined, bool active) = beacon.getPoolInfo(poolId);
        assertTrue(active);
        assertEq(poolMined, 12.5 ether);
    }

    function testRevealRejectsMalformedTemporalSeed() public {
        bytes32 previousOutput = beacon.outputHistory(0);
        uint8 poolId = 0;
        uint64 miningNonce = 7;
        uint256 signatureNonce = beacon.nonces(miner);
        uint64 revealTimestamp = uint64(block.timestamp + 600);
        bytes32 revealPrevrandao = keccak256("bad-seed-randao");
        bytes memory temporalSeed = hex"0102030405060708";
        bytes32 secretValue = keccak256("bad-seed-secret");
        uint256 deadline = revealTimestamp + 3600;

        vm.warp(revealTimestamp);
        vm.prevrandao(revealPrevrandao);

        bytes memory revealSignature = _buildRevealSignature(previousOutput, temporalSeed, miningNonce, secretValue);

        bytes32 commitHash = keccak256(
            abi.encodePacked(previousOutput, temporalSeed, miningNonce, revealSignature, secretValue, miner)
        );

        bytes memory commitmentSignature = _buildCommitmentSignature(commitHash, poolId, signatureNonce, deadline);

        vm.prank(miner);
        beacon.submitMiningCommitment(commitHash, poolId, signatureNonce, deadline, commitmentSignature);

        vm.roll(block.number + beacon.minCommitmentAge());
        vm.prevrandao(revealPrevrandao);

        vm.expectRevert();
        vm.prank(miner);
        beacon.revealMiningCommitmentHarness(previousOutput, temporalSeed, miningNonce, revealSignature, secretValue, poolId);
    }

    function testRandomnessReceiptTracksProgressAndProofInputs() public {
        bytes32 userSeed = keccak256("receipt-seed");
        address contributorA = vm.addr(0xC0FFEE1);
        address contributorB = vm.addr(0xC0FFEE2);
        address contributorC = vm.addr(0xC0FFEE3);
        bytes32 contributionA = keccak256("contribution-a");
        bytes32 contributionB = keccak256("contribution-b");
        bytes32 contributionC = keccak256("contribution-c");

        vm.prank(miner);
        uint256 requestId = beacon.requestRandomness(userSeed);

        (
            address requesterBefore,
            uint256 requestedAtBefore,
            bool fulfilledBefore,
            bytes32 storedSeedBefore,
            bytes32 resultBefore,
            uint256 contributionsBefore,
            uint256 minContributionsBefore,
            uint256 contributionsRemainingBefore,
            uint256 maxContributionsBefore,
            uint256 emergencyFeeBefore
        ) = beacon.getRandomnessReceipt(requestId);

        assertEq(requesterBefore, miner);
        assertEq(requestedAtBefore, block.timestamp);
        assertFalse(fulfilledBefore);
        assertEq(storedSeedBefore, userSeed);
        assertEq(uint256(resultBefore), 0);
        assertEq(contributionsBefore, 0);
        assertEq(minContributionsBefore, 3);
        assertEq(contributionsRemainingBefore, 3);
        assertEq(maxContributionsBefore, 10);
        assertEq(emergencyFeeBefore, 100 ether);

        vm.prank(contributorA);
        beacon.contributeEntropy(requestId, contributionA);

        vm.prank(contributorB);
        beacon.contributeEntropy(requestId, contributionB);

        vm.prank(contributorC);
        beacon.contributeEntropy(requestId, contributionC);

        (
            address requesterAfter,
            uint256 requestedAtAfter,
            bool fulfilledAfter,
            bytes32 storedSeedAfter,
            bytes32 resultAfter,
            uint256 contributionsAfter,
            uint256 minContributionsAfter,
            uint256 contributionsRemainingAfter,
            uint256 maxContributionsAfter,
            uint256 emergencyFeeAfter
        ) = beacon.getRandomnessReceipt(requestId);

        assertEq(requesterAfter, miner);
        assertEq(requestedAtAfter, requestedAtBefore);
        assertTrue(fulfilledAfter);
        assertEq(storedSeedAfter, userSeed);
        assertTrue(resultAfter != bytes32(0));
        assertEq(contributionsAfter, 3);
        assertEq(minContributionsAfter, 3);
        assertEq(contributionsRemainingAfter, 0);
        assertEq(maxContributionsAfter, 10);
        assertEq(emergencyFeeAfter, 130 ether);

        (address[] memory contributors, bytes32[] memory contributions) = beacon.getRandomnessContributionDetails(requestId);
        assertEq(contributors.length, 3);
        assertEq(contributions.length, 3);
        assertEq(contributors[0], contributorA);
        assertEq(contributors[1], contributorB);
        assertEq(contributors[2], contributorC);
        assertEq(contributions[0], contributionA);
        assertEq(contributions[1], contributionB);
        assertEq(contributions[2], contributionC);
    }

    function _buildRevealSignature(
        bytes32 previousOutput,
        bytes memory temporalSeed,
        uint64 miningNonce,
        bytes32 secretValue
    ) internal returns (bytes memory) {
        bytes32 entropyHash = beacon.computeRevealEntropyHash(
            previousOutput,
            temporalSeed,
            miningNonce,
            miner,
            secretValue
        );

        (uint8 v, bytes32 r, bytes32 s) = vm.sign(minerPk, entropyHash);
        return abi.encodePacked(r, s, v);
    }

    function _buildCommitmentSignature(
        bytes32 commitHash,
        uint8 poolId,
        uint256 nonce,
        uint256 deadline
    ) internal returns (bytes memory) {
        bytes32 structHash = keccak256(
            abi.encode(MINING_COMMITMENT_TYPEHASH, miner, commitHash, poolId, nonce, deadline)
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", _domainSeparator(), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(minerPk, digest);
        return abi.encodePacked(r, s, v);
    }

    function _domainSeparator() internal view returns (bytes32) {
        return keccak256(abi.encode(DOMAIN_TYPEHASH, NAME_HASH, VERSION_HASH, block.chainid, address(beacon)));
    }

    function _encodeTemporalSeed(uint64 timestamp) internal pure returns (bytes memory) {
        bytes memory seed = new bytes(8);
        bytes8 timestampBytes = bytes8(timestamp);
        seed[0] = 0x00;
        for (uint256 i = 1; i < 8; i++) {
            seed[i] = timestampBytes[i];
        }
        return seed;
    }

}
