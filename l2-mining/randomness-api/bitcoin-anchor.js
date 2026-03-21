const { execFile } = require('child_process');
const path = require('path');
const fs = require('fs');

const RUST_MANIFEST = path.resolve(__dirname, '..', 'rust', 'Cargo.toml');
const COMPILED_BINARY = path.resolve(__dirname, '..', 'rust', 'target', 'release', 'tg-bitcoin-anchor.exe');
const BITCOIN_ANCHOR_TIMEOUT_MS = Number(process.env.BITCOIN_ANCHOR_TIMEOUT_MS || 180000);
const BITCOIN_ANCHOR_PREFERENCE = process.env.BITCOIN_ANCHOR_PREFERENCE || 'op_return';

function anchorEpochMerkleRoot(epochId, merkleRoot, storageReference = null) {
  // Prefer the pre-compiled binary for speed; fall back to cargo run
  const useCompiled = fs.existsSync(COMPILED_BINARY);

  let command, args;
  if (useCompiled) {
    command = COMPILED_BINARY;
    args = [
      '--epoch-id', String(epochId),
      '--merkle-root', String(merkleRoot),
      '--preference', BITCOIN_ANCHOR_PREFERENCE,
    ];
  } else {
    command = 'cargo';
    args = [
      'run', '--quiet',
      '--manifest-path', RUST_MANIFEST,
      '-p', 'temporal-gradient-miner-installer',
      '--bin', 'tg-bitcoin-anchor',
      '--',
      '--epoch-id', String(epochId),
      '--merkle-root', String(merkleRoot),
      '--preference', BITCOIN_ANCHOR_PREFERENCE,
    ];
  }

  if (storageReference) {
    args.push('--storage-ref', String(storageReference));
  }

  return new Promise((resolve, reject) => {
    execFile(command, args, {
      timeout: BITCOIN_ANCHOR_TIMEOUT_MS,
      maxBuffer: 10 * 1024 * 1024,
    }, (error, stdout, stderr) => {
      if (error) {
        const detail = stderr?.trim() || stdout?.trim() || error.message;
        reject(new Error(`Bitcoin anchoring failed for epoch ${epochId}: ${detail}`));
        return;
      }

      try {
        resolve(JSON.parse(stdout));
      } catch (parseError) {
        reject(new Error(`Bitcoin anchoring returned invalid JSON for epoch ${epochId}: ${parseError.message}`));
      }
    });
  });
}

module.exports = {
  anchorEpochMerkleRoot,
};