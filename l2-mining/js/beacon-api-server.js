// beacon-api-server.js
// Temporal Gradient — Production Randomness API Server
//
// Supports TWO mining stacks:
//   • Classic commit-reveal mining (MiningModule + TemporalGradientCore)
//   • Batch epoch-based mining (BatchMiningModule) — optional
//
// Architecture:
//   Client dApp → beacon-api-server → Redis cache → Arbitrum contracts
//                                    → PostgreSQL (usage logs)
//
// Fixes applied:
//   1. Redis initialised once at module level (ioredis)
//   2. Bearer token auth wired to real JWT verification (jsonwebtoken)
//   3. Signature auth placeholder removed — verifyMessage enforced
//   4. Per-user rate limiter instantiated once, applied as middleware
//   5. beacon-sdk replaced with direct ethers contract wiring
//   6. Double global rate-limit removed (one at top, none inline)
//   7. All auth paths fail-closed (no accidental isAuthorized = true)
//   8. Classic mining stack support (Core + MiningModule + TokenomicsModule)

import 'dotenv/config';
import express         from 'express';
import cors            from 'cors';
import helmet          from 'helmet';
import rateLimit       from 'express-rate-limit';
import RedisStore      from 'rate-limit-redis';
import bodyParser      from 'body-parser';
import crypto          from 'crypto';
import winston         from 'winston';
import swaggerJsdoc    from 'swagger-jsdoc';
import swaggerUi       from 'swagger-ui-express';
import { ethers, verifyMessage } from 'ethers';
import axios           from 'axios';
import { Pool }        from 'pg';
import { v4 as uuidv4 } from 'uuid';
import jwt             from 'jsonwebtoken';
import Redis           from 'ioredis';

// ─────────────────────────────────────────────────────────────────
// Environment validation — fail fast on startup
// ─────────────────────────────────────────────────────────────────

const REQUIRED_ENV = [
  'DATABASE_URL',
  'REDIS_URL',
  'RPC_URL',
  'CORE_ADDRESS',
  'TOKENOMICS_ADDRESS',
  'JWT_SECRET',
];

for (const key of REQUIRED_ENV) {
  if (!process.env[key]) {
    console.error(`[FATAL] Missing required environment variable: ${key}`);
    process.exit(1);
  }
}

const PORT              = parseInt(process.env.PORT || '3000', 10);
const JWT_SECRET        = process.env.JWT_SECRET;
const API_KEY           = process.env.API_KEY; // optional static key
const LOG_LEVEL         = process.env.LOG_LEVEL || 'info';

// ─────────────────────────────────────────────────────────────────
// Logger
// ─────────────────────────────────────────────────────────────────

const logger = winston.createLogger({
  level: LOG_LEVEL,
  format: winston.format.combine(
    winston.format.timestamp(),
    winston.format.json(),
  ),
  transports: [
    new winston.transports.Console({
      format: winston.format.combine(
        winston.format.colorize(),
        winston.format.simple(),
      ),
    }),
  ],
});

// ─────────────────────────────────────────────────────────────────
// Redis — Fix #1: single instance, initialised at module level
// ─────────────────────────────────────────────────────────────────

const redis = new Redis(process.env.REDIS_URL, {
  maxRetriesPerRequest: 3,
  enableReadyCheck: true,
  lazyConnect: false,
});

redis.on('error', (err) => logger.error('Redis error:', { message: err.message }));
redis.on('connect', () => logger.info('Redis connected'));

// ─────────────────────────────────────────────────────────────────
// PostgreSQL
// ─────────────────────────────────────────────────────────────────

const db = new Pool({
  connectionString: process.env.DATABASE_URL,
  min: 5,
  max: 50,
  idleTimeoutMillis: 30_000,
  connectionTimeoutMillis: 5_000,
});

db.on('error', (err) => logger.error('DB pool error:', { message: err.message }));

// Create table on startup if it doesn't exist
async function ensureSchema() {
  await db.query(`
    CREATE TABLE IF NOT EXISTS randomness_requests (
      id                  SERIAL PRIMARY KEY,
      request_id          UUID UNIQUE NOT NULL,
      client_ip           TEXT,
      user_agent          TEXT,
      endpoint            TEXT NOT NULL,
      entropy_source      TEXT,
      num_words           INTEGER DEFAULT 0,
      seed                TEXT,
      output              TEXT,
      randomness          JSONB,
      success             BOOLEAN NOT NULL DEFAULT TRUE,
      error_message       TEXT,
      timestamp           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
      fulfilled_at        TIMESTAMPTZ,
      webhook_url         TEXT,
      webhook_status      TEXT,
      webhook_response_code INTEGER,
      webhook_response    TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_rr_request_id ON randomness_requests(request_id);
    CREATE INDEX IF NOT EXISTS idx_rr_timestamp  ON randomness_requests(timestamp DESC);
  `);
  logger.info('DB schema verified');
}

// ─────────────────────────────────────────────────────────────────
// Contract wiring — supports classic + batch mining stacks
// ─────────────────────────────────────────────────────────────────

// Classic mining stack ABIs (commit-reveal via MiningModule + Core)
const CORE_ABI = [
  'function getOutputHistory() view returns (bytes32[32])',
  'function outputHistoryAt(uint256 index) view returns (bytes32)',
  'function getCurrentOutputIndex() view returns (uint64)',
  'function isPaused() view returns (bool)',
  'function moduleAddress(bytes32 moduleId) view returns (address)',
];

const MINING_MODULE_ABI = [
  'function getPoolInfo(uint8 poolId) view returns (uint256 difficulty, uint256 emission, uint256 mined, bool active)',
  'function getMiningChallenge(uint8 poolId) view returns (bytes32[] outputs, uint256 difficulty)',
  'function getActivePools() view returns (uint8[] activePools, uint256[] difficulties, uint256[] emissions)',
];

const TOKENOMICS_ABI = [
  'function getMiningEconomics() view returns (uint256 currentReward, uint256 currentEpoch, uint256 blocksPerEpoch, uint256 halvingInterval, uint256 nextHalvingBlock, uint256 currentBonusThreshold, uint256 currentBonusMultiplier, uint256 minedSoFar, uint256 remainingAllocation)',
  'function getTokenomicsInfo() view returns (uint256 cap, uint256 miningAlloc, uint256 currentBlockReward, uint256 epoch, uint256 totalMinedToDate, uint256 remaining, uint256 nextHalvingBlock)',
];

const TGBT_ABI = [
  'function balanceOf(address) view returns (uint256)',
  'function totalSupply() view returns (uint256)',
];

// Batch mining stack ABIs (optional — epoch-based Merkle trees)
const BATCH_MINING_ABI = [
  'function getEpochRoot(uint256 epochId) view returns (bytes32)',
  'function nextEpochId() view returns (uint256)',
  'function verifyRandomnessLeaf(uint256 epochId, uint256 leafIndex, bytes32 outputHash, bytes32[] proof) view returns (bool)',
  'function latestOutput() view returns (bytes32)',
];

const provider = new ethers.JsonRpcProvider(process.env.RPC_URL);

// Classic mining contracts (always available)
const coreContract = new ethers.Contract(
  process.env.CORE_ADDRESS,
  CORE_ABI,
  provider,
);

const tokenomicsContract = new ethers.Contract(
  process.env.TOKENOMICS_ADDRESS,
  TOKENOMICS_ABI,
  provider,
);

// Mining module — resolve from Core or use env override
let miningModuleContract = null;
const miningModuleAddr = process.env.MINING_MODULE_ADDRESS;
if (miningModuleAddr) {
  miningModuleContract = new ethers.Contract(miningModuleAddr, MINING_MODULE_ABI, provider);
}

// TGBT token (optional)
let tgbtContract = null;
if (process.env.TGBT_TOKEN_ADDRESS) {
  tgbtContract = new ethers.Contract(process.env.TGBT_TOKEN_ADDRESS, TGBT_ABI, provider);
}

// Batch mining contract (optional — only if BATCH_MINING_ADDRESS is set)
let batchMiningContract = null;
if (process.env.BATCH_MINING_ADDRESS) {
  batchMiningContract = new ethers.Contract(
    process.env.BATCH_MINING_ADDRESS,
    BATCH_MINING_ABI,
    provider,
  );
}

/**
 * Get the latest randomness output from whichever stack is available.
 * Prefers classic mining (Core output history) when available.
 */
async function getLatestRandomnessOutput() {
  // Classic mining: read the ring buffer head from Core
  try {
    const currentIndex = await coreContract.getCurrentOutputIndex();
    // The most recent output is at (currentIndex - 1) mod 32, but outputHistory
    // is always overwritten at currentIndex before increment, so the latest
    // written slot is (currentIndex - 1 + 32) % 32
    const latestIdx = (Number(currentIndex) - 1 + 32) % 32;
    const output = await coreContract.outputHistoryAt(latestIdx);
    if (output && output !== ethers.ZeroHash) {
      return { output, source: 'classic-mining', index: latestIdx };
    }
  } catch (err) {
    logger.debug('Classic mining output unavailable:', { message: err.message });
  }

  // Fallback: batch mining
  if (batchMiningContract) {
    try {
      const output = await batchMiningContract.latestOutput();
      if (output && output !== ethers.ZeroHash) {
        return { output, source: 'batch-mining' };
      }
    } catch (err) {
      logger.debug('Batch mining output unavailable:', { message: err.message });
    }
  }

  throw new Error('No randomness output available from any mining stack');
}

// Derive random words from a beacon output + seed (off-chain, deterministic)
function generateRandomWords(latestOutput, numWords, normalizedSeed) {
  const words = [];
  for (let i = 0; i < numWords; i++) {
    const h = crypto.createHash('sha256')
      .update(latestOutput)
      .update(normalizedSeed)
      .update(i.toString())
      .digest('hex');
    words.push(h);
  }
  return words;
}

// ─────────────────────────────────────────────────────────────────
// Rate limiters — Fix #4: instantiated once, applied as middleware
// ─────────────────────────────────────────────────────────────────

const globalLimiter = rateLimit({
  windowMs: 60_000,
  max: 500_000,
  standardHeaders: true,
  legacyHeaders: false,
  store: new RedisStore({
    sendCommand: (...args) => redis.call(...args),
    prefix: 'rl:global:',
  }),
});

const userLimiter = rateLimit({
  windowMs: 60_000,
  max: 100,
  standardHeaders: true,
  keyGenerator: (req) => req.headers['x-api-key'] || req.ip,
  store: new RedisStore({
    sendCommand: (...args) => redis.call(...args),
    prefix: 'rl:user:',
  }),
});

const strictLimiter = rateLimit({
  windowMs: 60_000,
  max: 20,  // on-chain requests are expensive
  standardHeaders: true,
  keyGenerator: (req) => req.headers['x-api-key'] || req.ip,
  store: new RedisStore({
    sendCommand: (...args) => redis.call(...args),
    prefix: 'rl:strict:',
  }),
});

// ─────────────────────────────────────────────────────────────────
// Express app
// ─────────────────────────────────────────────────────────────────

const app = express();

app.use(helmet());
app.use(cors());
app.use(bodyParser.json({ limit: '64kb' }));
app.use(globalLimiter);

// Request logging middleware
app.use((req, _res, next) => {
  if (Math.random() < 0.1) { // 10% sample at info level
    logger.info(`${req.method} ${req.originalUrl}`, { ip: req.ip });
  }
  next();
});

// ─────────────────────────────────────────────────────────────────
// Authentication — Fix #2 & #3: all paths fail-closed
// ─────────────────────────────────────────────────────────────────

/**
 * Verify a JWT bearer token.
 * @param {string} token
 * @returns {{ valid: boolean, payload?: object, error?: string }}
 */
function verifyJwt(token) {
  try {
    const payload = jwt.verify(token, JWT_SECRET, {
      algorithms: ['HS256'],
      issuer: 'tg-beacon',
    });
    return { valid: true, payload };
  } catch (err) {
    return { valid: false, error: err.message };
  }
}

/**
 * Multi-strategy authentication middleware.
 *
 * Strategy priority:
 *   1. Static API key (x-api-key header) — server-to-server
 *   2. JWT bearer token — session-based or OAuth flows
 *   3. EIP-191 signed message (x-signature + x-signer-address + x-timestamp) — Web3 native
 *
 * All strategies fail-closed. No placeholder bypasses remain.
 */
const verifyAuth = async (req, res, next) => {
  const apiKey      = req.headers['x-api-key'];
  const bearerToken = req.headers['authorization']?.replace(/^Bearer\s+/i, '');
  const signature   = req.headers['x-signature'];
  const signer      = req.headers['x-signer-address'];
  const timestamp   = req.headers['x-timestamp'];

  // ── Strategy 1: Static API key ──────────────────────────────
  if (apiKey) {
    if (API_KEY && apiKey === API_KEY) {
      req.authMethod = 'api-key';
      return next();
    }
    logger.warn('Invalid API key', { ip: req.ip, url: req.originalUrl });
    return res.status(401).json({ error: 'Unauthorized', reason: 'Invalid API key' });
  }

  // ── Strategy 2: JWT bearer token ────────────────────────────
  if (bearerToken) {
    const result = verifyJwt(bearerToken);
    if (result.valid) {
      req.authMethod  = 'jwt';
      req.authPayload = result.payload;
      return next();
    }
    logger.warn('Invalid JWT', { ip: req.ip, error: result.error });
    return res.status(401).json({ error: 'Unauthorized', reason: 'Invalid or expired token' });
  }

  // ── Strategy 3: EIP-191 signed message ──────────────────────
  if (signature && signer && timestamp) {
    // Reject stale timestamps (5 minute window)
    const ts = parseInt(timestamp, 10);
    const age = Date.now() - ts;
    if (isNaN(ts) || age < 0 || age > 5 * 60 * 1000) {
      return res.status(401).json({ error: 'Unauthorized', reason: 'Timestamp out of window' });
    }

    // Message format: "<method>:<url>:<timestamp>"
    const message = `${req.method}:${req.originalUrl}:${timestamp}`;
    try {
      const recovered = verifyMessage(message, signature);
      if (recovered.toLowerCase() !== signer.toLowerCase()) {
        logger.warn('Signature signer mismatch', { expected: signer, recovered, ip: req.ip });
        return res.status(401).json({ error: 'Unauthorized', reason: 'Signature mismatch' });
      }
      req.authMethod = 'web3-signature';
      req.authSigner = recovered;
      return next();
    } catch (err) {
      logger.warn('Signature verification error', { error: err.message, ip: req.ip });
      return res.status(401).json({ error: 'Unauthorized', reason: 'Signature verification failed' });
    }
  }

  // ── No recognised credential ─────────────────────────────────
  logger.warn('No auth credentials', { ip: req.ip, url: req.originalUrl });
  return res.status(401).json({ error: 'Unauthorized', reason: 'No credentials provided' });
};

// ─────────────────────────────────────────────────────────────────
// Utilities
// ─────────────────────────────────────────────────────────────────

function normalizeSeed(seed) {
  if (!seed) return Date.now().toString(16).padStart(64, '0');
  if (Buffer.isBuffer(seed)) return seed.toString('hex');
  if (typeof seed === 'string') return seed.replace(/^0x/, '').padStart(64, '0');
  throw new Error('Invalid seed format');
}

async function verifySeedSignature(seed, signature, expectedAddress) {
  try {
    const recovered = verifyMessage(seed, signature);
    const match = recovered.toLowerCase() === expectedAddress.toLowerCase();
    if (!match) logger.warn('Seed signature mismatch', { expected: expectedAddress, recovered });
    return match;
  } catch (err) {
    logger.error('Seed signature error:', { message: err.message });
    return false;
  }
}

async function logUsage(data) {
  const requestId = uuidv4();
  try {
    await db.query(
      `INSERT INTO randomness_requests (
         request_id, client_ip, user_agent, endpoint, entropy_source,
         num_words, seed, output, randomness, success, error_message,
         webhook_url, webhook_status
       ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)`,
      [
        requestId,
        data.ipAddress  || null,
        data.userAgent  || null,
        data.endpoint,
        data.source     || null,
        data.numItems   || 0,
        data.seedUsed   || null,
        data.output     || null,
        data.randomness ? JSON.stringify(data.randomness) : null,
        !data.error,
        data.error      || null,
        data.webhookUrl || null,
        data.webhookUrl ? 'pending' : null,
      ],
    );
  } catch (err) {
    logger.error('logUsage failed:', { message: err.message });
  }
  return requestId;
}

async function triggerWebhook(onChainRequestId, resultData) {
  let row;
  try {
    const { rows } = await db.query(
      `SELECT request_id, webhook_url, webhook_status
         FROM randomness_requests
        WHERE endpoint = '/api/v1/request-onchain-randomness'
          AND randomness->>'requestId' = $1
        ORDER BY timestamp DESC LIMIT 1`,
      [String(onChainRequestId)],
    );
    if (!rows.length) return;
    row = rows[0];
  } catch (err) {
    logger.error('triggerWebhook lookup failed:', { message: err.message });
    return;
  }

  const { request_id, webhook_url, webhook_status } = row;
  if (!webhook_url || webhook_status !== 'pending') return;

  let success = false;
  let code = null;
  let body = null;

  try {
    const resp = await axios.post(webhook_url, {
      requestId: onChainRequestId,
      fulfilled: resultData.fulfilled,
      result:    resultData.result,
      timestamp: Date.now(),
    }, { timeout: 5_000 });
    code    = resp.status;
    body    = JSON.stringify(resp.data).slice(0, 255);
    success = resp.status >= 200 && resp.status < 300;
    logger.info(`Webhook ${onChainRequestId} → ${code}`);
  } catch (err) {
    code = err.response?.status || 500;
    body = err.message.slice(0, 255);
    logger.warn(`Webhook ${onChainRequestId} failed:`, { message: err.message });
  }

  try {
    await db.query(
      `UPDATE randomness_requests
          SET webhook_status = $1, webhook_response_code = $2,
              webhook_response = $3, fulfilled_at = NOW()
        WHERE request_id = $4`,
      [success ? 'success' : 'failed', code, body, request_id],
    );
  } catch (err) {
    logger.error('Webhook status update failed:', { message: err.message });
  }
}

// ─────────────────────────────────────────────────────────────────
// Swagger
// ─────────────────────────────────────────────────────────────────

const swaggerSpec = swaggerJsdoc({
  definition: {
    openapi: '3.0.0',
    info: {
      title: 'Temporal Gradient Randomness API',
      version: '1.0.0',
      description: 'Verifiable randomness from CPU mining — Merkle-proven, signed, on-chain.',
    },
    servers: [{ url: `http://localhost:${PORT}` }],
    components: {
      securitySchemes: {
        ApiKeyAuth:  { type: 'apiKey', in: 'header', name: 'x-api-key' },
        BearerAuth:  { type: 'http', scheme: 'bearer', bearerFormat: 'JWT' },
        Web3Sig:     { type: 'apiKey', in: 'header', name: 'x-signature' },
      },
    },
  },
  apis: ['./*.js'],
});

app.use('/api-docs', swaggerUi.serve, swaggerUi.setup(swaggerSpec));

// ─────────────────────────────────────────────────────────────────
// Routes
// ─────────────────────────────────────────────────────────────────

/**
 * @swagger
 * /healthz:
 *   get:
 *     summary: Health check
 *     responses:
 *       200:
 *         description: Healthy
 *       503:
 *         description: Unhealthy
 */
app.get('/healthz', async (_req, res) => {
  try {
    const [block, dbResult, ping] = await Promise.all([
      provider.getBlockNumber(),
      db.query('SELECT 1'),
      redis.ping(),
    ]);
    res.json({
      status:    'ok',
      blockNumber: block,
      db:        dbResult.rowCount === 1 ? 'ok' : 'error',
      redis:     ping === 'PONG' ? 'ok' : 'error',
      timestamp: Date.now(),
    });
  } catch (err) {
    logger.error('Health check failed:', { message: err.message });
    res.status(503).json({ status: 'unhealthy', error: err.message });
  }
});

/**
 * @swagger
 * /api/v1/status:
 *   get:
 *     summary: Beacon status
 *     responses:
 *       200:
 *         description: Current beacon state
 */
app.get('/api/v1/status', async (_req, res) => {
  try {
    const [blockNumber, isPaused, currentIndex] = await Promise.all([
      provider.getBlockNumber(),
      coreContract.isPaused(),
      coreContract.getCurrentOutputIndex(),
    ]);

    // Get latest output from the ring buffer
    const latestIdx = (Number(currentIndex) - 1 + 32) % 32;
    const latestOutput = await coreContract.outputHistoryAt(latestIdx);

    // Get tokenomics info
    let economics = null;
    try {
      const econ = await tokenomicsContract.getMiningEconomics();
      economics = {
        currentReward: ethers.formatEther(econ.currentReward),
        currentEpoch: econ.currentEpoch.toString(),
        blocksPerEpoch: econ.blocksPerEpoch.toString(),
        halvingInterval: econ.halvingInterval.toString(),
        nextHalvingBlock: econ.nextHalvingBlock.toString(),
        bonusThreshold: econ.currentBonusThreshold.toString(),
        bonusMultiplier: econ.currentBonusMultiplier.toString(),
        totalMined: ethers.formatEther(econ.minedSoFar),
        remainingAllocation: ethers.formatEther(econ.remainingAllocation),
      };
    } catch (err) {
      logger.debug('Tokenomics unavailable:', { message: err.message });
    }

    // Get pool info if mining module is available
    let pool = null;
    if (miningModuleContract) {
      try {
        const info = await miningModuleContract.getPoolInfo(0);
        pool = {
          difficulty: info.difficulty.toString(),
          remainingEmission: ethers.formatEther(info.emission),
          totalMined: ethers.formatEther(info.mined),
          active: info.active,
        };
      } catch (err) {
        logger.debug('Pool info unavailable:', { message: err.message });
      }
    }

    // Batch mining status (optional)
    let batchStatus = null;
    if (batchMiningContract) {
      try {
        const [batchOutput, nextEpochId] = await Promise.all([
          batchMiningContract.latestOutput(),
          batchMiningContract.nextEpochId(),
        ]);
        batchStatus = { latestOutput: batchOutput, nextEpochId: nextEpochId.toString() };
      } catch (err) {
        logger.debug('Batch mining status unavailable:', { message: err.message });
      }
    }

    res.json({
      status: isPaused ? 'paused' : 'ok',
      blockNumber,
      latestOutput,
      outputIndex: Number(currentIndex),
      economics,
      pool,
      batchMining: batchStatus,
      timestamp: Date.now(),
    });
  } catch (err) {
    logger.error('Status failed:', { message: err.message });
    res.status(500).json({ error: 'Status check failed', details: err.message });
  }
});

/**
 * @swagger
 * /api/v1/latest:
 *   get:
 *     summary: Latest beacon output
 *     responses:
 *       200:
 *         description: Latest randomness output hash
 */
app.get('/api/v1/latest', async (_req, res) => {
  try {
    const cacheKey = 'latest:output';
    const cached   = await redis.get(cacheKey);
    if (cached) return res.json(JSON.parse(cached));

    const [{ output, source }, blockNumber] = await Promise.all([
      getLatestRandomnessOutput(),
      provider.getBlockNumber(),
    ]);

    const payload = { output, source, blockNumber, timestamp: Date.now() };
    await redis.set(cacheKey, JSON.stringify(payload), 'EX', 12); // ~1 block
    res.json(payload);
  } catch (err) {
    logger.error('Latest fetch failed:', { message: err.message });
    res.status(500).json({ error: 'Failed to fetch latest output', details: err.message });
  }
});

/**
 * @swagger
 * /api/v1/verify:
 *   post:
 *     summary: Verify a randomness leaf proof
 *     requestBody:
 *       required: true
 *       content:
 *         application/json:
 *           schema:
 *             type: object
 *             required: [epochId, leafIndex, outputHash, proof]
 *             properties:
 *               epochId:     { type: integer }
 *               leafIndex:   { type: integer }
 *               outputHash:  { type: string }
 *               proof:       { type: array, items: { type: string } }
 *     responses:
 *       200:
 *         description: Verification result
 */
app.post('/api/v1/verify', userLimiter, async (req, res) => {
  try {
    const { epochId, leafIndex, outputHash, proof } = req.body;
    if (
      epochId    === undefined || leafIndex === undefined ||
      !outputHash || !Array.isArray(proof)
    ) {
      return res.status(400).json({ error: 'epochId, leafIndex, outputHash, proof required' });
    }

    if (!batchMiningContract) {
      return res.status(501).json({ error: 'Batch mining not configured — Merkle proof verification unavailable' });
    }

    const cacheKey = `verify:${epochId}:${leafIndex}:${outputHash}`;
    const cached   = await redis.get(cacheKey);
    if (cached) return res.json(JSON.parse(cached));

    const valid = await batchMiningContract.verifyRandomnessLeaf(
      epochId, leafIndex, outputHash, proof,
    );

    const payload = { valid, epochId, leafIndex, outputHash, timestamp: Date.now() };
    await redis.set(cacheKey, JSON.stringify(payload), 'EX', 300); // 5 min — proofs don't change
    res.json(payload);
  } catch (err) {
    logger.error('Verify failed:', { message: err.message });
    res.status(500).json({ error: 'Verification failed', details: err.message });
  }
});

/**
 * @swagger
 * /api/v1/randomness:
 *   post:
 *     summary: Generate off-chain random words from beacon output
 *     description: Deterministic, beacon-seeded random words. Optionally signed for provable fairness.
 *     requestBody:
 *       content:
 *         application/json:
 *           schema:
 *             type: object
 *             properties:
 *               numWords:      { type: integer, default: 1, minimum: 1, maximum: 100 }
 *               seed:          { type: string }
 *               signature:     { type: string }
 *               signerAddress: { type: string }
 *     responses:
 *       200:
 *         description: Random words
 *       400:
 *         description: Bad request
 *       403:
 *         description: Invalid signature
 */
app.post('/api/v1/randomness', userLimiter, async (req, res) => {
  const logData = {
    endpoint: '/api/v1/randomness',
    ipAddress: req.ip,
    userAgent: req.headers['user-agent'],
  };
  try {
    const { numWords = 1, seed = '', signature, signerAddress } = req.body;

    // Signature check if provided
    if (signature) {
      if (!signerAddress) {
        return res.status(400).json({ error: 'signerAddress required with signature' });
      }
      const valid = await verifySeedSignature(seed, signature, signerAddress);
      if (!valid) {
        return res.status(403).json({ error: 'Invalid seed signature' });
      }
    }

    if (!Number.isInteger(numWords) || numWords < 1 || numWords > 100) {
      return res.status(400).json({ error: 'numWords must be 1–100' });
    }

    const normalizedSeed = normalizeSeed(seed);
    const batchKey       = `batch:rw:${numWords}:${normalizedSeed.slice(0, 8)}`;
    const cached         = await redis.get(batchKey);
    if (cached) return res.json(JSON.parse(cached));

    const { output: latestOutput } = await getLatestRandomnessOutput();
    const randomWords  = generateRandomWords(latestOutput, numWords, normalizedSeed);

    Object.assign(logData, {
      source:    latestOutput,
      seedUsed:  normalizedSeed,
      numItems:  numWords,
      output:    randomWords[0],
      randomness: randomWords,
    });
    const requestId = await logUsage(logData);

    const payload = { source: latestOutput, randomWords, timestamp: Date.now(), requestId };
    await redis.set(batchKey, JSON.stringify(payload), 'PX', 200); // 200 ms batch window
    res.json(payload);
  } catch (err) {
    logger.error('Randomness failed:', { message: err.message });
    Object.assign(logData, { error: err.message });
    await logUsage(logData);
    res.status(500).json({ error: 'Failed to generate randomness', details: err.message });
  }
});

/**
 * @swagger
 * /api/v1/physical-randomness:
 *   post:
 *     summary: Cryptographically secure random bytes (OS entropy)
 *     requestBody:
 *       content:
 *         application/json:
 *           schema:
 *             type: object
 *             properties:
 *               numBytes: { type: integer, default: 32, minimum: 1, maximum: 1024 }
 *     responses:
 *       200:
 *         description: Random bytes as hex
 */
app.post('/api/v1/physical-randomness', userLimiter, async (req, res) => {
  const logData = {
    endpoint: '/api/v1/physical-randomness',
    ipAddress: req.ip,
    userAgent: req.headers['user-agent'],
  };
  try {
    const { numBytes = 32 } = req.body;
    if (!Number.isInteger(numBytes) || numBytes < 1 || numBytes > 1024) {
      return res.status(400).json({ error: 'numBytes must be 1–1024' });
    }
    const hex = crypto.randomBytes(numBytes).toString('hex');
    Object.assign(logData, { source: 'os-csprng', numItems: numBytes, output: hex });
    const requestId = await logUsage(logData);
    res.json({ source: 'os-csprng', randomHex: hex, timestamp: Date.now(), requestId });
  } catch (err) {
    logger.error('Physical randomness failed:', { message: err.message });
    Object.assign(logData, { error: err.message });
    await logUsage(logData);
    res.status(500).json({ error: 'Failed to generate physical randomness', details: err.message });
  }
});

/**
 * @swagger
 * /api/v1/slot-spin:
 *   post:
 *     summary: Beacon-derived slot machine spin
 *     security:
 *       - ApiKeyAuth: []
 *       - BearerAuth: []
 *     requestBody:
 *       content:
 *         application/json:
 *           schema:
 *             type: object
 *             properties:
 *               numReels:       { type: integer, default: 3, minimum: 1, maximum: 10 }
 *               symbolsPerReel: { type: integer, default: 10, minimum: 2, maximum: 100 }
 *               seed:           { type: string }
 *               signature:      { type: string }
 *               signerAddress:  { type: string }
 *     responses:
 *       200:
 *         description: Reel positions
 */
app.post('/api/v1/slot-spin', verifyAuth, userLimiter, async (req, res) => {
  const logData = {
    endpoint: '/api/v1/slot-spin',
    ipAddress: req.ip,
    userAgent: req.headers['user-agent'],
  };
  try {
    const { numReels = 3, symbolsPerReel = 10, seed = '', signature, signerAddress } = req.body;

    if (signature) {
      if (!signerAddress) {
        return res.status(400).json({ error: 'signerAddress required with signature' });
      }
      const valid = await verifySeedSignature(seed, signature, signerAddress);
      if (!valid) return res.status(403).json({ error: 'Invalid seed signature' });
    }

    if (!Number.isInteger(numReels) || numReels < 1 || numReels > 10) {
      return res.status(400).json({ error: 'numReels must be 1–10' });
    }
    if (!Number.isInteger(symbolsPerReel) || symbolsPerReel < 2 || symbolsPerReel > 100) {
      return res.status(400).json({ error: 'symbolsPerReel must be 2–100' });
    }

    const normalizedSeed = normalizeSeed(seed);
    const { output: latestOutput } = await getLatestRandomnessOutput();
    const words          = generateRandomWords(latestOutput, numReels, normalizedSeed);
    const reelPositions  = words.map(w => Number(BigInt(`0x${w}`) % BigInt(symbolsPerReel)));

    Object.assign(logData, {
      source:    latestOutput,
      seedUsed:  normalizedSeed,
      numItems:  numReels,
      randomness: reelPositions,
    });
    const requestId = await logUsage(logData);
    res.json({ source: latestOutput, seedUsed: normalizedSeed, reelPositions, timestamp: Date.now(), requestId });
  } catch (err) {
    logger.error('Slot spin failed:', { message: err.message });
    Object.assign(logData, { error: err.message });
    await logUsage(logData);
    res.status(500).json({ error: 'Failed to simulate slot spin', details: err.message });
  }
});

/**
 * @swagger
 * /api/v1/request-onchain-randomness:
 *   post:
 *     summary: Submit an on-chain randomness request
 *     security:
 *       - ApiKeyAuth: []
 *       - BearerAuth: []
 *     requestBody:
 *       required: true
 *       content:
 *         application/json:
 *           schema:
 *             type: object
 *             required: [userSeed]
 *             properties:
 *               userSeed:      { type: string, description: '32-byte hex (0x...)' }
 *               feeMultiplier: { type: number, default: 1.0 }
 *               webhookUrl:    { type: string, format: uri }
 *     responses:
 *       200:
 *         description: Transaction submitted
 *       400:
 *         description: Invalid seed
 */
app.post('/api/v1/request-onchain-randomness', verifyAuth, strictLimiter, async (req, res) => {
  const logData = {
    endpoint: '/api/v1/request-onchain-randomness',
    ipAddress: req.ip,
    userAgent: req.headers['user-agent'],
  };
  try {
    const { userSeed, feeMultiplier = 1.0, webhookUrl } = req.body;

    if (!userSeed || !/^0x[a-fA-F0-9]{64}$/.test(userSeed)) {
      return res.status(400).json({ error: 'Invalid userSeed. Must be 32-byte hex (0x...)' });
    }

    if (!batchMiningContract || !batchMiningContract.submitRandomnessRequest) {
      return res.status(501).json({ error: 'submitRandomnessRequest not available — batch mining not configured' });
    }

    const result = await batchMiningContract.submitRandomnessRequest(userSeed, feeMultiplier);

    Object.assign(logData, {
      source:     'on-chain',
      seedUsed:   userSeed,
      numItems:   1,
      randomness: result,
      webhookUrl: webhookUrl || null,
    });
    const requestId = await logUsage(logData);

    logger.info('On-chain randomness request submitted', {
      requestId: result.requestId,
      txHash: result.transactionHash,
    });
    res.json({ ...result, dbRequestId: requestId });
  } catch (err) {
    logger.error('On-chain request failed:', { message: err.message });
    Object.assign(logData, { error: err.message });
    await logUsage(logData);
    res.status(500).json({ error: 'On-chain request failed', details: err.message });
  }
});

/**
 * @swagger
 * /api/v1/get-onchain-result/{requestId}:
 *   get:
 *     summary: Poll for on-chain randomness result
 *     parameters:
 *       - in: path
 *         name: requestId
 *         required: true
 *         schema: { type: string }
 *     responses:
 *       200:
 *         description: Result or pending status
 *       404:
 *         description: Request not found
 */
app.get('/api/v1/get-onchain-result/:requestId', userLimiter, async (req, res) => {
  const { requestId } = req.params;
  try {
    const cacheKey = `onchain:result:${requestId}`;
    const cached   = await redis.get(cacheKey);
    if (cached) return res.json(JSON.parse(cached));

    if (!batchMiningContract || !batchMiningContract.getRandomnessResult) {
      return res.status(501).json({ error: 'getRandomnessResult not available — batch mining not configured' });
    }

    const resultData = await batchMiningContract.getRandomnessResult(requestId);
    if (resultData == null) {
      return res.status(404).json({ error: 'Request ID not found' });
    }

    if (resultData.fulfilled) {
      // Cache fulfilled results for 10 minutes — they won't change
      await redis.set(cacheKey, JSON.stringify({ requestId, ...resultData }), 'EX', 600);
      // Fire webhook asynchronously
      triggerWebhook(requestId, resultData).catch((err) =>
        logger.error('Async webhook error:', { message: err.message }),
      );
    }

    res.json({ requestId, ...resultData });
  } catch (err) {
    logger.error('Get on-chain result failed:', { message: err.message });
    res.status(500).json({ error: 'Failed to get on-chain result', details: err.message });
  }
});

// ─────────────────────────────────────────────────────────────────
// Classic mining endpoints — output history, pools, economics
// ─────────────────────────────────────────────────────────────────

/**
 * @swagger
 * /api/v1/output-history:
 *   get:
 *     summary: Get the full 32-slot output ring buffer from Core
 *     responses:
 *       200:
 *         description: Array of 32 output hashes
 */
app.get('/api/v1/output-history', userLimiter, async (_req, res) => {
  try {
    const cacheKey = 'output:history';
    const cached   = await redis.get(cacheKey);
    if (cached) return res.json(JSON.parse(cached));

    const [history, currentIndex, blockNumber] = await Promise.all([
      coreContract.getOutputHistory(),
      coreContract.getCurrentOutputIndex(),
      provider.getBlockNumber(),
    ]);

    const payload = {
      history: Array.from(history),
      currentIndex: Number(currentIndex),
      blockNumber,
      timestamp: Date.now(),
    };
    await redis.set(cacheKey, JSON.stringify(payload), 'EX', 12);
    res.json(payload);
  } catch (err) {
    logger.error('Output history failed:', { message: err.message });
    res.status(500).json({ error: 'Failed to fetch output history', details: err.message });
  }
});

/**
 * @swagger
 * /api/v1/economics:
 *   get:
 *     summary: Get current mining economics (reward, epoch, halving)
 *     responses:
 *       200:
 *         description: Mining economics data
 */
app.get('/api/v1/economics', userLimiter, async (_req, res) => {
  try {
    const cacheKey = 'economics';
    const cached   = await redis.get(cacheKey);
    if (cached) return res.json(JSON.parse(cached));

    const econ = await tokenomicsContract.getMiningEconomics();
    const blockNumber = await provider.getBlockNumber();

    const payload = {
      currentReward: ethers.formatEther(econ.currentReward),
      currentEpoch: econ.currentEpoch.toString(),
      blocksPerEpoch: econ.blocksPerEpoch.toString(),
      halvingInterval: econ.halvingInterval.toString(),
      nextHalvingBlock: econ.nextHalvingBlock.toString(),
      bonusThreshold: econ.currentBonusThreshold.toString(),
      bonusMultiplier: econ.currentBonusMultiplier.toString(),
      totalMined: ethers.formatEther(econ.minedSoFar),
      remainingAllocation: ethers.formatEther(econ.remainingAllocation),
      blockNumber,
      timestamp: Date.now(),
    };
    await redis.set(cacheKey, JSON.stringify(payload), 'EX', 30);
    res.json(payload);
  } catch (err) {
    logger.error('Economics failed:', { message: err.message });
    res.status(500).json({ error: 'Failed to fetch economics', details: err.message });
  }
});

/**
 * @swagger
 * /api/v1/pools:
 *   get:
 *     summary: Get active mining pools
 *     responses:
 *       200:
 *         description: List of active mining pools
 */
app.get('/api/v1/pools', userLimiter, async (_req, res) => {
  if (!miningModuleContract) {
    return res.status(501).json({ error: 'Mining module not configured' });
  }
  try {
    const cacheKey = 'pools:active';
    const cached   = await redis.get(cacheKey);
    if (cached) return res.json(JSON.parse(cached));

    const result = await miningModuleContract.getActivePools();
    const pools = [];
    for (let i = 0; i < result.activePools.length; i++) {
      pools.push({
        poolId: Number(result.activePools[i]),
        difficulty: result.difficulties[i].toString(),
        remainingEmission: ethers.formatEther(result.emissions[i]),
      });
    }

    const payload = { pools, timestamp: Date.now() };
    await redis.set(cacheKey, JSON.stringify(payload), 'EX', 30);
    res.json(payload);
  } catch (err) {
    logger.error('Pools failed:', { message: err.message });
    res.status(500).json({ error: 'Failed to fetch pools', details: err.message });
  }
});

/**
 * @swagger
 * /api/v1/supply:
 *   get:
 *     summary: TGBT token supply and balance info
 *     responses:
 *       200:
 *         description: Token supply data
 */
app.get('/api/v1/supply', userLimiter, async (_req, res) => {
  if (!tgbtContract) {
    return res.status(501).json({ error: 'TGBT token not configured' });
  }
  try {
    const cacheKey = 'tgbt:supply';
    const cached   = await redis.get(cacheKey);
    if (cached) return res.json(JSON.parse(cached));

    const totalSupply = await tgbtContract.totalSupply();
    const payload = {
      totalSupply: ethers.formatEther(totalSupply),
      timestamp: Date.now(),
    };
    await redis.set(cacheKey, JSON.stringify(payload), 'EX', 60);
    res.json(payload);
  } catch (err) {
    logger.error('Supply failed:', { message: err.message });
    res.status(500).json({ error: 'Failed to fetch supply', details: err.message });
  }
});

// ─────────────────────────────────────────────────────────────────
// JWT issuance endpoint (for operators to generate API tokens)
// ─────────────────────────────────────────────────────────────────

/**
 * @swagger
 * /api/v1/auth/token:
 *   post:
 *     summary: Issue a JWT for API access
 *     description: Operators authenticate with their API key to receive a JWT for session use.
 *     security:
 *       - ApiKeyAuth: []
 *     requestBody:
 *       content:
 *         application/json:
 *           schema:
 *             type: object
 *             properties:
 *               expiresIn: { type: string, default: '24h' }
 *               subject:   { type: string, description: 'Identifier for this token' }
 *     responses:
 *       200:
 *         description: JWT token
 *       401:
 *         description: Invalid API key
 */
app.post('/api/v1/auth/token', async (req, res) => {
  const apiKey = req.headers['x-api-key'];
  if (!apiKey || !API_KEY || apiKey !== API_KEY) {
    return res.status(401).json({ error: 'Valid x-api-key required to issue tokens' });
  }
  const { expiresIn = '24h', subject = 'api-consumer' } = req.body;
  const token = jwt.sign(
    { sub: subject, iss: 'tg-beacon', iat: Math.floor(Date.now() / 1000) },
    JWT_SECRET,
    { algorithm: 'HS256', expiresIn },
  );
  res.json({ token, expiresIn, subject });
});

// ─────────────────────────────────────────────────────────────────
// Global error handler
// ─────────────────────────────────────────────────────────────────

app.use((err, req, res, _next) => {
  logger.error('Unhandled error:', { message: err.message, url: req.originalUrl });
  res.status(500).json({ error: 'Internal Server Error' });
});

// ─────────────────────────────────────────────────────────────────
// Startup
// ─────────────────────────────────────────────────────────────────

async function start() {
  await ensureSchema();

  // Verify chain connectivity
  const block = await provider.getBlockNumber();
  logger.info(`Chain connected — block ${block}`);

  // Log contract wiring
  logger.info(`Core:        ${process.env.CORE_ADDRESS}`);
  logger.info(`Tokenomics:  ${process.env.TOKENOMICS_ADDRESS}`);
  if (miningModuleContract) logger.info(`MiningModule: ${process.env.MINING_MODULE_ADDRESS}`);
  if (tgbtContract)         logger.info(`TGBT Token:   ${process.env.TGBT_TOKEN_ADDRESS}`);
  if (batchMiningContract)  logger.info(`BatchMining:  ${process.env.BATCH_MINING_ADDRESS}`);

  // Verify Core is accessible
  try {
    const isPaused = await coreContract.isPaused();
    logger.info(`Core status: ${isPaused ? 'PAUSED ⚠️' : 'ACTIVE ✓'}`);
  } catch (err) {
    logger.error('Core contract unreachable:', { message: err.message });
    process.exit(1);
  }

  const server = app.listen(PORT, () => {
    logger.info(`Temporal Gradient Beacon API running on port ${PORT}`);
    logger.info(`Swagger docs: http://localhost:${PORT}/api-docs`);
  });

  // Tune keep-alive for load balancer compatibility
  server.keepAliveTimeout = 65_000;
  server.headersTimeout   = 66_000;

  // Graceful shutdown
  const shutdown = async (signal) => {
    logger.info(`${signal} received — shutting down`);
    server.close(async () => {
      await db.end();
      await redis.quit();
      logger.info('Shutdown complete');
      process.exit(0);
    });
  };

  process.on('SIGTERM', () => shutdown('SIGTERM'));
  process.on('SIGINT',  () => shutdown('SIGINT'));
}

start().catch((err) => {
  logger.error('Startup failed:', { message: err.message });
  process.exit(1);
});