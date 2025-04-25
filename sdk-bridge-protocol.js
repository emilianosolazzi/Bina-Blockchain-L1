// sdk-bridge-protocol.js
// JS-based protocol bridge between physical slot machines and the Beacon API

import WebSocket from 'ws';
import axios from 'axios';

const BEACON_API_URL = process.env.BEACON_API_URL || 'http://localhost:3000/api/v1/slot-spin';
const PORT = process.env.BRIDGE_PORT || 7070;

// Start WebSocket server for physical machines
const wss = new WebSocket.Server({ port: PORT }, () => {
  console.log(`🧠 Slot Machine SDK Bridge running on ws://localhost:${PORT}`);
});

wss.on('connection', (socket, req) => {
  const clientIP = req.socket.remoteAddress;
  console.log(`🔌 New slot machine connected: ${clientIP}`);

  socket.on('message', async (message) => {
    try {
      const payload = JSON.parse(message);
      const { machineId, reels = 3, symbols = 10, seed, signature, address } = payload;

      // Forward to Beacon API
      const response = await axios.post(BEACON_API_URL, {
        numReels: reels,
        symbolsPerReel: symbols,
        seed,
        signature,
        signerAddress: address
      });

      // Send response back to the machine
      socket.send(JSON.stringify({
        status: 'ok',
        machineId,
        reelPositions: response.data.reelPositions,
        timestamp: response.data.timestamp
      }));

      console.log(`🎰 Spin result sent to machine ${machineId}`);

    } catch (error) {
      console.error(`❌ Error processing slot spin: ${error.message}`);
      socket.send(JSON.stringify({ status: 'error', error: error.message }));
    }
  });

  socket.on('close', () => {
    console.log(`🔌 Machine disconnected: ${clientIP}`);
  });
});
