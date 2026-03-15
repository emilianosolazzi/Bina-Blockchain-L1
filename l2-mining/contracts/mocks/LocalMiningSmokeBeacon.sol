// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ECDSA } from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";

contract LocalMiningSmokeBeacon {
    using ECDSA for bytes32;

    bytes32 private constant DOMAIN_TYPEHASH =
        keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)");
    bytes32 private constant MINING_COMMITMENT_TYPEHASH =
        keccak256("MiningCommitment(address miner,bytes32 commitHash,uint256 poolId,uint256 nonce,uint256 deadline)");
    bytes32 private constant NAME_HASH = keccak256(bytes("TemporalGradientBeacon"));
    bytes32 private constant VERSION_HASH = keccak256(bytes("1"));

    struct Commitment {
        bytes32 commitHash;
        uint64 blockNumber;
        uint8 poolId;
        bool revealed;
    }

    mapping(address => uint256) public nonces;
    mapping(address => Commitment) public commitments;

    uint256 public immutable minCommitmentAge;
    uint256 public immutable targetDifficulty;
    uint256 public immutable rewardAmount;
    bytes32 public latestOutput;

    event CommitmentSubmitted(address indexed miner, bytes32 commitHash, uint8 poolId);
    event CommitmentRevealed(address indexed miner, bytes32 revealedValue, uint8 poolId);
    event BeaconBlockMined(address indexed miner, bytes32 hmacOutput, uint256 reward, uint64 nonce, uint64 timestamp, uint8 poolId);

    error InvalidPoolId();
    error DeadlineExpired();
    error InvalidNonce();
    error InvalidSignature();
    error ActiveCommitmentExists();
    error NoCommitmentFound();
    error CommitmentAlreadyRevealed();
    error CommitmentTooRecent();
    error InvalidCommitment();
    error InvalidTemporalSeed();
    error InvalidSigner();
    error SolutionTooEasy();

    constructor(uint256 _difficulty, uint256 _rewardAmount, uint256 _minCommitmentAge) {
        targetDifficulty = _difficulty;
        rewardAmount = _rewardAmount;
        minCommitmentAge = _minCommitmentAge;
        latestOutput = keccak256(abi.encodePacked("LOCAL_MINING_GENESIS", block.timestamp, block.prevrandao));
    }

    function submitMiningCommitment(
        bytes32 commitHash,
        uint8 poolId,
        uint256 nonce,
        uint256 deadline,
        bytes calldata signature
    ) external returns (bool) {
        if (poolId != 0) revert InvalidPoolId();
        if (block.timestamp > deadline) revert DeadlineExpired();
        if (nonces[msg.sender] != nonce) revert InvalidNonce();

        bytes32 digest = keccak256(
            abi.encodePacked(
                "\x19\x01",
                _domainSeparator(),
                keccak256(abi.encode(MINING_COMMITMENT_TYPEHASH, msg.sender, commitHash, poolId, nonce, deadline))
            )
        );
        if (digest.recover(signature) != msg.sender) revert InvalidSignature();

        Commitment storage commitment = commitments[msg.sender];
        if (commitment.commitHash != bytes32(0) && !commitment.revealed) revert ActiveCommitmentExists();

        nonces[msg.sender] = nonce + 1;
        commitments[msg.sender] = Commitment({
            commitHash: commitHash,
            blockNumber: uint64(block.number),
            poolId: poolId,
            revealed: false
        });

        emit CommitmentSubmitted(msg.sender, commitHash, poolId);
        return true;
    }

    function revealMiningCommitment(
        bytes32 previousOutput,
        bytes calldata temporalSeed,
        uint64 nonce,
        bytes calldata signature,
        bytes32 secretValue,
        uint8 poolId
    ) external {
        if (poolId != 0) revert InvalidPoolId();

        Commitment storage commitment = commitments[msg.sender];
        if (commitment.commitHash == bytes32(0)) revert NoCommitmentFound();
        if (commitment.revealed) revert CommitmentAlreadyRevealed();
        if (block.number < uint256(commitment.blockNumber) + minCommitmentAge) revert CommitmentTooRecent();

        bytes32 expectedCommitment = keccak256(
            abi.encodePacked(previousOutput, temporalSeed, nonce, signature, secretValue, msg.sender)
        );
        if (expectedCommitment != commitment.commitHash) revert InvalidCommitment();
        if (temporalSeed.length != 8 || temporalSeed[0] != 0x00) revert InvalidTemporalSeed();
        if (previousOutput != latestOutput) revert InvalidCommitment();

        bytes32 entropyHash = keccak256(abi.encodePacked(previousOutput, temporalSeed, nonce, msg.sender, secretValue));
        if (entropyHash.recover(signature) != msg.sender) revert InvalidSigner();

        bytes32 solutionHash = _quantumResistantHashLive(signature, entropyHash, secretValue);
        if (uint256(solutionHash) >= targetDifficulty) revert SolutionTooEasy();

        commitment.revealed = true;
        latestOutput = solutionHash;

        emit CommitmentRevealed(msg.sender, solutionHash, poolId);
        emit BeaconBlockMined(msg.sender, solutionHash, rewardAmount, nonce, uint64(block.timestamp), poolId);
    }

    function getMiningChallenge(uint8 poolId) external view returns (bytes32[] memory outputs, uint256 difficulty) {
        if (poolId != 0) revert InvalidPoolId();
        outputs = new bytes32[](1);
        outputs[0] = latestOutput;
        difficulty = targetDifficulty;
    }

    function _domainSeparator() internal view returns (bytes32) {
        return keccak256(abi.encode(DOMAIN_TYPEHASH, NAME_HASH, VERSION_HASH, block.chainid, address(this)));
    }

    function _quantumResistantHashLive(
        bytes calldata signature,
        bytes32 entropyHash,
        bytes32 secretValue
    ) internal pure returns (bytes32 h) {
        h = keccak256(abi.encodePacked(signature, entropyHash, secretValue));
        unchecked {
            for (uint8 i = 0; i < 3; i++) {
                h = keccak256(abi.encodePacked(bytes32(uint256(h) ^ (uint256(i + 1) << 248))));
                uint256 rotated = (uint256(h) << 7) | (uint256(h) >> 249);
                h = bytes32(rotated);
            }
        }
    }
}