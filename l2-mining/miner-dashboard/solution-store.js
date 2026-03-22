/**
 * SolutionStore – Persistent storage for miner solutions & rewards.
 *
 * Backends:
 *   1. File-based (default) – writes a solutions.json file next to the server
 *   2. MongoDB – activates when MONGODB_URI env var is set
 *
 * Both backends expose the same async API so the dashboard server
 * never has to care which one is active.
 */

const fs = require('fs');
const path = require('path');

// ───────────────────────────────────────────────────────────
//  File-based backend
// ───────────────────────────────────────────────────────────
class FileStore {
	constructor(filePath) {
		this.filePath = filePath;
		this._data = { solutions: [], stats: { totalRewards: 0, accepted: 0, rejected: 0 } };
		this._load();
	}

	_load() {
		try {
			if (fs.existsSync(this.filePath)) {
				const raw = fs.readFileSync(this.filePath, 'utf8').trim();
				if (!raw) {
					console.warn('[SolutionStore/File] File is empty, starting fresh.');
					return;
				}
				let parsed;
				try {
					parsed = JSON.parse(raw);
				} catch (parseErr) {
					// If file starts with '<' it's HTML (proxy error page saved somehow)
					const isHtml = raw.startsWith('<') || raw.startsWith('<!');
					console.warn(`[SolutionStore/File] ${isHtml ? 'HTML content' : 'Invalid JSON'} in ${this.filePath}, starting fresh:`, parseErr.message);
					return;
				}
				if (parsed && Array.isArray(parsed.solutions)) {
					this._data = parsed;
				} else if (Array.isArray(parsed)) {
					// Bare array — wrap in expected structure
					console.warn('[SolutionStore/File] Found bare array in file, wrapping in expected structure.');
					const solutions = parsed;
					this._data = {
						solutions,
						stats: {
							totalRewards: solutions.reduce((sum, s) => sum + Number(s.reward || 0), 0),
							accepted: solutions.filter(s => s.accepted).length,
							rejected: solutions.filter(s => !s.accepted).length,
						},
					};
				} else {
					console.warn('[SolutionStore/File] Unexpected data format, starting fresh.');
				}
			}
		} catch (err) {
			console.warn('[SolutionStore/File] Could not load existing data, starting fresh:', err.message);
		}
	}

	_save() {
		try {
			fs.writeFileSync(this.filePath, JSON.stringify(this._data, null, 2), 'utf8');
		} catch (err) {
			console.error('[SolutionStore/File] Failed to save:', err.message);
		}
	}

	async init() {
		console.log(`[SolutionStore] Using file backend → ${this.filePath}`);
	}

	async insertSolution(doc) {
		doc.id = this._data.solutions.length + 1;
		doc.createdAt = new Date().toISOString();
		this._data.solutions.push(doc);
		this._data.stats.totalRewards = (this._data.stats.totalRewards || 0) + (doc.reward || 0);
		if (doc.accepted) this._data.stats.accepted++;
		else this._data.stats.rejected++;
		this._save();
		return doc;
	}

	async getSolutions({ limit = 50, skip = 0, sort = 'desc' } = {}) {
		const sorted = sort === 'asc'
			? [...this._data.solutions]
			: [...this._data.solutions].reverse();
		return sorted.slice(skip, skip + limit);
	}

	async getStats() {
		return { ...this._data.stats, total: this._data.solutions.length };
	}

	async getLatest() {
		return this._data.solutions.length > 0
			? this._data.solutions[this._data.solutions.length - 1]
			: null;
	}

	async updateSolutionDetails({ nonce, accepted, ...patch }) {
		if (nonce == null) return null;
		for (let i = this._data.solutions.length - 1; i >= 0; i--) {
			const item = this._data.solutions[i];
			if (item.nonce !== nonce) continue;
			if (accepted != null && item.accepted !== accepted) continue;

			for (const [key, value] of Object.entries(patch)) {
				if (value !== undefined && value !== null && value !== '') {
					item[key] = value;
				}
			}

			this._save();
			return item;
		}
		return null;
	}

	async close() {}
}

// ───────────────────────────────────────────────────────────
//  MongoDB backend
// ───────────────────────────────────────────────────────────
class MongoStore {
	constructor(uri, dbName = 'tgbt_miner') {
		this.uri = uri;
		this.dbName = dbName;
		this.client = null;
		this.collection = null;
	}

	async init() {
		const { MongoClient } = require('mongodb');
		this.client = new MongoClient(this.uri);
		await this.client.connect();
		const db = this.client.db(this.dbName);
		this.collection = db.collection('solutions');

		// Create indexes for efficient querying
		await this.collection.createIndex({ timestamp: -1 });
		await this.collection.createIndex({ accepted: 1 });
		await this.collection.createIndex({ nonce: 1 });

		console.log(`[SolutionStore] Using MongoDB backend → ${this.dbName}.solutions`);
	}

	async insertSolution(doc) {
		doc.createdAt = new Date();
		const result = await this.collection.insertOne(doc);
		doc._id = result.insertedId;
		return doc;
	}

	async getSolutions({ limit = 50, skip = 0, sort = 'desc' } = {}) {
		const sortDir = sort === 'asc' ? 1 : -1;
		return this.collection
			.find({})
			.sort({ timestamp: sortDir })
			.skip(skip)
			.limit(limit)
			.toArray();
	}

	async getStats() {
		const pipeline = [
			{
				$group: {
					_id: null,
					total: { $sum: 1 },
					totalRewards: { $sum: '$reward' },
					accepted: { $sum: { $cond: ['$accepted', 1, 0] } },
					rejected: { $sum: { $cond: ['$accepted', 0, 1] } },
				},
			},
		];
		const [result] = await this.collection.aggregate(pipeline).toArray();
		return result || { total: 0, totalRewards: 0, accepted: 0, rejected: 0 };
	}

	async getLatest() {
		return this.collection.findOne({}, { sort: { timestamp: -1 } });
	}

	async updateSolutionDetails({ nonce, accepted, ...patch }) {
		if (nonce == null) return null;
		const set = {};
		for (const [key, value] of Object.entries(patch)) {
			if (value !== undefined && value !== null && value !== '') {
				set[key] = value;
			}
		}
		if (Object.keys(set).length === 0) return null;

		const query = { nonce };
		if (accepted != null) query.accepted = accepted;

		const result = await this.collection.findOneAndUpdate(
			query,
			{ $set: set },
			{ sort: { timestampMs: -1 }, returnDocument: 'after' }
		);

		return result.value || null;
	}

	async close() {
		if (this.client) {
			await this.client.close();
		}
	}
}

// ───────────────────────────────────────────────────────────
//  Factory – picks backend based on environment
// ───────────────────────────────────────────────────────────
async function createStore() {
	const mongoUri = process.env.MONGODB_URI;

	if (mongoUri) {
		try {
			const store = new MongoStore(mongoUri, process.env.MONGODB_DB || 'tgbt_miner');
			await store.init();
			return store;
		} catch (err) {
			console.warn('[SolutionStore] MongoDB connection failed, falling back to file:', err.message);
		}
	}

	const filePath = process.env.SOLUTIONS_FILE || path.join(__dirname, 'solutions.json');
	const store = new FileStore(filePath);
	await store.init();
	return store;
}

module.exports = { createStore, FileStore, MongoStore };
