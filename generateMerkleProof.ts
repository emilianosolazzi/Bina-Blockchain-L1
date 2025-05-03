import keccak256 from 'keccak256';
import { MerkleTree } from 'merkletreejs';
import { ethers } from 'ethers';

/**
 * Hashes a leaf using abi.encodePacked(address, uint256), then keccak256
 * @param address Ethereum address (checksummed)
 * @param value uint256 value (e.g. claim amount, ID, etc.)
 * @returns A keccak256 hash representing the leaf
 */
export function hashLeaf(address: string, value: string | number | bigint): Buffer {
  const packed = ethers.utils.solidityPack(['address', 'uint256'], [address, value]);
  return Buffer.from(ethers.utils.keccak256(packed).slice(2), 'hex');
}

/**
 * Generates Merkle tree and proof for a specific user leaf
 * @param entries Array of [address, value] pairs
 * @param targetAddress Target address to generate proof for
 * @param targetValue Value associated with the target address
 */
export function generateMerkleProof(
  entries: [string, string | number | bigint][],
  targetAddress: string,
  targetValue: string | number | bigint
) {
  const leaves = entries.map(([addr, val]) => hashLeaf(addr, val));
  const tree = new MerkleTree(leaves, keccak256, { sortPairs: true });

  const targetLeaf = hashLeaf(targetAddress, targetValue);
  const proof = tree.getHexProof(targetLeaf);
  const root = tree.getHexRoot();

  return {
    merkleRoot: root,
    leaf: `0x${targetLeaf.toString('hex')}`,
    proof,
  };
}

/**
 * Validates a Merkle proof against a root and leaf
 * @param root The Merkle root (hex string)
 * @param leaf The leaf to verify (hex string)
 * @param proof Array of proof elements (hex strings)
 * @returns Boolean indicating whether the proof is valid
 */
export function validateMerkleProof(
  root: string,
  leaf: string,
  proof: string[]
): boolean {
  const tree = new MerkleTree([], keccak256, { sortPairs: true });
  return tree.verify(proof, leaf, root);
}
