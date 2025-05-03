// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title IMerkle
 * @notice Interface for Merkle tree verification operations
 * @dev Provides a standardized interface for verifying Merkle proofs and supporting operations
 */
interface IMerkle {
    /**
     * @notice Verifies a Merkle proof for a leaf against the current root
     * @param leaf The Merkle leaf (typically a hash of some data)
     * @param proof Array of sibling hashes from leaf to root
     * @return valid True if the proof is valid
     */
    function verify(bytes32 leaf, bytes32[] calldata proof) external view returns (bool valid);
    
    /**
     * @notice Verifies a Merkle proof against either current or previous root (for seamless updates)
     * @param leaf The Merkle leaf
     * @param proof Array of sibling hashes from leaf to root
     * @return valid True if the proof is valid against either root
     */
    function verifyFlexible(bytes32 leaf, bytes32[] calldata proof) external view returns (bool valid);
    
    /**
     * @notice Verify multiple leaves against the current root in a single transaction
     * @param leaves Array of Merkle leaves
     * @param multiProof Array of arrays containing proofs for each leaf
     * @return results Array of booleans indicating validity of each proof
     */
    function batchVerify(
        bytes32[] calldata leaves, 
        bytes32[][] calldata multiProof
    ) external view returns (bool[] memory results);
    
    /**
     * @notice Verify multiple leaves with an optimized flattened proof structure
     * @dev Saves gas by using a single array for all proofs with length indicators
     * @param leaves Array of Merkle leaves
     * @param proofs Flattened array containing all proof elements concatenated
     * @param proofLengths Array indicating the length of each proof
     * @return results Array of booleans indicating validity of each proof
     */
    function optimizedBatchVerify(
        bytes32[] calldata leaves,
        bytes32[] calldata proofs,
        uint256[] calldata proofLengths
    ) external view returns (bool[] memory results);
    
    /**
     * @notice Creates a leaf hash from an address and value
     * @param user Address component of the leaf
     * @param value Integer component of the leaf
     * @return The keccak256 hash combining the inputs
     */
    function hashLeaf(address user, uint256 value) external pure returns (bytes32);
    
    /**
     * @notice Returns the current Merkle root
     * @return The current root hash
     */
    function merkleRoot() external view returns (bytes32);
    
    /**
     * @notice Returns the previous Merkle root (if available)
     * @return The previous root hash
     */
    function previousMerkleRoot() external view returns (bytes32);
}
