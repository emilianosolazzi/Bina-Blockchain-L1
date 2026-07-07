import {
  Contract,
  JsonRpcProvider,
  dataSlice,
  getAddress,
  keccak256,
  solidityPacked,
  toBeHex,
  toUtf8Bytes,
  zeroPadValue,
} from 'ethers';

export const BINA_ORACLE_ABI = [
  'function getLatestSeed(bytes32 purpose) view returns (bytes32 seed, uint64 height, uint64 btcHeight, bytes32 blockHash)',
  'function deriveWord(bytes32 purpose, bytes32 salt, address consumer) view returns (bytes32)',
  'function randomUintFor(bytes32 purpose, bytes32 salt, address consumer, uint256 upperBound) view returns (uint256)',
  'function isPQResistant() pure returns (bool)',
  'function pqSecurityBits() pure returns (uint8)',
  'function signingScheme() pure returns (string)',
] as const;

export const PURPOSES = {
  generic: 'BINA_GENERIC_UTILITY',
  validatorSelection: 'BINA_VALIDATOR_SELECTION',
  batchSeed: 'BINA_BATCH_SEED',
  defi: 'BINA_DEFI',
  gaming: 'BINA_GAMING',
  ai: 'BINA_AI',
} as const;

export type PurposeName = keyof typeof PURPOSES;

export interface BinaOracleClientConfig {
  rpcUrl: string;
  oracleAddress: string;
  chainId: bigint;
}

export interface LatestRandomnessProof {
  seed: string;
  height: number;
  btcHeight: number;
  blockHash: string;
  purpose: string;
  purposeHash: string;
  pqResistant: boolean;
  pqSecurityBits: number;
  signingScheme: string;
  source: string;
}

export interface DerivedRandomnessProof extends LatestRandomnessProof {
  salt: string;
  saltHash: string;
  consumer: string;
  consumerAddress: string;
  requestId: number;
  randomWord: string;
  derivation: string;
}

export interface BoundedRandomnessProof extends DerivedRandomnessProof {
  number: number;
  max: number;
}

export function purposeHash(purpose: string): string {
  return keccak256(toUtf8Bytes(purpose));
}

export function saltHash(salt: string): string {
  return keccak256(toUtf8Bytes(salt));
}

export function pseudoConsumerAddress(consumerId: string): string {
  const digest = keccak256(solidityPacked(['string', 'string'], ['BINA_API_CONSUMER_V1', consumerId]));
  return getAddress(dataSlice(digest, 0, 20));
}

export function createOracle(config: BinaOracleClientConfig): Contract {
  const provider = new JsonRpcProvider(config.rpcUrl, Number(config.chainId));
  return new Contract(config.oracleAddress, BINA_ORACLE_ABI, provider);
}

export async function latestRandomness(
  oracle: Contract,
  purpose = PURPOSES.generic,
): Promise<LatestRandomnessProof> {
  const purposeHashValue = purposeHash(purpose);
  const [seed, height, btcHeight, blockHash] = await oracle.getLatestSeed(purposeHashValue);
  const [pqResistant, pqBits, scheme] = await Promise.all([
    oracle.isPQResistant(),
    oracle.pqSecurityBits(),
    oracle.signingScheme(),
  ]);

  return {
    seed,
    height: Number(height),
    btcHeight: Number(btcHeight),
    blockHash,
    purpose,
    purposeHash: purposeHashValue,
    pqResistant,
    pqSecurityBits: Number(pqBits),
    signingScheme: scheme,
    source: 'BINA L1 via BinaOracle',
  };
}

export async function deriveRandomWord(
  oracle: Contract,
  purpose: string,
  salt: string,
  consumer: string,
): Promise<DerivedRandomnessProof> {
  const latest = await latestRandomness(oracle, purpose);
  const saltHashValue = saltHash(salt);
  const consumerAddress = pseudoConsumerAddress(consumer);
  const randomWord = await oracle.deriveWord(latest.purposeHash, saltHashValue, consumerAddress);

  return {
    ...latest,
    salt,
    saltHash: saltHashValue,
    consumer,
    consumerAddress,
    requestId: 0,
    randomWord,
    derivation: 'keccak256(BINA_EVM_UTILITY_V1, chainId, oracle, seed, purpose, salt, consumerAddress, requestId)',
  };
}

export async function randomNumber(
  oracle: Contract,
  purpose: string,
  salt: string,
  consumer: string,
  max: number,
): Promise<BoundedRandomnessProof> {
  if (!Number.isSafeInteger(max) || max <= 0) {
    throw new Error('max must be a positive safe integer');
  }

  const proof = await deriveRandomWord(oracle, purpose, salt, consumer);
  const number = Number(BigInt(proof.randomWord) % BigInt(max));
  return { ...proof, number, max };
}

export function deterministicShuffle<T>(items: readonly T[], seed: string, salt: string): T[] {
  const shuffled = [...items];
  for (let i = shuffled.length - 1; i > 0; i--) {
    const iBytes = zeroPadValue(toBeHex(i), 32);
    const hash = keccak256(solidityPacked(['bytes32', 'bytes32', 'bytes32'], [seed, saltHash(salt), iBytes]));
    const j = Number(BigInt(hash) % BigInt(i + 1));
    [shuffled[i], shuffled[j]] = [shuffled[j], shuffled[i]];
  }
  return shuffled;
}
