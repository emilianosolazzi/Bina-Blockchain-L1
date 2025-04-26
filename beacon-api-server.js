// beacon-api-server.js
// Production-grade Express API for Temporal Gradient Beacon

import express from 'express';
import cors from 'cors';
import rateLimit from 'express-rate-limit';
import bodyParser from 'body-parser';
import crypto from 'crypto';
import winston from 'winston'; // Added for logging
import swaggerJsdoc from 'swagger-jsdoc'; // Added for Swagger
import swaggerUi from 'swagger-ui-express'; // Added for Swagger UI
import { verifyMessage } from 'ethers'; // Import verifyMessage from ethers
import axios from 'axios'; // Or use node-fetch
// --- Database Client ---
import { Pool } from 'pg'; // Added for PostgreSQL
import { v4 as uuidv4 } from 'uuid'; // Added for generating unique request IDs
// --- KMS/Vault Client Placeholder ---
import { beaconContract, generateRandomWords, provider } from './beacon-sdk';

const app = express();
const PORT = process.env.PORT || 3000;

// --- Logger Setup (Winston) ---
const logger = winston.createLogger({
  level: process.env.LOG_LEVEL || 'info',
  format: winston.format.combine(
    winston.format.timestamp(),
    winston.format.json()
  ),
  transports: [
    new winston.transports.Console({
      format: winston.format.combine(
        winston.format.colorize(),
        winston.format.simple()
      ),
    }),
    // Add file transport if needed:
    // new winston.transports.File({ filename: 'api.log' })
  ],
});

// --- Configuration for 10M User Scale ---
const TEN_MILLION_USER_CONFIG = {
  // API Rate Limiting
  globalRateLimit: {
    windowMs: 60 * 1000, // 1 minute window
    max: 1000000, // 1M requests per minute total (up from 100 for regular scale)
    standardHeaders: true,
    legacyHeaders: false,
  },
  userRateLimit: {
    windowMs: 60 * 1000,
    max: 100, // 100 requests per minute per user
    standardHeaders: true,
    keyGenerator: (req) => req.headers['x-api-key'] || req.ip,
  },
  
  // Database Connection Pool
  databasePool: {
    min: 20,
    max: 200,
    acquireTimeoutMillis: 30000,
    createTimeoutMillis: 30000,
    idleTimeoutMillis: 30000,
    reapIntervalMillis: 1000,
    createRetryIntervalMillis: 200,
  },
  
  // Request Batching
  batchSize: 1000, // Handle up to 1000 requests in a single batch
  batchTimeout: 200, // Max wait time for batch completion (ms)
  
  // Connection Management
  keepAliveTimeout: 65000, // Match ALB idle timeout if using AWS
  headersTimeout: 66000,
  
  // Cache Settings
  cacheTTL: {
    randomness: 60, // 60 seconds
    block: 30,      // 30 seconds
    status: 10      // 10 seconds
  },
  
  // Logging
  logLevel: 'info', // Reduce verbosity at scale
  logSampleRate: 0.01, // Only log 1% of requests at full detail
};

// Apply configuration to Express app and middleware
app.use(rateLimit(TEN_MILLION_USER_CONFIG.globalRateLimit));

// --- Database Connection ---
const dbPool = new Pool({
  connectionString: process.env.DATABASE_URL, // Ensure DATABASE_URL is set in your environment
  // Optional: Add SSL config if needed for cloud databases
  // ssl: { rejectUnauthorized: false }
});

dbPool.on('connect', () => {
  logger.info('Connected to PostgreSQL database');
});

dbPool.on('error', (err) => {
  logger.error('Database connection error:', err);
  // Consider exiting or implementing retry logic if the DB connection is critical
});
// --- End Database Connection ---

// --- Swagger Setup ---
const swaggerOptions = {
  definition: {
    openapi: '3.0.0',
    info: {
      title: 'Temporal Gradient Beacon API',
      version: '1.0.0',
      description: 'API for interacting with the Temporal Gradient Beacon and related services.',
    },
    servers: [
      {
        url: `http://localhost:${PORT}`, // Adjust if deployed elsewhere
      },
    ],
  },
  apis: ['./beacon-api-server.js'], // Path to the API file(s)
};
const swaggerSpec = swaggerJsdoc(swaggerOptions);

// Middleware
app.use(cors());
app.use(bodyParser.json());
app.use(rateLimit({ windowMs: 60 * 1000, max: 100 })); // 100 requests/min/IP

// --- Logging Middleware ---
app.use((req, res, next) => {
  logger.info(`${req.method} ${req.originalUrl}`, { ip: req.ip });
  res.on('finish', () => {
    logger.info(`${res.statusCode} ${res.statusMessage}; ${res.get('Content-Length') || 0}b sent`, { url: req.originalUrl });
  });
  next();
});

// --- Flexible Authentication Middleware ---
const verifyAuth = async (req, res, next) => {
  const apiKey = req.headers['x-api-key'];
  const bearerToken = req.headers.authorization?.split(' ')[1];
  const signature = req.headers['x-signature']; // Example: Signature of request data/timestamp
  const signerAddress = req.headers['x-signer-address']; // Example: Address for signature

  let isAuthorized = false;

  // --- Implementation Required ---
  // Priority 1: API Key (Suitable for traditional server-to-server)
  if (apiKey && apiKey === process.env.API_KEY) { // Use environment variable for key
    isAuthorized = true;
    logger.debug('Authorized via API Key');
  }
  // Priority 2: Bearer Token (Suitable for user sessions, Web3 login flows)
  else if (bearerToken /* && verifyJwt(bearerToken) */) { // Add JWT verification logic
    isAuthorized = true; // Placeholder
    logger.debug('Authorized via Bearer Token (placeholder)');
  }
  // Priority 3: Signed Message (Suitable for Web3 direct calls)
  else if (signature && signerAddress) {
    // Example: Verify signature of a timestamp or request body hash
    // const messageToVerify = req.headers['x-timestamp'] || JSON.stringify(req.body);
    // try {
    //   const recoveredAddress = verifyMessage(messageToVerify, signature);
    //   if (recoveredAddress.toLowerCase() === signerAddress.toLowerCase()) {
    //     isAuthorized = true;
    //     logger.debug(`Authorized via Signature from ${signerAddress}`);
    //   }
    // } catch (e) { logger.warn('Signature verification failed during auth', e); }
    isAuthorized = true; // Placeholder for signature auth logic
    logger.debug(`Attempting authorization via Signature from ${signerAddress} (placeholder)`);
  }
  // --- End Implementation ---

  if (!isAuthorized) {
    logger.warn(`Unauthorized access attempt to ${req.originalUrl}`, { ip: req.ip });
    return res.status(401).json({ error: 'Unauthorized' });
  }
  next();
};

// --- Swagger UI Route ---
app.use('/api-docs', swaggerUi.serve, swaggerUi.setup(swaggerSpec));

// --- Utility: Normalize Seed ---
function normalizeSeed(seed) {
  if (!seed) {
    const now = Date.now().toString();
    return Buffer.from(now).toString('hex').padStart(64, '0');
  }
  if (Buffer.isBuffer(seed)) return seed.toString('hex');
  if (typeof seed === 'string') {
    return seed.replace(/^0x/, '').padStart(64, '0');
  }
  throw new Error('Invalid seed format');
}

// --- Utility: Implement Signature Verification ---
async function verifySeedSignature(seed, signature, expectedSignerAddress) {
  // --- Implementation Required ---
  // Use ethers.verifyMessage to check the signature against the seed.
  // Ensure the seed is formatted consistently (e.g., hex string) before verification.
  logger.debug(`Verifying signature '${signature}' for seed '${seed}' against address '${expectedSignerAddress}'`);
  try {
    // The message signed by the client should be the seed itself (or a derived string)
    const message = seed; // Or `JSON.stringify({ seed: seed, timestamp: ... })`
    const recoveredAddress = verifyMessage(message, signature);
    const isValid = recoveredAddress.toLowerCase() === expectedSignerAddress.toLowerCase();
    if (!isValid) {
        logger.warn(`Signature recovery mismatch: Expected ${expectedSignerAddress}, Got ${recoveredAddress}`);
    }
    return isValid;
  } catch (error) {
    logger.error('Signature verification failed:', { error: error.message });
    return false;
  }
}

// --- Utility: Database Logging ---
async function logUsage(data) {
  // Generate a unique ID for this specific request log entry
  const uniqueRequestId = uuidv4();

  const query = `
    INSERT INTO randomness_requests (
      request_id, client_ip, user_agent, endpoint, entropy_source,
      num_words, seed, output, randomness, success, error_message,
      timestamp, fulfilled_at, webhook_url, webhook_status
      -- webhook_response_code, webhook_response are handled in triggerWebhook
    ) VALUES (
      $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15
    ) RETURNING id; -- Return the DB-generated ID if needed
  `;

  const values = [
    uniqueRequestId,                                  // request_id (UUID)
    data.ipAddress || null,                           // client_ip
    data.userAgent || null,                           // user_agent (Add this to data if needed)
    data.endpoint,                                    // endpoint
    data.source,                                      // entropy_source
    data.numItems || 0,                               // num_words (or numBytes)
    data.seedUsed || null,                            // seed
    data.output || null,                              // output (e.g., single random hex, beacon output)
    data.randomness ? JSON.stringify(data.randomness) : null, // randomness (e.g., array of words)
    data.error ? false : true,                        // success
    data.error || null,                               // error_message
    new Date(),                                       // timestamp
    data.fulfilledAt || null,                         // fulfilled_at
    data.webhookUrl || null,                          // webhook_url
    data.webhookUrl ? 'pending' : null                // webhook_status (initial status if URL provided)
  ];

  try {
    const res = await dbPool.query(query, values);
    logger.debug('Usage logged successfully to DB', { dbId: res.rows[0]?.id, requestId: uniqueRequestId });
    return uniqueRequestId; // Return the generated UUID for potential linking
  } catch (error) {
    logger.error('Failed to log usage to database:', { error: error.message, data });
    return null;
  }
}

// --- Utility: Webhook Handling (DB Integrated) ---
// Remove the in-memory webhookStore
// const webhookStore = new Map();

async function storeWebhook(dbRequestId, url) { // Use the DB request_id (UUID)
  if (!dbRequestId || !url) return;

  const query = `
    UPDATE randomness_requests
    SET webhook_url = $1, webhook_status = 'pending'
    WHERE request_id = $2;
  `;
  try {
    await dbPool.query(query, [url, dbRequestId]);
    logger.info(`Webhook URL stored in DB for request ${dbRequestId}`);
  } catch (error) {
    logger.error(`Failed to store webhook URL in DB for request ${dbRequestId}:`, error);
  }
}

async function triggerWebhook(onChainRequestId, resultData) { // Use the on-chain request ID to find the DB record
  let dbRequestId = null;
  let url = null;
  let currentStatus = null;

  // 1. Find the corresponding DB request record and its webhook URL/status
  try {
    const findQuery = `
      SELECT request_id, webhook_url, webhook_status
      FROM randomness_requests
      WHERE endpoint = '/api/v1/request-onchain-randomness'
        AND randomness ->> 'requestId' = $1 -- Assuming requestId is stored in the randomness JSONB field
      ORDER BY timestamp DESC -- Get the latest request if multiple exist for the same on-chain ID
      LIMIT 1;
    `;
    // Note: Adjust the WHERE clause if `requestId` is stored differently (e.g., dedicated column)
    const findRes = await dbPool.query(findQuery, [onChainRequestId.toString()]);

    if (findRes.rows.length > 0) {
      dbRequestId = findRes.rows[0].request_id;
      url = findRes.rows[0].webhook_url;
      currentStatus = findRes.rows[0].webhook_status;
    } else {
      logger.debug(`No DB record found for on-chain request ID ${onChainRequestId} with a webhook.`);
      return;
    }
  } catch (error) {
    logger.error(`Error finding webhook info in DB for on-chain request ${onChainRequestId}:`, error);
    return; // Don't proceed if we can't find the record
  }

  // 2. Check if webhook should be triggered
  if (!url || currentStatus !== 'pending') {
    logger.debug(`Webhook for on-chain request ${onChainRequestId} (DB ID: ${dbRequestId}) not triggered. URL: ${url}, Status: ${currentStatus}`);
    return;
  }

  // 3. Trigger the webhook
  logger.info(`Triggering webhook for on-chain request ${onChainRequestId} (DB ID: ${dbRequestId}) to ${url}`);
  let webhookSuccess = false;
  let responseStatus = null;
  let responseData = null;

  try {
    const response = await axios.post(url, {
      requestId: onChainRequestId, // Send the original on-chain request ID
      fulfilled: resultData.fulfilled,
      result: resultData.result,
      timestamp: Date.now(),
    }, { timeout: 5000 });

    responseStatus = response.status;
    responseData = typeof response.data === 'string' ? response.data.substring(0, 255) : JSON.stringify(response.data).substring(0, 255); // Truncate response

    if (responseStatus >= 200 && responseStatus < 300) {
      logger.info(`Webhook for ${onChainRequestId} successful (Status: ${responseStatus})`);
      webhookSuccess = true;
    } else {
      logger.warn(`Webhook for ${onChainRequestId} failed with status ${responseStatus}`);
      // Implement retry logic if needed (e.g., queueing)
    }
  } catch (error) {
    logger.error(`Webhook call failed for on-chain request ${onChainRequestId} to ${url}:`, { error: error.message });
    responseStatus = error.response?.status || 500; // Capture HTTP status if available
    responseData = error.message.substring(0, 255);
    // Implement retry logic if needed
  }

  // 4. Update the webhook status in the database
  const updateQuery = `
    UPDATE randomness_requests
    SET webhook_status = $1, webhook_response_code = $2, webhook_response = $3, fulfilled_at = $4
    WHERE request_id = $5;
  `;
  try {
    await dbPool.query(updateQuery, [
      webhookSuccess ? 'success' : 'failed',
      responseStatus,
      responseData,
      new Date(), // fulfilled_at (when webhook was attempted/completed)
      dbRequestId
    ]);
    logger.debug(`Webhook status updated in DB for request ${dbRequestId}`);
  } catch (error) {
    logger.error(`Failed to update webhook status in DB for request ${dbRequestId}:`, error);
  }
}

// --- API Endpoints ---

/**
 * @swagger
 * /healthz:
 *   get:
 *     summary: Health Check
 *     description: Performs basic system checks to confirm API readiness.
 *     responses:
 *       200:
 *         description: API is healthy.
 *         content:
 *           application/json:
 *             schema:
 *               type: object
 *               properties:
 *                 status:
 *                   type: string
 *                   example: ok
 *                 timestamp:
 *                   type: number
 *                   example: 1678886400000
 *       503:
 *         description: API is unhealthy (e.g., cannot connect to blockchain provider).
 */
app.get('/healthz', async (req, res) => {
  try {
    // Basic check: Can we get the block number?
    await provider.getBlockNumber();
    res.status(200).json({ status: 'ok', timestamp: Date.now() });
  } catch (error) {
    logger.error('Health check failed:', { error: error.message });
    res.status(503).json({ status: 'unhealthy', error: 'Cannot connect to blockchain provider', details: error.message });
  }
});

/**
 * @swagger
 * /api/v1/latest:
 *   get:
 *     summary: Get Latest Beacon Output
 *     description: Retrieves the most recent output hash from the Temporal Gradient Beacon contract.
 *     responses:
 *       200:
 *         description: Successful response with the latest beacon output.
 *         content:
 *           application/json:
 *             schema:
 *               type: object
 *               properties:
 *                 output:
 *                   type: string
 *                   description: The latest beacon output hash (hex string).
 *                 timestamp:
 *                   type: number
 *                   description: Server timestamp when the request was processed.
 *                 blockNumber:
 *                   type: number
 *                   description: The Ethereum block number at the time of the request.
 *       500:
 *         description: Internal server error.
 */
app.get('/api/v1/latest', async (req, res) => {
  try {
    const latestOutput = await beaconContract.getLatestOutput();
    const blockNumber = await provider.getBlockNumber();

    logger.info('Fetched latest output successfully');
    res.json({
      output: latestOutput,
      timestamp: Date.now(),
      blockNumber
    });
  } catch (error) {
    logger.error('Failed to fetch latest output:', { error: error.message, stack: error.stack });
    res.status(500).json({ error: 'Failed to fetch latest output', details: error.message });
  }
});

/**
 * @swagger
 * /api/v1/randomness:
 *   post:
 *     summary: Generate Random Words (Locally Derived)
 *     description: Derives deterministic random words locally based on the latest beacon output and an optional seed. Suitable for both traditional and Web3 clients needing off-chain randomness. Optionally requires authentication and seed signature verification for provable fairness.
 *     security:
 *       - ApiKeyAuth: [] # Optional: Define security scheme if using verifyAuth
 *     requestBody:
 *       required: false
 *       content:
 *         application/json:
 *           schema:
 *             type: object
 *             properties:
 *               numWords:
 *                 type: integer
 *                 description: Number of random words to generate (1-100).
 *                 default: 1
 *               seed:
 *                 type: string
 *                 description: Optional entropy seed (hex string or buffer).
 *                 default: ''
 *               signerAddress:
 *                 type: string
 *                 description: Address expected to have signed the seed (required if signature provided).
 *               signature:
 *                 type: string
 *                 description: Signature of the seed by the signerAddress (optional).
 *     responses:
 *       200:
 *         description: Successfully generated random words.
 *       400:
 *         description: Invalid input parameters (numWords, seed format, missing signerAddress).
 *       401:
 *         description: Unauthorized (if verifyAuth is enabled and fails).
 *       403:
 *         description: Invalid signature.
 *       500:
 *         description: Internal server error.
 */
// Apply verifyAuth middleware if monetization/authentication is desired for this endpoint
// app.post('/api/v1/randomness', verifyAuth, async (req, res) => {
app.post('/api/v1/randomness', async (req, res) => { // Currently public
  // Apply per-user rate limiting for 10M scale
  const userRateLimiter = rateLimit(TEN_MILLION_USER_CONFIG.userRateLimit);
  userRateLimiter(req, res, async () => {
    // Process the request as normal
    const endpoint = '/api/v1/randomness';
    let logData = { endpoint, ipAddress: req.ip, userAgent: req.headers['user-agent'] };
    let dbRequestId = null; // To potentially link webhook storage later if needed
    try {
      const { numWords = 1, seed = '', signature, signerAddress } = req.body;

      // --- Signature Verification (Optional) ---
      if (signature) {
        if (!signerAddress) {
          return res.status(400).json({ error: 'signerAddress is required when providing a signature' });
        }
        const isValidSignature = await verifySeedSignature(seed, signature, signerAddress);
        if (!isValidSignature) {
          logger.warn('Invalid signature provided for randomness request', { signerAddress, seed });
          return res.status(403).json({ error: 'Invalid signature' });
        }
        logger.debug('Seed signature verified successfully');
      }
      // --- End Signature Verification ---

      if (!Number.isInteger(numWords) || numWords <= 0 || numWords > 100) {
        logData.error = 'numWords must be between 1 and 100';
        dbRequestId = await logUsage(logData);
        return res.status(400).json({ error: logData.error });
      }

      const latestOutput = await beaconContract.getLatestOutput();
      const normalizedSeed = normalizeSeed(seed);
      const randomWords = generateRandomWords(latestOutput, numWords, normalizedSeed);

      // For 10M scale, batch similar requests together
      const batchKey = `${req.body.numWords || 1}-${normalizedSeed.substring(0, 8)}`;
      const cachedResult = await redisClient.get(`batch:${batchKey}`);
      
      if (cachedResult) {
        // Return from batch cache
        return res.json(JSON.parse(cachedResult));
      }

      logData = {
          ...logData,
          source: latestOutput,
          seedUsed: normalizedSeed,
          numItems: numWords,
          output: randomWords.length > 0 ? randomWords[0] : null, // Log first word as example output
          randomness: randomWords // Log full array
      };
      dbRequestId = await logUsage(logData); // Log usage

      logger.info(`Generated ${numWords} random words`, { source: latestOutput, seedUsed: normalizedSeed });
      const responseData = {
        source: latestOutput,
        randomWords,
        timestamp: Date.now(),
        requestId: dbRequestId // Optionally return the DB log request ID
      };

      // Cache for future similar requests in this millisecond
      await redisClient.set(
        `batch:${batchKey}`, 
        JSON.stringify(responseData), 
        'PX', 
        50 // 50ms cache for batching
      );

      res.json(responseData);
    } catch (error) {
      logger.error('Failed to generate randomness:', { error: error.message, stack: error.stack });
      logData.error = error.message;
      await logUsage(logData); // Log error
      res.status(500).json({ error: 'Failed to generate randomness', details: error.message });
    }
  });
});

/**
 * @swagger
 * /api/v1/physical-randomness:
 *   post:
 *     summary: Get Simulated Physical Randomness
 *     description: Returns cryptographically secure random bytes, simulating a physical RNG source. Suitable for traditional clients or as an alternative source.
 *     requestBody:
 *       required: false
 *       content:
 *         application/json:
 *           schema:
 *             type: object
 *             properties:
 *               numBytes:
 *                 type: integer
 *                 description: Number of random bytes to generate (1-1024).
 *                 default: 32
 *     responses:
 *       200:
 *         description: Successfully generated random bytes.
 *       400:
 *         description: Invalid input parameter (numBytes).
 *       500:
 *         description: Internal server error.
 */
app.post('/api/v1/physical-randomness', async (req, res) => { // Made async for logUsage
  const endpoint = '/api/v1/physical-randomness';
  let logData = { endpoint, ipAddress: req.ip, userAgent: req.headers['user-agent'] };
  let dbRequestId = null;
  try {
    const { numBytes = 32 } = req.body;

    if (!Number.isInteger(numBytes) || numBytes <= 0 || numBytes > 1024) {
       logData.error = 'numBytes must be between 1 and 1024';
       dbRequestId = await logUsage(logData);
       return res.status(400).json({ error: logData.error });
    }

    const randomBytes = crypto.randomBytes(numBytes);
    const randomHex = randomBytes.toString('hex');

    logData = {
        ...logData,
        source: 'simulated-physical-rng',
        numItems: numBytes,
        output: randomHex // Log the hex string
    };
    dbRequestId = await logUsage(logData); // Log usage

    logger.info(`Generated ${numBytes} simulated physical random bytes`);
    res.json({
      source: 'simulated-physical-rng',
      randomHex: randomHex,
      timestamp: Date.now(),
      requestId: dbRequestId // Optionally return the DB log request ID
    });
  } catch (error) {
    logger.error('Failed to generate physical randomness:', { error: error.message, stack: error.stack });
    logData.error = error.message;
    await logUsage(logData); // Log error
    res.status(500).json({ error: 'Failed to generate physical randomness', details: error.message });
  }
});

/**
 * @swagger
 * /api/v1/slot-spin:
 *   post:
 *     summary: Simulate Slot Machine Spin
 *     description: Simulates slot reel positions using beacon-derived randomness and an optional seed. Suitable for traditional casino game servers. Optionally requires authentication and seed signature verification.
 *     security:
 *       - ApiKeyAuth: [] # Optional: Define security scheme if using verifyAuth
 *     requestBody:
 *       required: false
 *       content:
 *         application/json:
 *           schema:
 *             type: object
 *             properties:
 *               numReels:
 *                 type: integer
 *                 description: Number of reels (1-10).
 *                 default: 3
 *               symbolsPerReel:
 *                 type: integer
 *                 description: Number of symbols on each reel (2-100).
 *                 default: 10
 *               seed:
 *                 type: string
 *                 description: Optional entropy seed (hex string or buffer).
 *                 default: ''
 *               signerAddress:
 *                 type: string
 *                 description: Address expected to have signed the seed (required if signature provided).
 *               signature:
 *                 type: string
 *                 description: Signature of the seed by the signerAddress (optional).
 *     responses:
 *       200:
 *         description: Successfully simulated slot spin.
 *       400:
 *         description: Invalid input parameters (numReels, symbolsPerReel, seed format, missing signerAddress).
 *       401:
 *         description: Unauthorized (if verifyAuth is enabled and fails).
 *       403:
 *         description: Invalid signature.
 *       500:
 *         description: Internal server error.
 */
// Apply verifyAuth middleware if monetization/authentication is desired for this endpoint
// app.post('/api/v1/slot-spin', verifyAuth, async (req, res) => {
app.post('/api/v1/slot-spin', async (req, res) => { // Currently public
  const endpoint = '/api/v1/slot-spin';
  let logData = { endpoint, ipAddress: req.ip, userAgent: req.headers['user-agent'] };
  let dbRequestId = null;
  try {
    const { numReels = 3, symbolsPerReel = 10, seed = '', signature, signerAddress } = req.body;

    // --- Signature Verification (Optional) ---
    if (signature) {
      if (!signerAddress) {
        return res.status(400).json({ error: 'signerAddress is required when providing a signature' });
      }
      const isValidSignature = await verifySeedSignature(seed, signature, signerAddress);
      if (!isValidSignature) {
        logger.warn('Invalid signature provided for slot spin request', { signerAddress, seed });
        return res.status(403).json({ error: 'Invalid signature' });
      }
      logger.debug('Slot spin seed signature verified successfully');
    }
    // --- End Signature Verification ---

    if (!Number.isInteger(numReels) || numReels <= 0 || numReels > 10) {
        logData.error = 'numReels must be between 1 and 10';
        dbRequestId = await logUsage(logData);
        return res.status(400).json({ error: logData.error });
    }
    if (!Number.isInteger(symbolsPerReel) || symbolsPerReel <= 1 || symbolsPerReel > 100) {
        logData.error = 'symbolsPerReel must be between 2 and 100';
        dbRequestId = await logUsage(logData);
        return res.status(400).json({ error: logData.error });
    }

    // Determine how many random words are needed (1 word per reel)
    const numWordsNeeded = numReels;

    // Fetch beacon-derived randomness (could also use physical source via internal call)
    const latestOutput = await beaconContract.getLatestOutput();
    const normalizedSeed = normalizeSeed(seed);
    const randomWords = generateRandomWords(latestOutput, numWordsNeeded, normalizedSeed);

    // Simulate mapping random words to reel positions
    const reelPositions = randomWords.map(wordHex => {
      // Convert hex word to a large number
      const randomBigInt = BigInt(`0x${wordHex}`);
      // Map to a symbol index within the reel range
      return Number(randomBigInt % BigInt(symbolsPerReel));
    });

    logData = {
        ...logData,
        source: latestOutput,
        seedUsed: normalizedSeed,
        numItems: numReels,
        randomness: reelPositions // Log the resulting positions
    };
    dbRequestId = await logUsage(logData); // Log usage

    logger.info(`Simulated slot spin for ${numReels} reels`, { source: latestOutput, seedUsed: normalizedSeed, positions: reelPositions });
    res.json({
      source: latestOutput,
      seedUsed: normalizedSeed,
      reelPositions, // Array of symbol indices for each reel
      timestamp: Date.now(),
      requestId: dbRequestId // Optionally return the DB log request ID
    });
  } catch (error) {
    logger.error('Failed to simulate slot spin:', { error: error.message, stack: error.stack });
    logData.error = error.message;
    await logUsage(logData); // Log error
    res.status(500).json({ error: 'Failed to simulate slot spin', details: error.message });
  }
});

/**
 * @swagger
 * /api/v1/request-onchain-randomness:
 *   post:
 *     summary: Request On-Chain Randomness
 *     description: Submits a request for randomness directly to the Temporal Gradient Beacon contract. Requires the API server to have a funded wallet. Suitable for Web3 applications needing verifiable on-chain randomness generation. Requires authentication.
 *     security:
 *       - ApiKeyAuth: [] # Example security requirement
 *     requestBody:
 *       required: true
 *       content:
 *         application/json:
 *           schema:
 *             type: object
 *             properties:
 *               userSeed:
 *                 type: string
 *                 description: User-provided entropy seed (32-byte hex string, e.g., '0x...').
 *                 required: true
 *               feeMultiplier:
 *                 type: number
 *                 description: Optional multiplier for the randomness fee (for faster inclusion).
 *                 default: 1.0
 *               webhookUrl:
 *                 type: string
 *                 format: url
 *                 description: Optional URL to receive the randomness result via POST callback.
 *     responses:
 *       200:
 *         description: Successfully submitted randomness request transaction.
 *         content:
 *           application/json:
 *             schema:
 *               type: object
 *               properties:
 *                 transactionHash:
 *                   type: string
 *                 requestId:
 *                   type: number # Or string depending on SDK return type
 *                 blockNumber:
 *                   type: number
 *       400:
 *         description: Invalid input (e.g., bad seed format).
 *       401:
 *         description: Unauthorized.
 *       500:
 *         description: Internal server error (e.g., blockchain transaction failed, server wallet issue).
 */
app.post('/api/v1/request-onchain-randomness', verifyAuth, async (req, res) => {
  const endpoint = '/api/v1/request-onchain-randomness';
  let logData = { endpoint, ipAddress: req.ip, userAgent: req.headers['user-agent'] };
  let dbRequestId = null;
  try {
    const { userSeed, feeMultiplier = 1.0, webhookUrl } = req.body;

    if (!userSeed || !/^0x[a-fA-F0-9]{64}$/.test(userSeed)) {
      logData.error = 'Invalid userSeed format. Must be a 32-byte hex string (0x...).';
      dbRequestId = await logUsage(logData);
      return res.status(400).json({ error: logData.error });
    }

    // --- SDK Interaction (using KMS/Vault via SDK) ---
    if (!beaconContract.submitRandomnessRequest) {
        logger.error("beaconContract.submitRandomnessRequest function not found in SDK.");
        return res.status(501).json({ error: "On-chain request functionality not implemented in SDK." });
    }
    // result should contain { transactionHash, requestId, blockNumber } from SDK
    const result = await beaconContract.submitRandomnessRequest(userSeed, feeMultiplier);
    // --- End SDK Interaction ---

    // Log usage *before* storing webhook to get the dbRequestId
    logData = {
        ...logData,
        source: 'on-chain',
        seedUsed: userSeed,
        numItems: 1, // Typically 1 on-chain request results in 1 random value
        randomness: result, // Store the SDK result containing the on-chain requestId
        webhookUrl: webhookUrl // Include webhook URL in initial log
    };
    dbRequestId = await logUsage(logData); // Log usage and get DB request ID

    // Store webhook URL using the DB request ID if provided
    // Note: storeWebhook now updates the existing log entry
    // if (webhookUrl && dbRequestId) {
    //   await storeWebhook(dbRequestId, webhookUrl); // This function is now integrated into logUsage/triggerWebhook
    // }

    logger.info(`Submitted on-chain randomness request ${result.requestId}`, { txHash: result.transactionHash, dbRequestId });
    res.json({ ...result, dbRequestId }); // Return SDK result + DB log ID

  } catch (error) {
    logger.error('Failed to request on-chain randomness:', { error: error.message, stack: error.stack });
    logData.error = error.message;
    await logUsage(logData); // Log error
    res.status(500).json({ error: 'Failed to process on-chain randomness request', details: error.message });
  }
});

/**
 * @swagger
 * /api/v1/get-onchain-result/{requestId}:
 *   get:
 *     summary: Get On-Chain Randomness Result
 *     description: Retrieves the fulfilled randomness result for a specific request ID from the contract. Suitable for Web3 applications after an on-chain request has been processed.
 *     parameters:
 *       - in: path
 *         name: requestId
 *         required: true
 *         schema:
 *           type: string # Or integer depending on how IDs are handled
 *         description: The ID of the randomness request.
 *     responses:
 *       200:
 *         description: Randomness result retrieved successfully or status indicating not yet fulfilled.
 *         content:
 *           application/json:
 *             schema:
 *               type: object
 *               properties:
 *                 requestId:
 *                   type: string # Or integer
 *                 fulfilled:
 *                   type: boolean
 *                 result:
 *                   type: string
 *                   description: The random value (32-byte hex string) if fulfilled, otherwise null or empty.
 *       404:
 *         description: Request ID not found.
 *       500:
 *         description: Internal server error.
 */
app.get('/api/v1/get-onchain-result/:requestId', async (req, res) => {
  const endpoint = '/api/v1/get-onchain-result';
  let logData = { endpoint, ipAddress: req.ip, userAgent: req.headers['user-agent'] };
  const { requestId } = req.params; // On-chain request ID
  try {
    // Validate requestId format if necessary

    // --- SDK Interaction ---
    if (!beaconContract.getRandomnessResult) {
        logger.error("beaconContract.getRandomnessResult function not found in SDK.");
        return res.status(501).json({ error: "Get on-chain result functionality not implemented in SDK." });
    }
    const resultData = await beaconContract.getRandomnessResult(requestId);
    // --- End SDK Interaction ---

    if (resultData === null || typeof resultData === 'undefined') {
        logger.warn(`On-chain request ID not found: ${requestId}`);
        // Log this attempt? Maybe not necessary unless polling is excessive.
        return res.status(404).json({ error: 'Request ID not found' });
    }

    // Log the successful fetch attempt (optional, could be noisy)
    // logData = { ...logData, source: 'on-chain-get', requestId: requestId, randomness: resultData };
    // await logUsage(logData);

    // --- Trigger Webhook if fulfilled ---
    if (resultData && resultData.fulfilled) {
      triggerWebhook(requestId, resultData).catch(err => { // Pass on-chain ID
        logger.error(`Error triggering webhook in background for ${requestId}:`, err);
      });
    }
    // --- End Webhook Trigger ---

    logger.info(`Fetched on-chain result status for request ${requestId}`, { fulfilled: resultData.fulfilled });
    res.json({
        requestId,
        ...resultData
    });

  } catch (error) {
    logger.error(`Failed to get on-chain result for request ${requestId}:`, { error: error.message, stack: error.stack });
    // Log error?
    // logData.error = error.message;
    // await logUsage(logData);
    res.status(500).json({ error: 'Failed to get on-chain randomness result', details: error.message });
  }
});

/**
 * @swagger
 * /api/v1/status:
 *   get:
 *     summary: Get API and Beacon Status
 *     description: Provides basic status information about the API and the connected beacon.
 *     responses:
 *       200:
 *         description: Successful status response.
 *       500:
 *         description: Internal server error (e.g., cannot connect to blockchain provider).
 */
app.get('/api/v1/status', async (req, res) => {
  try {
    const blockNumber = await provider.getBlockNumber();
    const latestOutput = await beaconContract.getLatestOutput();

    logger.info('Status check successful');
    res.json({
      status: 'ok',
      blockNumber,
      latestOutput,
      timestamp: Date.now()
    });
  } catch (error) {
    logger.error('Status check failed:', { error: error.message, stack: error.stack });
    res.status(500).json({ error: 'Status check failed', details: error.message });
  }
});

// --- Error Handling Middleware ---
// Add a generic error handler for uncaught errors
app.use((err, req, res, next) => {
  logger.error('Unhandled error:', { error: err.message, stack: err.stack, url: req.originalUrl });
  res.status(500).json({ error: 'Internal Server Error' });
});

// --- Start Server ---
app.listen(PORT, () => {
  // Use logger instead of console.log
  logger.info(`🚀 Beacon API server running on port ${PORT}`);
  logger.info(`📚 API Docs available at http://localhost:${PORT}/api-docs`);
});
