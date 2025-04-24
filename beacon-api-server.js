// beacon-api-server.js
// Production-grade Express API for Temporal Gradient Beacon

import express from 'express';
import cors from 'cors';
import rateLimit from 'express-rate-limit';
import bodyParser from 'body-parser';
import { beaconContract, generateRandomWords, provider } from './beacon-sdk';

const app = express();
const PORT = process.env.PORT || 3000;

// Middleware
app.use(cors());
app.use(bodyParser.json());
app.use(rateLimit({ windowMs: 60 * 1000, max: 100 })); // 100 requests/min/IP

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

// --- GET /api/v1/latest ---
app.get('/api/v1/latest', async (req, res) => {
  try {
    const latestOutput = await beaconContract.getLatestOutput();
    const blockNumber = await provider.getBlockNumber();

    res.json({
      output: latestOutput,
      timestamp: Date.now(),
      blockNumber
    });
  } catch (error) {
    res.status(500).json({ error: 'Failed to fetch latest output', details: error.message });
  }
});

// --- POST /api/v1/randomness ---
app.post('/api/v1/randomness', async (req, res) => {
  try {
    const { numWords = 1, seed = '' } = req.body;

    if (!Number.isInteger(numWords) || numWords <= 0 || numWords > 100) {
      return res.status(400).json({ error: 'numWords must be between 1 and 100' });
    }

    const latestOutput = await beaconContract.getLatestOutput();
    const normalizedSeed = normalizeSeed(seed);
    const randomWords = generateRandomWords(latestOutput, numWords, normalizedSeed);

    res.json({
      source: latestOutput,
      randomWords,
      timestamp: Date.now()
    });
  } catch (error) {
    res.status(500).json({ error: 'Failed to generate randomness', details: error.message });
  }
});

// --- GET /api/v1/status ---
app.get('/api/v1/status', async (req, res) => {
  try {
    const blockNumber = await provider.getBlockNumber();
    const latestOutput = await beaconContract.getLatestOutput();

    res.json({
      status: 'ok',
      blockNumber,
      latestOutput,
      timestamp: Date.now()
    });
  } catch (error) {
    res.status(500).json({ error: 'Status check failed', details: error.message });
  }
});

// --- Start Server ---
app.listen(PORT, () => {
  console.log(`🚀 Beacon API server running on port ${PORT}`);
});
