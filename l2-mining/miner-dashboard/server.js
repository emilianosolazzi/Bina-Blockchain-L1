const http = require('http');
const fs = require('fs');
const path = require('path');
const { URL } = require('url');
const { createStore } = require('./solution-store');

const HOST = process.env.HOST || '127.0.0.1';
const PORT = Number(process.env.PORT || 4173);
const ROOT = __dirname;
const INDEX = path.join(ROOT, 'index.html');
const TELEMETRY_FILE = process.env.TELEMETRY_FILE || path.resolve(ROOT, '..', 'rust', 'miner-telemetry.jsonl');

// Difficulty-based reward estimate (mirrors miner Rust logic)
const DIFFICULTY_BITS = 11;
const EST_TGBT_PER_SOLUTION = Math.max(DIFFICULTY_BITS / 8, 1); // 1.375

let store = null;

// ── Telemetry-to-solution detector ──────────────────────
let prevSnap = null;

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
			outputHash: snap.last_output_hash_hex || null,
			reward,
			estimated: onChainDelta <= 0,
			accepted: true,
			phase: snap.mining_phase,
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
			phase: snap.mining_phase,
			uptime: snap.uptime_seconds,
			hashrate: snap.hashrate_hs,
			totalHashes: snap.hashes,
			solutionNumber: null,
		}).catch(err => console.error('[SolutionStore] insert error:', err.message));
	}

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

	// ── Solution storage API ─────────────────────────────
	if (url.pathname === '/api/solutions' && req.method === 'GET') {
		try {
			const limit = Math.max(1, Math.min(200, Number(url.searchParams.get('limit') || 50)));
			const skip = Math.max(0, Number(url.searchParams.get('skip') || 0));
			const filter = url.searchParams.get('filter'); // 'accepted' | 'rejected' | null
			let solutions = await store.getSolutions({ limit: limit + skip, skip: 0 });
			if (filter === 'accepted') solutions = solutions.filter(s => s.accepted);
			if (filter === 'rejected') solutions = solutions.filter(s => !s.accepted);
			solutions = solutions.slice(skip, skip + limit);
			const stats = await store.getStats();
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
						phase: snap.mining_phase,
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
						phase: snap.mining_phase,
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
	});
})();
