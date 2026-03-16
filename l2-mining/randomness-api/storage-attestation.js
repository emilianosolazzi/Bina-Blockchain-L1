const fs = require('fs');
const path = require('path');
const { execFile } = require('child_process');

const RUST_MANIFEST = path.resolve(__dirname, '..', 'rust', 'Cargo.toml');
const STORAGE_PROVIDER = process.env.STORAGE_VERIFIER_PROVIDER || 'epoch-store-local';
const STORAGE_VERIFY_TIMEOUT_MS = Number(process.env.STORAGE_VERIFY_TIMEOUT_MS || 180000);

function getEpochFilePath(epochId) {
  return path.resolve(process.env.EPOCH_STORE || path.resolve(__dirname, 'epoch-store'), `epoch-${epochId}.json`);
}

function verifyEpochStorage(epochId) {
  const epochFile = getEpochFilePath(epochId);
  if (!fs.existsSync(epochFile)) {
    return Promise.reject(new Error(`Epoch file not found: ${epochFile}`));
  }

  const args = [
    'run',
    '--quiet',
    '--manifest-path',
    RUST_MANIFEST,
    '-p',
    'temporal-gradient-miner-installer',
    '--bin',
    'tg-storage-attestation',
    '--',
    '--epoch-file',
    epochFile,
    '--provider',
    STORAGE_PROVIDER,
  ];

  return new Promise((resolve, reject) => {
    execFile('cargo', args, {
      timeout: STORAGE_VERIFY_TIMEOUT_MS,
      maxBuffer: 10 * 1024 * 1024,
    }, (error, stdout, stderr) => {
      if (error) {
        const detail = stderr?.trim() || stdout?.trim() || error.message;
        reject(new Error(`Storage verification failed for epoch ${epochId}: ${detail}`));
        return;
      }

      try {
        resolve(JSON.parse(stdout));
      } catch (parseError) {
        reject(new Error(`Storage verification returned invalid JSON for epoch ${epochId}: ${parseError.message}`));
      }
    });
  });
}

module.exports = {
  getEpochFilePath,
  verifyEpochStorage,
};