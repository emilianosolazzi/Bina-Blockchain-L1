const fs = require('fs');
const path = require('path');
const { execFile } = require('child_process');

// Use the pre-built release binary — never use `cargo run` which serialises
// all calls through the package-cache lock and spawns hundreds of processes
// when finalizePendingEpochs() iterates many epochs in one sweep.
const RUST_RELEASE_DIR = path.resolve(__dirname, '..', 'rust', 'target', 'release');
const ATTESTATION_BIN = path.join(RUST_RELEASE_DIR, 'tg-storage-attestation.exe');
const STORAGE_PROVIDER = process.env.STORAGE_VERIFIER_PROVIDER || 'epoch-store-local';
const STORAGE_VERIFY_TIMEOUT_MS = Number(process.env.STORAGE_VERIFY_TIMEOUT_MS || 30000);

function getEpochFilePath(epochId) {
  return path.resolve(process.env.EPOCH_STORE || path.resolve(__dirname, 'epoch-store'), `epoch-${epochId}.json`);
}

function verifyEpochStorage(epochId) {
  const epochFile = getEpochFilePath(epochId);
  if (!fs.existsSync(epochFile)) {
    return Promise.reject(new Error(`Epoch file not found: ${epochFile}`));
  }
  if (!fs.existsSync(ATTESTATION_BIN)) {
    return Promise.reject(new Error(`tg-storage-attestation binary not found at ${ATTESTATION_BIN} — run: cargo build --release -p temporal-gradient-miner-installer`));
  }

  const args = [
    '--epoch-file', epochFile,
    '--provider', STORAGE_PROVIDER,
  ];

  return new Promise((resolve, reject) => {
    execFile(ATTESTATION_BIN, args, {
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