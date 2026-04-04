const http = require('http');
const https = require('https');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { URL } = require('url');
const { ethers } = require('ethers');
const { createStore } = require('./solution-store');

const HOST = process.env.HOST || '127.0.0.1';
const PORT = Number(process.env.PORT || 4173);
const ROOT = __dirname;
const INDEX = path.join(ROOT, 'index.html');
const TELEMETRY_FILE = process.env.TELEMETRY_FILE || (process.env.LOCALAPPDATA
	? path.join(process.env.LOCALAPPDATA, 'entropy', 'TemporalGradientMiner', 'data', 'logs', 'telemetry.jsonl')
	: path.resolve(ROOT, '..', 'rust', 'miner-telemetry.jsonl'));
const RELAY_STATUS_FILE = process.env.RELAY_STATUS_FILE || path.join(path.dirname(TELEMETRY_FILE), `${path.parse(TELEMETRY_FILE).name}.relay-status.json`);
const CONTROL_FILE = process.env.CONTROL_FILE || path.join(path.dirname(TELEMETRY_FILE), 'miner-control.json');
const RANDOMNESS_API_URL = process.env.RANDOMNESS_API_URL || 'http://127.0.0.1:4271';
const RANDOMNESS_API_FALLBACK_URL = process.env.RANDOMNESS_API_FALLBACK_URL || 'http://127.0.0.1:3100';
const HEARTBEAT_API_URL = process.env.HEARTBEAT_API_URL || 'http://127.0.0.1:4380';
const RANDOMNESS_HEALTH_PATH = process.env.RANDOMNESS_HEALTH_PATH || '/api/health';
const RANDOMNESS_LATEST_PATH = process.env.RANDOMNESS_LATEST_PATH || '/api/randomness/latest';
const RANDOMNESS_FALLBACK_HEALTH_PATH = process.env.RANDOMNESS_FALLBACK_HEALTH_PATH || '/healthz';
const RANDOMNESS_FALLBACK_LATEST_PATH = process.env.RANDOMNESS_FALLBACK_LATEST_PATH || '/api/v1/latest';
const RPC_URL = process.env.RPC_URL || 'https://api.nativebtc.org/v1/arb';
const CORE_ADDRESS = process.env.CORE_ADDRESS || '0xF6556DDC7CdD3635A05428BD85BCf33A09F752e6';
const TGBT_ADDRESS = process.env.TGBT_ADDRESS || '0x31228eE520e895DA19f728DE5459b1b317d9b8D8';
const TOKENOMICS_ADDRESS = process.env.TOKENOMICS_ADDRESS || '0xF6069614FE09B91e5B00DA0a13A11B2BFcCabC36';
const BATCH_ADDRESS = process.env.BATCH_ADDRESS || '0xAf07E37D104E9be17639FE7a51B36972D4738651';
const WALLET_ADDRESS = process.env.WALLET_ADDRESS || '0x5cB4D906f0464b34c44d6555A770BF6aF4A2cEfe';
const RPC_API_KEY = process.env.RPC_API_KEY || 'fp_2d93df5e6cebe485b69c363a62e237fc9d0f88b9';
const CHALLENGE_WINDOW = Number(process.env.CHALLENGE_WINDOW || 28800);
const TELEMETRY_TAIL_CHUNK_BYTES = Number(process.env.TELEMETRY_TAIL_CHUNK_BYTES || 64 * 1024);
const TELEMETRY_TAIL_DEFAULT_MAX_BYTES = Number(process.env.TELEMETRY_TAIL_DEFAULT_MAX_BYTES || 4 * 1024 * 1024);
const STALE_PROOF_SCAN_LIMIT = Number(process.env.STALE_PROOF_SCAN_LIMIT || 5000);
const STALE_PROOF_SCAN_MAX_BYTES = Number(process.env.STALE_PROOF_SCAN_MAX_BYTES || 16 * 1024 * 1024);
const MODULE_IDS = {
	BATCH_MINING_MODULE: ethers.id('BATCH_MINING_MODULE'),
	STALE_BLOCK_MODULE: ethers.id('STALE_BLOCK_MODULE'),
	TOKENOMICS_MODULE: ethers.id('TOKENOMICS_MODULE'),
};

const fetchReq = new ethers.FetchRequest(RPC_URL);
if (RPC_API_KEY) fetchReq.setHeader('x-api-key', RPC_API_KEY);
const provider = new ethers.JsonRpcProvider(fetchReq, 42161, { staticNetwork: true, batchMaxCount: 1 });
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
const STALE_ORACLE_ABI = [
	'function getStaleProof(bytes32 blockHash) view returns ((bytes32 blockHash, bytes32 canonicalHash, bytes32 entropyDigest, uint64 height, uint32 reorgDepth, uint32 leadingZeros, uint32 qualityScore, uint64 submittedAt, address submitter, bool rewarded))',
	'function pendingReward(bytes32 blockHash) view returns (uint256)',
	'event StaleBlockSubmitted(bytes32 indexed blockHash, bytes32 indexed canonicalHash, address indexed submitter, uint64 height, uint32 reorgDepth, uint32 qualityScore, bytes32 entropyDigest)',
	'event StaleRewardClaimed(bytes32 indexed blockHash, address indexed submitter, uint256 reward)',
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

function toBytes32HexOrNull(value) {
	if (!value) return null;
	try {
		return ethers.hexlify(ethers.zeroPadValue(ethers.getBytes(value), 32));
	} catch {
		try {
			const normalized = String(value).trim().replace(/^0x/i, '');
			if (!/^[0-9a-fA-F]{64}$/.test(normalized)) return null;
			return `0x${normalized.toLowerCase()}`;
		} catch {
			return null;
		}
	}
}

function reverseBytes32Hex(value) {
	const normalized = toBytes32HexOrNull(value);
	if (!normalized) return null;
	const bytes = Array.from(ethers.getBytes(normalized)).reverse();
	return ethers.hexlify(Uint8Array.from(bytes));
}

function shortError(err) {
	if (!err) return 'Unknown error';
	return err.message || String(err);
}

function median(values) {
	if (!Array.isArray(values) || values.length === 0) return 0;
	const sorted = values.filter(v => Number.isFinite(Number(v))).map(Number).sort((a, b) => a - b);
	if (sorted.length === 0) return 0;
	const mid = Math.floor(sorted.length / 2);
	return sorted.length % 2 === 0
		? (sorted[mid - 1] + sorted[mid]) / 2
		: sorted[mid];
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
					// Return a structured error instead of silently wrapping HTML/text as {raw: ...}
					const contentType = (res.headers['content-type'] || '').toLowerCase();
					const isHtml = contentType.includes('text/html') || raw.trimStart().startsWith('<');
					json = {
						error: isHtml
							? 'Service returned HTML instead of JSON (likely an error page or proxy gateway)'
							: 'Service returned non-JSON response',
						_parseError: true,
						status: res.statusCode,
					};
					console.warn(`[requestJson] Non-JSON response from ${targetUrl} (${res.statusCode}): ${raw.slice(0, 200)}`);
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

async function fetchRandomnessHealth() {
	const primary = await requestJson(`${RANDOMNESS_API_URL}${RANDOMNESS_HEALTH_PATH}`, { timeoutMs: 4000 }).catch(err => ({
		status: 503,
		json: { error: shortError(err) },
	}));
	if (primary.status >= 200 && primary.status < 300) {
		return primary;
	}
	const fallback = await requestJson(`${RANDOMNESS_API_FALLBACK_URL}${RANDOMNESS_FALLBACK_HEALTH_PATH}`, { timeoutMs: 4000 }).catch(err => ({
		status: 503,
		json: { error: shortError(err) },
	}));
	if (fallback.status >= 200 && fallback.status < 300) {
		return {
			...fallback,
			json: {
				...(fallback.json || {}),
				source: 'beacon-api',
			},
		};
	}
	return primary;
}

async function fetchLatestRandomness() {
	const primary = await requestJson(`${RANDOMNESS_API_URL}${RANDOMNESS_LATEST_PATH}`, { timeoutMs: 4000 }).catch(err => ({
		status: 503,
		json: { error: shortError(err) },
	}));
	if (primary.status >= 200 && primary.status < 300) {
		return primary;
	}
	return requestJson(`${RANDOMNESS_API_FALLBACK_URL}${RANDOMNESS_FALLBACK_LATEST_PATH}`, { timeoutMs: 4000 }).catch(err => ({
		status: 503,
		json: { error: shortError(err) },
	}));
}

async function requestPrimaryRandomnessApi(targetPath, options = {}) {
	return requestJson(`${RANDOMNESS_API_URL}${targetPath}`, {
		method: options.method || 'GET',
		body: options.body ? JSON.stringify(options.body) : null,
		headers: options.body ? { 'Content-Type': 'application/json' } : {},
		timeoutMs: options.timeoutMs || 10000,
	}).catch(err => ({
		status: 503,
		json: { error: shortError(err) },
	}));
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

// ── L1 block helper ─────────────────────────────────────
// On Arbitrum, Solidity `block.number` returns the L1 (Ethereum mainnet) block number,
// but ethers' provider.getBlockNumber() returns the L2 block number.
// We must compare against L1 blocks when checking the challenge window.
async function getL1BlockNumber() {
	try {
		const raw = await provider.send('eth_getBlockByNumber', ['latest', false]);
		if (raw && raw.l1BlockNumber) {
			return parseInt(raw.l1BlockNumber, 16);
		}
	} catch { /* fall through */ }
	// Fallback: use provider block number (will be wrong on Arbitrum, but safe)
	console.warn('[Dashboard] Warning: could not fetch L1 block number, using L2 as fallback');
	return Number(await provider.getBlockNumber());
}

async function getSystemStatus() {
	const core = new ethers.Contract(CORE_ADDRESS, CORE_ABI, provider);
	const tgbt = new ethers.Contract(TGBT_ADDRESS, TGBT_ABI, provider);

	// Use allSettled so individual RPC failures don't break the entire status response
	const results = await Promise.allSettled([
		fetchRandomnessHealth(),
		requestJson(`${HEARTBEAT_API_URL}/api/health`, { timeoutMs: 4000 }).catch(err => ({
			status: 503,
			json: { error: shortError(err) },
		})),
		provider.getBalance(WALLET_ADDRESS),
		tgbt.balanceOf(WALLET_ADDRESS),
		tgbt.decimals(),
		tgbt.symbol(),
		core.moduleAddress(MODULE_IDS.BATCH_MINING_MODULE).catch(() => ethers.ZeroAddress),
		core.moduleAddress(MODULE_IDS.TOKENOMICS_MODULE).catch(() => ethers.ZeroAddress),
		getL1BlockNumber(),
		provider.getNetwork(),
	]);

	const val = (i, fallback) => results[i].status === 'fulfilled' ? results[i].value : fallback;
	const rpcErrors = results.filter(r => r.status === 'rejected').map(r => shortError(r.reason));

	const randomnessApi = val(0, { status: 503, json: { error: 'Unreachable' } });
	const heartbeatApi = val(1, { status: 503, json: { error: 'Unreachable' } });
	const ethBalance = val(2, 0n);
	const tokenBalance = val(3, 0n);
	const tokenDecimals = val(4, 18);
	const tokenSymbol = val(5, 'TGBT');
	const coreBatchModule = val(6, ethers.ZeroAddress);
	const coreTokenomicsModule = val(7, ethers.ZeroAddress);
	const l1Block = val(8, 0);
	const network = val(9, { chainId: 42161n });

	const liveBatchAddress = toChecksumOrNull(coreBatchModule);
	const batchWiredCorrectly = liveBatchAddress?.toLowerCase() === BATCH_ADDRESS.toLowerCase();
	const batchEnabled = !!liveBatchAddress && liveBatchAddress !== ethers.ZeroAddress && batchWiredCorrectly;
	let nextEpochId = 0;

	let latestOnChainEpoch = null;
	if (batchEnabled) {
		try {
			const batch = new ethers.Contract(BATCH_ADDRESS, BATCH_ABI, provider);
			nextEpochId = Number(await batch.currentEpochId().catch(() => 0n));
			if (Number(nextEpochId) <= 0) {
				latestOnChainEpoch = null;
			} else {
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
				l1Block,
				blocksUntilFinalizable: Math.max(0, Number(info.startBlock) + CHALLENGE_WINDOW - l1Block),
				blocksPastChallenge: Math.max(0, l1Block - (Number(info.startBlock) + CHALLENGE_WINDOW)),
				etaHours: Math.max(0, (Number(info.startBlock) + CHALLENGE_WINDOW - l1Block) * 12 / 3600),
			};
			}
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
			l1Block,
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
				coreBatchModule: liveBatchAddress,
				coreTokenomicsModule: toChecksumOrNull(coreTokenomicsModule),
				batchWiredCorrectly,
				batchEnabled,
				tokenomicsWiredCorrectly: toChecksumOrNull(coreTokenomicsModule)?.toLowerCase() === TOKENOMICS_ADDRESS.toLowerCase(),
			},
			nextEpochId: Number(nextEpochId),
			latestOnChainEpoch,
		},
		hardware: getHardwareInfo(),
		_rpcErrors: rpcErrors.length ? rpcErrors : undefined,
	};
}

/**
 * Collect host hardware info (CPU, memory, platform).
 * Uses Node.js built-in `os` module — no external deps needed.
 */
function getHardwareInfo() {
	const cpus = os.cpus();
	const cpuModel = cpus.length > 0 ? cpus[0].model.trim() : 'Unknown';
	const coreCount = cpus.length;
	const totalMemBytes = os.totalmem();
	const freeMemBytes = os.freemem();
	const usedMemBytes = totalMemBytes - freeMemBytes;
	const memUsagePercent = totalMemBytes > 0 ? Math.round((usedMemBytes / totalMemBytes) * 100) : 0;

	// Try to extract manufacturer from CPU model string
	let manufacturer = 'Unknown';
	const modelLower = cpuModel.toLowerCase();
	if (modelLower.includes('intel')) manufacturer = 'Intel';
	else if (modelLower.includes('amd')) manufacturer = 'AMD';
	else if (modelLower.includes('apple') || modelLower.includes('m1') || modelLower.includes('m2') || modelLower.includes('m3') || modelLower.includes('m4')) manufacturer = 'Apple';
	else if (modelLower.includes('arm') || modelLower.includes('qualcomm') || modelLower.includes('snapdragon')) manufacturer = 'ARM';

	// CPU speed (MHz → GHz)
	const cpuSpeedMhz = cpus.length > 0 ? cpus[0].speed : 0;
	const cpuSpeedGhz = cpuSpeedMhz > 0 ? (cpuSpeedMhz / 1000).toFixed(2) : null;

	return {
		platform: os.platform(),
		arch: os.arch(),
		hostname: os.hostname(),
		cpu: {
			model: cpuModel,
			manufacturer,
			cores: coreCount,
			speedGhz: cpuSpeedGhz,
		},
		memory: {
			totalBytes: totalMemBytes,
			freeBytes: freeMemBytes,
			usedBytes: usedMemBytes,
			usagePercent: memUsagePercent,
			totalGb: (totalMemBytes / (1024 ** 3)).toFixed(1),
			freeGb: (freeMemBytes / (1024 ** 3)).toFixed(1),
			usedGb: (usedMemBytes / (1024 ** 3)).toFixed(1),
		},
		uptime: os.uptime(),
		nodeVersion: process.version,
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

function deriveTamperStatus(snapshot = latestSnapshot()) {
	const locked = snapshot?.tamper_locked === true;
	const status = snapshot?.tamper_status || (locked ? 'tamper_locked' : 'uninitialized');
	const reason = snapshot?.tamper_reason || null;
	return {
		locked,
		status,
		reason,
		triggeredAtUnixMs: snapshot?.tamper_triggered_at_unix_ms ?? null,
		triggeredAt: snapshot?.tamper_triggered_at_unix_ms ? new Date(Number(snapshot.tamper_triggered_at_unix_ms)).toISOString() : null,
		sealHash: snapshot?.tamper_seal_hash || null,
		latestTimestampUnixMs: snapshot?.timestamp_unix_ms ?? null,
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
	const tamperStatus = deriveTamperStatus();
	const ransomwareStatus = heartbeatStatus?.security?.ransomware || null;
	const tamperAlert = tamperStatus.locked ? [{
		type: 'tamper_lock',
		severity: 'critical',
		message: tamperStatus.reason || 'Miner entered tamper-lock mode.',
		details: {
			status: tamperStatus.status,
			sealHash: tamperStatus.sealHash,
			triggeredAt: tamperStatus.triggeredAt,
		},
	}] : [];
	return {
		heartbeatStatus,
		heartbeatAlerts: {
			...(heartbeatAlerts || {}),
			active: [...tamperAlert, ...(Array.isArray(heartbeatAlerts?.active) ? heartbeatAlerts.active : [])],
		},
		telemetry,
		tamperStatus,
		ransomwareStatus,
	};
}

async function buildSecurityEvidenceExport() {
	const [threatProfile, relayProfile, systemStatus] = await Promise.all([
		getThreatProfile(),
		getRelayProfile().catch(() => null),
		getSystemStatus().catch(() => null),
	]);
	const latest = latestSnapshot();
	const ransomwareStatus = threatProfile?.ransomwareStatus || null;
	const evidencePath = ransomwareStatus?.evidencePath || null;
	const ransomwareEvidence = tryReadJson(evidencePath);

	return {
		version: 1,
		exportedAt: new Date().toISOString(),
		source: 'miner-dashboard',
		telemetryFile: TELEMETRY_FILE,
		latestSnapshot: latest ? {
			timestampUnixMs: latest.timestamp_unix_ms ?? null,
			state: latest.state || null,
			miningPhase: latest.mining_phase || null,
			tamperLocked: latest.tamper_locked === true,
			tamperStatus: latest.tamper_status || null,
			tamperReason: latest.tamper_reason || null,
		} : null,
		tamperStatus: threatProfile?.tamperStatus || null,
		ransomwareStatus,
		ransomwareEvidencePath: evidencePath,
		ransomwareEvidence,
		threatProfile,
		relayProfile,
		systemStatus,
	};
}

async function getRelayProfile() {
	const [system, heartbeatStatus, latestRandomness] = await Promise.all([
		getSystemStatus(),
		getHeartbeatStatus(),
		fetchLatestRandomness(),
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
	const l1Block = await getL1BlockNumber();
	const info = await batch.getEpochInfo(epochId);
	if (!info || !info.merkleRoot || /^0x0+$/.test(info.merkleRoot)) {
		throw new Error(`Epoch ${epochId} not found on-chain`);
	}
	const blocksUntil = Math.max(0, Number(info.startBlock) + CHALLENGE_WINDOW - l1Block);
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
		l1Block,
		challengeWindow: CHALLENGE_WINDOW,
		blocksUntilFinalizable: blocksUntil,
		blocksPastChallenge: Math.max(0, l1Block - (Number(info.startBlock) + CHALLENGE_WINDOW)),
		etaHours: Math.max(0, blocksUntil * 12 / 3600),
	};
}

async function listOnChainEpochs(limit = 50) {
	const batch = new ethers.Contract(BATCH_ADDRESS, BATCH_ABI, provider);
	const currentEpochId = Number(await batch.currentEpochId().catch(() => 0n));
	if (currentEpochId <= 0) {
		return [];
	}

	const endEpochId = currentEpochId - 1;
	const startEpochId = Math.max(0, endEpochId - Math.max(1, limit) + 1);
	const epochIds = [];
	for (let epochId = endEpochId; epochId >= startEpochId; epochId -= 1) {
		epochIds.push(epochId);
	}

	const epochs = await Promise.all(epochIds.map(async (epochId) => {
		try {
			const epoch = await getOnChainEpoch(epochId);
			return {
				...epoch,
				createdAt: null,
				leaves: [],
				storageVerification: null,
				onChainAttestation: epoch.attestationHash && !/^0x0+$/.test(epoch.attestationHash)
					? { attestationHash: epoch.attestationHash, txHash: null }
					: null,
			};
		} catch {
			return null;
		}
	}));

	return epochs.filter(Boolean);
}

async function getCompatibleLatestRandomness() {
	const response = await fetchLatestRandomness();
	if (!(response.status >= 200 && response.status < 300)) {
		return { status: response.status, json: response.json };
	}

	const payload = response.json || {};
	return {
		status: 200,
		json: {
			outputHash: payload.output || payload.outputHash || null,
			epochId: payload.epochId ?? null,
			leafIndex: payload.leafIndex ?? null,
			timestamp: payload.timestamp ?? null,
			signature: payload.signature ?? null,
			source: payload.source ?? null,
			blockNumber: payload.blockNumber ?? null,
		},
	};
}

async function getRewardLookup() {
	if (!store) {
		return { byOutputHash: new Map(), defaultReward: EST_TGBT_PER_SOLUTION };
	}

	const solutions = await store.getSolutions({ limit: 100000, skip: 0 });
	const byOutputHash = new Map();
	const recentRewards = [];
	for (const solution of solutions) {
		if (!solution?.accepted) continue;
		const reward = Number(solution.reward || 0);
		if (!(reward > 0)) continue;
		if (solution.outputHash && !byOutputHash.has(solution.outputHash)) {
			byOutputHash.set(solution.outputHash, reward);
		}
		if (recentRewards.length < 200) {
			recentRewards.push(reward);
		}
	}

	return {
		byOutputHash,
		defaultReward: median(recentRewards) || EST_TGBT_PER_SOLUTION,
	};
}

function enrichEpochReward(epoch, rewardLookup) {
	const originalReward = Number(epoch?.totalReward || 0);
	if (originalReward > 0) {
		return {
			...epoch,
			totalReward: originalReward,
			rewardEstimated: false,
		};
	}

	const leaves = Array.isArray(epoch?.leaves) ? epoch.leaves : [];
	let derivedReward = 0;
	for (const leaf of leaves) {
		const reward = rewardLookup.byOutputHash.get(leaf?.outputHash);
		if (reward > 0) {
			derivedReward += reward;
		}
	}

	if (!(derivedReward > 0)) {
		const leafCount = Number(epoch?.leafCount || leaves.length || 0);
		if (leafCount > 0) {
			derivedReward = leafCount * rewardLookup.defaultReward;
		}
	}

	return {
		...epoch,
		totalReward: derivedReward > 0 ? derivedReward : 0,
		rewardEstimated: derivedReward > 0,
	};
}

function enrichEpochSummaryReward(epoch, rewardLookup) {
	const originalReward = Number(epoch?.totalReward || 0);
	if (originalReward > 0) {
		return {
			...epoch,
			totalReward: originalReward,
			rewardEstimated: false,
		};
	}

	const leafCount = Number(epoch?.leafCount || 0);
	const derivedReward = leafCount > 0 ? leafCount * rewardLookup.defaultReward : 0;
	return {
		...epoch,
		totalReward: derivedReward,
		rewardEstimated: derivedReward > 0,
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

	// Detect new stale blocks and insert them as stale-type solution entries
	const prevStale = prevSnap.stale_block_count || 0;
	const currStale = snap.stale_block_count || 0;
	if (currStale > prevStale) {
		const sq = snap.stale_quality || 0;
		const sd = snap.stale_fork_depth || 0;
		const staleReward = sq > 0 ? (50 * sq * Math.min(sd + 1, 7)) / 100 : 0;
		for (let i = 0; i < (currStale - prevStale); i++) {
			store.insertSolution({
				timestamp: new Date(Number(snap.timestamp_unix_ms)).toISOString(),
				timestampMs: Number(snap.timestamp_unix_ms),
				type: 'stale',
				nonce: null,
				hash: snap.stale_xor_hex || null,
				commitHash: null,
				outputHash: snap.stale_xor_hex || null,
				reward: staleReward,
				estimated: true,
				accepted: true,
				phase: 'stale_harvest',
				uptime: snap.uptime_seconds,
				hashrate: snap.hashrate_hs,
				totalHashes: snap.hashes,
				solutionNumber: null,
				staleQuality: sq,
				staleForkDepth: sd,
				staleZeroBits: snap.stale_zero_bits || 0,
				bitcoinTipHeight: snap.bitcoin_tip_height || 0,
				proofId: snap.stale_proof_id || null,
				rawHeaderHex: snap.stale_raw_header_hex || null,
				telemetryBlockHash: snap.stale_block_hash_hex || null,
				blockHashHex: null,
				canonicalHash: snap.stale_canonical_hash || null,
				entropyDigest: snap.stale_entropy_digest || null,
				submitter: snap.stale_submitter || null,
				staleCreatedAt: snap.stale_created_at || null,
			}).catch(err => console.error('[SolutionStore] stale insert error:', err.message));
		}
	}

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

function sendJsonDownload(res, status, payload, filename) {
	res.writeHead(status, {
		'Content-Type': 'application/json; charset=utf-8',
		'Content-Disposition': `attachment; filename="${filename}"`,
		'Cache-Control': 'no-store',
		'Access-Control-Allow-Origin': '*',
	});
	res.end(JSON.stringify(payload, null, 2));
}

function exportTimestampStamp(value = new Date()) {
	const iso = value.toISOString().replace(/[-:]/g, '').replace(/\.\d+Z$/, 'Z');
	return iso.replace('T', '-');
}

function tryReadJson(filePath) {
	if (!filePath || !fs.existsSync(filePath)) return null;
	try {
		return JSON.parse(fs.readFileSync(filePath, 'utf8'));
	} catch (err) {
		return { error: shortError(err), path: filePath };
	}
}

function readTelemetryTail(limit = 120, options = {}) {
	if (!fs.existsSync(TELEMETRY_FILE)) {
		return [];
	}

	const normalizedLimit = Math.max(1, Number(limit) || 1);
	const chunkBytes = Math.max(4096, Number(options.chunkBytes) || TELEMETRY_TAIL_CHUNK_BYTES);
	const maxBytes = Math.max(
		chunkBytes,
		Number(options.maxBytes) || Math.max(TELEMETRY_TAIL_DEFAULT_MAX_BYTES, normalizedLimit * 4096),
	);

	let fd;
	try {
		const stat = fs.statSync(TELEMETRY_FILE);
		if (!stat.size) return [];

		fd = fs.openSync(TELEMETRY_FILE, 'r');
		let position = stat.size;
		let collectedBytes = 0;
		let text = '';
		let newlineCount = 0;

		while (position > 0 && collectedBytes < maxBytes && newlineCount <= normalizedLimit) {
			const bytesToRead = Math.min(chunkBytes, position, maxBytes - collectedBytes);
			position -= bytesToRead;
			const buffer = Buffer.allocUnsafe(bytesToRead);
			const bytesRead = fs.readSync(fd, buffer, 0, bytesToRead, position);
			if (bytesRead <= 0) break;

			collectedBytes += bytesRead;
			text = buffer.toString('utf8', 0, bytesRead) + text;
			newlineCount = (text.match(/\n/g) || []).length;
		}

		return text
			.split(/\r?\n/)
			.map((line) => line.trim())
			.filter(Boolean)
			.slice(-normalizedLimit);
	} catch (err) {
		console.warn(`[readTelemetryTail] Failed to read telemetry tail: ${shortError(err)}`);
		return [];
	} finally {
		if (fd != null) {
			try {
				fs.closeSync(fd);
			} catch {
			}
		}
	}
}

function readSnapshots(limit = 120, options = {}) {
	if (!fs.existsSync(TELEMETRY_FILE)) {
		return [];
	}

	const lines = readTelemetryTail(limit, options);

	let parseErrors = 0;
	const snapshots = lines
		.map((line) => {
			try {
				return JSON.parse(line);
			} catch {
				parseErrors++;
				return null;
			}
		})
		.filter(Boolean);

	if (parseErrors > 0 && parseErrors === lines.length) {
		console.warn(`[readSnapshots] All ${parseErrors} lines failed JSON parse — telemetry file may be corrupted or contain non-JSON data`);
	}

	return snapshots;
}

function latestSnapshot() {
	const snapshots = readSnapshots(1);
	return snapshots[0] || null;
}

function latestSnapshotWithStaleProof() {
	const snapshots = readSnapshots(STALE_PROOF_SCAN_LIMIT, { maxBytes: STALE_PROOF_SCAN_MAX_BYTES });
	for (let i = snapshots.length - 1; i >= 0; i--) {
		const snap = snapshots[i];
		if (snap?.stale_block_hash_hex || snap?.stale_proof_id || snap?.stale_raw_header_hex) {
			return snap;
		}
	}
	return null;
}

function normalizeHashForMatch(value) {
	const normalized = toBytes32HexOrNull(value);
	return normalized ? normalized.toLowerCase() : null;
}

async function listStoredStaleProofs(limit = 100) {
	let solutions = await store.getSolutions({ limit: 100000, skip: 0, sort: 'desc' });
	solutions = solutions.filter(s => s.type === 'stale').slice(0, Math.max(1, Math.min(200, Number(limit) || 100)));
	return solutions.map((s) => ({
		id: s.id,
		proofId: s.proofId || null,
		blockHash: s.blockHashHex || null,
		telemetryBlockHash: s.telemetryBlockHash || null,
		hash: s.hash || s.outputHash || null,
		entropyDigest: s.entropyDigest || null,
		submitter: s.submitter || null,
		detectedAt: s.timestamp || s.createdAt || null,
		timestampMs: Number(s.timestampMs || Date.parse(s.timestamp || s.createdAt || 0) || 0),
		qualityScore: Number(s.staleQuality || 0),
		reorgDepth: Number(s.staleForkDepth || 0),
		leadingZeros: Number(s.staleZeroBits || 0),
		reward: Number(s.reward || 0),
		bitcoinTipHeight: Number(s.bitcoinTipHeight || 0),
	}));
}

async function resolveStaleOracleAddress() {
	const core = new ethers.Contract(CORE_ADDRESS, CORE_ABI, provider);
	const addr = await core.moduleAddress(MODULE_IDS.STALE_BLOCK_MODULE);
	return addr && addr !== ethers.ZeroAddress ? addr : null;
}

async function fetchOnChainStaleProof(blockHashHex) {
	const normalizedBlockHash = toBytes32HexOrNull(blockHashHex);
	if (!normalizedBlockHash) return null;
	const staleOracleAddress = await resolveStaleOracleAddress();
	if (!staleOracleAddress) return null;
	const oracle = new ethers.Contract(staleOracleAddress, STALE_ORACLE_ABI, provider);
	const iface = new ethers.Interface(STALE_ORACLE_ABI);
	const candidateHashes = [normalizedBlockHash];
	const reversedBlockHash = reverseBytes32Hex(normalizedBlockHash);
	if (reversedBlockHash && reversedBlockHash !== normalizedBlockHash) {
		candidateHashes.push(reversedBlockHash);
	}

	let proof = null;
	let matchedHash = null;
	for (const candidateHash of candidateHashes) {
		const candidateProof = await oracle.getStaleProof(candidateHash).catch(() => null);
		if (candidateProof && Number(candidateProof.submittedAt || 0n) > 0) {
			proof = candidateProof;
			matchedHash = candidateHash;
			break;
		}
	}

	if (!proof || !matchedHash) {
		return null;
	}
	const [pendingRewardWei, submitLogs, claimLogs] = await Promise.all([
		oracle.pendingReward(matchedHash).catch(() => 0n),
		provider.getLogs({
			address: staleOracleAddress,
			fromBlock: 0,
			toBlock: 'latest',
			topics: iface.encodeFilterTopics('StaleBlockSubmitted', [matchedHash]),
		}).catch(() => []),
		provider.getLogs({
			address: staleOracleAddress,
			fromBlock: 0,
			toBlock: 'latest',
			topics: iface.encodeFilterTopics('StaleRewardClaimed', [matchedHash]),
		}).catch(() => []),
	]);
	const submitTxHash = submitLogs.length ? submitLogs[submitLogs.length - 1].transactionHash : null;
	const claimTxHash = claimLogs.length ? claimLogs[claimLogs.length - 1].transactionHash : null;
	return {
		oracleAddress: staleOracleAddress,
		lookupBlockHash: matchedHash,
		displayBlockHash: normalizedBlockHash,
		blockHash: proof.blockHash,
		canonicalHash: proof.canonicalHash,
		entropyDigest: proof.entropyDigest,
		height: Number(proof.height),
		reorgDepth: Number(proof.reorgDepth),
		leadingZeros: Number(proof.leadingZeros),
		qualityScore: Number(proof.qualityScore),
		submittedAt: Number(proof.submittedAt),
		submitter: toChecksumOrNull(proof.submitter),
		rewarded: !!proof.rewarded,
		submitTxHash,
		claimTxHash,
		pendingRewardWei: pendingRewardWei.toString(),
		pendingRewardTgbt: Number(ethers.formatUnits(pendingRewardWei, 18)),
	};
}

async function getLatestStaleDeveloperProof(selection = {}) {
	const latest = latestSnapshot();
	const latestProofSnap = latestSnapshotWithStaleProof();
	const recent = await store.getSolutions({ limit: 500, skip: 0, sort: 'desc' });
	const staleSolutions = recent.filter(s => s.type === 'stale');
	const normalizedSelectedBlockHash = normalizeHashForMatch(selection.blockHash);
	const selectedStale = staleSolutions.find((s) => {
		if (selection.id != null && String(s.id) === String(selection.id)) return true;
		if (selection.proofId && s.proofId === selection.proofId) return true;
		if (normalizedSelectedBlockHash) {
			return [s.blockHashHex, s.telemetryBlockHash, s.hash, s.outputHash]
				.map(normalizeHashForMatch)
				.filter(Boolean)
				.includes(normalizedSelectedBlockHash);
		}
		return false;
	}) || null;
	const latestStale = selectedStale || staleSolutions[0] || null;
	const staleCount = Number(latest?.stale_block_count || 0);
	const pendingProofs = Number(latest?.stale_pending_proofs || 0);
	const canBorrowLatestTelemetry = !selectedStale || staleSolutions.length <= 1 || selectedStale?.id === staleSolutions[0]?.id;
	const proofSnap = canBorrowLatestTelemetry ? (latestProofSnap || latest) : null;
	const proofId = latestStale?.proofId || proofSnap?.stale_proof_id || null;
	const rawHeaderHex = latestStale?.rawHeaderHex || proofSnap?.stale_raw_header_hex || null;
	const blockHashHex = latestStale?.telemetryBlockHash || latestStale?.blockHashHex || proofSnap?.stale_block_hash_hex || null;
	const canonicalHash = latestStale?.canonicalHash || proofSnap?.stale_canonical_hash || null;
	const entropyDigest = latestStale?.entropyDigest || proofSnap?.stale_entropy_digest || null;
	const submitter = latestStale?.submitter || proofSnap?.stale_submitter || null;
	const createdAt = latestStale?.staleCreatedAt || proofSnap?.stale_created_at || null;
	const hasTelemetryProof = Boolean(
		proofId ||
		rawHeaderHex ||
		blockHashHex ||
		canonicalHash ||
		entropyDigest ||
		submitter
	);
	const onChainProof = await fetchOnChainStaleProof(blockHashHex).catch(() => null);
	if (onChainProof && latestStale && !latestStale.blockHashHex) {
		latestStale.blockHashHex = onChainProof.blockHash;
	}
	const reorgDepth = onChainProof?.reorgDepth ?? latestStale?.staleForkDepth ?? latest?.stale_fork_depth ?? null;
	const qualityScore = onChainProof?.qualityScore ?? latestStale?.staleQuality ?? latest?.stale_quality ?? null;
	const leadingZeros = onChainProof?.leadingZeros ?? latestStale?.staleZeroBits ?? latest?.stale_zero_bits ?? null;
	const bitcoinTipHeight = latestStale?.bitcoinTipHeight ?? latest?.bitcoin_tip_height ?? null;
	const timestampMs = latestStale?.timestampMs || (createdAt ? createdAt * 1000 : null);
	const detectedAt = createdAt
		? new Date(createdAt * 1000).toISOString()
		: (latestStale?.timestamp || latestStale?.createdAt || null);
	const estimatedRewardTgbt = onChainProof
		? (onChainProof.rewarded
			? (50 * onChainProof.qualityScore * Math.min(onChainProof.reorgDepth + 1, 7)) / 100
			: (onChainProof.pendingRewardTgbt || 0))
		: Number.isFinite(qualityScore)
		? (50 * qualityScore * Math.min((reorgDepth ?? 0) + 1, 7)) / 100
		: (latestStale?.reward ?? null);
	const submitTxHash = onChainProof?.submitTxHash || null;
	const claimTxHash = onChainProof?.claimTxHash || null;
	const status = onChainProof
		? (onChainProof.rewarded ? 'reward_claimed' : 'submitted_onchain')
		: (latestStale || staleCount > 0 || pendingProofs > 0 || hasTelemetryProof)
			? (pendingProofs > 0 ? 'pending_submit' : 'summary_only')
			: 'no-stale-detected';

	return {
		source: onChainProof ? 'dashboard+onchain' : 'dashboard-summary',
		status,
		selected: latestStale ? {
			id: latestStale.id,
			proofId,
			blockHash: onChainProof?.blockHash || latestStale?.blockHashHex || blockHashHex || null,
			telemetryBlockHash: latestStale?.telemetryBlockHash || blockHashHex || null,
		} : null,
		proof: status === 'no-stale-detected' ? null : {
			id: latestStale?.id || null,
			proofId,
			rawHeaderHex,
			telemetryBlockHash: blockHashHex,
			blockHash: onChainProof?.blockHash || blockHashHex || null,
			canonicalHash: onChainProof?.canonicalHash || canonicalHash,
			entropyDigest: onChainProof?.entropyDigest || entropyDigest,
			submitter: onChainProof?.submitter || submitter,
			createdAt,
			detectedAt,
			timestampMs,
			bitcoinTipHeight,
			height: onChainProof?.height ?? null,
			reorgDepth,
			leadingZeros,
			qualityScore,
			rewarded: onChainProof?.rewarded ?? null,
			estimatedRewardTgbt,
			pendingProofs,
			telemetryStaleCount: staleCount,
		},
		tx: {
			submitTxHash,
			claimTxHash,
		},
		onChain: onChainProof ? {
			oracleAddress: onChainProof.oracleAddress,
			lookupBlockHash: onChainProof.lookupBlockHash,
			displayBlockHash: onChainProof.displayBlockHash,
			submittedAt: onChainProof.submittedAt,
			height: onChainProof.height,
			reorgDepth: onChainProof.reorgDepth,
			leadingZeros: onChainProof.leadingZeros,
			qualityScore: onChainProof.qualityScore,
			rewarded: onChainProof.rewarded,
			pendingRewardWei: onChainProof.pendingRewardWei,
			pendingRewardTgbt: onChainProof.pendingRewardTgbt,
		} : null,
	};
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

	// ── Mining control (pause / power rate) ──────────────────────────
	if (url.pathname === '/api/miner/control' && req.method === 'GET') {
		try {
			const data = fs.existsSync(CONTROL_FILE)
				? JSON.parse(fs.readFileSync(CONTROL_FILE, 'utf8'))
				: { paused: false, power_pct: 100, submit_stale_now: false, tamper_reseal_now: false };
			sendJson(res, 200, {
				paused: !!data.paused,
				power_pct: Number(data.power_pct) || 100,
				tamper_reseal_now: !!data.tamper_reseal_now,
			});
		} catch (err) {
			sendJson(res, 200, { paused: false, power_pct: 100, tamper_reseal_now: false });
		}
		return;
	}
	if (url.pathname === '/api/miner/control' && req.method === 'POST') {
		let body = '';
		req.on('data', (c) => (body += c));
		req.on('end', () => {
			try {
				const payload = JSON.parse(body);
				const existing = fs.existsSync(CONTROL_FILE)
					? JSON.parse(fs.readFileSync(CONTROL_FILE, 'utf8'))
					: { submit_stale_now: false, tamper_reseal_now: false };
				const ctrl = {
					paused: !!payload.paused,
					power_pct: [25, 50, 75, 100].includes(Number(payload.power_pct))
						? Number(payload.power_pct) : 100,
					submit_stale_now: !!existing.submit_stale_now,
					tamper_reseal_now: !!existing.tamper_reseal_now,
				};
				fs.mkdirSync(path.dirname(CONTROL_FILE), { recursive: true });
				fs.writeFileSync(CONTROL_FILE, JSON.stringify(ctrl, null, 2));
				sendJson(res, 200, ctrl);
			} catch (err) {
				sendJson(res, 400, { error: 'Invalid JSON body' });
			}
		});
		return;
	}

	if (url.pathname === '/api/stale/submit' && req.method === 'POST') {
		try {
			const existing = fs.existsSync(CONTROL_FILE)
				? JSON.parse(fs.readFileSync(CONTROL_FILE, 'utf8'))
				: { paused: false, power_pct: 100, tamper_reseal_now: false };
			const ctrl = {
				paused: !!existing.paused,
				power_pct: [25, 50, 75, 100].includes(Number(existing.power_pct))
					? Number(existing.power_pct) : 100,
				submit_stale_now: true,
				tamper_reseal_now: !!existing.tamper_reseal_now,
			};
			fs.mkdirSync(path.dirname(CONTROL_FILE), { recursive: true });
			fs.writeFileSync(CONTROL_FILE, JSON.stringify(ctrl, null, 2));
			const latest = latestSnapshot();
			sendJson(res, 200, {
				ok: true,
				queued: true,
				pendingProofs: Number(latest?.stale_pending_proofs || 0),
				message: 'Manual stale proof submit requested. The miner will retry shortly.',
			});
		} catch (err) {
			sendJson(res, 500, { ok: false, error: shortError(err) });
		}
		return;
	}

	if (url.pathname === '/api/security/tamper-status' && req.method === 'GET') {
		sendJson(res, 200, deriveTamperStatus());
		return;
	}

	if (url.pathname === '/api/security/tamper-reseal' && req.method === 'POST') {
		try {
			const existing = fs.existsSync(CONTROL_FILE)
				? JSON.parse(fs.readFileSync(CONTROL_FILE, 'utf8'))
				: { paused: false, power_pct: 100, submit_stale_now: false };
			const ctrl = {
				paused: !!existing.paused,
				power_pct: [25, 50, 75, 100].includes(Number(existing.power_pct))
					? Number(existing.power_pct) : 100,
				submit_stale_now: !!existing.submit_stale_now,
				tamper_reseal_now: true,
			};
			fs.mkdirSync(path.dirname(CONTROL_FILE), { recursive: true });
			fs.writeFileSync(CONTROL_FILE, JSON.stringify(ctrl, null, 2));
			sendJson(res, 200, {
				ok: true,
				queued: true,
				message: 'Tamper reseal requested. The miner will rebuild its local trust seal on the next control cycle.',
			});
		} catch (err) {
			sendJson(res, 500, { ok: false, error: shortError(err) });
		}
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

	if (url.pathname === '/api/security/evidence/export' && req.method === 'GET') {
		try {
			const payload = await buildSecurityEvidenceExport();
			const filename = `temporal-gradient-security-evidence-${exportTimestampStamp()}.json`;
			sendJsonDownload(res, 200, payload, filename);
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
		const health = await fetchRandomnessHealth();
		sendJson(res, health.status, health.json ?? {});
		return;
	}

	if (url.pathname === '/api/network/randomness/latest' && req.method === 'GET') {
		try {
			const latest = await getCompatibleLatestRandomness();
			sendJson(res, latest.status, latest.json ?? {});
		} catch (err) {
			sendJson(res, 500, { error: shortError(err) });
		}
		return;
	}

	const proofProxyMatch = url.pathname.match(/^\/api\/network\/randomness\/([0-9a-fA-Fx]+)\/proof$/);
	if (proofProxyMatch && req.method === 'GET') {
		const response = await requestPrimaryRandomnessApi(`/api/randomness/${encodeURIComponent(proofProxyMatch[1])}/proof`);
		sendJson(res, response.status, response.json ?? {
			error: 'Proof lookup is unavailable on the current API deployment.',
			outputHash: proofProxyMatch[1],
		});
		return;
	}

	if (url.pathname === '/api/network/epochs' && req.method === 'GET') {
		try {
			const limit = Math.max(1, Math.min(100, Number(url.searchParams.get('limit') || 50)));
			const legacy = await requestPrimaryRandomnessApi(`/api/epochs?limit=${limit}`);
			if (legacy.status >= 200 && legacy.status < 300) {
				const rewardLookup = await getRewardLookup();
				sendJson(res, 200, {
					...(legacy.json || {}),
					epochs: (legacy.json?.epochs || []).map(epoch => enrichEpochSummaryReward(epoch, rewardLookup)),
					source: 'randomness-api',
					proofsAvailable: true,
				});
				return;
			}
			const epochs = await listOnChainEpochs(limit);
			sendJson(res, 200, {
				epochs,
				source: 'on-chain-batch',
				proofsAvailable: false,
			});
		} catch (err) {
			sendJson(res, 500, { error: shortError(err) });
		}
		return;
	}

	const epochProxyMatch = url.pathname.match(/^\/api\/network\/epochs\/(\d+)$/);
	if (epochProxyMatch && req.method === 'GET') {
		try {
			const legacy = await requestPrimaryRandomnessApi(`/api/epochs/${epochProxyMatch[1]}`);
			if (legacy.status >= 200 && legacy.status < 300) {
				const rewardLookup = await getRewardLookup();
				sendJson(res, 200, enrichEpochReward(legacy.json ?? {}, rewardLookup));
				return;
			}
			const epoch = await getOnChainEpoch(Number(epochProxyMatch[1]));
			sendJson(res, 200, {
				...epoch,
				createdAt: null,
				leaves: [],
				storageVerification: null,
				onChainAttestation: epoch.attestationHash && !/^0x0+$/.test(epoch.attestationHash)
					? { attestationHash: epoch.attestationHash, txHash: null }
					: null,
			});
		} catch (err) {
			sendJson(res, 404, { error: shortError(err) });
		}
		return;
	}

	const verifyStorageMatch = url.pathname.match(/^\/api\/network\/epochs\/(\d+)\/verify-storage$/);
	if (verifyStorageMatch && req.method === 'POST') {
		const response = await requestPrimaryRandomnessApi(`/api/epochs/${verifyStorageMatch[1]}/verify-storage`, { method: 'POST' });
		sendJson(res, response.status, response.json ?? {
			error: 'Storage verification is unavailable on the current API deployment.',
			epochId: Number(verifyStorageMatch[1]),
		});
		return;
	}

	// ── Solution storage API ─────────────────────────────
	if (url.pathname === '/api/stale/proofs' && req.method === 'GET') {
		try {
			const limit = Math.max(1, Math.min(200, Number(url.searchParams.get('limit') || 100)));
			const proofs = await listStoredStaleProofs(limit);
			sendJson(res, 200, { proofs, total: proofs.length });
		} catch (err) {
			sendJson(res, 500, { error: err.message });
		}
		return;
	}

	if (url.pathname === '/api/stale/developer-proof' && req.method === 'GET') {
		try {
			const payload = await getLatestStaleDeveloperProof({
				id: url.searchParams.get('id') || null,
				proofId: url.searchParams.get('proofId') || null,
				blockHash: url.searchParams.get('blockHash') || null,
			});
			sendJson(res, 200, payload);
		} catch (err) {
			sendJson(res, 500, { error: err.message });
		}
		return;
	}

	if (url.pathname === '/api/solutions' && req.method === 'GET') {
		try {
			const limit = Math.max(1, Math.min(200, Number(url.searchParams.get('limit') || 50)));
			const skip = Math.max(0, Number(url.searchParams.get('skip') || 0));
			const filter = url.searchParams.get('filter'); // 'accepted' | 'rejected' | null
			const sinceMs = Number(url.searchParams.get('sinceMs') || 0);
			let solutions = await store.getSolutions({ limit: 100000, skip: 0 });
			if (filter === 'accepted') solutions = solutions.filter(s => s.accepted && s.type !== 'stale');
			if (filter === 'rejected') solutions = solutions.filter(s => !s.accepted);
			if (filter === 'stale') solutions = solutions.filter(s => s.type === 'stale');
			if (sinceMs > 0) {
				solutions = solutions.filter(s => Number(s.timestampMs || Date.parse(s.timestamp || s.createdAt || 0)) >= sinceMs);
			}
			const stats = {
				total: solutions.length,
				accepted: solutions.filter(s => s.accepted && s.type !== 'stale').length,
				rejected: solutions.filter(s => !s.accepted).length,
				stale: solutions.filter(s => s.type === 'stale').length,
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
