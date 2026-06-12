/**
 * Epoch Builder
 *
 * Watches the miner telemetry file, accumulates accepted solutions into
 * an epoch, builds Merkle trees, and:
 *   1. POSTs epoch data to the Randomness API server
 *   2. Submits the epoch root on-chain via BatchMiningModule.commitEpochRoot()
 *   3. After the challenge window, calls finalizeEpoch() to claim TGBT
 *
 * Usage:
 *   MINER_PRIVATE_KEY=0x... node epoch-builder.js
 *
 * Config (env vars or defaults):
 *   TELEMETRY_FILE     path to miner-telemetry.jsonl
 *   SOLUTIONS_PER_EPOCH  how many solutions per epoch (default: 50)
 *   RPC_URL            chain RPC endpoint
 *   BATCH_CONTRACT     deployed BatchMiningModule address
 *   RANDOMNESS_API     Randomness API base URL
 *   MINER_PRIVATE_KEY  hex private key (no 0x prefix ok)
 *   POOL_ID            mining pool id (default: 0)
 *   PINATA_JWT         optional Pinata JWT for pinJSONToIPFS
 *   WEB3_STORAGE_TOKEN optional web3.storage bearer token fallback
 */

const fs = require('fs');
const path = require('path');
const { ethers } = require('ethers');
const { anchorEpochMerkleRoot } = require('./bitcoin-anchor');

// ── Load .env file ──────────────────────────────────────
const envFile = path.resolve(__dirname, '.env');
if (fs.existsSync(envFile)) {
	fs.readFileSync(envFile, 'utf8').split('\n').forEach(line => {
		const m = line.match(/^\s*([^#][^=]*?)\s*=\s*(.*?)\s*$/);
		if (m && !process.env[m[1]]) process.env[m[1]] = m[2];
	});
}

// ── Config ──────────────────────────────────────────────
const TELEMETRY_FILE = process.env.TELEMETRY_FILE
	? path.resolve(__dirname, process.env.TELEMETRY_FILE)
	: path.resolve(__dirname, '..', 'rust', 'miner-telemetry.jsonl');
const SOLUTIONS_PER_EPOCH = Number(process.env.SOLUTIONS_PER_EPOCH || 50);
const RPC_URL = process.env.RPC_URL || 'https://api.nativebtc.org/v1/arb';
const BATCH_CONTRACT = process.env.BATCH_CONTRACT || '';
const RANDOMNESS_API = process.env.RANDOMNESS_API || 'http://127.0.0.1:4271';
const POOL_ID = Number(process.env.POOL_ID || 0);
const MINER_PRIVATE_KEY = process.env.MINER_PRIVATE_KEY;
const EPOCH_STATE_FILE = path.resolve(__dirname, 'epoch-state.json');
const PINATA_JWT = process.env.PINATA_JWT || '';
const WEB3_STORAGE_TOKEN = process.env.WEB3_STORAGE_TOKEN || process.env.WEB3_STORAGE_API_TOKEN || '';

// ── ABI fragment for BatchMiningModule ──────────────────
const BATCH_ABI = [
	'function commitEpochRoot(uint256 epochId, bytes32 merkleRoot, uint32 leafCount, uint8 poolId, uint256 deadline, bytes signature) external',
	'function finalizeEpoch(uint256 epochId) external',
	'function recordStorageAttestation(uint256 epochId, bytes32 attestationHash) external',
	'function currentEpochId() view returns (uint256)',
	'function getEpochInfo(uint256 epochId) view returns (tuple(bytes32 merkleRoot, uint64 startBlock, uint64 endBlock, uint32 leafCount, address operator, uint8 poolId, bool finalized, uint256 totalReward, bool storageAttested, bytes32 attestationHash))',
	'event EpochRootCommitted(uint256 indexed epochId, address indexed operator, bytes32 merkleRoot, uint32 leafCount, uint8 poolId)',
	'event EpochFinalized(uint256 indexed epochId, uint256 totalReward)',
	'event StorageAttested(uint256 indexed epochId, bytes32 attestationHash)',
];

// EIP-712 domain & types for epoch root signing
const EIP712_DOMAIN = {
	name: 'TemporalGradientBatch',
	version: '1',
};

const EIP712_TYPES = {
	EpochRoot: [
		{ name: 'operator',   type: 'address' },
		{ name: 'epochId',    type: 'uint256' },
		{ name: 'merkleRoot', type: 'bytes32' },
		{ name: 'leafCount',  type: 'uint32'  },
		{ name: 'poolId',     type: 'uint8'   },
		{ name: 'deadline',   type: 'uint256' },
	],
};

// ── State ───────────────────────────────────────────────
let state = { nextEpochId: 0, processedLines: 0, pendingLeaves: [] };

function loadState() {
	if (fs.existsSync(EPOCH_STATE_FILE)) {
		try {
			state = JSON.parse(fs.readFileSync(EPOCH_STATE_FILE, 'utf8'));
			console.log(`[EpochBuilder] Resumed: epoch ${state.nextEpochId}, ${state.pendingLeaves.length} pending leaves, ${state.processedLines} lines processed`);
		} catch {
			console.warn('[EpochBuilder] Corrupt state file, starting fresh');
		}
	}
}

function saveState() {
	fs.writeFileSync(EPOCH_STATE_FILE, JSON.stringify(state, null, 2));
}

// ── Merkle tree ─────────────────────────────────────────

/**
 * Build a Merkle tree from an array of leaf hashes.
 * Each leaf is hashed as keccak256(abi.encodePacked(index, outputHash))
 * to match the Solidity verifier in BatchMiningModule.
 */
function buildMerkleTree(outputHashes) {
	if (outputHashes.length === 0) return { root: ethers.constants.HashZero, proofs: [] };

	// Create leaves: keccak256(index || outputHash)
	let leaves = outputHashes.map((hash, i) => {
		return ethers.utils.keccak256(ethers.utils.solidityPack(['uint256', 'bytes32'], [i, hash]));
	});

	// Pad to next power of 2
	const nextPow2 = Math.pow(2, Math.ceil(Math.log2(Math.max(leaves.length, 2))));
	while (leaves.length < nextPow2) {
		leaves.push(ethers.constants.HashZero);
	}

	// Build tree layers (bottom-up)
	const layers = [leaves.slice()];
	while (layers[layers.length - 1].length > 1) {
		const prev = layers[layers.length - 1];
		const next = [];
		for (let i = 0; i < prev.length; i += 2) {
			const left = prev[i];
			const right = prev[i + 1] || ethers.constants.HashZero;
			// Sort to match OpenZeppelin MerkleProof convention
			const pair = [left, right].sort();
				next.push(ethers.utils.keccak256(ethers.utils.solidityPack(['bytes32', 'bytes32'], pair)));
		}
		layers.push(next);
	}

	const root = layers[layers.length - 1][0];

	// Generate proofs for each original leaf
	const proofs = outputHashes.map((_, leafIdx) => {
		const proof = [];
		let idx = leafIdx;
		for (let layer = 0; layer < layers.length - 1; layer++) {
			const siblingIdx = idx % 2 === 0 ? idx + 1 : idx - 1;
			if (siblingIdx < layers[layer].length) {
				proof.push(layers[layer][siblingIdx]);
			}
			idx = Math.floor(idx / 2);
		}
		return proof;
	});

	return { root, proofs, layers };
}

// ── Telemetry watcher ───────────────────────────────────

/**
 * Parse new lines from the telemetry JSONL file and extract accepted solutions.
 *
 * Uses byte-offset tracking (state.processedBytes) instead of reading the
 * entire file.  This avoids V8's ~512 MB single-string limit on large
 * telemetry files and keeps each poll O(new data) instead of O(total file).
 */
function scanTelemetry() {
	if (!fs.existsSync(TELEMETRY_FILE)) return [];

	const stat = fs.statSync(TELEMETRY_FILE);

	// Migration: if processedBytes not tracked yet, initialise to current
	// file size so we start watching for NEW lines from this point forward.
	if (state.processedBytes == null) {
		state.processedBytes = stat.size;
		console.log(`[EpochBuilder] Initialised processedBytes=${stat.size} (byte-offset migration)`);
		return [];
	}

	// File was truncated / rotated — reset
	if (stat.size < state.processedBytes) {
		console.log('[EpochBuilder] Telemetry file shrank — assuming rotation, resetting offset');
		state.processedBytes = 0;
		state.processedLines = 0;
		state.lastAcceptedCount = null;
	}

	const startByte = state.processedBytes;
	if (startByte >= stat.size) return []; // no new data

	// Read only the new portion of the file
	const readSize = stat.size - startByte;
	const fd = fs.openSync(TELEMETRY_FILE, 'r');
	const buf = Buffer.alloc(readSize);
	fs.readSync(fd, buf, 0, readSize, startByte);
	fs.closeSync(fd);

	let newContent = buf.toString('utf8');

	// If we started mid-file the first chunk might be a partial JSON line — drop it
	if (startByte > 0 && newContent.length > 0 && newContent[0] !== '{' && newContent[0] !== '\n') {
		const firstNewline = newContent.indexOf('\n');
		if (firstNewline >= 0) {
			newContent = newContent.slice(firstNewline + 1);
		} else {
			return []; // entire chunk is a partial line — wait for more data
		}
	}

	const lines = newContent.split('\n').filter(l => l.trim());
	const acceptedOutputs = [];
	let prevAccepted = state.lastAcceptedCount ?? null;

	for (const line of lines) {
		try {
			const snap = JSON.parse(line);
			const currAccepted = snap.accepted_submissions || 0;
			const outputHash = snap.last_output_hash_hex || snap.last_commit_hash_hex || snap.last_solution_hash_hex;

			if (prevAccepted !== null && currAccepted > prevAccepted && outputHash) {
				acceptedOutputs.push({
					outputHash,
					timestamp: snap.timestamp_unix_ms,
					nonce: snap.last_solution_nonce,
					hashrate: snap.hashrate_hs,
				});
			}
			prevAccepted = currAccepted;
		} catch { /* skip malformed lines */ }
	}

	state.processedBytes = stat.size;
	state.processedLines += lines.length;
	state.lastAcceptedCount = prevAccepted;
	return acceptedOutputs;
}

// ── Chain interaction ───────────────────────────────────

async function signEpochRoot(wallet, epochId, merkleRoot, leafCount, poolId, deadline, chainId) {
	const domain = { ...EIP712_DOMAIN, chainId, verifyingContract: BATCH_CONTRACT };
	const value = {
		operator:   wallet.address,
		epochId:    epochId,
		merkleRoot: merkleRoot,
		leafCount:  leafCount,
		poolId:     poolId,
		deadline:   deadline,
	};
	return wallet._signTypedData(domain, EIP712_TYPES, value);
}

async function commitEpochOnChain(provider, wallet, epochId, merkleRoot, leafCount) {
	const contract = new ethers.Contract(BATCH_CONTRACT, BATCH_ABI, wallet);
	const deadline = Math.floor(Date.now() / 1000) + 3600; // 1 hour from now
	const network = await provider.getNetwork();
	const chainId = network.chainId;

	const signature = await signEpochRoot(wallet, epochId, merkleRoot, leafCount, POOL_ID, deadline, chainId);

	console.log(`[EpochBuilder] Committing epoch ${epochId} on-chain (root=${merkleRoot.slice(0, 18)}…, leaves=${leafCount})`);
	const tx = await contract.commitEpochRoot(epochId, merkleRoot, leafCount, POOL_ID, deadline, signature);
	const receipt = await tx.wait();
	console.log(`[EpochBuilder] ✓ Epoch ${epochId} committed in tx ${receipt.transactionHash} (gas: ${receipt.gasUsed})`);
	return receipt;
}

async function finalizeEpochOnChain(wallet, epochId) {
	const contract = new ethers.Contract(BATCH_CONTRACT, BATCH_ABI, wallet);

	console.log(`[EpochBuilder] Finalizing epoch ${epochId}…`);
	const tx = await contract.finalizeEpoch(epochId, { gasLimit: 500_000 });
	const receipt = await tx.wait();
	console.log(`[EpochBuilder] ✓ Epoch ${epochId} finalized in tx ${receipt.transactionHash} (gas: ${receipt.gasUsed})`);
	return receipt;
}

async function recordStorageAttestationOnChain(wallet, epochId, attestationHash) {
	const contract = new ethers.Contract(BATCH_CONTRACT, BATCH_ABI, wallet);

	console.log(`[EpochBuilder] Recording storage attestation for epoch ${epochId}…`);
	const tx = await contract.recordStorageAttestation(epochId, attestationHash, { gasLimit: 300_000 });
	const receipt = await tx.wait();
	console.log(`[EpochBuilder] ✓ Storage attestation recorded for epoch ${epochId} in tx ${receipt.transactionHash}`);
	return receipt;
}

// ── API push ────────────────────────────────────────────

async function pushEpochToApi(epochData) {
	try {
		const resp = await fetch(`${RANDOMNESS_API}/api/epochs`, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify(epochData),
		});
		const json = await resp.json();
		if (resp.ok) {
			console.log(`[EpochBuilder] Pushed epoch ${epochData.epochId} to Randomness API (${json.leafCount} leaves)`);
		} else {
			console.error(`[EpochBuilder] API push failed:`, json);
		}
	} catch (err) {
		console.error(`[EpochBuilder] API push error:`, err.message);
	}
}

async function verifyEpochStorageWithApi(epochId) {
	const resp = await fetch(`${RANDOMNESS_API}/api/epochs/${epochId}/verify-storage`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
	});

	const raw = await resp.text();
	const json = JSON.parse(raw);
	if (!resp.ok) {
		throw new Error(json.error || `storage verification failed for epoch ${epochId}`);
	}

	if (!json.settlement_gate?.approved) {
		const reasons = json.settlement_gate?.reasons?.join('; ') || 'storage gate rejected';
		throw new Error(reasons);
	}

	return {
		report: json,
		attestationHash: ethers.utils.keccak256(ethers.utils.toUtf8Bytes(raw)),
	};
}

async function persistOnChainAttestationToApi(epochId, attestationHash, receipt, wallet) {
	const payload = {
		attestationHash,
		txHash: receipt.transactionHash,
		recordedAt: new Date().toISOString(),
		recorder: wallet.address,
	};

	const resp = await fetch(`${RANDOMNESS_API}/api/epochs/${epochId}/attestation-onchain`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify(payload),
	});

	const json = await resp.json();
	if (!resp.ok) {
		throw new Error(json.error || `failed to persist on-chain attestation for epoch ${epochId}`);
	}

	return json;
}

async function uploadEpochToIpfs(epochRecord) {
	const payload = JSON.stringify(epochRecord, null, 2);
	const filename = `epoch-${epochRecord.epochId}.json`;

	if (PINATA_JWT) {
		const resp = await fetch('https://api.pinata.cloud/pinning/pinJSONToIPFS', {
			method: 'POST',
			headers: {
				'Content-Type': 'application/json',
				'Authorization': `Bearer ${PINATA_JWT}`,
			},
			body: JSON.stringify({
				pinataMetadata: { name: filename },
				pinataContent: epochRecord,
			}),
		});
		const json = await resp.json();
		if (!resp.ok) {
			throw new Error(json.error?.details || json.error || json.message || 'Pinata upload failed');
		}
		return json.IpfsHash || json.cid || null;
	}

	if (WEB3_STORAGE_TOKEN) {
		const resp = await fetch('https://api.web3.storage/upload', {
			method: 'POST',
			headers: {
				'Authorization': `Bearer ${WEB3_STORAGE_TOKEN}`,
				'Content-Type': 'application/json',
				'X-NAME': filename,
			},
			body: payload,
		});
		const json = await resp.json();
		if (!resp.ok) {
			throw new Error(json.message || json.error || 'web3.storage upload failed');
		}
		return json.cid || json.value?.cid || null;
	}

	return null;
}

async function persistBitcoinAnchorToApi(epochId, anchorReport) {
	const payload = {
		anchorId: anchorReport.anchor_id,
		anchoredAt: new Date().toISOString(),
		storageReference: anchorReport.storage_reference || null,
		preference: anchorReport.preference || 'op_return',
		anchor: anchorReport.anchor || null,
	};

	const resp = await fetch(`${RANDOMNESS_API}/api/epochs/${epochId}/bitcoin-anchor`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify(payload),
	});

	const json = await resp.json();
	if (!resp.ok) {
		throw new Error(json.error || `failed to persist Bitcoin anchor for epoch ${epochId}`);
	}

	return json;
}

function normalizeStorageReference(ref) {
	if (!ref || typeof ref !== 'string') return null;
	const trimmed = ref.trim();
	if (!trimmed) return null;
	if (trimmed.startsWith('ipfs://')) return trimmed;
	if (/^(Qm[1-9A-HJ-NP-Za-km-z]{44}|bafy[1-9A-HJ-NP-Za-km-z]+)/.test(trimmed)) {
		return `ipfs://${trimmed}`;
	}
	return trimmed;
}

function resolveEpochStorageReference(epochDetail, verification) {
	const explicit = normalizeStorageReference(
		epochDetail?.storageReference || epochDetail?.ipfsCid || epochDetail?.ipfs_cid || epochDetail?.cid
	);
	if (explicit) return explicit;

	const attestation = verification?.report?.settlement_gate?.attestation;
	const providerId = String(attestation?.provider_id || verification?.report?.provider || '').toLowerCase();
	const fileId = normalizeStorageReference(attestation?.file_id || verification?.report?.verification_result?.file_id || null);
	if (fileId && (providerId.includes('ipfs') || fileId.startsWith('ipfs://'))) {
		return fileId.startsWith('ipfs://') ? fileId : `ipfs://${fileId}`;
	}

	return fileId;
}

function getEpochRecord(epochId) {
	const epochFile = path.resolve(__dirname, 'epoch-store', `epoch-${epochId}.json`);
	if (!fs.existsSync(epochFile)) return null;
	try {
		return JSON.parse(fs.readFileSync(epochFile, 'utf8'));
	} catch {
		return null;
	}
}

// ── Main loop ───────────────────────────────────────────

async function processEpoch() {
	// 1. Scan telemetry for new accepted solutions
	const newOutputs = scanTelemetry();
	if (newOutputs.length > 0) {
		state.pendingLeaves.push(...newOutputs);
		console.log(`[EpochBuilder] +${newOutputs.length} new solutions → ${state.pendingLeaves.length} pending`);
	}

	// 2. If we have enough, build an epoch
	if (state.pendingLeaves.length >= SOLUTIONS_PER_EPOCH) {
		const batch = state.pendingLeaves.splice(0, SOLUTIONS_PER_EPOCH);
		const epochId = state.nextEpochId;
		const outputHashes = batch.map(l => l.outputHash);

		// Build Merkle tree
		const { root, proofs } = buildMerkleTree(outputHashes);

		// Prepare epoch data
		const epochData = {
			epochId,
			merkleRoot: root,
			leafCount: batch.length,
			poolId: POOL_ID,
			finalized: false,
			createdAt: new Date().toISOString(),
			ipfs_cid: null,
			storageReference: null,
			bitcoinAnchor: null,
			leaves: batch.map((leaf, i) => ({
				index:      i,
				outputHash: leaf.outputHash,
				proof:      proofs[i],
				timestamp:  leaf.timestamp,
				nonce:      leaf.nonce,
			})),
		};

		try {
			const cid = await uploadEpochToIpfs(epochData);
			if (cid) {
				epochData.ipfs_cid = cid;
				epochData.storageReference = `ipfs://${cid}`;
				console.log(`[EpochBuilder] Pinned epoch ${epochId} to IPFS (${cid})`);
			}
		} catch (err) {
			console.error(`[EpochBuilder] IPFS pin failed for epoch ${epochId}:`, err.message);
		}

		// Push to Randomness API
		await pushEpochToApi(epochData);

		// Commit on-chain if we have contract + key
		if (BATCH_CONTRACT && MINER_PRIVATE_KEY) {
			try {
				const provider = new ethers.providers.JsonRpcProvider(RPC_URL);
				const wallet = new ethers.Wallet(MINER_PRIVATE_KEY, provider);
				epochData.operator = wallet.address;

				// Check if this epoch was already committed on-chain
				const contract = new ethers.Contract(BATCH_CONTRACT, BATCH_ABI, provider);
				const onChainNextId = Number(await contract.currentEpochId());
				if (epochId < onChainNextId) {
					console.log(`[EpochBuilder] Epoch ${epochId} already committed on-chain (currentEpochId=${onChainNextId}), skipping commit`);
				} else if (epochId > onChainNextId) {
					// Self-correction: local state got ahead of on-chain (e.g. failed commit was counted locally)
					console.warn(`[EpochBuilder] ⚠ Local epochId ${epochId} is ahead of on-chain _nextEpochId ${onChainNextId} — resetting to ${onChainNextId}`);
					state.nextEpochId = onChainNextId;
					state.pendingLeaves.unshift(...batch);
					saveState();
					return;
				} else {
					// Check cooldown (50 L1 blocks) before attempting commit
					if (onChainNextId > 0) {
						const prevEpoch = await contract.getEpochInfo(onChainNextId - 1);
						const lastCommitL1 = Number(prevEpoch.startBlock);
						const l1Block = await getL1BlockNumber(provider);
						const COOLDOWN_BLOCKS = 50;
						if (l1Block < lastCommitL1 + COOLDOWN_BLOCKS) {
							const remain = lastCommitL1 + COOLDOWN_BLOCKS - l1Block;
							console.log(`[EpochBuilder] Cooldown: ${remain} L1 blocks remaining before epoch ${epochId} can be committed (~${remain * 12}s)`);
							state.pendingLeaves.unshift(...batch);
							saveState();
							return;
						}
					}
					await commitEpochOnChain(provider, wallet, epochId, root, batch.length);
				}
			} catch (err) {
				console.error(`[EpochBuilder] On-chain commit failed:`, err.message);
				// Re-queue leaves so they aren't lost
				state.pendingLeaves.unshift(...batch);
				saveState();
				return;
			}
		} else {
			console.log(`[EpochBuilder] No BATCH_CONTRACT or key configured — epoch ${epochId} stored locally only`);
		}

		state.nextEpochId++;
		saveState();
		console.log(`[EpochBuilder] ✓ Epoch ${epochId} complete (${batch.length} solutions, root ${root.slice(0, 18)}…)`);
	}

	saveState();
}

// ── L1 block helper ─────────────────────────────────────
// On Arbitrum, Solidity `block.number` returns the L1 (Ethereum mainnet) block number,
// but ethers' provider.getBlockNumber() returns the L2 block number.
// We must compare against L1 blocks when checking the challenge window.

async function getL1BlockNumber(provider) {
	const raw = await provider.send('eth_getBlockByNumber', ['latest', false]);
	if (raw && raw.l1BlockNumber) {
		return parseInt(raw.l1BlockNumber, 16);
	}
	// Never fall back to L2 block number — on Arbitrum L2 blocks are ~454M while
	// L1 blocks are ~24M. Using L2 as a fallback would make every epoch appear
	// past its 28,800-block challenge window and trigger premature finalizeEpoch()
	// calls that revert on-chain with CooldownNotElapsed().
	throw new Error('Could not fetch L1 block number from Arbitrum RPC (l1BlockNumber field missing)');
}

// ── Finalisation sweep ──────────────────────────────────

async function finalizePendingEpochs() {
	if (!BATCH_CONTRACT || !MINER_PRIVATE_KEY) return;

	const provider = new ethers.providers.JsonRpcProvider(RPC_URL);
	const wallet = new ethers.Wallet(MINER_PRIVATE_KEY, provider);
	const contract = new ethers.Contract(BATCH_CONTRACT, BATCH_ABI, provider);
	let l1Block;
	try {
		l1Block = await getL1BlockNumber(provider);
	} catch (err) {
		console.error('[EpochBuilder] Skipping finalization sweep — could not determine L1 block:', err.message);
		return;
	}

	// Must match contract: CHALLENGE_WINDOW = 28_800 L1 blocks (~96 hours on Ethereum mainnet)
	const CHALLENGE_WINDOW = 28_800;

	for (let epId = 0; epId < state.nextEpochId; epId++) {
		try {
			const info = await contract.getEpochInfo(epId);
			if (info.merkleRoot === ethers.constants.HashZero) continue;
			const epochRecord = getEpochRecord(epId);
			if (info.finalized && info.storageAttested && epochRecord?.bitcoinAnchor?.anchorId) continue;

			const readyBlock = Number(info.startBlock) + CHALLENGE_WINDOW;
			if (l1Block >= readyBlock) {
				const verification = await verifyEpochStorageWithApi(epId);
				console.log(`[EpochBuilder] Storage verified for epoch ${epId} via ${verification.report.provider}`);

				if (!info.finalized) {
					await finalizeEpochOnChain(wallet, epId);
				}

				if (!info.storageAttested) {
					const attestationReceipt = await recordStorageAttestationOnChain(wallet, epId, verification.attestationHash);
					await persistOnChainAttestationToApi(epId, verification.attestationHash, attestationReceipt, wallet);
				}

				if (!epochRecord?.bitcoinAnchor?.anchorId) {
					const storageReference = resolveEpochStorageReference(epochRecord, verification);
					const anchorReport = await anchorEpochMerkleRoot(epId, info.merkleRoot, storageReference);
					await persistBitcoinAnchorToApi(epId, anchorReport);
					console.log(`[EpochBuilder] ✓ Epoch ${epId} Bitcoin-anchored via ${anchorReport.anchor_id}`);
				}
			} else {
				const remain = readyBlock - l1Block;
				const etaH = ((remain * 12) / 3600).toFixed(1);
				console.log(`[EpochBuilder] Epoch ${epId} challenge window: ${remain} L1 blocks remaining (~${etaH} h), ready at L1 block ${readyBlock} (current: ${l1Block})`);
			}
		} catch (err) {
			// Epoch might not be on-chain yet (local-only mode)
			if (!err.message.includes('EpochNotFound')) {
				console.error(`[EpochBuilder] Finalize check for epoch ${epId}:`, err.message);
			}
		}
	}
}

// ── Entry point ─────────────────────────────────────────

const POLL_INTERVAL_MS = Number(process.env.POLL_INTERVAL || 30_000); // 30 seconds

async function main() {
	console.log('\n═══════════════════════════════════════════════');
	console.log(' Epoch Builder — Batch Mining Orchestrator');
	console.log('═══════════════════════════════════════════════');
	console.log(` Telemetry:       ${TELEMETRY_FILE}`);
	console.log(` Solutions/epoch: ${SOLUTIONS_PER_EPOCH}`);
	console.log(` RPC:             ${RPC_URL}`);
	console.log(` Contract:        ${BATCH_CONTRACT || '(not set — local only)'}`);
	console.log(` Randomness API:  ${RANDOMNESS_API}`);
	console.log(` Poll interval:   ${POLL_INTERVAL_MS / 1000}s`);
	console.log('═══════════════════════════════════════════════\n');

	loadState();

	// Initial run
	await processEpoch();
	await finalizePendingEpochs();

	// Poll loop
	setInterval(async () => {
		try {
			await processEpoch();
			await finalizePendingEpochs();
		} catch (err) {
			console.error('[EpochBuilder] Loop error:', err.message);
		}
	}, POLL_INTERVAL_MS);
}

main().catch(err => {
	console.error('[EpochBuilder] Fatal:', err);
	process.exit(1);
});

module.exports = { buildMerkleTree, scanTelemetry };
