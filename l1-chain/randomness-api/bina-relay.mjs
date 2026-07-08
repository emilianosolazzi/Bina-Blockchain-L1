#!/usr/bin/env node
/**
 * bina-relay.mjs — BinaOracle publisher relay daemon
 *
 * Watches the BINA L1 node, waits N confirmations, then submits
 * finalized block outputs to the EVM oracle. No manual field copying.
 *
 * Prerequisites:
 *   npm install   (first time only, installs ethers)
 *
 * Usage:
 *   node bina-relay.mjs \
 *     --bina    http://127.0.0.1:9944 \
 *     --evm     http://127.0.0.1:8545 \
 *     --oracle  0xYourBinaOracleAddress \
 *     --purpose BINA_GENERIC_UTILITY \
 *     --key     $env:PUBLISHER_KEY \
 *   [ --confirmations  20 ]           blocks to wait before relaying (default: 20)
 *   [ --poll-ms        2000 ]         poll interval in ms (default: 2000)
 *   [ --proof-uri-base https://... ]  proof URI prefix, appended with /block/{height}
 *
 * The relay tracks state in memory only. Restart is safe — the contract's
 * AlreadyAttested guard makes re-submission a no-op.
 */

import { ethers } from 'ethers';

// ── Oracle ABI (read + write paths the relay uses) ─────────────────────────

const ORACLE_ABI = [
  'function submitOutput((uint64,bytes32,bytes32,bytes32,bytes20,uint64,bytes32,uint64,uint8,bytes32,bytes32,bool),bytes32,bytes,string) external',
  'function getAttestationStatus(bytes32 blockHash) view returns (uint32 count, uint32 threshold, bool finalized)',
  'function hasAttested(bytes32 blockHash, address publisher) view returns (bool)',
  'function publishers(address) view returns (bool)',
  'function publisherBonds(address) view returns (uint256)',
  'function lockedBonds(address) view returns (uint256)',
];

// ── Arg parsing ────────────────────────────────────────────────────────────

function parseArgs(argv) {
  const a = {};
  for (let i = 2; i < argv.length; i += 2) {
    if (!argv[i].startsWith('--') || !argv[i + 1]) throw new Error(`bad argument near "${argv[i]}"`);
    a[argv[i].slice(2)] = argv[i + 1];
  }
  for (const r of ['bina', 'evm', 'oracle', 'purpose', 'key']) {
    if (!a[r]) throw new Error(`missing required --${r}`);
  }
  return {
    binaUrl:       a.bina,
    evmUrl:        a.evm,
    oracleAddress: a.oracle,
    purpose:       a.purpose,
    privateKey:    a.key,
    confirmations: Number(a.confirmations ?? 20),
    pollMs:        Number(a['poll-ms'] ?? 2000),
    proofUriBase:  a['proof-uri-base'] ?? '',
  };
}

// ── BINA node helpers ──────────────────────────────────────────────────────

async function binaLatestHeight(binaUrl) {
  const res = await fetch(`${binaUrl}/chain/latest`);
  if (!res.ok) throw new Error(`/chain/latest HTTP ${res.status}`);
  const body = await res.json();
  const h = body.height ?? body.latest_height;
  if (h == null) throw new Error('/chain/latest missing "height" field');
  return Number(h);
}

async function binaBlock(binaUrl, height) {
  const res = await fetch(`${binaUrl}/block/${height}`);
  if (!res.ok) throw new Error(`/block/${height} HTTP ${res.status}`);
  return res.json();
}

// ── BinaOutput tuple builder ───────────────────────────────────────────────

function buildBinaOutput(block) {
  const h32 = (s) => '0x' + s.replace(/^0x/, '').padStart(64, '0');
  const h20 = (s) => '0x' + s.replace(/^0x/, '').slice(-40).padStart(40, '0');
  return [
    BigInt(block.height),
    h32(block.block_hash),
    h32(block.randomness_output),
    h32(block.nullifier),
    h20(block.miner_address),
    BigInt(block.btc_height),
    h32(block.btc_seed),
    BigInt(block.mined_timestamp_secs),  // seconds — not ms
    Math.min(block.zero_bits, 64),       // uint8, practical cap at 64
    h32(block.claim_digest),
    h32(block.election_score),
    true,  // falconVerified: publisher attests based on own node validation
  ];
}

// ── In-memory state  ───────────────────────────────────────────────────────
// Map<blockHash, { height, status, txHash, attempts, error }>
// 'done'    — finalized or attested, nothing more to do
// 'pending' — submission in progress
// 'error'   — last attempt failed, will retry

const state = new Map();
const setState = (h, patch) => state.set(h, { ...state.get(h), ...patch });

// ── Relay tick ─────────────────────────────────────────────────────────────

async function tick(cfg, oracle, wallet, purposeHash) {
  const latestHeight = await binaLatestHeight(cfg.binaUrl);
  const relayHeight  = latestHeight - cfg.confirmations;
  if (relayHeight <= 0) return;

  const block     = await binaBlock(cfg.binaUrl, relayHeight);
  const blockHash = '0x' + block.block_hash.replace(/^0x/, '');

  // Already done locally?
  if (state.get(blockHash)?.status === 'done') return;

  // Already finalized on-chain?
  const { finalized } = await oracle.getAttestationStatus(blockHash);
  if (finalized) {
    setState(blockHash, { height: relayHeight, status: 'done', txHash: null });
    console.log(`[relay] h=${relayHeight} ${blockHash.slice(0, 10)}…  finalized on-chain (skipping)`);
    return;
  }

  // Already attested by this publisher?
  if (await oracle.hasAttested(blockHash, wallet.address)) {
    setState(blockHash, { height: relayHeight, status: 'done', txHash: 'attested' });
    console.log(`[relay] h=${relayHeight} already attested by us`);
    return;
  }

  // Submit
  const output   = buildBinaOutput(block);
  const proofUri = cfg.proofUriBase ? `${cfg.proofUriBase}/block/${relayHeight}` : '';
  const attempts = (state.get(blockHash)?.attempts ?? 0) + 1;
  setState(blockHash, { height: relayHeight, status: 'pending', attempts });

  try {
    const tx  = await oracle.submitOutput(output, purposeHash, '0x', proofUri);
    const rec = await tx.wait();
    setState(blockHash, { status: 'done', txHash: tx.hash });
    console.log(`[relay] h=${relayHeight} SUBMITTED  tx=${tx.hash}  gas=${rec.gasUsed}`);
  } catch (err) {
    const msg = String(err.message ?? err);
    const safeRevert = msg.includes('AlreadyAttested') || msg.includes('AlreadySubmitted');
    if (safeRevert) {
      setState(blockHash, { status: 'done', txHash: 'reverted-ok' });
      console.log(`[relay] h=${relayHeight} already attested (safe revert)`);
    } else {
      setState(blockHash, { status: 'error', error: msg.slice(0, 200) });
      console.error(`[relay] h=${relayHeight} FAILED attempt ${attempts}: ${msg.slice(0, 150)}`);
    }
  }
}

// ── Startup ────────────────────────────────────────────────────────────────

async function main() {
  const cfg = parseArgs(process.argv);

  const provider    = new ethers.JsonRpcProvider(cfg.evmUrl);
  const wallet      = new ethers.Wallet(cfg.privateKey, provider);
  const oracle      = new ethers.Contract(cfg.oracleAddress, ORACLE_ABI, wallet);
  const purposeHash = ethers.id(cfg.purpose);  // keccak256(utf8(purpose))

  // Startup status
  const [authorized, bond, locked] = await Promise.all([
    oracle.publishers(wallet.address),
    oracle.publisherBonds(wallet.address),
    oracle.lockedBonds(wallet.address),
  ]);

  console.log('BINA Relay Daemon');
  console.log('─────────────────────────────────────────');
  console.log(`BINA node:     ${cfg.binaUrl}`);
  console.log(`EVM oracle:    ${cfg.oracleAddress}`);
  console.log(`Publisher:     ${wallet.address}`);
  console.log(`Authorized:    ${authorized ? 'YES' : 'NO — ask owner to call setPublisher(address, true)'}`);
  console.log(`Bond:          ${ethers.formatEther(bond)} ETH  (locked: ${ethers.formatEther(locked)} ETH)`);
  console.log(`Purpose:       ${cfg.purpose}`);
  console.log(`Purpose hash:  ${purposeHash}`);
  console.log(`Confirmations: ${cfg.confirmations} blocks`);
  console.log(`Poll interval: ${cfg.pollMs} ms`);
  console.log('─────────────────────────────────────────\n');

  if (!authorized) {
    console.error('[relay] WARNING: not an authorized publisher — submissions will revert.');
  }

  // Poll loop
  while (true) {
    try {
      await tick(cfg, oracle, wallet, purposeHash);
    } catch (err) {
      console.error(`[relay] tick error: ${err.message}`);
    }
    await new Promise(r => setTimeout(r, cfg.pollMs));
  }
}

// ── Graceful shutdown: print last 10 entries ──────────────────────────────

process.on('SIGINT', () => {
  console.log('\n[relay] shutting down. Recent submissions:');
  const recent = [...state.entries()].slice(-10);
  if (!recent.length) {
    console.log('  (none)');
  }
  for (const [hash, s] of recent) {
    const tx = s.txHash ?? s.error ?? '';
    console.log(`  h=${String(s.height ?? '?').padEnd(6)} ${hash.slice(0, 10)}…  ${String(s.status).padEnd(8)}  ${tx.slice(0, 66)}`);
  }
  process.exit(0);
});

main().catch(err => {
  console.error('[relay] fatal:', err.message);
  process.exit(1);
});
