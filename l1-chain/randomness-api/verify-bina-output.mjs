#!/usr/bin/env node
// Public verifier for BinaOracle outputs (checklist item #6).
//
// Cross-checks what a BinaOracle contract stored on an EVM chain against the
// live BINA L1 node that produced the block, field by field. Anyone can run
// this with zero npm dependencies — plain Node 18+ (built-in fetch) talking
// to two public endpoints:
//
//   1. BINA node HTTP API   GET {--node}/block/{height}
//   2. EVM JSON-RPC         eth_call getOutput(bytes32) / getAttestationStatus(bytes32)
//
// Usage:
//   node verify-bina-output.mjs \
//     --node   http://127.0.0.1:9944 \
//     --rpc    http://127.0.0.1:8545 \
//     --oracle 0xYourBinaOracleAddress \
//     --height 86
//
// Exit code 0 = every field matches (PASS). Exit code 1 = any mismatch or
// the output is not on-chain.

const SEL_GET_OUTPUT = '0x3bd60483'; // getOutput(bytes32)
const SEL_GET_ATTESTATION_STATUS = '0xa3e2887b'; // getAttestationStatus(bytes32)

function parseArgs(argv) {
  const args = {};
  for (let i = 2; i < argv.length; i += 2) {
    if (!argv[i].startsWith('--') || argv[i + 1] === undefined) {
      throw new Error(`bad argument pair near "${argv[i]}"`);
    }
    args[argv[i].slice(2)] = argv[i + 1];
  }
  for (const required of ['node', 'rpc', 'oracle', 'height']) {
    if (!args[required]) throw new Error(`missing --${required}`);
  }
  return args;
}

function hex32(value) {
  const clean = value.toLowerCase().replace(/^0x/, '');
  if (!/^[0-9a-f]{64}$/.test(clean)) throw new Error(`expected 32-byte hex, got "${value}"`);
  return '0x' + clean;
}

async function fetchBinaBlock(nodeUrl, height) {
  const res = await fetch(`${nodeUrl.replace(/\/$/, '')}/block/${height}`);
  if (!res.ok) throw new Error(`BINA node returned HTTP ${res.status} for /block/${height}`);
  return res.json();
}

async function ethCall(rpcUrl, to, data) {
  const res = await fetch(rpcUrl, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'eth_call', params: [{ to, data }, 'latest'] }),
  });
  const body = await res.json();
  if (body.error) throw new Error(`eth_call reverted or failed: ${JSON.stringify(body.error)}`);
  return body.result;
}

// getOutput returns StoredOutput: 11 static fields = 11 ABI words.
function decodeStoredOutput(hexResult) {
  const raw = hexResult.replace(/^0x/, '');
  if (raw.length < 64 * 11) throw new Error(`getOutput return too short (${raw.length / 2} bytes)`);
  const word = (i) => raw.slice(i * 64, (i + 1) * 64);
  return {
    randomnessOutput: '0x' + word(0),
    nullifier: '0x' + word(1),
    binaMiner: '0x' + word(2).slice(0, 40), // bytes20, left-aligned
    height: BigInt('0x' + word(3)),
    btcHeight: BigInt('0x' + word(4)),
    minedTimestamp: BigInt('0x' + word(5)),
    workBits: BigInt('0x' + word(6)),
    falconVerified: BigInt('0x' + word(7)) === 1n,
    proofHash: '0x' + word(8),
    attestationCount: BigInt('0x' + word(9)),
    publisher: '0x' + word(10).slice(24), // address, right-aligned
  };
}

function decodeAttestationStatus(hexResult) {
  const raw = hexResult.replace(/^0x/, '');
  const word = (i) => raw.slice(i * 64, (i + 1) * 64);
  return {
    count: BigInt('0x' + word(0)),
    threshold: BigInt('0x' + word(1)),
    finalized: BigInt('0x' + word(2)) === 1n,
  };
}

function check(label, onChain, fromNode) {
  const a = typeof onChain === 'bigint' ? onChain.toString() : String(onChain).toLowerCase();
  const b = typeof fromNode === 'bigint' ? fromNode.toString() : String(fromNode).toLowerCase();
  const ok = a === b;
  console.log(`  ${ok ? 'PASS' : 'FAIL'}  ${label.padEnd(18)} chain=${a}  node=${b}`);
  return ok;
}

async function main() {
  const args = parseArgs(process.argv);
  const height = Number(args.height);

  console.log(`Fetching BINA block ${height} from ${args.node} ...`);
  const block = await fetchBinaBlock(args.node, height);
  const blockHash = hex32(block.block_hash);
  console.log(`BINA block hash: ${blockHash}\n`);

  console.log(`Querying oracle ${args.oracle} via ${args.rpc} ...`);
  const outputHex = await ethCall(args.rpc, args.oracle, SEL_GET_OUTPUT + blockHash.slice(2));
  const stored = decodeStoredOutput(outputHex);
  const statusHex = await ethCall(args.rpc, args.oracle, SEL_GET_ATTESTATION_STATUS + blockHash.slice(2));
  const status = decodeAttestationStatus(statusHex);

  console.log(`\nField-by-field verification (on-chain vs live BINA node):`);
  const results = [
    check('randomnessOutput', stored.randomnessOutput, hex32(block.randomness_output)),
    check('nullifier', stored.nullifier, hex32(block.nullifier)),
    check('binaMiner', stored.binaMiner, '0x' + block.miner_address.toLowerCase().replace(/^0x/, '')),
    check('height', stored.height, BigInt(block.height)),
    check('btcHeight', stored.btcHeight, BigInt(block.btc_height)),
    check('minedTimestamp', stored.minedTimestamp, BigInt(block.mined_timestamp_secs)),
    check('workBits', stored.workBits, BigInt(block.zero_bits)),
  ];

  console.log(`\nAttestation status: ${status.count}/${status.threshold} publishers, finalized=${status.finalized}`);
  console.log(`Proof hash on-chain: ${stored.proofHash}`);
  console.log(`Finalizing publisher: ${stored.publisher}`);
  console.log(`falconVerified flag (publisher-asserted, not re-proven on-chain): ${stored.falconVerified}`);

  const allPass = results.every(Boolean) && status.finalized;
  console.log(`\n${allPass ? 'VERIFIED: on-chain output matches the live BINA block.' : 'MISMATCH: on-chain output does NOT match the BINA node.'}`);
  process.exit(allPass ? 0 : 1);
}

main().catch((err) => {
  console.error(`verifier error: ${err.message}`);
  process.exit(1);
});
