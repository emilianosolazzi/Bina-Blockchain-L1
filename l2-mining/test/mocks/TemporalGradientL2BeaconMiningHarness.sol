// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { TemporalGradientL2Beacon } from "../../contracts/TemporalGradientL2Beacon.sol";
import { CoreUtilsLib } from "../../contracts/CoreUtilsLib.sol";
import { BloomFilterLib } from "../../contracts/BloomFilterLib.sol";
import { MiningLib } from "../../contracts/MiningLib.sol";
import { TokenomicsLib } from "../../contracts/TokenomicsLib.sol";

contract TemporalGradientL2BeaconMiningHarness is TemporalGradientL2Beacon {
    using BloomFilterLib for BloomFilterLib.Filter;
    using CoreUtilsLib for bytes32[32];

    function computeRevealEntropyHash(
        bytes32 previousOutput,
        bytes calldata temporalSeed,
        uint64 nonce,
        address miner,
        bytes32 secretValue
    ) external view returns (bytes32) {
        uint64 seedTimestamp = 0;
        for (uint256 i = 1; i < temporalSeed.length; i++) {
            seedTimestamp = (seedTimestamp << 8) | uint64(uint8(temporalSeed[i]));
        }

        bytes32 timeBasedEntropy = keccak256(
            abi.encodePacked(block.timestamp, bytes32(block.prevrandao), seedTimestamp, address(this))
        );

        return keccak256(
            abi.encodePacked(previousOutput, temporalSeed, nonce, miner, timeBasedEntropy, secretValue)
        );
    }

    function revealMiningCommitmentHarness(
        bytes32 previousOutput,
        bytes calldata temporalSeed,
        uint64 nonce,
        bytes calldata signature,
        bytes32 secretValue,
        uint8 poolId
    ) external nonReentrant whenNotPaused {
        _updateActivity(msg.sender);

        MiningLib.RevealParams memory params = MiningLib.RevealParams({
            miner: msg.sender,
            previousOutput: previousOutput,
            temporalSeed: temporalSeed,
            nonce: nonce,
            signature: signature,
            secretValue: secretValue,
            poolId: poolId
        });

        _processMiningRevealHarness(params);
    }

    function _processMiningRevealHarness(MiningLib.RevealParams memory params) internal {
        MiningLib.Commitment storage commitment = minerCommitments[params.miner];
        require(commitment.commitHash != bytes32(0), "NoCommitmentFound");
        require(!commitment.flags.revealed, "CommitmentAlreadyRevealed");
        require(block.number >= commitment.timestamp + minCommitmentAge, "CommitmentTooRecent");
        require(block.number <= commitment.timestamp + maxCommitmentAge, "CommitmentExpired");
        require(commitment.poolId == params.poolId, "InvalidPoolId");
        require(miningPools[params.poolId].active, "InvalidPoolId");

        bytes32 computedHash = keccak256(
            abi.encodePacked(
                params.previousOutput,
                params.temporalSeed,
                params.nonce,
                params.signature,
                params.secretValue,
                params.miner
            )
        );
        require(computedHash == commitment.commitHash, "InvalidCommitment");
        require(
            CoreUtilsLib.validatePreviousOutput(params.previousOutput, outputHistory, OUTPUT_HISTORY_SIZE),
            "InvalidPreviousOutput"
        );

        function(address) view returns (uint256) difficultyWeightFn = _getHarnessDifficultyWeight;

        bytes32 hmacOutput = MiningLib.processMiningReveal(
            params.previousOutput,
            params.temporalSeed,
            params.nonce,
            params.signature,
            params.secretValue,
            miningPools[params.poolId].targetDifficulty,
            params.miner,
            bloomFilter,
            usedOutputs,
            _testHash,
            difficultyWeightFn
        );

        commitment.revealedValue = hmacOutput;
        commitment.flags.revealed = true;

        currentOutputIndex = outputHistory.updateOutputHistory(currentOutputIndex, hmacOutput);
        lastOutputTimestamp = uint64(block.timestamp);
        usedOutputs[hmacOutput] = block.number;
        lastMinerBlock[params.miner] = uint64(block.number);

        BloomFilterLib.updateFilter(bloomFilter, hmacOutput);
        outputCount++;

        epochState.rewardAmount = TokenomicsLib.checkEpochTransition(epochState);

        uint256 calculatedReward = MiningLib.calculateMiningReward(
            hmacOutput,
            epochState.rewardAmount,
            bonusThreshold,
            bonusMultiplier,
            totalMined,
            MINING_ALLOCATION,
            miningPools[params.poolId]
        );

        if (calculatedReward > 0) {
            tgbtToken.mint(params.miner, calculatedReward);
            totalMined += calculatedReward;
            miningPools[params.poolId].totalMined += calculatedReward;
        }

        emit CommitmentRevealed(params.miner, hmacOutput, params.poolId);
        emit BeaconBlockMined(
            params.miner,
            hmacOutput,
            calculatedReward,
            params.nonce,
            uint64(block.timestamp),
            params.poolId
        );
        emit OutputHistoryUpdated(hmacOutput, currentOutputIndex);
    }

    function _testHash(bytes memory) internal pure returns (bytes32) {
        return bytes32(uint256(1));
    }

    function _getHarnessDifficultyWeight(address) internal pure returns (uint256) {
        return 1e18;
    }
}
