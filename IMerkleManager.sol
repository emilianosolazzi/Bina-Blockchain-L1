// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title IMerkleManager
 * @notice Interface for MerkleManager with standard and quantum-resistant verification methods
 * @dev Includes batch processing optimizations for 10M+ scale operations
 */
interface IMerkleManager {
    // Single verification methods
    function verify(bytes32 leaf, bytes32[] calldata proof) external view returns (bool);
    function verifyFlexible(bytes32 leaf, bytes32[] calldata proof) external view returns (bool);
    function verifyQuantumResistant(bytes32 leaf, bytes32[] calldata proof) external view returns (bool);
    function verifyFlexibleQuantumResistant(bytes32 leaf, bytes32[] calldata proof) external view returns (bool);
    function verifyAndEmit(bytes32 leaf, bytes32[] calldata proof) external returns (bool valid);
    
    // Batch verification methods
    function batchVerify(bytes32[] calldata leaves, bytes32[][] calldata multiProof) external view returns (bool[] memory results);
    function processBatchVerification(bytes32[] calldata leaves, bytes32[][] calldata multiProof) external returns (uint256 validCount);
    function batchVerifyQuantumResistant(bytes32[] calldata leaves, bytes32[][] calldata multiProof) external view returns (bool[] memory results);
    
    // Optimized batch verification with flattened proof structure
    function optimizedBatchVerify(bytes32[] calldata leaves, bytes32[] calldata proofs, uint256[] calldata proofLengths) 
        external view returns (bool[] memory results);
    function optimizedBatchVerifyQuantumResistant(bytes32[] calldata leaves, bytes32[] calldata proofs, uint256[] calldata proofLengths) 
        external view returns (bool[] memory results);
    
    // Utility functions
    function hashLeaf(address user, uint256 value) external pure returns (bytes32);
    function quantumResistantHash(bytes32 input) external view returns (bytes32);
    function batchQuantumHash(bytes32[] calldata inputs) external view returns (bytes32 qrHash);
    
    // Management functions
    function updateMerkleRoot(bytes32 newRoot) external;
    function pause() external;
    function unpause() external;
    
    // View functions to get current state
    function merkleRoot() external view returns (bytes32);
    function previousMerkleRoot() external view returns (bytes32);
    function qrMerkleRoot() external view returns (bytes32);
    function qrPreviousMerkleRoot() external view returns (bytes32);
    function rootHistory(bytes32 root) external view returns (uint256);
}
