// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ECDSA } from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import { EIP712 } from "@openzeppelin/contracts/utils/cryptography/EIP712.sol";
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { ModuleBase } from "./ModuleBase.sol";
import { CoreUtilsLib } from "../CoreUtilsLib.sol";
import { MiningLib } from "../MiningLib.sol";
import { IRateLimitModule } from "../interfaces/IRateLimitModule.sol";
import { ITokenomicsModule } from "../interfaces/ITokenomicsModule.sol";

contract MiningModule is ModuleBase, EIP712("TemporalGradientBeacon", "1") {
    using ECDSA for bytes32;

    bytes32 public constant MODULE_RATE_LIMIT = keccak256("RATE_LIMIT_MODULE");
    bytes32 public constant MODULE_TOKENOMICS = keccak256("TOKENOMICS_MODULE");

    uint256 public constant MIN_DIFFICULTY = 1000;
    uint256 public constant MAX_DIFFICULTY = 2**245;
    // Hold requirement removed — genesis miners can mine with 0 TGBT
    uint256 private constant DEFAULT_SUBMISSION_COST = 1;
    uint256 private constant DEFAULT_REVEAL_COST = 2;
    uint256 private constant MAX_BATCH = 20;

    bytes32 private constant MINING_COMMITMENT_TYPEHASH =
        keccak256("MiningCommitment(address miner,bytes32 commitHash,uint256 poolId,uint256 nonce,uint256 deadline)");

    IERC20 public holdToken;
    uint8 public poolCount;
    uint8 public minBlockInterval;
    uint8 public minCommitmentAge;
    uint16 public maxCommitmentAge;

    mapping(address => uint256) public nonces;
    mapping(address => uint64) public lastMinerBlock;
    mapping(address => MiningLib.Commitment) public minerCommitments;
    mapping(uint8 => MiningLib.MiningPool) public miningPools;
    mapping(bytes32 => uint256) public usedOutputs;

    event CommitmentSubmitted(address indexed miner, bytes32 commitHash, uint8 poolId);
    event CommitmentRevealed(address indexed miner, bytes32 revealedValue, uint8 poolId);
    event MiningPoolCreated(uint8 indexed poolId, uint256 targetDifficulty, uint256 emissionBucket);

    error InvalidPoolId();
    error DeadlineExpired();
    error InvalidNonce();
    error ActiveCommitmentExists();
    error MiningTooFrequently();
    error InvalidSignature();
    // error InsufficientHoldBalance(); // removed — no hold requirement
    error BatchTooLarge();
    error ArrayLengthMismatch();
    error InvalidPreviousOutput();
    error NoCommitmentFound();
    error CommitmentAlreadyRevealed();
    error CommitmentTooRecent();
    error CommitmentExpired();
    error PoolIdMismatch();

    function initialize(address coreAddress, address holdTokenAddress, uint256 initialDifficulty, uint256 initialEmission) external {
        __ModuleBase_init(coreAddress);

        holdToken = IERC20(holdTokenAddress);
        poolCount = 1;
        minBlockInterval = 1;
        minCommitmentAge = 2;
        maxCommitmentAge = 500;

        miningPools[0] = MiningLib.MiningPool({
            targetDifficulty: initialDifficulty,
            emissionBucket: initialEmission,
            totalMined: 0,
            active: true,
            lastUpdateBlock: uint64(block.number),
            minerCount: 0
        });

        emit MiningPoolCreated(0, initialDifficulty, initialEmission);
    }

    function submitMiningCommitment(
        bytes32 commitHash,
        uint8 poolId,
        uint256 nonce,
        uint256 deadline,
        bytes calldata signature
    ) public whenSystemActive {
        _rateLimit().consumeOrRevert(msg.sender, DEFAULT_SUBMISSION_COST, keccak256("MINING_COMMIT"));
        if (poolId >= poolCount || !miningPools[poolId].active) revert InvalidPoolId();
        if (block.timestamp > deadline) revert DeadlineExpired();

        bytes32 digest = _hashTypedDataV4(
            keccak256(abi.encode(MINING_COMMITMENT_TYPEHASH, msg.sender, commitHash, poolId, nonce, deadline))
        );
        address recoveredSigner = ECDSA.recover(digest, signature);
        if (recoveredSigner != msg.sender || recoveredSigner == address(0)) revert InvalidSignature();
        if (nonces[msg.sender] != nonce) revert InvalidNonce();
        nonces[msg.sender]++;

        MiningLib.Commitment storage commitment = minerCommitments[msg.sender];
        if (
            commitment.commitHash != bytes32(0) &&
            !commitment.flags.revealed &&
            block.number <= commitment.timestamp + maxCommitmentAge
        ) revert ActiveCommitmentExists();

        if (
            minBlockInterval != 0 &&
            lastMinerBlock[msg.sender] != 0 &&
            block.number - lastMinerBlock[msg.sender] < minBlockInterval
        ) revert MiningTooFrequently();

        commitment.commitHash = commitHash;
        commitment.timestamp = uint64(block.number);
        commitment.flags.revealed = false;
        commitment.revealedValue = bytes32(0);
        commitment.poolId = poolId;
        commitment.deadline = deadline;
        commitment.lastUpdateBlock = uint64(block.number);

        emit CommitmentSubmitted(msg.sender, commitHash, poolId);
    }

    function revealMiningCommitment(
        bytes32 previousOutput,
        bytes calldata temporalSeed,
        uint64 nonce,
        bytes calldata signature,
        bytes32 secretValue,
        uint8 poolId
    ) external whenSystemActive {
        _rateLimit().consumeOrRevert(msg.sender, DEFAULT_REVEAL_COST, keccak256("MINING_REVEAL"));
        if (poolId >= poolCount || !miningPools[poolId].active) revert InvalidPoolId();

        MiningLib.Commitment storage commitment = minerCommitments[msg.sender];
        if (commitment.commitHash == bytes32(0)) revert NoCommitmentFound();
        if (commitment.flags.revealed) revert CommitmentAlreadyRevealed();
        if (block.number < commitment.timestamp + minCommitmentAge) revert CommitmentTooRecent();
        if (block.number > commitment.timestamp + maxCommitmentAge) revert CommitmentExpired();
        if (commitment.poolId != poolId) revert PoolIdMismatch();

        MiningLib.RevealParams memory revealParams = MiningLib.RevealParams({
            miner: msg.sender,
            previousOutput: previousOutput,
            temporalSeed: temporalSeed,
            nonce: nonce,
            signature: signature,
            secretValue: secretValue,
            poolId: poolId
        });

        MiningLib.checkCommitmentValidity(revealParams, commitment);

        if (!_historyContains(previousOutput)) revert InvalidPreviousOutput();

        bytes32 hmacOutput = MiningLib.processMiningReveal(
            revealParams,
            miningPools[poolId].targetDifficulty,
            usedOutputs,
            MiningLib.iterativeEntropyHash
        );

        commitment.revealedValue = hmacOutput;
        commitment.flags.revealed = true;
        lastMinerBlock[msg.sender] = uint64(block.number);
        usedOutputs[hmacOutput] = block.number;

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

    function batchSubmitCommitments(
        bytes32[] calldata commitHashes,
        uint8[] calldata poolIds,
        uint256[] calldata deadlines,
        bytes[] calldata signatures
    ) external whenSystemActive {
        if (commitHashes.length > MAX_BATCH) revert BatchTooLarge();
        if (
            commitHashes.length != poolIds.length ||
            commitHashes.length != deadlines.length ||
            commitHashes.length != signatures.length
        ) revert ArrayLengthMismatch();

        uint256 startNonce = nonces[msg.sender];

        for (uint256 i = 0; i < commitHashes.length; i++) {
            submitMiningCommitment(commitHashes[i], poolIds[i], startNonce + i, deadlines[i], signatures[i]);
        }
    }

    function createMiningPool(uint256 targetDifficulty, uint256 emissionBucket) external onlyGovernance {
        require(poolCount < type(uint8).max, "MaxPoolsReached");
        require(targetDifficulty >= MIN_DIFFICULTY && targetDifficulty <= MAX_DIFFICULTY, "InvalidDifficulty");

        uint8 newPoolId = poolCount;
        miningPools[newPoolId] = MiningLib.MiningPool({
            targetDifficulty: targetDifficulty,
            emissionBucket: emissionBucket,
            totalMined: 0,
            active: true,
            lastUpdateBlock: uint64(block.number),
            minerCount: 0
        });
        poolCount++;
        emit MiningPoolCreated(newPoolId, targetDifficulty, emissionBucket);
    }

    // updateMiningPool removed — pools are immutable after creation (Bitcoin-style).
    // Deactivation is the ONLY allowed mutation: governance can retire broken or
    // exhausted pools so getActivePools() stays accurate and no miner accidentally
    // commits to an unmineable target.  Once deactivated a pool cannot be re-activated.

    event MiningPoolDeactivated(uint8 indexed poolId);

    function deactivatePool(uint8 poolId) external onlyGovernance {
        require(poolId < poolCount, "InvalidPoolId");
        require(miningPools[poolId].active, "AlreadyInactive");
        miningPools[poolId].active = false;
        emit MiningPoolDeactivated(poolId);
    }

    function getPoolInfo(uint8 poolId) external view returns (uint256 difficulty, uint256 emission, uint256 mined, bool active) {
        if (poolId >= poolCount) revert InvalidPoolId();
        MiningLib.MiningPool storage pool = miningPools[poolId];
        return (
            pool.targetDifficulty,
            pool.emissionBucket > pool.totalMined ? pool.emissionBucket - pool.totalMined : 0,
            pool.totalMined,
            pool.active
        );
    }

    function getMiningChallenge(uint8 poolId)
        external
        view
        returns (bytes32[] memory outputs, uint256 difficulty)
    {
        if (poolId >= poolCount || !miningPools[poolId].active) revert InvalidPoolId();

        bytes32[32] memory history = _outputHistory();
        outputs = new bytes32[](history.length);
        for (uint256 i = 0; i < history.length;) {
            outputs[i] = history[i];
            unchecked { ++i; }
        }

        return (outputs, miningPools[poolId].targetDifficulty);
    }

    function getActivePools()
        external
        view
        returns (uint8[] memory activePools, uint256[] memory difficulties, uint256[] memory emissions)
    {
        activePools = new uint8[](poolCount);
        difficulties = new uint256[](poolCount);
        emissions = new uint256[](poolCount);

        uint8 activeCount = 0;
        for (uint8 i = 0; i < poolCount; i++) {
            if (miningPools[i].active) {
                activePools[activeCount] = i;
                difficulties[activeCount] = miningPools[i].targetDifficulty;
                emissions[activeCount] = miningPools[i].emissionBucket > miningPools[i].totalMined
                    ? miningPools[i].emissionBucket - miningPools[i].totalMined
                    : 0;
                activeCount++;
            }
        }

        assembly {
            mstore(activePools, activeCount)
            mstore(difficulties, activeCount)
            mstore(emissions, activeCount)
        }
    }

    function _historyContains(bytes32 previousOutput) internal view returns (bool) {
        bytes32[32] memory history = _outputHistory();
        for (uint256 i = 0; i < history.length;) {
            if (history[i] == previousOutput) {
                return true;
            }
            unchecked { ++i; }
        }
        return false;
    }

    function _rateLimit() internal view returns (IRateLimitModule) {
        return IRateLimitModule(_module(MODULE_RATE_LIMIT));
    }

    function _tokenomics() internal view returns (ITokenomicsModule) {
        return ITokenomicsModule(_module(MODULE_TOKENOMICS));
    }
}
