// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {AccessControlUpgradeable} from "@openzeppelin/contracts-upgradeable/access/AccessControlUpgradeable.sol";
import {PausableUpgradeable} from "@openzeppelin/contracts-upgradeable/security/PausableUpgradeable.sol";
import {Initializable} from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import {UUPSUpgradeable} from "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";
import {IGroth16Verifier} from "./interfaces/IGroth16Verifier.sol";
import {EntropyQualityLib} from "./EntropyQualityLib.sol";

/**
 * @title ZKEntropyVerifier
 * @author Entropy Team
 * @notice Enables zero-knowledge entropy contributions that hide source but prove quality
 * @dev Uses Groth16 zk-SNARK proofs to verify entropy quality without revealing the source
 *      Compatible with entropy contributions for the Temporal Gradient Beacon
 */
contract ZKEntropyVerifier is 
    Initializable, 
    AccessControlUpgradeable, 
    PausableUpgradeable, 
    UUPSUpgradeable 
{
    using EntropyQualityLib for bytes32;

    // Role definitions
    bytes32 public constant ADMIN_ROLE = keccak256("ADMIN_ROLE");
    bytes32 public constant UPDATER_ROLE = keccak256("UPDATER_ROLE");
    bytes32 public constant BEACON_ROLE = keccak256("BEACON_ROLE");
    bytes32 public constant MULTI_SIG_ROLE = keccak256("MULTI_SIG_ROLE"); // For multi-sig operations
    
    // Constants for entropy scoring
    uint256 public constant ENTROPY_MIN_SCORE = 100;
    uint256 public constant ENTROPY_MAX_SCORE = 1000;
    uint256 public constant ENTROPY_TIER_1_THRESHOLD = 300;
    uint256 public constant ENTROPY_TIER_2_THRESHOLD = 600;
    uint256 public constant ENTROPY_TIER_3_THRESHOLD = 850;
    
    // State variables
    IGroth16Verifier public zkVerifier;
    bytes32 public verificationKey;
    uint256 public minRequiredScore;
    uint256 public totalContributions;
    uint256 public successfulVerifications;
    uint256 public failedVerifications;
    mapping(bytes32 => bool) public usedCommitments;
    mapping(bytes32 => uint256) public entropyScores;
    mapping(bytes32 => uint8) public entropyTiers;
    
    // Multi-sig zkVerifier change proposal
    struct VerifierChangeProposal {
        address proposedVerifier;
        uint256 proposedAt;
        uint256 approvalCount;
        uint256 requiredApprovals;
        uint256 timelock;
        bool executed;
        mapping(address => bool) hasApproved;
    }
    VerifierChangeProposal public verifierChangeProposal;
    uint256 public verifierChangeTimelockPeriod = 2 days;
    uint256 public requiredApprovals = 3;
    
    // Staking and slashing
    mapping(address => uint256) public stakedAmount;
    mapping(address => uint256) public lastVerificationTime; // Track verification timing for withdrawal cooldown
    uint256 public requiredStake = 0.1 ether;
    uint256 public slashAmount = 0.01 ether;
    uint256 public stakeCooldownPeriod = 1 days; // Cooldown period before withdrawal
    
    // Manual verification
    mapping(bytes32 => bool) public manuallyApproved;
    mapping(bytes32 => uint256) public manualApprovalsCount;
    mapping(bytes32 => mapping(address => bool)) public manualApprovals; // Track per-admin approvals
    uint256 public requiredManualApprovals = 2;
    
    // Context for replay protection
    uint256 public contextId;
    
    // Configuration parameters
    struct VerificationParams {
        uint256 minShannonEntropy;
        uint256 minMinEntropy;
        bool requireHighTierEntropy;
        uint8 defaultTier;
        uint256 timeLockDuration;
        bool checkHistoricalPatterns;
    }
    VerificationParams public params;

    // Events
    event EntropyVerified(bytes32 indexed commitment, uint256 score, uint8 tier);
    event VerificationFailed(bytes32 indexed commitment, string reason);
    event VerificationKeyUpdated(bytes32 oldKey, bytes32 newKey);
    event MinScoreUpdated(uint256 oldScore, uint256 newScore);
    event ZKVerifierUpdated(address oldVerifier, address newVerifier);
    event VerificationParamsUpdated();
    event QualityAssessmentComplete(bytes32 indexed commitment, uint256 shannonEntropy, uint256 minEntropy);
    event VerifierChangeProposed(address indexed proposer, address newVerifier);
    event VerifierChangeApproved(address indexed approver, address newVerifier);
    event VerifierChangeExecuted(address oldVerifier, address newVerifier);
    event StakeAdded(address indexed staker, uint256 amount);
    event StakeWithdrawn(address indexed staker, uint256 amount);
    event Slashed(address indexed staker, uint256 amount, string reason);
    event ManualVerificationRequested(address indexed requester, bytes32 commitment);
    event ManualVerificationApproved(address indexed approver, bytes32 commitment);
    event ContextUpdated(uint256 oldContext, uint256 newContext);
    
    // Errors
    error InvalidProof();
    error InvalidCommitment();
    error CommitmentAlreadyUsed();
    error ScoreTooLow(uint256 score, uint256 required);
    error ZeroAddress();
    error InvalidScoreThreshold();
    error InvalidEntropy();
    error TimelockNotSatisfied(uint256 current, uint256 required);
    error InsufficientStake(uint256 required, uint256 provided);
    error NoActiveProposal();
    error AlreadyApproved();
    error TimelockNotExpired(uint256 current, uint256 required);
    error InsufficientApprovals(uint256 current, uint256 required);
    error InvalidContext(uint256 expected, uint256 provided);
    error BatchSizeMismatch();
    error ProofTooShort();
    error ProofTooLong();
    error InvalidProofFormat();

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor() {
        _disableInitializers();
    }

    /**
     * @notice Initialize the ZK entropy verifier contract
     * @param _zkVerifier Address of the Groth16 verifier contract
     * @param _verificationKey Initial verification key hash
     * @param _minRequiredScore Minimum quality score for entropy to be accepted
     */
    function initialize(
        address _zkVerifier,
        bytes32 _verificationKey,
        uint256 _minRequiredScore
    ) public initializer {
        __AccessControl_init();
        __Pausable_init();
        __UUPSUpgradeable_init();
        
        if (_zkVerifier == address(0)) revert ZeroAddress();
        if (_verificationKey == bytes32(0)) revert InvalidCommitment();
        if (_minRequiredScore < ENTROPY_MIN_SCORE) revert InvalidScoreThreshold();
        
        zkVerifier = IGroth16Verifier(_zkVerifier);
        verificationKey = _verificationKey;
        minRequiredScore = _minRequiredScore;
        
        // Set up default verification parameters
        params = VerificationParams({
            minShannonEntropy: 7 * 1e18, // 7 bits of Shannon entropy (scaled)
            minMinEntropy: 4 * 1e18,     // 4 bits of min-entropy (scaled)
            requireHighTierEntropy: false,
            defaultTier: 1,
            timeLockDuration: 5 minutes,
            checkHistoricalPatterns: true
        });
        
        // Grant roles
        _grantRole(DEFAULT_ADMIN_ROLE, msg.sender);
        _grantRole(ADMIN_ROLE, msg.sender);
        _grantRole(UPDATER_ROLE, msg.sender);

        // Initialize context ID with a unique value
        contextId = uint256(keccak256(abi.encodePacked(block.chainid, address(this), block.timestamp)));
    }

    /**
     * @notice Add stake to participate in entropy verification
     * @dev Required to submit entropy to prevent spam and enable slashing
     */
    function addStake() external payable {
        stakedAmount[msg.sender] += msg.value;
        emit StakeAdded(msg.sender, msg.value);
    }
    
    /**
     * @notice Withdraw staked amount
     * @param amount Amount to withdraw
     */
    function withdrawStake(uint256 amount) external {
        require(stakedAmount[msg.sender] >= amount, "Insufficient balance");
        require(block.timestamp > lastVerificationTime[msg.sender] + stakeCooldownPeriod, "Cooldown period active");
        
        stakedAmount[msg.sender] -= amount;
        
        (bool success, ) = msg.sender.call{value: amount}("");
        require(success, "Transfer failed");
        
        emit StakeWithdrawn(msg.sender, amount);
    }

    /**
     * @notice Verify a zero-knowledge proof of high-quality entropy
     * @dev Validates that the entropy meets quality thresholds without revealing the value
     * @param entropyCommitment Commitment to the entropy value (hash of entropy)
     * @param zkProof Zero-knowledge proof that entropy meets quality criteria
     * @param proofContext Context ID to prevent replay attacks
     * @return valid Whether the proof is valid
     * @return entropyScore Quality score of the entropy (0-1000)
     */
    function verifyZKEntropyProof(
        bytes32 entropyCommitment,
        bytes calldata zkProof,
        uint256 proofContext
    ) external whenNotPaused returns (bool valid, uint256 entropyScore) {
        if (entropyCommitment == bytes32(0)) revert InvalidCommitment();
        if (usedCommitments[entropyCommitment]) revert CommitmentAlreadyUsed();
        if (stakedAmount[msg.sender] < requiredStake) revert InsufficientStake(requiredStake, stakedAmount[msg.sender]);
        if (proofContext != contextId) revert InvalidContext(contextId, proofContext);
        
        totalContributions++;
        
        // Check if manually approved
        if (manuallyApproved[entropyCommitment]) {
            usedCommitments[entropyCommitment] = true;
            entropyScore = minRequiredScore; // Assign minimum acceptable score
            entropyScores[entropyCommitment] = entropyScore;
            entropyTiers[entropyCommitment] = determineEntropyTier(entropyScore);
            successfulVerifications++;
            
            emit EntropyVerified(entropyCommitment, entropyScore, entropyTiers[entropyCommitment]);
            return (true, entropyScore);
        }
        
        // Parse ZK proof components
        (
            uint256[2] memory a,
            uint256[2][2] memory b,
            uint256[2] memory c,
            uint256[4] memory publicInputs
        ) = parseZkProof(zkProof);
        
        // Public inputs: [entropy commitment, shannon entropy, min entropy, timestamp]
        bool proofValid = zkVerifier.verifyProof(a, b, c, publicInputs);
        if (!proofValid) {
            failedVerifications++;
            emit VerificationFailed(entropyCommitment, "Invalid ZK proof");
            
            // Slash for invalid proof
            if (slashAmount > 0 && stakedAmount[msg.sender] >= slashAmount) {
                stakedAmount[msg.sender] -= slashAmount;
                emit Slashed(msg.sender, slashAmount, "Invalid ZK proof");
            }
            
            return (false, 0);
        }
        
        // Extract entropy quality metrics from public inputs 
        uint256 shannonEntropy = publicInputs[1];
        uint256 minEntropy = publicInputs[2];
        uint256 timestamp = publicInputs[3];
        
        // Verify commitment matches
        if (bytes32(publicInputs[0]) != entropyCommitment) {
            failedVerifications++;
            emit VerificationFailed(entropyCommitment, "Commitment mismatch");
            return (false, 0);
        }
        
        // Check timelock if configured
        if (params.timeLockDuration > 0) {
            if (block.timestamp < timestamp + params.timeLockDuration) {
                emit VerificationFailed(entropyCommitment, "Timelock not satisfied");
                revert TimelockNotSatisfied(block.timestamp, timestamp + params.timeLockDuration);
            }
        }
        
        // Validate entropy quality metrics
        if (shannonEntropy < params.minShannonEntropy || minEntropy < params.minMinEntropy) {
            failedVerifications++;
            emit VerificationFailed(entropyCommitment, "Entropy quality too low");
            return (false, 0);
        }
        
        // Calculate entropy score (0-1000)
        entropyScore = calculateEntropyScore(shannonEntropy, minEntropy, entropyCommitment);
        
        // Check if score meets minimum requirement
        if (entropyScore < minRequiredScore) {
            failedVerifications++;
            emit VerificationFailed(entropyCommitment, "Score below threshold");
            
            // Slash for low quality entropy
            if (slashAmount > 0 && stakedAmount[msg.sender] >= slashAmount) {
                stakedAmount[msg.sender] -= slashAmount;
                emit Slashed(msg.sender, slashAmount, "Entropy score below threshold");
            }
            
            revert ScoreTooLow(entropyScore, minRequiredScore);
        }
        
        // Assign entropy tier based on score
        uint8 tier = determineEntropyTier(entropyScore);
        
        // Check if high tier is required
        if (params.requireHighTierEntropy && tier < 2) {
            failedVerifications++;
            emit VerificationFailed(entropyCommitment, "Tier too low");
            return (false, entropyScore);
        }
        
        // Mark commitment as used
        usedCommitments[entropyCommitment] = true;
        entropyScores[entropyCommitment] = entropyScore;
        entropyTiers[entropyCommitment] = tier;
        
        successfulVerifications++;
        
        // Emit events
        emit EntropyVerified(entropyCommitment, entropyScore, tier);
        emit QualityAssessmentComplete(entropyCommitment, shannonEntropy, minEntropy);
        
        // If verification is successful, update last verification time
        if (valid) {
            lastVerificationTime[msg.sender] = block.timestamp;
        }
        
        return (valid, entropyScore);
    }

    /**
     * @notice Calculate entropy score based on quality metrics
     * @param shannonEntropy Shannon entropy value (scaled by 1e18)
     * @param minEntropy Min-entropy value (scaled by 1e18)
     * @param commitment Entropy commitment for additional pattern analysis
     * @return score Quality score from 0-1000
     */
    function calculateEntropyScore(
        uint256 shannonEntropy, 
        uint256 minEntropy,
        bytes32 commitment
    ) public view returns (uint256 score) {
        // Shannon entropy has max theoretical value of 8 bits per byte
        // Min-entropy has max theoretical value of 8 bits per byte
        // Scale to 0-500 points each, for a total of 0-1000
        
        uint256 shannonScore = (shannonEntropy * 500) / (8 * 1e18);
        uint256 minEntropyScore = (minEntropy * 500) / (8 * 1e18);
        
        // Cap scores at maximum points
        shannonScore = shannonScore > 500 ? 500 : shannonScore;
        minEntropyScore = minEntropyScore > 500 ? 500 : minEntropyScore;
        
        // Apply additional heuristic scoring based on commitment patterns
        uint256 patternScore = 0;
        if (params.checkHistoricalPatterns) {
            patternScore = EntropyQualityLib.assessPatternQuality(commitment);
        }
        
        // Calculate weighted final score
        uint256 baseScore = shannonScore + minEntropyScore;
        uint256 adjustedScore = baseScore;
        
        // Apply pattern adjustment (±10%)
        if (patternScore > 0) {
            // Positive adjustment (up to +10%)
            adjustedScore = baseScore + ((baseScore * patternScore) / 1000);
        } else if (patternScore < 0) {
            // Negative adjustment (up to -10%)
            adjustedScore = baseScore - ((baseScore * uint256(-patternScore)) / 1000);
        }
        
        // Ensure score is within bounds
        if (adjustedScore > ENTROPY_MAX_SCORE) {
            return ENTROPY_MAX_SCORE;
        }
        
        return adjustedScore;
    }
    
    /**
     * @notice Determine entropy tier based on score
     * @param score Entropy quality score (0-1000)
     * @return tier Entropy tier (1-4, higher is better)
     */
    function determineEntropyTier(uint256 score) public pure returns (uint8) {
        if (score >= ENTROPY_TIER_3_THRESHOLD) {
            return 4; // Exceptional
        } else if (score >= ENTROPY_TIER_2_THRESHOLD) {
            return 3; // High
        } else if (score >= ENTROPY_TIER_1_THRESHOLD) {
            return 2; // Medium
        } else {
            return 1; // Basic
        }
    }

    /**
     * @notice Parse ZK proof bytes into component arrays
     * @param zkProof Raw proof bytes
     * @return a Proof component a
     * @return b Proof component b
     * @return c Proof component c
     * @return inputs Public inputs to the proof
     */
    function parseZkProof(
        bytes calldata zkProof
    ) public pure returns (
        uint256[2] memory a,
        uint256[2][2] memory b,
        uint256[2] memory c,
        uint256[4] memory inputs
    ) {
        // Enhanced validation
        if (zkProof.length < 384) revert ProofTooShort();
        if (zkProof.length > 416) revert ProofTooLong(); // Allow some extra data
        
        // Verify proof structure - check some basic validity conditions
        // (would normally check if points are on curve, but that's complex)
        
        // Extract components using assembly for efficiency
        assembly {
            // Proof component a (2 elements)
            mstore(a, calldataload(zkProof.offset))
            mstore(add(a, 32), calldataload(add(zkProof.offset, 32)))
            
            // Proof component b (2x2 elements)
            mstore(b, calldataload(add(zkProof.offset, 64)))
            mstore(add(b, 32), calldataload(add(zkProof.offset, 96)))
            mstore(add(b, 64), calldataload(add(zkProof.offset, 128)))
            mstore(add(b, 96), calldataload(add(zkProof.offset, 160)))
            
            // Proof component c (2 elements)
            mstore(c, calldataload(add(zkProof.offset, 192)))
            mstore(add(c, 32), calldataload(add(zkProof.offset, 224)))
            
            // Public inputs (4 elements)
            mstore(inputs, calldataload(add(zkProof.offset, 256)))
            mstore(add(inputs, 32), calldataload(add(zkProof.offset, 288)))
            mstore(add(inputs, 64), calldataload(add(zkProof.offset, 320)))
            mstore(add(inputs, 96), calldataload(add(zkProof.offset, 352)))
        }
        
        // Additional validation: verify inputs are within field modulus
        // Groth16 operates in Fr field with modulus < 2^256
        uint256 fieldModulus = 21888242871839275222246405745257275088548364400416034343698204186575808495617;
        for (uint i = 0; i < 2; i++) {
            if (a[i] >= fieldModulus) revert InvalidProofFormat();
        }
        for (uint i = 0; i < 2; i++) {
            for (uint j = 0; j < 2; j++) {
                if (b[i][j] >= fieldModulus) revert InvalidProofFormat();
            }
        }
        for (uint i = 0; i < 2; i++) {
            if (c[i] >= fieldModulus) revert InvalidProofFormat();
        }
        
        return (a, b, c, inputs);
    }
    
    /**
     * @notice Batch verify multiple ZK proofs for efficient gas usage
     * @param commitments Array of entropy commitments
     * @param zkProofs Array of ZK proofs
     * @param proofContext Context ID to prevent replay attacks
     * @return validProofs Array indicating which proofs were valid
     * @return scores Array of entropy scores for valid proofs
     */
    function batchVerifyZKProofs(
        bytes32[] calldata commitments,
        bytes[] calldata zkProofs,
        uint256 proofContext
    ) external whenNotPaused returns (
        bool[] memory validProofs,
        uint256[] memory scores
    ) {
        if (commitments.length != zkProofs.length) revert BatchSizeMismatch();
        if (commitments.length > 10) revert BatchTooLarge();
        if (proofContext != contextId) revert InvalidContext(contextId, proofContext);
        
        // Track gas consumption to prevent excessive usage
        uint256 gasAtStart = gasleft();
        
        validProofs = new bool[](commitments.length);
        scores = new uint256[](commitments.length);
        
        for (uint256 i = 0; i < commitments.length; i++) {
            // Skip if insufficient stake
            if (stakedAmount[msg.sender] < requiredStake) {
                continue;
            }
            
            // Skip already used commitments
            if (usedCommitments[commitments[i]]) {
                continue;
            }
            
            totalContributions++;
            
            // Check if manually approved
            if (manuallyApproved[commitments[i]]) {
                usedCommitments[commitments[i]] = true;
                scores[i] = minRequiredScore;
                entropyScores[commitments[i]] = scores[i];
                entropyTiers[commitments[i]] = determineEntropyTier(scores[i]);
                validProofs[i] = true;
                successfulVerifications++;
                emit EntropyVerified(commitments[i], scores[i], entropyTiers[commitments[i]]);
                continue;
            }
            
            try this.verifyIndividualProof(commitments[i], zkProofs[i]) returns (bool isValid, uint256 score) {
                if (isValid) {
                    usedCommitments[commitments[i]] = true;
                    entropyScores[commitments[i]] = score;
                    entropyTiers[commitments[i]] = determineEntropyTier(score);
                    validProofs[i] = true;
                    scores[i] = score;
                    successfulVerifications++;
                    emit EntropyVerified(commitments[i], score, entropyTiers[commitments[i]]);
                } else {
                    failedVerifications++;
                }
            } catch {
                failedVerifications++;
            }
        }
        
        // If any verification was successful, update last verification time
        for (uint256 i = 0; i < validProofs.length; i++) {
            if (validProofs[i]) {
                lastVerificationTime[msg.sender] = block.timestamp;
                break; // Only need to set once
            }
        }
        
        // Ensure batch verification doesn't consume excessive gas
        require(gasAtStart - gasleft() < 5_000_000, "Gas limit exceeded");
        
        return (validProofs, scores);
    }
    
    /**
     * @notice Helper function for batch verification that validates a single proof
     * @dev This is internal logic separated out for cleaner error handling in batch verification
     * @param commitment Entropy commitment
     * @param zkProof ZK proof
     * @return isValid Whether the proof is valid
     * @return score Entropy score
     */
    function verifyIndividualProof(
        bytes32 commitment, 
        bytes calldata zkProof
    ) external view returns (bool isValid, uint256 score) {
        // Only allow this contract to call itself
        require(msg.sender == address(this), "External calls not allowed");
        
        // Parse ZK proof
        (
            uint256[2] memory a,
            uint256[2][2] memory b,
            uint256[2] memory c,
            uint256[4] memory publicInputs
        ) = parseZkProof(zkProof);
        
        // Verify the proof matches the commitment
        if (bytes32(publicInputs[0]) != commitment) {
            return (false, 0);
        }
        
        // Verify the proof
        bool proofValid = zkVerifier.verifyProof(a, b, c, publicInputs);
        if (!proofValid) {
            return (false, 0);
        }
        
        // Extract and validate entropy metrics
        uint256 shannonEntropy = publicInputs[1];
        uint256 minEntropy = publicInputs[2];
        
        if (shannonEntropy < params.minShannonEntropy || minEntropy < params.minMinEntropy) {
            return (false, 0);
        }
        
        // Calculate score
        score = calculateEntropyScore(shannonEntropy, minEntropy, commitment);
        
        if (score < minRequiredScore) {
            return (false, score);
        }
        
        // Check tier if required
        uint8 tier = determineEntropyTier(score);
        if (params.requireHighTierEntropy && tier < 2) {
            return (false, score);
        }
        
        return (true, score);
    }

    /**
     * @notice Check if an entropy commitment has been verified
     * @param commitment The entropy commitment to check
     * @return isVerified Whether the commitment has been verified
     * @return tier The assigned entropy tier (0 if not verified)
     */
    function isEntropyVerified(bytes32 commitment) external view returns (bool isVerified, uint8 tier) {
        isVerified = usedCommitments[commitment];
        tier = entropyTiers[commitment];
        return (isVerified, tier);
    }
    
    /**
     * @notice Get verification statistics
     * @return total Total number of verification attempts
     * @return successful Number of successful verifications
     * @return failed Number of failed verifications
     * @return successRate Success rate in basis points (0-10000)
     */
    function getVerificationStats() external view returns (
        uint256 total,
        uint256 successful,
        uint256 failed,
        uint256 successRate
    ) {
        total = totalContributions;
        successful = successfulVerifications;
        failed = failedVerifications;
        
        if (total > 0) {
            successRate = (successful * 10000) / total;
        } else {
            successRate = 0;
        }
        
        return (total, successful, failed, successRate);
    }

    /**
     * @notice Request manual verification for a commitment when ZK verification fails
     * @param commitment The entropy commitment to verify manually
     */
    function requestManualVerification(bytes32 commitment) external {
        if (usedCommitments[commitment]) revert CommitmentAlreadyUsed();
        emit ManualVerificationRequested(msg.sender, commitment);
    }
    
    /**
     * @notice Approve a manual verification request
     * @param commitment The entropy commitment to approve
     */
    function approveManualVerification(bytes32 commitment) external onlyRole(ADMIN_ROLE) {
        if (usedCommitments[commitment]) revert CommitmentAlreadyUsed();
        if (manualApprovals[commitment][msg.sender]) revert AlreadyApproved();
        
        // Mark this admin's approval
        manualApprovals[commitment][msg.sender] = true;
        manualApprovalsCount[commitment]++;
        
        emit ManualVerificationApproved(msg.sender, commitment);
        
        // If reached required approvals, mark as manually approved
        if (manualApprovalsCount[commitment] >= requiredManualApprovals) {
            manuallyApproved[commitment] = true;
        }
    }
    
    /**
     * @notice Update the context ID to prevent replay attacks across different epochs/deployments
     * @dev This can be called periodically to refresh the context
     */
    function updateContext() external onlyRole(ADMIN_ROLE) {
        uint256 oldContext = contextId;
        contextId = uint256(keccak256(abi.encodePacked(block.chainid, address(this), block.timestamp)));
        emit ContextUpdated(oldContext, contextId);
    }

    // --- Multi-sig verifier update ---
    
    /**
     * @notice Propose a new ZK verifier contract (requires multi-sig)
     * @param _newVerifier Address of the proposed new verifier
     */
    function proposeVerifierChange(address _newVerifier) external onlyRole(MULTI_SIG_ROLE) {
        if (_newVerifier == address(0)) revert ZeroAddress();
        
        // Clear existing approvals for all members with MULTI_SIG_ROLE
        address[] memory approvers = getRoleMembers(MULTI_SIG_ROLE);
        for (uint i = 0; i < approvers.length; i++) {
            verifierChangeProposal.hasApproved[approvers[i]] = false;
        }
        
        // Reset proposal fields
        delete verifierChangeProposal.proposedVerifier;
        delete verifierChangeProposal.proposedAt;
        delete verifierChangeProposal.approvalCount;
        delete verifierChangeProposal.executed;
        
        // Create new proposal
        verifierChangeProposal.proposedVerifier = _newVerifier;
        verifierChangeProposal.proposedAt = block.timestamp;
        verifierChangeProposal.requiredApprovals = requiredApprovals;
        verifierChangeProposal.timelock = verifierChangeTimelockPeriod;
        
        // Auto-approve by proposer
        verifierChangeProposal.hasApproved[msg.sender] = true;
        verifierChangeProposal.approvalCount = 1;
        
        emit VerifierChangeProposed(msg.sender, _newVerifier);
    }
    
    /**
     * @notice Approve a verifier change proposal
     */
    function approveVerifierChange() external onlyRole(MULTI_SIG_ROLE) {
        if (verifierChangeProposal.proposedVerifier == address(0)) revert NoActiveProposal();
        if (verifierChangeProposal.hasApproved[msg.sender]) revert AlreadyApproved();
        
        verifierChangeProposal.hasApproved[msg.sender] = true;
        verifierChangeProposal.approvalCount++;
        
        emit VerifierChangeApproved(msg.sender, verifierChangeProposal.proposedVerifier);
    }
    
    /**
     * @notice Execute a verifier change after timelock and sufficient approvals
     */
    function executeVerifierChange() external onlyRole(MULTI_SIG_ROLE) {
        if (verifierChangeProposal.proposedVerifier == address(0)) revert NoActiveProposal();
        if (verifierChangeProposal.executed) revert AlreadyApproved();
        
        // Check timelock
        if (block.timestamp < verifierChangeProposal.proposedAt + verifierChangeProposal.timelock) {
            revert TimelockNotExpired(
                block.timestamp, 
                verifierChangeProposal.proposedAt + verifierChangeProposal.timelock
            );
        }
        
        // Check approvals
        if (verifierChangeProposal.approvalCount < verifierChangeProposal.requiredApprovals) {
            revert InsufficientApprovals(
                verifierChangeProposal.approvalCount,
                verifierChangeProposal.requiredApprovals
            );
        }
        
        // Execute the change
        address oldVerifier = address(zkVerifier);
        zkVerifier = IGroth16Verifier(verifierChangeProposal.proposedVerifier);
        verifierChangeProposal.executed = true;
        
        emit VerifierChangeExecuted(oldVerifier, verifierChangeProposal.proposedVerifier);
    }

    // --- Admin functions ---

    /**
     * @notice Set the ZK verifier contract
     * @param _zkVerifier New verifier contract address
     * @dev This function is retained for backwards compatibility but uses the multi-sig approach
     */
    function setZkVerifier(address _zkVerifier) external pure {
        // Remove backdoor and force usage of the multi-sig process
        revert("Use multi-sig proposeVerifierChange instead");
    }
    
    /**
     * @notice Set multi-sig parameters for verifier change
     * @param _requiredApprovals Number of approvals required
     * @param _timelockPeriod Timelock period in seconds
     */
    function setMultiSigParameters(
        uint256 _requiredApprovals, 
        uint256 _timelockPeriod
    ) external onlyRole(ADMIN_ROLE) {
        require(_requiredApprovals > 0, "Approvals must be > 0");
        requiredApprovals = _requiredApprovals;
        verifierChangeTimelockPeriod = _timelockPeriod;
    }
    
    /**
     * @notice Set staking parameters including cooldown
     * @param _requiredStake Amount required to stake for verification
     * @param _slashAmount Amount to slash for invalid submissions
     * @param _cooldownPeriod Time required between verification and withdrawal
     */
    function setStakingParameters(
        uint256 _requiredStake, 
        uint256 _slashAmount,
        uint256 _cooldownPeriod
    ) external onlyRole(ADMIN_ROLE) {
        requiredStake = _requiredStake;
        slashAmount = _slashAmount;
        stakeCooldownPeriod = _cooldownPeriod;
    }
    
    /**
     * @notice Set manual verification parameters
     * @param _requiredApprovals Number of approvals required for manual verification
     */
    function setManualVerificationParameters(uint256 _requiredApprovals) external onlyRole(ADMIN_ROLE) {
        require(_requiredApprovals > 0, "Approvals must be > 0");
        requiredManualApprovals = _requiredApprovals;
    }
    
    /**
     * @notice Update the verification key
     * @param _verificationKey New verification key hash
     */
    function setVerificationKey(bytes32 _verificationKey) external onlyRole(UPDATER_ROLE) {
        if (_verificationKey == bytes32(0)) revert InvalidCommitment();
        bytes32 oldKey = verificationKey;
        verificationKey = _verificationKey;
        emit VerificationKeyUpdated(oldKey, _verificationKey);
    }
    
    /**
     * @notice Set minimum required entropy score
     * @param _minScore New minimum score threshold
     */
    function setMinRequiredScore(uint256 _minScore) external onlyRole(ADMIN_ROLE) {
        if (_minScore < ENTROPY_MIN_SCORE || _minScore > ENTROPY_MAX_SCORE) {
            revert InvalidScoreThreshold();
        }
        uint256 oldScore = minRequiredScore;
        minRequiredScore = _minScore;
        emit MinScoreUpdated(oldScore, _minScore);
    }
    
    /**
     * @notice Update verification parameters
     * @param _minShannonEntropy Minimum Shannon entropy required
     * @param _minMinEntropy Minimum min-entropy required
     * @param _requireHighTierEntropy Whether to require high tier entropy
     * @param _defaultTier Default tier for verified entropy
     * @param _timeLockDuration Duration entropy must age before verification
     * @param _checkHistoricalPatterns Whether to check for historical patterns
     */
    function updateVerificationParams(
        uint256 _minShannonEntropy,
        uint256 _minMinEntropy,
        bool _requireHighTierEntropy,
        uint8 _defaultTier,
        uint256 _timeLockDuration,
        bool _checkHistoricalPatterns
    ) external onlyRole(ADMIN_ROLE) {
        params.minShannonEntropy = _minShannonEntropy;
        params.minMinEntropy = _minMinEntropy;
        params.requireHighTierEntropy = _requireHighTierEntropy;
        params.defaultTier = _defaultTier;
        params.timeLockDuration = _timeLockDuration;
        params.checkHistoricalPatterns = _checkHistoricalPatterns;
        
        emit VerificationParamsUpdated();
    }
    
    /**
     * @notice Pause the verifier
     */
    function pause() external onlyRole(ADMIN_ROLE) {
        _pause();
    }
    
    /**
     * @notice Unpause the verifier
     */
    function unpause() external onlyRole(ADMIN_ROLE) {
        _unpause();
    }
    
    /**
     * @notice Authorize an upgrade to the implementation
     * @param newImplementation Address of the new implementation
     */
    function _authorizeUpgrade(address newImplementation) internal override onlyRole(ADMIN_ROLE) {}
}

/**
 * @dev Minimal interface for a Groth16 ZK-SNARK verifier
 */
interface IGroth16Verifier {
    function verifyProof(
        uint256[2] memory a,
        uint256[2][2] memory b,
        uint256[2] memory c,
        uint256[4] memory input
    ) external view returns (bool);
}
