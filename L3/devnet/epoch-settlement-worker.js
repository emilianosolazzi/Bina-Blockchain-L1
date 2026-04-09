const fs = require('fs/promises');
const path = require('path');
const { ethers } = require('ethers');

const EPOCH_SETTLEMENT_ABI = [
  'function commitEpoch(bytes32 merkleRoot,uint32 leafCount,string dataUri,uint64 sourceChainId,bytes32 sourceRef) returns (uint256)',
  'function nextEpochId() view returns (uint256)',
];

async function loadJson(filePath) {
  const raw = await fs.readFile(filePath, 'utf8');
  return JSON.parse(raw);
}

function requireEnv(name) {
  const value = process.env[name];
  if (!value) {
    throw new Error(`Missing environment variable: ${name}`);
  }
  return value;
}

function normalizeEpochInput(input) {
  if (!input.merkleRoot || !ethers.isHexString(input.merkleRoot, 32)) {
    throw new Error('Epoch input must include a 32-byte merkleRoot');
  }
  if (!Number.isInteger(input.leafCount) || input.leafCount <= 0) {
    throw new Error('Epoch input must include a positive integer leafCount');
  }

  return {
    merkleRoot: input.merkleRoot,
    leafCount: input.leafCount,
    dataUri: input.dataUri || '',
    sourceChainId: BigInt(input.sourceChainId || 42161),
    sourceRef: input.sourceRef && ethers.isHexString(input.sourceRef, 32)
      ? input.sourceRef
      : ethers.ZeroHash,
  };
}

async function main() {
  const rpcUrl = requireEnv('L3_RPC_URL');
  const privateKey = requireEnv('L3_OPERATOR_PRIVATE_KEY');
  const settlementAddress = requireEnv('L3_EPOCH_SETTLEMENT_ADDRESS');
  const inputFile = requireEnv('L3_EPOCH_INPUT_FILE');

  const inputPath = path.resolve(inputFile);
  const epochInput = normalizeEpochInput(await loadJson(inputPath));

  const provider = new ethers.JsonRpcProvider(rpcUrl);
  const signer = new ethers.Wallet(privateKey, provider);
  const settlement = new ethers.Contract(settlementAddress, EPOCH_SETTLEMENT_ABI, signer);

  const nextEpochId = await settlement.nextEpochId();
  console.log(`Next epoch id: ${nextEpochId}`);
  console.log(`Submitting merkle root: ${epochInput.merkleRoot}`);

  const tx = await settlement.commitEpoch(
    epochInput.merkleRoot,
    epochInput.leafCount,
    epochInput.dataUri,
    epochInput.sourceChainId,
    epochInput.sourceRef
  );

  console.log(`Submitted tx: ${tx.hash}`);
  const receipt = await tx.wait();
  console.log(`Confirmed in block ${receipt.blockNumber}`);
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});