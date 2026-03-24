/**
 * Randomness API Server
 * 
 * Serves mined randomness outputs from the epoch batch-mining system.
 * Consumers can request the latest randomness, verify proofs against
 * on-chain Merkle roots, and browse epoch metadata.
 *
 * Endpoints:
 *   GET  /api/randomness/latest           → latest random output + signature
 *   GET  /api/randomness/:outputHash/proof → Merkle proof for a specific output
 *   GET  /api/epochs                       → list of epochs (paginated)
 *   GET  /api/epochs/:epochId              → single epoch detail
 *   GET  /api/health                       → service health
 *
 * Storage: epoch data & leaves are kept in a local JSON store (epoch-store/).
 * The miner writes leaves after each mining cycle; the finaliser script
 * anchors the root on-chain periodically.
 */

const http = require('http');
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const { ethers } = require('ethers');
const { getEpochFilePath, verifyEpochStorage } = require('./storage-attestation');
const utxoScanner = require('./utxo-scanner');

// ── Load .env file (same as epoch-builder.js) ───────────
const envFile = path.resolve(__dirname, '.env');
if (fs.existsSync(envFile)) {
	fs.readFileSync(envFile, 'utf8').split('\n').forEach(line => {
		const m = line.match(/^\s*([^#][^=]*?)\s*=\s*(.*?)\s*$/);
		if (m && !process.env[m[1]]) process.env[m[1]] = m[2];
	});
}

// ── Config ──────────────────────────────────────────────
const HOST = process.env.RANDOMNESS_HOST || '127.0.0.1';
const PORT = Number(process.env.RANDOMNESS_PORT || 4271);
const EPOCH_STORE_DIR = process.env.EPOCH_STORE || path.resolve(__dirname, 'epoch-store');
const MINER_PRIVATE_KEY = process.env.MINER_PRIVATE_KEY || null;

// ── Ensure store directory ──────────────────────────────
if (!fs.existsSync(EPOCH_STORE_DIR)) {
	fs.mkdirSync(EPOCH_STORE_DIR, { recursive: true });
}

// ── In-memory epoch index (loaded from disk on boot) ────
const epochIndex = [];      // [ { epochId, merkleRoot, leafCount, finalized, createdAt } ]
const epochLeaves = {};     // epochId → [ { index, outputHash, proof: bytes32[] } ]
let latestOutput = null;    // { outputHash, epochId, leafIndex, timestamp, signature }

function projectEpochSummary(data) {
	return {
		epochId:    data.epochId,
		merkleRoot: data.merkleRoot,
		leafCount:  data.leafCount,
		finalized:  data.finalized || false,
		createdAt:  data.createdAt || new Date().toISOString(),
		poolId:     data.poolId || 0,
		operator:   data.operator || null,
		totalReward: data.totalReward || 0,
		ipfs_cid: data.ipfs_cid || null,
		storageReference: data.storageReference || null,
		storageVerification: data.storageVerification || null,
		onChainAttestation: data.onChainAttestation || null,
		bitcoinAnchor: data.bitcoinAnchor || null,
	};
}

/**
 * Load all epoch JSON files from disk into memory.
 */
function loadEpochStore() {
	const files = fs.readdirSync(EPOCH_STORE_DIR).filter(f => f.startsWith('epoch-') && f.endsWith('.json'));
	for (const file of files) {
		try {
			const data = JSON.parse(fs.readFileSync(path.join(EPOCH_STORE_DIR, file), 'utf8'));
			epochIndex.push(projectEpochSummary(data));
			epochLeaves[data.epochId] = data.leaves || [];

			// Track latest output
			if (data.leaves && data.leaves.length > 0) {
				const lastLeaf = data.leaves[data.leaves.length - 1];
				if (!latestOutput || data.epochId > latestOutput.epochId ||
					(data.epochId === latestOutput.epochId && lastLeaf.index > latestOutput.leafIndex)) {
					latestOutput = {
						outputHash: lastLeaf.outputHash,
						epochId:    data.epochId,
						leafIndex:  lastLeaf.index,
						timestamp:  data.createdAt || new Date().toISOString(),
						signature:  lastLeaf.signature || null,
					};
				}
			}
		} catch (err) {
			console.error(`[EpochStore] Error loading ${file}:`, err.message);
		}
	}
	epochIndex.sort((a, b) => a.epochId - b.epochId);
	console.log(`[EpochStore] Loaded ${epochIndex.length} epochs, ${Object.values(epochLeaves).reduce((s, l) => s + l.length, 0)} total leaves`);
}

/**
 * Save a new epoch (called by epoch-builder when miner accumulates enough leaves).
 */
function saveEpoch(epochData) {
	const file = path.join(EPOCH_STORE_DIR, `epoch-${epochData.epochId}.json`);
	fs.writeFileSync(file, JSON.stringify(epochData, null, 2));

	// Update in-memory index
	const existing = epochIndex.find(e => e.epochId === epochData.epochId);
	if (existing) {
		Object.assign(existing, projectEpochSummary(epochData));
	} else {
		epochIndex.push(projectEpochSummary(epochData));
	}
	epochLeaves[epochData.epochId] = epochData.leaves || [];

	// Update latest
	if (epochData.leaves && epochData.leaves.length > 0) {
		const last = epochData.leaves[epochData.leaves.length - 1];
		latestOutput = {
			outputHash: last.outputHash,
			epochId:    epochData.epochId,
			leafIndex:  last.index,
			timestamp:  epochData.createdAt,
			signature:  last.signature || null,
		};
	}
}

function updateEpoch(epochId, patch) {
	const file = getEpochFilePath(epochId);
	if (!fs.existsSync(file)) {
		throw new Error(`Epoch file not found: ${file}`);
	}

	const current = JSON.parse(fs.readFileSync(file, 'utf8'));
	const next = { ...current, ...patch };
	fs.writeFileSync(file, JSON.stringify(next, null, 2));
	saveEpoch(next);
	return next;
}

// ── HTTP Helpers ────────────────────────────────────────
function sendJson(res, status, payload) {
	const body = JSON.stringify(payload, null, 2);
	res.writeHead(status, {
		'Content-Type': 'application/json; charset=utf-8',
		'Access-Control-Allow-Origin': '*',
		'Cache-Control': 'no-cache',
	});
	res.end(body);
}

function parseUrl(req) {
	return new URL(req.url, `http://${HOST}:${PORT}`);
}

function readBody(req) {
	return new Promise((resolve, reject) => {
		let body = '';
		req.on('data', c => body += c);
		req.on('end', () => {
			if (!body.trim()) return resolve({});
			try { resolve(JSON.parse(body)); }
			catch (e) { reject(e); }
		});
		req.on('error', reject);
	});
}

function sha256HexUtf8(value) {
	return crypto.createHash('sha256').update(String(value ?? ''), 'utf8').digest('hex');
}

function normalizeHex(value) {
	return String(value || '').trim().replace(/^0x/i, '').toLowerCase();
}

function ensureBytes32Hex(value, fieldName) {
	const hex = normalizeHex(value);
	if (!/^[0-9a-f]{64}$/.test(hex)) {
		throw new Error(`${fieldName} must be a 32-byte hex string`);
	}
	return `0x${hex}`;
}

function resolveDocumentHash(body) {
	if (body.documentHash) return ensureBytes32Hex(body.documentHash, 'documentHash');
	if (body.documentText) return `0x${sha256HexUtf8(body.documentText)}`;
	throw new Error('Missing documentHash or documentText');
}

function canonicalizeValue(value) {
	if (Array.isArray(value)) return value.map(canonicalizeValue);
	if (value && typeof value === 'object') {
		return Object.keys(value).sort().reduce((acc, key) => {
			acc[key] = canonicalizeValue(value[key]);
			return acc;
		}, {});
	}
	return value;
}

function canonicalJson(value) {
	return JSON.stringify(canonicalizeValue(value));
}

function certTypeToIndex(value) {
	if (value === undefined || value === null || value === '') return 7;
	if (Number.isInteger(value)) {
		if (value < 0 || value > 7) throw new Error('certType must be between 0 and 7');
		return value;
	}
	const labels = {
		documentnotarisation: 0,
		document_notarisation: 0,
		supplychain: 1,
		supply_chain: 1,
		legalevidence: 2,
		legal_evidence: 2,
		carboncredit: 3,
		carbon_credit: 3,
		academicpriority: 4,
		academic_priority: 4,
		softwarebuild: 5,
		software_build: 5,
		financialaudit: 6,
		financial_audit: 6,
		custom: 7,
	};
	const key = String(value).trim().toLowerCase();
	if (!(key in labels)) throw new Error(`Unknown certType '${value}'`);
	return labels[key];
}

function resolveAnchorRecord(body = {}) {
	if (body.anchorId) {
		const anchor = utxoScanner.getAnchorById(body.anchorId);
		if (!anchor) throw new Error(`Unknown anchorId '${body.anchorId}'`);
		return anchor;
	}
	if (body.scanId) {
		const scan = utxoScanner.getScanById(body.scanId);
		if (!scan?.summary?.anchorId) throw new Error(`Unknown scanId '${body.scanId}'`);
		const anchor = utxoScanner.getAnchorById(scan.summary.anchorId);
		if (!anchor) throw new Error(`No stored anchor for scanId '${body.scanId}'`);
		return anchor;
	}
	const latest = utxoScanner.getLatestAnchor();
	if (!latest) throw new Error('No anchor available yet. Run /api/utxo/scan first.');
	return latest;
}

function buildUtxoCertificatePayload(body = {}) {
	const anchor = resolveAnchorRecord(body);
	const documentHash = resolveDocumentHash(body);
	const recipient = body.recipient || body.issuedTo || null;
	const metadataURI = String(body.metadataURI || '');
	const certType = certTypeToIndex(body.certType);
	const attestor = body.attestor || anchor.attestor || null;
	const documentDescriptor = body.documentText ? { source: 'documentText', sha256: documentHash } : { source: 'documentHash', sha256: documentHash };

	const verifierRegistration = utxoScanner.buildVerifierRegistrationPayload(anchor, attestor);
	const certificateMint = utxoScanner.buildCertificateMintPayload(anchor, {
		recipient,
		documentHash,
		certType,
		metadataURI,
		attestationSignature: body.attestationSignature || null,
	});

	return {
		anchor,
		document: documentDescriptor,
		digests: {
			anchorId: `0x${anchor.anchorId}`,
			metadataDigest: `0x${anchor.metadataDigest}`,
			utxoIdHash: `0x${sha256HexUtf8(anchor.utxoId)}`,
			storageReferenceHash: `0x${sha256HexUtf8(anchor.storageReference || '')}`,
			documentHash,
			metadataCanonicalJson: canonicalJson(anchor.metadata),
		},
		verifierRegistration,
		certificateMint,
		context: {
			attestor,
			recipient,
			metadataURI,
			certType,
			attestationSignatureRequired: true,
			contractExpectation: 'Anchor must be registered in UTXOAnchorVerifier before mintCertificate succeeds.',
		},
	};
}

// ── Signature helper ────────────────────────────────────
async function signOutput(outputHash) {
	if (!MINER_PRIVATE_KEY) return null;
	try {
		const wallet = new ethers.Wallet(MINER_PRIVATE_KEY);
		const message = ethers.getBytes
			? ethers.getBytes(outputHash)
			: ethers.utils.arrayify(outputHash);
		return await wallet.signMessage(message);
	} catch {
		return null;
	}
}

// ── Route handlers ──────────────────────────────────────

function handleLatestRandomness(_req, res) {
	if (!latestOutput) {
		return sendJson(res, 404, {
			error: 'No randomness available yet',
			hint: 'The miner has not produced any epoch outputs yet.',
		});
	}
	sendJson(res, 200, {
		outputHash:  latestOutput.outputHash,
		epochId:     latestOutput.epochId,
		leafIndex:   latestOutput.leafIndex,
		timestamp:   latestOutput.timestamp,
		signature:   latestOutput.signature,
		totalEpochs: epochIndex.length,
	});
}

function handleOutputProof(req, res, outputHash) {
	const sortedEpochs = [...epochIndex].sort((a, b) => b.epochId - a.epochId);
	for (const epMeta of sortedEpochs) {
		const leaves = epochLeaves[epMeta.epochId] || [];
		const leaf = leaves.find(l => l.outputHash === outputHash);
		if (leaf) {
			return sendJson(res, 200, {
				epochId:    epMeta.epochId,
				leafIndex:  leaf.index,
				outputHash: leaf.outputHash,
				proof:      leaf.proof || [],
				merkleRoot: epMeta.merkleRoot || null,
				finalized:  epMeta.finalized || false,
				verifyOnChain: {
					contract: 'BatchMiningModule',
					method: 'verifyRandomnessLeaf(uint256 epochId, uint256 leafIndex, bytes32 outputHash, bytes32[] proof)',
					args: [epMeta.epochId, leaf.index, leaf.outputHash, leaf.proof || []],
				},
			});
		}
	}
	sendJson(res, 404, { error: 'Output not found in any epoch' });
}

function handleEpochList(_req, res, url) {
	const page = Number(url.searchParams.get('page') || 1);
	const limit = Math.min(Number(url.searchParams.get('limit') || 20), 100);
	const start = (page - 1) * limit;

	const sorted = [...epochIndex].sort((a, b) => b.epochId - a.epochId);
	const slice = sorted.slice(start, start + limit);

	sendJson(res, 200, {
		epochs: slice,
		total: epochIndex.length,
		page,
		limit,
		hasMore: start + limit < epochIndex.length,
	});
}

function handleEpochDetail(_req, res, epochId) {
	const ep = epochIndex.find(e => e.epochId === epochId);
	if (!ep) return sendJson(res, 404, { error: 'Epoch not found' });

	const leaves = (epochLeaves[epochId] || []).map(l => ({
		index:      l.index,
		outputHash: l.outputHash,
		signature:  l.signature || null,
		proof:      l.proof || [],
		proofSize:  (l.proof || []).length,
	}));

	sendJson(res, 200, {
		...ep,
		storageVerification: ep.storageVerification || null,
		onChainAttestation: ep.onChainAttestation || null,
		leaves,
	});
}

function handleHealth(_req, res) {
	sendJson(res, 200, {
		status: 'ok',
		epochs: epochIndex.length,
		totalLeaves: Object.values(epochLeaves).reduce((s, l) => s + l.length, 0),
		latestOutput: latestOutput ? latestOutput.outputHash : null,
		uptime: process.uptime(),
	});
}

// POST /api/epochs — accept a new epoch from the epoch-builder script
async function handleCreateEpoch(req, res) {
	try {
		const body = await readBody(req);
		if (!body.epochId && body.epochId !== 0) return sendJson(res, 400, { error: 'Missing epochId' });
		if (!body.merkleRoot)  return sendJson(res, 400, { error: 'Missing merkleRoot' });
		if (!body.leaves || !Array.isArray(body.leaves)) return sendJson(res, 400, { error: 'Missing leaves array' });

		body.createdAt = body.createdAt || new Date().toISOString();
		body.leafCount = body.leaves.length;
		body.ipfs_cid = body.ipfs_cid || null;
		body.storageReference = body.storageReference || null;
		body.storageVerification = body.storageVerification || null;
		body.onChainAttestation = body.onChainAttestation || null;
		body.bitcoinAnchor = body.bitcoinAnchor || null;

		// Sign each leaf if we have a key
		if (MINER_PRIVATE_KEY) {
			for (const leaf of body.leaves) {
				if (!leaf.signature) {
					leaf.signature = await signOutput(leaf.outputHash);
				}
			}
		}

		saveEpoch(body);
		console.log(`[EpochStore] Saved epoch ${body.epochId} with ${body.leafCount} leaves`);
		sendJson(res, 201, { ok: true, epochId: body.epochId, leafCount: body.leafCount });
	} catch (err) {
		sendJson(res, 400, { error: err.message });
	}
}

async function handleRecordOnChainAttestation(req, res, epochId) {
	const ep = epochIndex.find(e => e.epochId === epochId);
	if (!ep) return sendJson(res, 404, { error: 'Epoch not found' });

	try {
		const body = await readBody(req);
		if (!body.attestationHash) {
			return sendJson(res, 400, { error: 'Missing attestationHash' });
		}

		const updated = updateEpoch(epochId, {
			onChainAttestation: {
				attestationHash: body.attestationHash,
				txHash: body.txHash || null,
				recordedAt: body.recordedAt || new Date().toISOString(),
				recorder: body.recorder || null,
			},
		});

		sendJson(res, 200, {
			ok: true,
			epochId,
			onChainAttestation: updated.onChainAttestation,
		});
	} catch (err) {
		sendJson(res, 400, { error: err.message, epochId });
	}
}

async function handleRecordBitcoinAnchor(req, res, epochId) {
	const ep = epochIndex.find(e => e.epochId === epochId);
	if (!ep) return sendJson(res, 404, { error: 'Epoch not found' });

	try {
		const body = await readBody(req);
		if (!body.anchorId) {
			return sendJson(res, 400, { error: 'Missing anchorId' });
		}

		const updated = updateEpoch(epochId, {
			bitcoinAnchor: {
				anchorId: body.anchorId,
				anchoredAt: body.anchoredAt || new Date().toISOString(),
				storageReference: body.storageReference || null,
				preference: body.preference || 'op_return',
				anchor: body.anchor || null,
			},
		});

		sendJson(res, 200, {
			ok: true,
			epochId,
			bitcoinAnchor: updated.bitcoinAnchor,
		});
	} catch (err) {
		sendJson(res, 400, { error: err.message, epochId });
	}
}

async function handleVerifyEpochStorage(_req, res, epochId) {
	const ep = epochIndex.find(e => e.epochId === epochId);
	if (!ep) return sendJson(res, 404, { error: 'Epoch not found' });

	try {
		const report = await verifyEpochStorage(epochId);
		updateEpoch(epochId, {
			storageVerification: {
				...report,
				verifiedAt: new Date().toISOString(),
			},
		});
		sendJson(res, 200, report);
	} catch (err) {
		sendJson(res, 500, {
			error: err.message,
			epochId,
		});
	}
}

async function handleBuildUtxoCertificatePayload(req, res) {
	try {
		const body = await readBody(req);
		return sendJson(res, 200, buildUtxoCertificatePayload(body));
	} catch (err) {
		return sendJson(res, 400, { error: err.message });
	}
}

// ── Router ──────────────────────────────────────────────
const server = http.createServer((req, res) => {
	const url = parseUrl(req);
	const p = url.pathname;

	// CORS preflight
	if (req.method === 'OPTIONS') {
		res.writeHead(204, {
			'Access-Control-Allow-Origin': '*',
			'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
			'Access-Control-Allow-Headers': 'Content-Type',
		});
		return res.end();
	}

	try {
		// GET /api/randomness/latest
		if (req.method === 'GET' && p === '/api/randomness/latest') {
			return handleLatestRandomness(req, res);
		}
		// GET /api/randomness/:outputHash/proof
		const proofMatch = p.match(/^\/api\/randomness\/([0-9a-fA-Fx]+)\/proof$/);
		if (req.method === 'GET' && proofMatch) {
			return handleOutputProof(req, res, proofMatch[1]);
		}
		// GET /api/epochs
		if (req.method === 'GET' && p === '/api/epochs') {
			return handleEpochList(req, res, url);
		}
		// GET /api/epochs/:epochId
		const epochMatch = p.match(/^\/api\/epochs\/(\d+)$/);
		if (req.method === 'GET' && epochMatch) {
			return handleEpochDetail(req, res, Number(epochMatch[1]));
		}
		const verifyMatch = p.match(/^\/api\/epochs\/(\d+)\/verify-storage$/);
		if (req.method === 'POST' && verifyMatch) {
			return handleVerifyEpochStorage(req, res, Number(verifyMatch[1]));
		}
		const onChainMatch = p.match(/^\/api\/epochs\/(\d+)\/attestation-onchain$/);
		if (req.method === 'POST' && onChainMatch) {
			return handleRecordOnChainAttestation(req, res, Number(onChainMatch[1]));
		}
		const bitcoinAnchorMatch = p.match(/^\/api\/epochs\/(\d+)\/bitcoin-anchor$/);
		if (req.method === 'POST' && bitcoinAnchorMatch) {
			return handleRecordBitcoinAnchor(req, res, Number(bitcoinAnchorMatch[1]));
		}
		// POST /api/epochs — epoch-builder pushes a new epoch
		if (req.method === 'POST' && p === '/api/epochs') {
			return handleCreateEpoch(req, res);
		}
		// GET /api/health
		if (req.method === 'GET' && p === '/api/health') {
			return handleHealth(req, res);
		}

		// ── UTXO live scan endpoints ──
		if (req.method === 'GET' && p === '/api/utxo/scan') {
			const seed = url.searchParams.get('seed') || `live-dashboard-scan-${Date.now()}`;
			const preference = url.searchParams.get('preference') || null;
			const storageReference = url.searchParams.get('storageReference') || url.searchParams.get('storage_reference') || '';
			utxoScanner.runFullScan(seed, { preference, storageReference })
				.then(result => sendJson(res, 200, result))
				.catch(err => sendJson(res, 500, { error: err.message }));
			return;
		}
		if (req.method === 'GET' && p === '/api/utxo/latest') {
			const last = utxoScanner.getLastScan();
			return last ? sendJson(res, 200, last) : sendJson(res, 404, { error: 'No scan yet. Hit /api/utxo/scan first.' });
		}
		if (req.method === 'GET' && p === '/api/utxo/inventory') {
			return sendJson(res, 200, utxoScanner.getInventoryInfo());
		}
		if (req.method === 'GET' && p === '/api/utxo/history') {
			return sendJson(res, 200, { scans: utxoScanner.getScanHistory() });
		}
		if (req.method === 'GET' && p === '/api/utxo/anchor/latest') {
			const latest = utxoScanner.getLatestAnchor();
			return latest
				? sendJson(res, 200, latest)
				: sendJson(res, 404, { error: 'No anchor yet. Hit /api/utxo/scan first.' });
		}
		const utxoAnchorMatch = p.match(/^\/api\/utxo\/anchor\/([0-9a-fA-F]+)$/);
		if (req.method === 'GET' && utxoAnchorMatch) {
			const anchor = utxoScanner.getAnchorById(utxoAnchorMatch[1]);
			return anchor
				? sendJson(res, 200, anchor)
				: sendJson(res, 404, { error: 'Anchor not found' });
		}
		if (req.method === 'POST' && p === '/api/utxo/certificate-payload') {
			return handleBuildUtxoCertificatePayload(req, res);
		}
		if (req.method === 'GET' && p === '/api/utxo/discover') {
			const url = new URL(req.url, `http://${req.headers.host}`);
			const blocks = Math.min(parseInt(url.searchParams.get('blocks') || '3', 10), 10);
			utxoScanner.discoverDeadUtxos({ blocks })
				.then(result => sendJson(res, 200, result))
				.catch(err => sendJson(res, 500, { error: err.message }));
			return;
		}

		sendJson(res, 404, { error: 'Not found' });
	} catch (err) {
		console.error('[API] Error:', err);
		sendJson(res, 500, { error: 'Internal server error' });
	}
});

// ── Boot ────────────────────────────────────────────────
loadEpochStore();

// Sign any unsigned leaves on startup (in case MINER_PRIVATE_KEY was missing before)
(async () => {
	if (!MINER_PRIVATE_KEY) {
		console.warn('[EpochStore] No MINER_PRIVATE_KEY — leaf signatures will be unavailable');
		return;
	}
	let signed = 0;
	for (const [epochId, leaves] of Object.entries(epochLeaves)) {
		let dirty = false;
		for (const leaf of leaves) {
			if (!leaf.signature && leaf.outputHash) {
				leaf.signature = await signOutput(leaf.outputHash);
				if (leaf.signature) { signed++; dirty = true; }
			}
		}
		if (dirty) {
			// Update the epoch file on disk
			const file = path.join(EPOCH_STORE_DIR, `epoch-${epochId}.json`);
			if (fs.existsSync(file)) {
				try {
					const data = JSON.parse(fs.readFileSync(file, 'utf8'));
					data.leaves = leaves;
					fs.writeFileSync(file, JSON.stringify(data, null, 2));
				} catch (err) {
					console.error(`[EpochStore] Failed to re-save epoch ${epochId}:`, err.message);
				}
			}
		}
	}
	// Update latestOutput signature if it was null
	if (latestOutput && !latestOutput.signature && latestOutput.outputHash) {
		latestOutput.signature = await signOutput(latestOutput.outputHash);
	}
	if (signed > 0) console.log(`[EpochStore] Signed ${signed} previously-unsigned leaves`);
})();

server.listen(PORT, HOST, () => {
	console.log(`\n🎲 Randomness API listening on http://${HOST}:${PORT}`);
	console.log(`    GET  /api/randomness/latest`);
	console.log(`    GET  /api/randomness/:outputHash/proof`);
	console.log(`    GET  /api/epochs`);
	console.log(`    GET  /api/epochs/:epochId`);
	console.log(`    POST /api/epochs`);
	console.log(`    GET  /api/health`);
	console.log(`    GET  /api/utxo/scan`);
	console.log(`    GET  /api/utxo/latest`);
	console.log(`    GET  /api/utxo/anchor/latest`);
	console.log(`    GET  /api/utxo/anchor/:anchorId`);
	console.log(`    GET  /api/utxo/inventory`);
	console.log(`    GET  /api/utxo/history`);
	console.log(`    POST /api/utxo/certificate-payload`);
	console.log(`    GET  /api/utxo/discover?blocks=3\n`);
});

module.exports = { saveEpoch, epochIndex, epochLeaves };
