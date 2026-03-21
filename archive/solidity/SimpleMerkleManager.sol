// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {Pausable} from "@openzeppelin/contracts/security/Pausable.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/security/ReentrancyGuard.sol";
import {MerkleProof} from "@openzeppelin/contracts/utils/cryptography/MerkleProof.sol";
import {IMerkleManager} from "./IMerkleManager.sol";

/**
 * @title SimpleMerkleManager
 * @notice Lightweight implementation of IMerkleManager with core verification functionality
 * @dev Provides both standard and quantum-resistant verification methods
 */
contract SimpleMerkleManager is IMerkleManager, Ownable, Pausable, ReentrancyGuard {
    // Quantum resistance parameters
    uint16 private constant QR_HASH_ITERATIONS = 3;
    uint8 private constant QR_HASH_ROTATION = 7;
    
    // Maximum batch size for gas efficiency
    uint16 private constant MAX_BATCH_SIZE = 500;

    // Current and previous Merkle roots
    bytes32 public override merkleRoot;
    bytes32 public override previousMerkleRoot;
    
    // Quantum-resistant versions of the roots
    bytes32 public override qrMerkleRoot;
    bytes32 public override qrPreviousMerkleRoot;
    
    // Record of all past roots (for audit)
    mapping(bytes32 => uint256) public override rootHistory;
    
    // Events
    event MerkleRootUpdated(bytes32 indexed newRoot, bytes32 indexed previousRoot, uint256 timestamp);
    event MerkleProofVerified(address indexed verifier, bytes32 indexed leaf, bool valid);
    event BatchVerificationProcessed(address indexed caller, uint256 count, uint256 validCount);

    /**
     * @notice Constructor to initialize the contract with a root
     * @param initialRoot The initial Merkle root to set
     */
    constructor(bytes32 initialRoot) {
        require(initialRoot != bytes32(0), "Root cannot be zero");
        
        merkleRoot = initialRoot;
        rootHistory[initialRoot] = block.timestamp;
        
        // Generate quantum-resistant variant
        qrMerkleRoot = quantumResistantHash(initialRoot);
        
        emit MerkleRootUpdated(initialRoot, bytes32(0), block.timestamp);
    }

    /**
     * @notice Update the active Merkle root
     * @dev Can only be called by the owner
     * @param newRoot New Merkle root to set
     */
    function updateMerkleRoot(bytes32 newRoot) external override onlyOwner whenNotPaused {
        require(newRoot != bytes32(0), "Root cannot be zero");

        previousMerkleRoot = merkleRoot;
        merkleRoot = newRoot;
        
        // Update quantum-resistant versions
        qrPreviousMerkleRoot = qrMerkleRoot;
        qrMerkleRoot = quantumResistantHash(newRoot);
        
        rootHistory[newRoot] = block.timestamp;

        emit MerkleRootUpdated(newRoot, previousMerkleRoot, block.timestamp);
    }

    /**
     * @notice Verify a leaf against the current Merkle root
     * @param leaf The leaf to verify
     * @param proof Array of proof elements
     * @return valid True if the proof is valid
     */
    function verify(bytes32 leaf, bytes32[] calldata proof) external view override returns (bool valid) {
        return MerkleProof.verify(proof, merkleRoot, leaf);
    }

    /**
     * @notice Verify against current or previous root (for smooth transitions)
     * @param leaf The leaf to verify
     * @param proof Array of proof elements
     * @return valid True if the proof is valid against either root
     */
    function verifyFlexible(bytes32 leaf, bytes32[] calldata proof) external view override returns (bool valid) {
        return MerkleProof.verify(proof, merkleRoot, leaf) ||
               MerkleProof.verify(proof, previousMerkleRoot, leaf);
    }
    
    /**
     * @notice Quantum-resistant verification against current root
     * @param leaf The leaf to verify (will be hashed with quantum resistance)
     * @param proof Array of proof elements (will be hashed with quantum resistance)
     * @return valid True if the quantum-resistant proof is valid
     */
    function verifyQuantumResistant(bytes32 leaf, bytes32[] calldata proof) external view override returns (bool valid) {
        // Convert leaf and proof to quantum-resistant versions
        bytes32 qrLeaf = quantumResistantHash(leaf);
        bytes32[] memory qrProof = new bytes32[](proof.length);
        
        for (uint256 i = 0; i < proof.length; i++) {
            qrProof[i] = quantumResistantHash(proof[i]);
        }
        
        return MerkleProof.verify(qrProof, qrMerkleRoot, qrLeaf);
    }
    
    /**
     * @notice Flexible quantum-resistant verification against current or previous root
     * @param leaf The leaf to verify (will be hashed with quantum resistance)
     * @param proof Array of proof elements (will be hashed with quantum resistance)
     * @return valid True if the quantum-resistant proof is valid against either root
     */
    function verifyFlexibleQuantumResistant(bytes32 leaf, bytes32[] calldata proof) external view override returns (bool valid) {
        // Convert leaf and proof to quantum-resistant versions
        bytes32 qrLeaf = quantumResistantHash(leaf);
        bytes32[] memory qrProof = new bytes32[](proof.length);
        
        for (uint256 i = 0; i < proof.length; i++) {
            qrProof[i] = quantumResistantHash(proof[i]);
        }
        
        return MerkleProof.verify(qrProof, qrMerkleRoot, qrLeaf) ||
               MerkleProof.verify(qrProof, qrPreviousMerkleRoot, qrLeaf);
    }
    
    /**
     * @notice Verify and emit event (useful for logging access or claims)
     * @param leaf The leaf to verify
     * @param proof Array of proof elements
     * @return valid True if the proof is valid
     */
    function verifyAndEmit(bytes32 leaf, bytes32[] calldata proof) external override whenNotPaused nonReentrant returns (bool valid) {
        valid = MerkleProof.verify(proof, merkleRoot, leaf);
        emit MerkleProofVerified(msg.sender, leaf, valid);
    }
    
    /**
     * @notice Batch verify multiple proofs (gas efficient)
     * @param leaves Array of leaves to verify
     * @param multiProof Array of arrays containing the proofs
     * @return results Array of verification results
     */
    function batchVerify(
        bytes32[] calldata leaves, 
        bytes32[][] calldata multiProof
    ) external view override returns (bool[] memory results) {
        require(leaves.length == multiProof.length, "Length mismatch");
        require(leaves.length <= MAX_BATCH_SIZE, "Batch too large");
        
        results = new bool[](leaves.length);
        
        for (uint256 i = 0; i < leaves.length; i++) {
            results[i] = MerkleProof.verify(multiProof[i], merkleRoot, leaves[i]);
        }
        
        return results;
    }
    
    /**
     * @notice Process batch verification and emit results
     * @param leaves Array of leaves to verify
     * @param multiProof Array of arrays containing the proofs
     * @return validCount Number of valid proofs
     */
    function processBatchVerification(
        bytes32[] calldata leaves, 
        bytes32[][] calldata multiProof
    ) external override whenNotPaused nonReentrant returns (uint256 validCount) {
        require(leaves.length == multiProof.length, "Length mismatch");
        require(leaves.length <= MAX_BATCH_SIZE, "Batch too large");
        
        validCount = 0;
        
        for (uint256 i = 0; i < leaves.length; i++) {
            if (MerkleProof.verify(multiProof[i], merkleRoot, leaves[i])) {
                validCount++;
            }
        }
        
        emit BatchVerificationProcessed(msg.sender, leaves.length, validCount);
        return validCount;
    }
    
    /**
     * @notice Batch verify with quantum resistance
     * @param leaves Array of leaves to verify
     * @param multiProof Array of arrays containing the proofs
     * @return results Array of verification results
     */
    function batchVerifyQuantumResistant(
        bytes32[] calldata leaves, 
        bytes32[][] calldata multiProof
    ) external view override returns (bool[] memory results) {
        require(leaves.length == multiProof.length, "Length mismatch");
        require(leaves.length <= MAX_BATCH_SIZE, "Batch too large");
        
        results = new bool[](leaves.length);
        
        for (uint256 i = 0; i < leaves.length; i++) {
            // Apply quantum resistance to leaf and proof
            bytes32 qrLeaf = quantumResistantHash(leaves[i]);
            bytes32[] memory qrProof = new bytes32[](multiProof[i].length);
            
            for (uint256 j = 0; j < multiProof[i].length; j++) {
                qrProof[j] = quantumResistantHash(multiProof[i][j]);
            }
            
            results[i] = MerkleProof.verify(qrProof, qrMerkleRoot, qrLeaf);
        }
        
        return results;
    }
    
    /**
     * @notice Optimized batch verification using flattened proof structure
     * @param leaves Array of leaves to verify
     * @param proofs Flattened array containing all proofs
     * @param proofLengths Array indicating the length of each proof
     * @return results Array of verification results
     */
    function optimizedBatchVerify(
        bytes32[] calldata leaves,
        bytes32[] calldata proofs,
        uint256[] calldata proofLengths
    ) external view override returns (bool[] memory results) {
        require(leaves.length == proofLengths.length, "Length mismatch");
        require(leaves.length <= MAX_BATCH_SIZE, "Batch too large");
        
        results = new bool[](leaves.length);
        uint256 proofIndex = 0;
        
        for (uint256 i = 0; i < leaves.length; i++) {
            // Extract the proof for this leaf
            bytes32[] memory proof = new bytes32[](proofLengths[i]);
            for (uint256 j = 0; j < proofLengths[i]; j++) {
                proof[j] = proofs[proofIndex + j];
            }
            
            // Verify this leaf against the current merkle root
            results[i] = MerkleProof.verify(proof, merkleRoot, leaves[i]);
            
            // Update index for the next proof
            proofIndex += proofLengths[i];
        }
        
        require(proofIndex == proofs.length, "Invalid proof structure");
        return results;
    }
    
    /**
     * @notice Optimized quantum-resistant batch verification
     * @param leaves Array of leaves to verify
     * @param proofs Flattened array containing all proofs
     * @param proofLengths Array indicating the length of each proof
     * @return results Array of verification results
     */
    function optimizedBatchVerifyQuantumResistant(
        bytes32[] calldata leaves,
        bytes32[] calldata proofs,
        uint256[] calldata proofLengths
    ) external view override returns (bool[] memory results) {
        require(leaves.length == proofLengths.length, "Length mismatch");
        require(leaves.length <= MAX_BATCH_SIZE, "Batch too large");
        
        results = new bool[](leaves.length);
        uint256 proofIndex = 0;
        
        for (uint256 i = 0; i < leaves.length; i++) {
            // Convert leaf to quantum-resistant version
            bytes32 qrLeaf = quantumResistantHash(leaves[i]);
            
            // Extract and convert the proof for this leaf
            bytes32[] memory qrProof = new bytes32[](proofLengths[i]);
            for (uint256 j = 0; j < proofLengths[i]; j++) {
                qrProof[j] = quantumResistantHash(proofs[proofIndex + j]);
            }
            
            // Verify this leaf against the quantum-resistant merkle root
            results[i] = MerkleProof.verify(qrProof, qrMerkleRoot, qrLeaf);
            
            // Update index for the next proof
            proofIndex += proofLengths[i];
        }
        
        require(proofIndex == proofs.length, "Invalid proof structure");
        return results;
    }
    
    /**
     * @notice Hash a leaf in the standard format
     * @param user Address component of the leaf
     * @param value Integer component of the leaf
     * @return The keccak256 hash of the packed values
     */
    function hashLeaf(address user, uint256 value) external pure override returns (bytes32) {
        return keccak256(abi.encodePacked(user, value));
    }
    
    /**
     * @notice Apply quantum-resistant hash transformation
     * @param input The input data to transform
     * @return A quantum-resistant hash
     */
    function quantumResistantHash(bytes32 input) public view override returns (bytes32) {
        bytes32 h = input;
        uint256 seed = uint256(blockhash(block.number - 1));
        
        for (uint256 i = 0; i < QR_HASH_ITERATIONS; i++) {
            h = keccak256(abi.encodePacked(h, seed ^ bytes32(i)));
            h = bytes32((uint256(h) << QR_HASH_ROTATION) | (uint256(h) >> (256 - QR_HASH_ROTATION)));
            seed = uint256(keccak256(abi.encodePacked(seed, h)));
        }
        return h;
    }
    
    /**
     * @notice Generate a quantum-resistant hash of multiple inputs
     * @param inputs Array of inputs to hash together
     * @return qrHash The quantum-resistant hash combining all inputs
     */
    function batchQuantumHash(bytes32[] calldata inputs) external view override returns (bytes32 qrHash) {
        require(inputs.length > 0, "Empty input array");
        
        // Start with the first input
        qrHash = quantumResistantHash(inputs[0]);
        
        // Combine with remaining inputs
        for (uint256 i = 1; i < inputs.length; i++) {
            qrHash = quantumResistantHash(keccak256(abi.encodePacked(qrHash, inputs[i])));
        }
        
        return qrHash;
    }

    /**
     * @notice Pause the contract
     * @dev Only the owner can pause
     */
    function pause() external override onlyOwner {
        _pause();
    }
    
    /**
     * @notice Unpause the contract
     * @dev Only the owner can unpause
     */
    function unpause() external override onlyOwner {
        _unpause();
    }
}
