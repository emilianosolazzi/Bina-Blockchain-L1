// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Test } from "forge-std/Test.sol";
import { TemporalGradientCore } from "../contracts/TemporalGradientCore.sol";
import { RateLimitModule } from "../contracts/modules/RateLimitModule.sol";
import { MiningModule } from "../contracts/modules/MiningModule.sol";
import { MiningLib } from "../contracts/MiningLib.sol";
import { MiningModuleHarness } from "./mocks/MiningModuleHarness.sol";
import { MockProtocolToken } from "./mocks/MockProtocolToken.sol";
import { MockTokenomicsModule } from "./mocks/MockTokenomicsModule.sol";

contract MiningModuleTest is Test {
    bytes32 internal constant DOMAIN_TYPEHASH =
        keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)");
    bytes32 internal constant MINING_COMMITMENT_TYPEHASH =
        keccak256("MiningCommitment(address miner,bytes32 commitHash,uint256 poolId,uint256 nonce,uint256 deadline)");
    bytes32 internal constant NAME_HASH = keccak256(bytes("TemporalGradientBeacon"));
    bytes32 internal constant VERSION_HASH = keccak256(bytes("1"));

    bytes32 internal constant MINING_MODULE_ID = keccak256("MINING_MODULE");
    bytes32 internal constant RATE_LIMIT_MODULE_ID = keccak256("RATE_LIMIT_MODULE");
    bytes32 internal constant TOKENOMICS_MODULE_ID = keccak256("TOKENOMICS_MODULE");
    uint256 internal constant INITIAL_DIFFICULTY = 2 ** 190;

    uint256 internal minerPk = 0xB0B;
    uint256 internal secondMinerPk = 0xC0DE;

    address internal miner;
    address internal secondMiner;

    TemporalGradientCore internal core;
    MiningModuleHarness internal mining;
    RateLimitModule internal rateLimit;
    MockTokenomicsModule internal tokenomics;
    MockProtocolToken internal holdToken;

    struct RevealFixture {
        bytes32 previousOutput;
        bytes temporalSeed;
        uint64 miningNonce;
        bytes32 secretValue;
        uint256 commitmentNonce;
        uint256 deadline;
        bytes revealSignature;
        bytes32 commitHash;
        bytes commitmentSignature;
        bytes32 expectedOutput;
    }

    function setUp() public {
        miner = vm.addr(minerPk);
        secondMiner = vm.addr(secondMinerPk);

        vm.warp(1_704_067_200 + 1_000);
        vm.roll(100);

        core = new TemporalGradientCore(address(this), bytes32(uint256(1)));

        holdToken = new MockProtocolToken("TGBT Token", "TGBT");

        rateLimit = new RateLimitModule();
        rateLimit.initialize(address(core));

        mining = new MiningModuleHarness();
        mining.initialize(address(core), address(holdToken), INITIAL_DIFFICULTY, type(uint256).max / 4);

        tokenomics = new MockTokenomicsModule();

        core.setModule(RATE_LIMIT_MODULE_ID, address(rateLimit));
        core.setModule(TOKENOMICS_MODULE_ID, address(tokenomics));
        core.setModule(MINING_MODULE_ID, address(mining));

        holdToken.mint(miner, 100 ether);   // optional — no hold gate, but keeps wallet funded
        holdToken.mint(secondMiner, 100 ether);
    }

    function testSubmitCommitmentStoresStateAndIncrementsNonce() public {
        RevealFixture memory fixture = _buildFixture(minerPk, miner, core.outputHistoryAt(0), 0, bytes32("secret-a"), 0);

        vm.prank(miner);
        mining.submitMiningCommitment(
            fixture.commitHash,
            0,
            fixture.commitmentNonce,
            fixture.deadline,
            fixture.commitmentSignature
        );

        assertEq(mining.nonces(miner), 1);

        (
            bytes32 storedCommitHash,
            uint64 timestamp,
            MiningLib.CommitmentFlags memory flags,
            bytes32 revealedValue,
            uint8 poolId,
            uint256 deadline,
            MiningLib.ValidationInfo memory validation,
            uint64 lastUpdateBlock
        ) = mining.minerCommitments(miner);

        assertEq(storedCommitHash, fixture.commitHash);
        assertEq(timestamp, block.number);
        assertFalse(flags.revealed);
        assertFalse(flags.validated);
        assertFalse(flags.revoked);
        assertFalse(flags.emergency);
        assertEq(revealedValue, bytes32(0));
        assertEq(poolId, 0);
        assertEq(deadline, fixture.deadline);
        assertEq(validation.blockNumber, 0);
        assertEq(validation.timestamp, 0);
        assertEq(validation.validatorHash, bytes32(0));
        assertFalse(validation.success);
        assertEq(lastUpdateBlock, block.number);
    }

    function testSubmitCommitmentRejectsInvalidSignatureHoldAndPool() public {
        RevealFixture memory fixture = _buildFixture(minerPk, miner, core.outputHistoryAt(0), 0, bytes32("secret-b"), 0);

        // No hold-balance gate — unfunded caller gets InvalidSignature (sig was built for miner, not 0xDEAD)
        vm.prank(vm.addr(0xDEAD));
        vm.expectRevert(MiningModule.InvalidSignature.selector);
        mining.submitMiningCommitment(fixture.commitHash, 0, fixture.commitmentNonce, fixture.deadline, fixture.commitmentSignature);

        vm.prank(miner);
        vm.expectRevert(MiningModule.InvalidPoolId.selector);
        mining.submitMiningCommitment(fixture.commitHash, 9, fixture.commitmentNonce, fixture.deadline, fixture.commitmentSignature);

        bytes memory badSignature = fixture.commitmentSignature;
        badSignature[10] = bytes1(uint8(badSignature[10]) ^ 0x01);

        vm.prank(miner);
        vm.expectRevert();
        mining.submitMiningCommitment(fixture.commitHash, 0, fixture.commitmentNonce, fixture.deadline, badSignature);
    }

    function testRevealCommitmentRecordsOutputRewardAndCoreHistory() public {
        RevealFixture memory fixture = _buildFixture(minerPk, miner, core.outputHistoryAt(0), 0, bytes32("secret-c"), 17);

        vm.prank(miner);
        mining.submitMiningCommitment(fixture.commitHash, 0, fixture.commitmentNonce, fixture.deadline, fixture.commitmentSignature);

        uint64 startIndex = core.getCurrentOutputIndex();
        vm.roll(block.number + mining.minCommitmentAge());

        vm.prank(miner);
        mining.revealMiningCommitmentHarness(
            fixture.previousOutput,
            fixture.temporalSeed,
            fixture.miningNonce,
            fixture.revealSignature,
            fixture.secretValue,
            0
        );

        assertEq(tokenomics.minedCallCount(), 1);
        assertEq(tokenomics.lastMiner(), miner);
        assertEq(tokenomics.lastOutput(), fixture.expectedOutput);
        assertEq(tokenomics.lastPoolId(), 0);
        assertEq(tokenomics.lastPoolTargetDifficulty(), INITIAL_DIFFICULTY);

        assertEq(mining.lastMinerBlock(miner), block.number);
        assertEq(mining.usedOutputs(fixture.expectedOutput), block.number);

        (, , uint256 totalMined, bool active) = mining.getPoolInfo(0);
        assertTrue(active);
        assertEq(totalMined, tokenomics.rewardToReturn());

        uint64 newIndex = core.getCurrentOutputIndex();
        assertEq(newIndex, startIndex + 1);
        assertEq(core.outputHistoryAt(newIndex), fixture.expectedOutput);
    }

    // testRevealCommitmentRequiresHoldBalanceAtRevealTime removed — hold gate no longer exists

    function testRevealCommitmentRemainsValidAfterTimestampAdvances() public {
        RevealFixture memory fixture = _buildFixture(minerPk, miner, core.outputHistoryAt(0), 0, bytes32("secret-c-time"), 20);

        vm.prank(miner);
        mining.submitMiningCommitment(fixture.commitHash, 0, fixture.commitmentNonce, fixture.deadline, fixture.commitmentSignature);

        vm.roll(block.number + mining.minCommitmentAge());
        vm.warp(block.timestamp + 10 minutes);

        vm.prank(miner);
        mining.revealMiningCommitmentHarness(
            fixture.previousOutput,
            fixture.temporalSeed,
            fixture.miningNonce,
            fixture.revealSignature,
            fixture.secretValue,
            0
        );

        assertEq(tokenomics.lastOutput(), fixture.expectedOutput);
    }

    function testRevealCommitmentRejectsInvalidPreviousOutputExpiredAndAlreadyRevealed() public {
        bytes32 fakePreviousOutput = keccak256("fake-output");
        RevealFixture memory invalidHistoryFixture = _buildFixture(minerPk, miner, fakePreviousOutput, 0, bytes32("secret-d"), 7);

        vm.prank(miner);
        mining.submitMiningCommitment(
            invalidHistoryFixture.commitHash,
            0,
            invalidHistoryFixture.commitmentNonce,
            invalidHistoryFixture.deadline,
            invalidHistoryFixture.commitmentSignature
        );

        vm.roll(block.number + mining.minCommitmentAge());
        vm.prank(miner);
        vm.expectRevert(MiningModule.InvalidPreviousOutput.selector);
        mining.revealMiningCommitmentHarness(
            invalidHistoryFixture.previousOutput,
            invalidHistoryFixture.temporalSeed,
            invalidHistoryFixture.miningNonce,
            invalidHistoryFixture.revealSignature,
            invalidHistoryFixture.secretValue,
            0
        );

        RevealFixture memory expiredFixture = _buildFixture(
            minerPk,
            miner,
            core.outputHistoryAt(0),
            mining.nonces(miner),
            bytes32("secret-e"),
            8
        );
        vm.roll(block.number + mining.maxCommitmentAge() + 1);

        vm.prank(miner);
        mining.submitMiningCommitment(
            expiredFixture.commitHash,
            0,
            expiredFixture.commitmentNonce,
            expiredFixture.deadline,
            expiredFixture.commitmentSignature
        );

        vm.roll(block.number + mining.maxCommitmentAge() + 1);
        vm.prank(miner);
        vm.expectRevert(bytes("CommitmentExpired"));
        mining.revealMiningCommitmentHarness(
            expiredFixture.previousOutput,
            expiredFixture.temporalSeed,
            expiredFixture.miningNonce,
            expiredFixture.revealSignature,
            expiredFixture.secretValue,
            0
        );

        RevealFixture memory successFixture = _buildFixture(
            minerPk,
            miner,
            core.outputHistoryAt(0),
            mining.nonces(miner),
            bytes32("secret-f"),
            9
        );
        vm.prank(miner);
        mining.submitMiningCommitment(
            successFixture.commitHash,
            0,
            successFixture.commitmentNonce,
            successFixture.deadline,
            successFixture.commitmentSignature
        );
        vm.roll(block.number + mining.minCommitmentAge());
        vm.prank(miner);
        mining.revealMiningCommitmentHarness(
            successFixture.previousOutput,
            successFixture.temporalSeed,
            successFixture.miningNonce,
            successFixture.revealSignature,
            successFixture.secretValue,
            0
        );

        vm.prank(miner);
        vm.expectRevert(bytes("CommitmentAlreadyRevealed"));
        mining.revealMiningCommitmentHarness(
            successFixture.previousOutput,
            successFixture.temporalSeed,
            successFixture.miningNonce,
            successFixture.revealSignature,
            successFixture.secretValue,
            0
        );
    }

    function testExactOutputTrackingRejectsDuplicateOutput() public {
        RevealFixture memory fixture = _buildFixture(minerPk, miner, core.outputHistoryAt(0), 0, bytes32("secret-g"), 11);

        vm.prank(miner);
        mining.submitMiningCommitment(fixture.commitHash, 0, fixture.commitmentNonce, fixture.deadline, fixture.commitmentSignature);
        vm.roll(block.number + mining.minCommitmentAge());
        vm.prank(miner);
        mining.revealMiningCommitmentHarness(
            fixture.previousOutput,
            fixture.temporalSeed,
            fixture.miningNonce,
            fixture.revealSignature,
            fixture.secretValue,
            0
        );

        uint256 nextNonce = mining.nonces(miner);
        bytes memory secondCommitSig = _buildCommitmentSignature(minerPk, miner, fixture.commitHash, 0, nextNonce, fixture.deadline);
        vm.roll(block.number + 1);
        vm.prank(miner);
        mining.submitMiningCommitment(fixture.commitHash, 0, nextNonce, fixture.deadline, secondCommitSig);

        vm.roll(block.number + mining.minCommitmentAge());
        vm.prank(miner);
        vm.expectRevert(abi.encodeWithSelector(MiningLib.OutputAlreadyUsed.selector, uint8(2), uint8(128)));
        mining.revealMiningCommitmentHarness(
            fixture.previousOutput,
            fixture.temporalSeed,
            fixture.miningNonce,
            fixture.revealSignature,
            fixture.secretValue,
            0
        );
    }

    function testBatchSubmissionsEnforcesBoundsAndRevertsOnActiveCommitment() public {
        bytes32 previousOutput = core.outputHistoryAt(0);
        RevealFixture memory first = _buildFixture(minerPk, miner, previousOutput, 0, bytes32("secret-h1"), 21);
        RevealFixture memory second = _buildFixture(minerPk, miner, previousOutput, 1, bytes32("secret-h2"), 22);

        bytes32[] memory commitHashes = new bytes32[](2);
        uint8[] memory poolIds = new uint8[](2);
        uint256[] memory deadlines = new uint256[](2);
        bytes[] memory signatures = new bytes[](2);

        commitHashes[0] = first.commitHash;
        commitHashes[1] = second.commitHash;
        poolIds[0] = 0;
        poolIds[1] = 0;
        deadlines[0] = first.deadline;
        deadlines[1] = second.deadline;
        signatures[0] = first.commitmentSignature;
        signatures[1] = second.commitmentSignature;

        vm.prank(miner);
        vm.expectRevert(MiningModule.ActiveCommitmentExists.selector);
        mining.batchSubmitCommitments(commitHashes, poolIds, deadlines, signatures);

        assertEq(mining.nonces(miner), 0);

        bytes32[] memory oversized = new bytes32[](21);
        uint8[] memory oversizedPools = new uint8[](21);
        uint256[] memory oversizedDeadlines = new uint256[](21);
        bytes[] memory oversizedSigs = new bytes[](21);

        vm.prank(miner);
        vm.expectRevert(MiningModule.BatchTooLarge.selector);
        mining.batchSubmitCommitments(oversized, oversizedPools, oversizedDeadlines, oversizedSigs);
    }

    function testDifficultyAdjustmentAndPoolManagement() public {
        mining.createMiningPool(10_000, 1_000 ether);
        assertEq(mining.poolCount(), 2);

        // Pool is immutable after creation — verify stored values
        (uint256 difficulty, uint256 emission, uint256 mined, bool active) = mining.getPoolInfo(1);

        assertEq(difficulty, 10_000);
        assertEq(emission, 1_000 ether);
        assertEq(mined, 0);
        assertTrue(active);

        vm.expectRevert(bytes("InvalidDifficulty"));
        mining.createMiningPool(999, 1 ether);
    }

    function testRateLimitIntegrationConsumesExpectedCosts() public {
        RevealFixture memory fixture = _buildFixture(minerPk, miner, core.outputHistoryAt(0), 0, bytes32("secret-i"), 31);

        (uint256 initialTokens, uint256 capacity) = rateLimit.getUserCapacity(miner);
        assertEq(initialTokens, 60);
        assertEq(capacity, 60);

        vm.prank(miner);
        mining.submitMiningCommitment(fixture.commitHash, 0, fixture.commitmentNonce, fixture.deadline, fixture.commitmentSignature);

        (uint256 afterCommitTokens, ) = rateLimit.getUserCapacity(miner);
        assertEq(afterCommitTokens, 59);

        vm.roll(block.number + mining.minCommitmentAge());
        vm.prank(miner);
        mining.revealMiningCommitmentHarness(
            fixture.previousOutput,
            fixture.temporalSeed,
            fixture.miningNonce,
            fixture.revealSignature,
            fixture.secretValue,
            0
        );

        (uint256 afterRevealTokens, ) = rateLimit.getUserCapacity(miner);
        assertEq(afterRevealTokens, 57);
    }

    function _buildFixture(
        uint256 signerPk,
        address signer,
        bytes32 previousOutput,
        uint256 commitmentNonce,
        bytes32 secretValue,
        uint64 miningNonce
    ) internal returns (RevealFixture memory fixture) {
        fixture.previousOutput = previousOutput;
        fixture.temporalSeed = _encodeTemporalSeed(uint64(block.timestamp));
        fixture.miningNonce = miningNonce;
        fixture.secretValue = secretValue;
        fixture.commitmentNonce = commitmentNonce;
        fixture.deadline = block.timestamp + 1 hours;
        fixture.revealSignature = _buildRevealSignature(
            signerPk,
            signer,
            previousOutput,
            fixture.temporalSeed,
            miningNonce,
            secretValue
        );
        fixture.commitHash = keccak256(
            abi.encodePacked(previousOutput, fixture.temporalSeed, miningNonce, fixture.revealSignature, secretValue, signer)
        );
        fixture.commitmentSignature = _buildCommitmentSignature(
            signerPk,
            signer,
            fixture.commitHash,
            0,
            commitmentNonce,
            fixture.deadline
        );
        fixture.expectedOutput = _computeExpectedOutput(
            fixture.revealSignature,
            previousOutput,
            fixture.temporalSeed,
            miningNonce,
            signer,
            secretValue
        );
    }

    function _buildRevealSignature(
        uint256 signerPk,
        address signer,
        bytes32 previousOutput,
        bytes memory temporalSeed,
        uint64 miningNonce,
        bytes32 secretValue
    ) internal returns (bytes memory) {
        bytes32 entropyHash = _computeEntropyHash(signer, previousOutput, temporalSeed, miningNonce, secretValue);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(signerPk, entropyHash);
        return abi.encodePacked(r, s, v);
    }

    function _buildCommitmentSignature(
        uint256 signerPk,
        address signer,
        bytes32 commitHash,
        uint8 poolId,
        uint256 nonce,
        uint256 deadline
    ) internal view returns (bytes memory) {
        bytes32 domainSeparator = keccak256(
            abi.encode(DOMAIN_TYPEHASH, NAME_HASH, VERSION_HASH, block.chainid, address(mining))
        );
        bytes32 structHash = keccak256(
            abi.encode(MINING_COMMITMENT_TYPEHASH, signer, commitHash, poolId, nonce, deadline)
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", domainSeparator, structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(signerPk, digest);
        return abi.encodePacked(r, s, v);
    }

    function _computeEntropyHash(
        address signer,
        bytes32 previousOutput,
        bytes memory temporalSeed,
        uint64 miningNonce,
        bytes32 secretValue
    ) internal pure returns (bytes32) {
        return keccak256(
            abi.encodePacked(previousOutput, temporalSeed, miningNonce, signer, secretValue)
        );
    }

    function _computeExpectedOutput(
        bytes memory revealSignature,
        bytes32 previousOutput,
        bytes memory temporalSeed,
        uint64 miningNonce,
        address signer,
        bytes32 secretValue
    ) internal view returns (bytes32) {
        bytes32 entropyHash = _computeEntropyHash(signer, previousOutput, temporalSeed, miningNonce, secretValue);
        return mining.deterministicPreview(abi.encodePacked(revealSignature, entropyHash, secretValue));
    }

    function _encodeTemporalSeed(uint64 timestampSecs) internal pure returns (bytes memory seed) {
        seed = new bytes(8);
        seed[0] = bytes1(0x00);
        bytes8 raw = bytes8(timestampSecs);
        for (uint256 i = 1; i < 8; i++) {
            seed[i] = raw[i];
        }
    }

}