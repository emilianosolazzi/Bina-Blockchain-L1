const http = require('http');
const https = require('https');
const fs = require('fs');
const path = require('path');
const { URL } = require('url');
const { ethers } = require('ethers');
const { createStore } = require('./solution-store');

const HOST = process.env.HOST || '127.0.0.1';
const PORT = Number(process.env.PORT || 4173);
const ROOT = __dirname;
const INDEX = path.join(ROOT, 'index.html');
const TELEMETRY_FILE = process.env.TELEMETRY_FILE || path.resolve(ROOT, '..', 'rust', 'miner-telemetry.jsonl');
const RELAY_STATUS_FILE = process.env.RELAY_STATUS_FILE || path.join(path.dirname(TELEMETRY_FILE), `${path.parse(TELEMETRY_FILE).name}.relay-status.json`);
const RANDOMNESS_API_URL = process.env.RANDOMNESS_API_URL || 'http://127.0.0.1:4271';
const HEARTBEAT_API_URL = process.env.HEARTBEAT_API_URL || 'http://127.0.0.1:4380';
const RPC_URL = process.env.RPC_URL || 'https://ethereum-sepolia-rpc.publicnode.com';
const CORE_ADDRESS = process.env.CORE_ADDRESS || '0x843fAc753610163776374Ab0261029BAEA0251b7';
const TGBT_ADDRESS = process.env.TGBT_ADDRESS || '0x496598fDeab78fb2986e89d396249779595418E9';
const TOKENOMICS_ADDRESS = process.env.TOKENOMICS_ADDRESS || '0xcf0a632A88D759f4A4ad0eA0317B5BE5A10638A5';
const BATCH_ADDRESS = process.env.BATCH_ADDRESS || '0xd52467e0C442c0817665fdB11f86FC47dC56ef3E';
const WALLET_ADDRESS = process.env.WALLET_ADDRESS || '0x3058bd411b9ec0dF6C7d0b04914C9bd2934b7fb3';
const CHALLENGE_WINDOW = Number(process.env.CHALLENGE_WINDOW || 100);
const MODULE_IDS = {
	BATCH_MINING_MODULE: ethers.id('BATCH_MINING_MODULE'),
	TOKENOMICS_MODULE: ethers.id('TOKENOMICS_MODULE'),
};

const provider = new ethers.JsonRpcProvider(RPC_URL);
const CORE_ABI = [
	'function moduleAddress(bytes32 moduleId) view returns (address)',
];
const TGBT_ABI = [
	'function balanceOf(address) view returns (uint256)',
	'function decimals() view returns (uint8)',
	'function symbol() view returns (string)',
];
const BATCH_ABI = [
	'function currentEpochId() view returns (uint256)',
	'function getEpochInfo(uint256 epochId) view returns (tuple(bytes32 merkleRoot, uint64 startBlock, uint64 endBlock, uint32 leafCount, address operator, uint8 poolId, bool finalized, uint256 totalReward, bool storageAttested, bytes32 attestationHash))',
];

// Difficulty-based reward estimate (mirrors miner Rust logic)
const DIFFICULTY_BITS = 11;
const EST_TGBT_PER_SOLUTION = Math.max(DIFFICULTY_BITS / 8, 1); // 1.375

let store = null;

function toChecksumOrNull(value) {
	if (!value) return null;
	try {
		return ethers.getAddress(value);
	} catch {
		return value;
	}
}

function shortError(err) {
	if (!err) return 'Unknown error';
	return err.message || String(err);
}

function requestJson(targetUrl, options = {}) {
	return new Promise((resolve, reject) => {
		const parsed = new URL(targetUrl);
		const transport = parsed.protocol === 'https:' ? https : http;
		const req = transport.request(parsed, {
			method: options.method || 'GET',
			headers: {
				'Accept': 'application/json',
				...(options.headers || {}),
			},
		}, (res) => {
			let raw = '';
			res.setEncoding('utf8');
			res.on('data', chunk => raw += chunk);
			res.on('end', () => {
				let json = null;
				try {
					json = raw ? JSON.parse(raw) : null;
				} catch {
					json = { raw };
				}
				resolve({ status: res.statusCode || 500, json, headers: res.headers });
			});
		});
		req.on('error', reject);
		if (options.timeoutMs) {
			req.setTimeout(options.timeoutMs, () => req.destroy(new Error(`Request timed out after ${options.timeoutMs}ms`)));
		}
		if (options.body) {
			req.write(options.body);
		}
		req.end();
	});
}

function readRelayStatus() {
	if (!fs.existsSync(RELAY_STATUS_FILE)) {
		return {
			enabled: false,
			state: 'disabled',
			endpoint: null,
			stats: {
				bytes_sent: 0,
				bytes_received: 0,
				messages_sent: 0,
				messages_received: 0,
				noise_bytes_sent: 0,
				reconnect_count: 0,
				key_refreshes: 0,
				integrity_failures: 0,
			},
			last_error: null,
			updated_at: null,
			available: false,
		};
	}

	try {
		return {
			...JSON.parse(fs.readFileSync(RELAY_STATUS_FILE, 'utf8')),
			available: true,
		};
	} catch (err) {
		return {
			enabled: false,
			state: 'error',
			endpoint: null,
			stats: {
				bytes_sent: 0,
				bytes_received: 0,
				messages_sent: 0,
				messages_received: 0,
				noise_bytes_sent: 0,
				reconnect_count: 0,
				key_refreshes: 0,
				integrity_failures: 0,
			},
			last_error: shortError(err),
			updated_at: null,
			available: false,
		};
	}
}

async function getSystemStatus() {
	const core = new ethers.Contract(CORE_ADDRESS, CORE_ABI, provider);
	const tgbt = new ethers.Contract(TGBT_ADDRESS, TGBT_ABI, provider);
	const batch = new ethers.Contract(BATCH_ADDRESS, BATCH_ABI, provider);

	const [randomnessApi, heartbeatApi, ethBalance, tokenBalance, tokenDecimals, tokenSymbol, coreBatchModule, coreTokenomicsModule, nextEpochId, currentBlock, network] = await Promise.all([
		requestJson(`${RANDOMNESS_API_URL}/api/health`, { timeoutMs: 4000 }).catch(err => ({
			status: 503,
			json: { error: shortError(err) },
		})),
		requestJson(`${HEARTBEAT_API_URL}/api/health`, { timeoutMs: 4000 }).catch(err => ({
			status: 503,
			json: { error: shortError(err) },
		})),
		provider.getBalance(WALLET_ADDRESS),
		tgbt.balanceOf(WALLET_ADDRESS),
		tgbt.decimals(),
		tgbt.symbol(),
		core.moduleAddress(MODULE_IDS.BATCH_MINING_MODULE),
		core.moduleAddress(MODULE_IDS.TOKENOMICS_MODULE),
		batch.currentEpochId(),
		provider.getBlockNumber(),
		provider.getNetwork(),
	]);

	let latestOnChainEpoch = null;
	if (Number(nextEpochId) > 0) {
		try {
			const epochId = Number(nextEpochId) - 1;
			const info = await batch.getEpochInfo(epochId);
			latestOnChainEpoch = {
				epochId,
				merkleRoot: info.merkleRoot,
				startBlock: Number(info.startBlock),
				endBlock: Number(info.endBlock),
				leafCount: Number(info.leafCount),
				operator: info.operator,
				poolId: Number(info.poolId),
				finalized: info.finalized,
				totalReward: ethers.formatEther(info.totalReward),
				storageAttested: info.storageAttested,
				attestationHash: info.attestationHash,
				blocksUntilFinalizable: Math.max(0, Number(info.startBlock) + CHALLENGE_WINDOW - Number(currentBlock)),
				blocksPastChallenge: Math.max(0, Number(currentBlock) - (Number(info.startBlock) + CHALLENGE_WINDOW)),
			};
		} catch (err) {
			latestOnChainEpoch = { error: shortError(err) };
		}
	}

	return {
		dashboard: {
			host: HOST,
			port: PORT,
			telemetryFile: TELEMETRY_FILE,
			relayStatusFile: RELAY_STATUS_FILE,
			solutionsBackend: process.env.MONGODB_URI ? 'mongodb' : 'file',
			relayStatus: readRelayStatus(),
		},
		randomnessApi: {
			url: RANDOMNESS_API_URL,
			online: randomnessApi.status >= 200 && randomnessApi.status < 300,
			status: randomnessApi.status,
			data: randomnessApi.json,
		},
		heartbeatApi: {
			url: HEARTBEAT_API_URL,
			online: heartbeatApi.status >= 200 && heartbeatApi.status < 300,
			status: heartbeatApi.status,
			data: heartbeatApi.json,
		},
		chain: {
			rpcUrl: RPC_URL,
			chainId: Number(network.chainId),
			currentBlock: Number(currentBlock),
			challengeWindow: CHALLENGE_WINDOW,
			walletAddress: WALLET_ADDRESS,
			ethBalance: ethers.formatEther(ethBalance),
			token: {
				address: TGBT_ADDRESS,
				symbol: tokenSymbol,
				balance: ethers.formatUnits(tokenBalance, tokenDecimals),
			},
			contracts: {
				core: CORE_ADDRESS,
				tokenomics: TOKENOMICS_ADDRESS,
				batch: BATCH_ADDRESS,
				coreBatchModule: toChecksumOrNull(coreBatchModule),
				coreTokenomicsModule: toChecksumOrNull(coreTokenomicsModule),
				batchWiredCorrectly: toChecksumOrNull(coreBatchModule)?.toLowerCase() === BATCH_ADDRESS.toLowerCase(),
				tokenomicsWiredCorrectly: toChecksumOrNull(coreTokenomicsModule)?.toLowerCase() === TOKENOMICS_ADDRESS.toLowerCase(),
			},
			nextEpochId: Number(nextEpochId),
			latestOnChainEpoch,
		},
	};
}

function deriveHeartbeatSummary(limit = 720) {
	const snapshots = readSnapshots(limit);
	const latest = snapshots[snapshots.length - 1] || null;
	if (!latest) {
		return {
			continuousVerifiedHours: 0,
			continuousVerifiedLabel: 'No telemetry yet',
			gapCount: 0,
			longestGapMs: 0,
			currentGapMs: null,
			averageSolutionIntervalMs: null,
			targetGapMs: 30000,
			history: [],
			snapshotCount: 0,
		};
	}

	const acceptedEvents = [];
	let prevAccepted = null;
	let prevNonce = null;
	for (const snap of snapshots) {
		const accepted = Number(snap.accepted_submissions || 0);
		const nonce = snap.last_solution_nonce == null ? null : Number(snap.last_solution_nonce);
		const changed = prevAccepted !== null && (accepted > prevAccepted || (nonce != null && nonce !== prevNonce));
		if (changed || (prevAccepted === null && accepted > 0 && nonce != null)) {
			acceptedEvents.push({
				timestampMs: Number(snap.timestamp_unix_ms || 0),
				hashrateHs: Number(snap.hashrate_hs || 0),
				temperatureC: snap.temperature_c == null ? null : Number(snap.temperature_c),
				acceptedSubmissions: accepted,
				nonce,
				outputHash: snap.last_output_hash_hex || snap.last_solution_hash_hex || null,
			});
		}
		prevAccepted = accepted;
		prevNonce = nonce;
	}

	const gaps = [];
	for (let i = 1; i < acceptedEvents.length; i += 1) {
		const gapMs = acceptedEvents[i].timestampMs - acceptedEvents[i - 1].timestampMs;
		if (gapMs > 0) gaps.push(gapMs);
	}
	const averageSolutionIntervalMs = gaps.length
		? gaps.reduce((sum, value) => sum + value, 0) / gaps.length
		: null;
	const targetGapMs = Math.max(30000, Math.round((averageSolutionIntervalMs || 5000) * 6));
	const gapCount = gaps.filter(gap => gap > targetGapMs).length;
	const longestGapMs = gaps.length ? Math.max(...gaps) : 0;
	const currentGapMs = acceptedEvents.length ? Math.max(0, Date.now() - acceptedEvents[acceptedEvents.length - 1].timestampMs) : null;
	const continuousVerifiedHours = Math.max(0, Number(latest.uptime_seconds || 0)) / 3600;
	const continuousVerifiedLabel = gapCount === 0
		? `Your device has been continuously verified for ${continuousVerifiedHours.toFixed(1)} hours. No gaps detected.`
		: `Your device has been continuously verified for ${continuousVerifiedHours.toFixed(1)} hours with ${gapCount} heartbeat gap${gapCount === 1 ? '' : 's'} detected.`;

	const history = acceptedEvents.slice(-24).map((event, index, arr) => ({
		timestamp: new Date(event.timestampMs).toISOString(),
		acceptedSubmissions: event.acceptedSubmissions,
		nonce: event.nonce,
		outputHash: event.outputHash,
		hashrateHs: event.hashrateHs,
		temperatureC: event.temperatureC,
		gapMs: index === 0 ? null : Math.max(0, event.timestampMs - arr[index - 1].timestampMs),
		gapFlag: index === 0 ? false : Math.max(0, event.timestampMs - arr[index - 1].timestampMs) > targetGapMs,
	}));

	return {
		continuousVerifiedHours,
		continuousVerifiedLabel,
		gapCount,
		longestGapMs,
		currentGapMs,
		averageSolutionIntervalMs,
		targetGapMs,
		history,
		snapshotCount: snapshots.length,
	};
}

async function getHeartbeatStatus() {
	const response = await requestJson(`${HEARTBEAT_API_URL}/api/heartbeat/status`, { timeoutMs: 4000 }).catch(err => ({
		status: 503,
		json: { error: shortError(err) },
	}));
	return response.status >= 200 && response.status < 300
		? response.json
		: { error: response.json?.error || `Heartbeat API returned ${response.status}` };
}

async function getHeartbeatAlerts() {
	const response = await requestJson(`${HEARTBEAT_API_URL}/api/heartbeat/alerts?all=1`, { timeoutMs: 4000 }).catch(err => ({
		status: 503,
		json: { error: shortError(err) },
	}));
	return response.status >= 200 && response.status < 300
		? response.json
		: { error: response.json?.error || `Heartbeat API returned ${response.status}`, active: [], history: [] };
}

async function getThreatProfile() {
	const [heartbeatStatus, heartbeatAlerts] = await Promise.all([
		getHeartbeatStatus(),
		getHeartbeatAlerts(),
	]);
	const telemetry = deriveHeartbeatSummary();
	return {
		heartbeatStatus,
		heartbeatAlerts,
		telemetry,
	};
}

async function getRelayProfile() {
	const [system, heartbeatStatus, latestRandomness] = await Promise.all([
		getSystemStatus(),
		getHeartbeatStatus(),
		requestJson(`${RANDOMNESS_API_URL}/api/randomness/latest`, { timeoutMs: 4000 }).catch(err => ({
			status: 503,
			json: { error: shortError(err) },
		})),
	]);

	const latest = latestRandomness.status >= 200 && latestRandomness.status < 300 ? latestRandomness.json : null;
	const relayStatus = readRelayStatus();
	const relayReady = !!(
		system.randomnessApi?.online &&
		system.heartbeatApi?.online &&
		heartbeatStatus?.heartbeat?.online &&
		heartbeatStatus?.security?.suspicious === false &&
		latest?.signature &&
		(relayStatus.state === 'connected' || relayStatus.enabled === false)
	);

	const profile = {
		version: 1,
		exportedAt: new Date().toISOString(),
		relayReady,
		miner: {
			name: heartbeatStatus?.miner?.name || 'Temporal Gradient miner',
			region: heartbeatStatus?.miner?.region || 'unknown',
			operator: heartbeatStatus?.miner?.operator || system.chain?.walletAddress || WALLET_ADDRESS,
		},
		transport: {
			rpcReachable: true,
			randomnessApiOnline: !!system.randomnessApi?.online,
			heartbeatOnline: !!heartbeatStatus?.heartbeat?.online,
			telemetryFresh: !!heartbeatStatus?.heartbeat?.telemetryFresh,
			intrusionScore: Number(heartbeatStatus?.security?.intrusionScore || 0),
			relayChannel: relayStatus,
		},
		proofOfPresence: latest ? {
			outputHash: latest.outputHash,
			epochId: latest.epochId,
			leafIndex: latest.leafIndex,
			timestamp: latest.timestamp,
			signature: latest.signature,
			recoveredSigner: system.chain?.walletAddress || WALLET_ADDRESS,
		} : null,
		capabilities: [
			'heartbeat-attested-egress',
			'verified-miner-identity',
			'continuous-connectivity-monitoring',
			'future-peer-relay-ready',
		],
		constraints: [
			'No packet forwarding plane implemented yet.',
			'Peer discovery and encrypted relay circuits still need to be added.',
			'Current build proves miner continuity and relay readiness only.',
		],
	};

	return profile;
}

async function getOnChainEpoch(epochId) {
	const batch = new ethers.Contract(BATCH_ADDRESS, BATCH_ABI, provider);
	const currentBlock = await provider.getBlockNumber();
	const info = await batch.getEpochInfo(epochId);
	if (!info || !info.merkleRoot || /^0x0+$/.test(info.merkleRoot)) {
		throw new Error(`Epoch ${epochId} not found on-chain`);
	}
	return {
		epochId: Number(epochId),
		merkleRoot: info.merkleRoot,
		startBlock: Number(info.startBlock),
		endBlock: Number(info.endBlock),
		leafCount: Number(info.leafCount),
		operator: info.operator,
		poolId: Number(info.poolId),
		finalized: info.finalized,
		totalReward: ethers.formatEther(info.totalReward),
		storageAttested: info.storageAttested,
		attestationHash: info.attestationHash,
		currentBlock: Number(currentBlock),
		challengeWindow: CHALLENGE_WINDOW,
		blocksUntilFinalizable: Math.max(0, Number(info.startBlock) + CHALLENGE_WINDOW - Number(currentBlock)),
		blocksPastChallenge: Math.max(0, Number(currentBlock) - (Number(info.startBlock) + CHALLENGE_WINDOW)),
	};
}

// ── Telemetry-to-solution detector ──────────────────────
let prevSnap = null;

function normalizePhase(snap) {
	if (!snap) return null;
	return snap.mining_phase || (snap.state === 'running' ? 'searching' : null);
}

function syncLatestSolutionDetails(snap) {
	if (!store || !snap || snap.last_solution_nonce == null) return;

	store.updateSolutionDetails({
		nonce: snap.last_solution_nonce,
		accepted: true,
		hash: snap.last_solution_hash_hex || undefined,
		commitHash: snap.last_commit_hash_hex || snap.last_solution_hash_hex || undefined,
		outputHash: snap.last_output_hash_hex || snap.last_solution_hash_hex || undefined,
		phase: normalizePhase(snap) || undefined,
	}).catch(err => console.error('[SolutionStore] update error:', err.message));
}

function detectAndStoreSolution(snap) {
	if (!store || !snap) return;
	if (!prevSnap) { prevSnap = snap; return; }

	const prevAcc = prevSnap.accepted_submissions || 0;
	const prevRej = prevSnap.rejected_submissions || 0;
	const currAcc = snap.accepted_submissions || 0;
	const currRej = snap.rejected_submissions || 0;

	// Detect new accepted solutions
	if (currAcc > prevAcc) {
		const onChainDelta = (snap.total_rewards_estimate || 0) - (prevSnap.total_rewards_estimate || 0);
		const reward = onChainDelta > 0 ? onChainDelta : EST_TGBT_PER_SOLUTION;
		store.insertSolution({
			timestamp: new Date(Number(snap.timestamp_unix_ms)).toISOString(),
			timestampMs: Number(snap.timestamp_unix_ms),
			nonce: snap.last_solution_nonce,
			hash: snap.last_solution_hash_hex,
			commitHash: snap.last_commit_hash_hex || snap.last_solution_hash_hex,
			outputHash: snap.last_output_hash_hex || snap.last_solution_hash_hex || null,
			reward,
			estimated: onChainDelta <= 0,
			accepted: true,
			phase: normalizePhase(snap),
			uptime: snap.uptime_seconds,
			hashrate: snap.hashrate_hs,
			totalHashes: snap.hashes,
			solutionNumber: currAcc,
		}).catch(err => console.error('[SolutionStore] insert error:', err.message));
	}

	// Detect new rejected submissions
	if (currRej > prevRej) {
		store.insertSolution({
			timestamp: new Date(Number(snap.timestamp_unix_ms)).toISOString(),
			timestampMs: Number(snap.timestamp_unix_ms),
			nonce: snap.last_solution_nonce,
			hash: snap.last_solution_hash_hex,
			commitHash: snap.last_commit_hash_hex || null,
			outputHash: snap.last_output_hash_hex || null,
			reward: 0,
			estimated: false,
			accepted: false,
			phase: normalizePhase(snap),
			uptime: snap.uptime_seconds,
			hashrate: snap.hashrate_hs,
			totalHashes: snap.hashes,
			solutionNumber: null,
		}).catch(err => console.error('[SolutionStore] insert error:', err.message));
	}

	syncLatestSolutionDetails(snap);

	prevSnap = snap;
}

// ── Utility ─────────────────────────────────────────────

function sendJson(res, status, payload) {
	res.writeHead(status, {
		'Content-Type': 'application/json; charset=utf-8',
		'Cache-Control': 'no-store',
		'Access-Control-Allow-Origin': '*',
	});
	res.end(JSON.stringify(payload));
}

function readSnapshots(limit = 120) {
	if (!fs.existsSync(TELEMETRY_FILE)) {
		return [];
	}

	const text = fs.readFileSync(TELEMETRY_FILE, 'utf8');
	return text
		.split(/\r?\n/)
		.map((line) => line.trim())
		.filter(Boolean)
		.slice(-limit)
		.map((line) => {
			try {
				return JSON.parse(line);
			} catch {
				return null;
			}
		})
		.filter(Boolean);
}

function latestSnapshot() {
	const snapshots = readSnapshots(1);
	return snapshots[0] || null;
}

function sendEvent(res, event, data) {
	res.write(`event: ${event}\n`);
	res.write(`data: ${JSON.stringify(data)}\n\n`);
}

function readBody(req) {
	return new Promise((resolve, reject) => {
		const chunks = [];
		req.on('data', c => chunks.push(c));
		req.on('end', () => {
			try { resolve(JSON.parse(Buffer.concat(chunks).toString())); }
			catch (e) { reject(e); }
		});
		req.on('error', reject);
	});
}

async function proxyRandomnessApi(req, res, targetPath, method = 'GET', body = null) {
	try {
		const targetUrl = `${RANDOMNESS_API_URL}${targetPath}`;
		const response = await requestJson(targetUrl, {
			method,
			body: body ? JSON.stringify(body) : null,
			headers: body ? { 'Content-Type': 'application/json' } : {},
			timeoutMs: 10000,
		});
		sendJson(res, response.status, response.json ?? {});
	} catch (err) {
		sendJson(res, 502, {
			error: 'Failed to reach randomness API',
			detail: shortError(err),
			target: `${RANDOMNESS_API_URL}${targetPath}`,
		});
	}
}

// ── HTTP Server ─────────────────────────────────────────

const server = http.createServer(async (req, res) => {
	const url = new URL(req.url, `http://${req.headers.host}`);

	if (url.pathname === '/') {
		res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
		fs.createReadStream(INDEX).pipe(res);
		return;
	}

	if (url.pathname === '/api/history') {
		const limit = Math.max(1, Math.min(500, Number(url.searchParams.get('limit') || 120)));
		const history = readSnapshots(limit);
		sendJson(res, 200, {
			telemetryPath: TELEMETRY_FILE,
			latest: history[history.length - 1] || null,
			history,
		});
		return;
	}

	if (url.pathname === '/api/latest') {
		sendJson(res, 200, {
			telemetryPath: TELEMETRY_FILE,
			latest: latestSnapshot(),
		});
		return;
	}

	if (url.pathname === '/api/system/status' && req.method === 'GET') {
		try {
			const status = await getSystemStatus();
			sendJson(res, 200, status);
		} catch (err) {
			sendJson(res, 500, { error: shortError(err) });
		}
		return;
	}

	if (url.pathname === '/api/security/heartbeat/status' && req.method === 'GET') {
		try {
			const status = await getHeartbeatStatus();
			sendJson(res, status?.error ? 503 : 200, status);
		} catch (err) {
			sendJson(res, 500, { error: shortError(err) });
		}
		return;
	}

	if (url.pathname === '/api/security/heartbeat/alerts' && req.method === 'GET') {
		try {
			const alerts = await getHeartbeatAlerts();
			sendJson(res, alerts?.error ? 503 : 200, alerts);
		} catch (err) {
			sendJson(res, 500, { error: shortError(err) });
		}
		return;
	}

	if (url.pathname === '/api/security/threat-profile' && req.method === 'GET') {
		try {
			const profile = await getThreatProfile();
			sendJson(res, 200, profile);
		} catch (err) {
			sendJson(res, 500, { error: shortError(err) });
		}
		return;
	}

	if (url.pathname === '/api/security/relay-profile' && req.method === 'GET') {
		try {
			const profile = await getRelayProfile();
			sendJson(res, 200, profile);
		} catch (err) {
			sendJson(res, 500, { error: shortError(err) });
		}
		return;
	}

	if (url.pathname === '/api/security/relay-status' && req.method === 'GET') {
		sendJson(res, 200, readRelayStatus());
		return;
	}

	const onChainEpochMatch = url.pathname.match(/^\/api\/system\/onchain-epoch\/(\d+)$/);
	if (onChainEpochMatch && req.method === 'GET') {
		try {
			const epoch = await getOnChainEpoch(Number(onChainEpochMatch[1]));
			sendJson(res, 200, epoch);
		} catch (err) {
			sendJson(res, 404, { error: shortError(err), epochId: Number(onChainEpochMatch[1]) });
		}
		return;
	}

	if (url.pathname === '/api/network/health' && req.method === 'GET') {
		await proxyRandomnessApi(req, res, '/api/health');
		return;
	}

	if (url.pathname === '/api/network/randomness/latest' && req.method === 'GET') {
		await proxyRandomnessApi(req, res, '/api/randomness/latest');
		return;
	}

	const proofProxyMatch = url.pathname.match(/^\/api\/network\/randomness\/([0-9a-fA-Fx]+)\/proof$/);
	if (proofProxyMatch && req.method === 'GET') {
		await proxyRandomnessApi(req, res, `/api/randomness/${proofProxyMatch[1]}/proof`);
		return;
	}

	if (url.pathname === '/api/network/epochs' && req.method === 'GET') {
		await proxyRandomnessApi(req, res, `/api/epochs${url.search || ''}`);
		return;
	}

	const epochProxyMatch = url.pathname.match(/^\/api\/network\/epochs\/(\d+)$/);
	if (epochProxyMatch && req.method === 'GET') {
		await proxyRandomnessApi(req, res, `/api/epochs/${epochProxyMatch[1]}`);
		return;
	}

	const verifyStorageMatch = url.pathname.match(/^\/api\/network\/epochs\/(\d+)\/verify-storage$/);
	if (verifyStorageMatch && req.method === 'POST') {
		await proxyRandomnessApi(req, res, `/api/epochs/${verifyStorageMatch[1]}/verify-storage`, 'POST');
		return;
	}

	// ── Solution storage API ─────────────────────────────
	if (url.pathname === '/api/solutions' && req.method === 'GET') {
		try {
			const limit = Math.max(1, Math.min(200, Number(url.searchParams.get('limit') || 50)));
			const skip = Math.max(0, Number(url.searchParams.get('skip') || 0));
			const filter = url.searchParams.get('filter'); // 'accepted' | 'rejected' | null
			const sinceMs = Number(url.searchParams.get('sinceMs') || 0);
			let solutions = await store.getSolutions({ limit: 100000, skip: 0 });
			if (filter === 'accepted') solutions = solutions.filter(s => s.accepted);
			if (filter === 'rejected') solutions = solutions.filter(s => !s.accepted);
			if (sinceMs > 0) {
				solutions = solutions.filter(s => Number(s.timestampMs || Date.parse(s.timestamp || s.createdAt || 0)) >= sinceMs);
			}
			const stats = {
				total: solutions.length,
				accepted: solutions.filter(s => s.accepted).length,
				rejected: solutions.filter(s => !s.accepted).length,
				totalRewards: solutions.reduce((sum, s) => sum + Number(s.reward || 0), 0),
			};
			solutions = solutions.slice(skip, skip + limit);
			sendJson(res, 200, { solutions, stats });
		} catch (err) {
			sendJson(res, 500, { error: err.message });
		}
		return;
	}

	if (url.pathname === '/api/solutions/stats' && req.method === 'GET') {
		try {
			const stats = await store.getStats();
			sendJson(res, 200, stats);
		} catch (err) {
			sendJson(res, 500, { error: err.message });
		}
		return;
	}

	if (url.pathname === '/api/solutions/latest' && req.method === 'GET') {
		try {
			const latest = await store.getLatest();
			sendJson(res, 200, { solution: latest });
		} catch (err) {
			sendJson(res, 500, { error: err.message });
		}
		return;
	}

	if (url.pathname === '/events') {
		res.writeHead(200, {
			'Content-Type': 'text/event-stream; charset=utf-8',
			'Cache-Control': 'no-cache, no-transform',
			Connection: 'keep-alive',
			'Access-Control-Allow-Origin': '*',
		});

		let lastTimestamp = null;
		sendEvent(res, 'meta', { telemetryPath: TELEMETRY_FILE });
		const initial = latestSnapshot();
		if (initial) {
			lastTimestamp = initial.timestamp_unix_ms;
			sendEvent(res, 'snapshot', initial);
		}

		const timer = setInterval(() => {
			const latest = latestSnapshot();
			if (!latest) {
				return;
			}
			syncLatestSolutionDetails(latest);
			if (latest.timestamp_unix_ms !== lastTimestamp) {
				lastTimestamp = latest.timestamp_unix_ms;
				detectAndStoreSolution(latest);
				sendEvent(res, 'snapshot', latest);
			}
		}, 1000);

		req.on('close', () => clearInterval(timer));
		return;
	}

	sendJson(res, 404, { error: 'Not found' });
});

// ── Bootstrap ───────────────────────────────────────────

(async () => {
	try {
		store = await createStore();
	} catch (err) {
		console.error('[SolutionStore] Failed to initialise store:', err.message);
		process.exit(1);
	}

	// Backfill: scan current telemetry for any solutions not yet stored
	const existing = await store.getStats();
	if (existing.total === 0) {
		console.log('[SolutionStore] Scanning telemetry for historical solutions…');
		const allSnaps = readSnapshots(500);
		let prev = null;
		for (const snap of allSnaps) {
			if (prev) {
				const prevAcc = prev.accepted_submissions || 0;
				const currAcc = snap.accepted_submissions || 0;
				const prevRej = prev.rejected_submissions || 0;
				const currRej = snap.rejected_submissions || 0;
				if (currAcc > prevAcc) {
					const onChainDelta = (snap.total_rewards_estimate || 0) - (prev.total_rewards_estimate || 0);
					const reward = onChainDelta > 0 ? onChainDelta : EST_TGBT_PER_SOLUTION;
					await store.insertSolution({
						timestamp: new Date(Number(snap.timestamp_unix_ms)).toISOString(),
						timestampMs: Number(snap.timestamp_unix_ms),
						nonce: snap.last_solution_nonce,
						hash: snap.last_solution_hash_hex,
						commitHash: snap.last_commit_hash_hex || snap.last_solution_hash_hex,
						outputHash: snap.last_output_hash_hex || null,
						reward,
						estimated: onChainDelta <= 0,
						accepted: true,
						phase: normalizePhase(snap),
						uptime: snap.uptime_seconds,
						hashrate: snap.hashrate_hs,
						totalHashes: snap.hashes,
						solutionNumber: currAcc,
					});
				}
				if (currRej > prevRej) {
					await store.insertSolution({
						timestamp: new Date(Number(snap.timestamp_unix_ms)).toISOString(),
						timestampMs: Number(snap.timestamp_unix_ms),
						nonce: snap.last_solution_nonce,
						hash: snap.last_solution_hash_hex,
						commitHash: snap.last_commit_hash_hex || null,
						outputHash: snap.last_output_hash_hex || null,
						reward: 0,
						estimated: false,
						accepted: false,
						phase: normalizePhase(snap),
						uptime: snap.uptime_seconds,
						hashrate: snap.hashrate_hs,
						totalHashes: snap.hashes,
						solutionNumber: null,
					});
				}
			}
			prev = snap;
		}
		prevSnap = prev;
		const stats = await store.getStats();
		console.log(`[SolutionStore] Backfill complete: ${stats.accepted} accepted, ${stats.rejected} rejected`);
	} else {
		console.log(`[SolutionStore] Loaded existing data: ${existing.total} solutions`);
		// Set prevSnap so live detection resumes correctly
		prevSnap = latestSnapshot();
	}

	server.listen(PORT, HOST, () => {
		console.log(`Temporal Gradient dashboard listening on http://${HOST}:${PORT}`);
		console.log(`Telemetry file: ${TELEMETRY_FILE}`);
		console.log(`Randomness API: ${RANDOMNESS_API_URL}`);
		console.log(`RPC URL: ${RPC_URL}`);
	});
})();
