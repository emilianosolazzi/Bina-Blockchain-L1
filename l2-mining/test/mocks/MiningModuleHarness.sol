// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { BloomFilterLib } from "../../contracts/BloomFilterLib.sol";
import { MiningLib } from "../../contracts/MiningLib.sol";
import { MiningModule } from "../../contracts/modules/MiningModule.sol";

contract MiningModuleHarness is MiningModule {
    function revealMiningCommitmentHarness(
        bytes32 previousOutput,
        bytes calldata temporalSeed,
        uint64 nonce,
        bytes calldata signature,
        bytes32 secretValue,
        uint8 poolId
    ) external whenSystemActive {
        _rateLimit().consumeOrRevert(msg.sender, 2, keccak256("MINING_REVEAL"));
        if (poolId >= poolCount || !miningPools[poolId].active) revert InvalidPoolId();

        MiningLib.Commitment storage commitment = minerCommitments[msg.sender];
        require(commitment.commitHash != bytes32(0), "NoCommitmentFound");
        require(!commitment.flags.revealed, "CommitmentAlreadyRevealed");
        require(block.number >= commitment.timestamp + minCommitmentAge, "CommitmentTooRecent");
        require(block.number <= commitment.timestamp + maxCommitmentAge, "CommitmentExpired");
        require(commitment.poolId == poolId, "InvalidPoolId");

        bytes32 computedHash = keccak256(
            abi.encodePacked(previousOutput, temporalSeed, nonce, signature, secretValue, msg.sender)
        );
        require(computedHash == commitment.commitHash, "InvalidCommitment");

        if (!_historyContains(previousOutput)) revert InvalidPreviousOutput();

        bytes32 hmacOutput = MiningLib.processMiningReveal(
            previousOutput,
            temporalSeed,
            nonce,
            signature,
            secretValue,
            miningPools[poolId].targetDifficulty,
            msg.sender,
            bloomFilter,
            usedOutputs,
            _deterministicHash,
            _difficultyWeight
        );

        commitment.revealedValue = hmacOutput;
        commitment.flags.revealed = true;
        lastMinerBlock[msg.sender] = uint64(block.number);
        usedOutputs[hmacOutput] = block.number;
        BloomFilterLib.updateFilter(bloomFilter, hmacOutput);

        MiningLib.MiningPool storage pool = miningPools[poolId];
        uint256 reward = _tokenomics().onBlockMined(
            msg.sender,
            hmacOutput,
            poolId,
            pool.targetDifficulty,
            pool.totalMined,
            pool.emissionBucket
        );
        if (reward > 0) {
            pool.totalMined += reward;
        }

        core.recordMinedOutput(hmacOutput, msg.sender, poolId, reward, nonce);

        emit CommitmentRevealed(msg.sender, hmacOutput, poolId);
    }

    function deterministicPreview(bytes memory input) external pure returns (bytes32) {
        return _deterministicHash(input);
    }

    function _deterministicHash(bytes memory input) internal pure returns (bytes32) {
        return bytes32(uint256(keccak256(input)) >> 128);
    }
}