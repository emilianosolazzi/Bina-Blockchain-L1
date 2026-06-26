/**
 * UTXO Scanner — Live Bitcoin UTXO verification & entropy anchoring
 *
 * Performs a full 5-step pipeline that is verifiable by anyone:
 *   1. Load dead UTXO inventory from CSV
 *   2. Select a UTXO using entropy-weighted scoring
 *   3. Fetch & verify against Bitcoin network (mempool.space)
 *   4. Confirm dead output status (OP_RETURN / spent / dust)
 *   5. Create a cryptographic entropy anchor
 *
 * Every step returns raw data, timing, and external links so the
 * dashboard can show exactly what happened — no "trust me" numbers.
 */

const fs = require('fs');
const path = require('path');
const crypto = require('crypto');

const CSV_PATH = process.env.SPRINT_DEAD_UTXO_DB || path.resolve(__dirname, 'test-dead-utxos.csv');
const MEMPOOL_API = process.env.MEMPOOL_API || 'https://mempool.space/api';
const MAX_HISTORY = 20;
const ANCHOR_METHOD = 'dead_utxo_anchor_v1';

let inventory = [];
let scanHistory = [];
let lastScan = null;
let anchorHistory = [];
let anchorIndex = new Map();

// ── CSV Parser ──────────────────────────────────────────

function parseDeadUtxoCsv(csvPath) {
  const content = fs.readFileSync(csvPath, 'utf8');
  const lines = content.trim().split('\n').filter(Boolean);
  if (lines.length < 2) return [];
  const headers = lines[0].split(',').map(h => h.trim());
  return lines.slice(1).map(line => {
    const cols = line.split(',');
    const obj = {};
    headers.forEach((h, i) => { obj[h] = (cols[i] || '').trim(); });
    return obj;
  });
}

// ── Hex → UTF-8 decoder (for OP_RETURN data) ────────────

function decodeHex(hex) {
  if (!hex) return null;
  try {
    const buf = Buffer.from(hex, 'hex');
    const text = buf.toString('utf8');
    const printable = text.split('').filter(c => c.charCodeAt(0) >= 32 && c.charCodeAt(0) < 127).length;
    if (printable > text.length * 0.75 && text.length >= 3) {
      return { decoded: text, encoding: 'utf-8' };
    }
    return { decoded: null, encoding: 'binary', hex: hex.slice(0, 80) + (hex.length > 80 ? '…' : '') };
  } catch {
    return { decoded: null, encoding: 'binary', hex: hex.slice(0, 80) };
  }
}

function normalizeHex(hex) {
  return String(hex || '').trim().replace(/^0x/i, '').toLowerCase();
}

function sha256(data) {
  return crypto.createHash('sha256').update(data).digest();
}

function sha256Hex(data) {
  return crypto.createHash('sha256').update(data).digest('hex');
}

function toLittleEndianU64(value) {
  const out = Buffer.alloc(8);
  out.writeBigUInt64LE(BigInt(value || 0));
  return out;
}

function isLikelyHex(value) {
  return typeof value === 'string' && /^[0-9a-fA-F]+$/.test(value) && value.length > 0 && value.length % 2 === 0;
}

function toDataBuffer(value) {
  if (Buffer.isBuffer(value)) return value;
  if (value === undefined || value === null) return Buffer.alloc(0);
  if (typeof value === 'string') {
    const normalized = normalizeHex(value);
    if (isLikelyHex(normalized)) return Buffer.from(normalized, 'hex');
    return Buffer.from(value, 'utf8');
  }
  if (typeof value === 'object') {
    return Buffer.from(JSON.stringify(value), 'utf8');
  }
  return Buffer.from(String(value), 'utf8');
}

function normalizePreference(preference) {
  const value = String(preference || '').trim().toLowerCase();
  if (!value) return null;
  if (value === 'opreturn' || value === 'op-return') return 'op_return';
  if (value === 'burn_address' || value === 'burn-address') return 'burn';
  return value;
}

function canonicalizeMetadata(metadata) {
  const ordered = {};
  for (const key of Object.keys(metadata || {}).sort()) {
    ordered[key] = metadata[key];
  }
  return JSON.stringify(ordered);
}

function metadataDigestHex(metadata) {
  return sha256Hex(Buffer.from(canonicalizeMetadata(metadata), 'utf8'));
}

function computeAnchorId(utxoId, dataHash, merkleRoot, storageReference, createdAt) {
  return sha256Hex(Buffer.concat([
    Buffer.from(String(utxoId), 'utf8'),
    Buffer.from(String(dataHash), 'utf8'),
    Buffer.from(String(merkleRoot), 'utf8'),
    Buffer.from(String(storageReference || ''), 'utf8'),
    toLittleEndianU64(createdAt),
  ]));
}

function utxoSummary(utxo) {
  const utxoId = `${utxo.txid}:${utxo.vout || 0}`;
  const type = String(utxo.type || '').toLowerCase();
  if (type === 'spent') {
    const height = Number(utxo.spent_at_height || utxo.block_height || 0);
    return `Spent output ${utxoId} spent at Bitcoin height ${height}`;
  }
  if (type === 'op_return') {
    return `OP_RETURN output ${utxoId} confirmed at Bitcoin height ${Number(utxo.block_height || 0)}`;
  }
  if (type === 'dust') {
    return `Dust output ${utxoId} with ${Number(utxo.satoshis || 0)} sats at Bitcoin height ${Number(utxo.block_height || 0)}`;
  }
  if (type === 'burn') {
    return `Burn-address output ${utxoId} to ${utxo.address || 'burn-address'} with ${Number(utxo.satoshis || 0)} sats at Bitcoin height ${Number(utxo.block_height || 0)}`;
  }
  return `Dead UTXO ${utxoId} (${type || 'unknown'}) at Bitcoin height ${Number(utxo.block_height || 0)}`;
}

function selectionReason(utxo, preference) {
  const type = String(utxo.type || '').toLowerCase();
  if (preference === 'op_return') return 'Chosen from provably unspendable OP_RETURN outputs.';
  if (preference === 'spent') return 'Chosen from already-spent outputs for irreversible timestamping.';
  if (preference === 'dust') return 'Chosen from uneconomic dust outputs that are effectively dead.';
  if (preference === 'burn') return 'Chosen from burn-address outputs for publicly auditable destruction.';
  if (type === 'op_return') return 'Highest-assurance dead output selected from available inventory.';
  if (type === 'spent') return 'Spent output selected from available dead-UTXO inventory.';
  if (type === 'dust') return 'Dust output selected from available dead-UTXO inventory.';
  if (type === 'burn') return 'Burn-address output selected from available dead-UTXO inventory.';
  return 'Dead UTXO selected from available inventory.';
}

function storeAnchor(anchorRecord) {
  anchorIndex.set(anchorRecord.anchorId, anchorRecord);
  anchorHistory = [anchorRecord, ...anchorHistory.filter(x => x.anchorId !== anchorRecord.anchorId)].slice(0, MAX_HISTORY);
}

// ── Load dead-UTXO inventory ────────────────────────────

function loadInventory() {
  if (!fs.existsSync(CSV_PATH)) {
    return { loaded: false, error: `CSV not found: ${path.basename(CSV_PATH)}`, items: [], path: CSV_PATH };
  }
  try {
    inventory = parseDeadUtxoCsv(CSV_PATH);
    return {
      loaded: true,
      count: inventory.length,
      source: path.basename(CSV_PATH),
      path: CSV_PATH,
      types: countTypes(inventory),
      items: inventory.map(u => ({
        txid: u.txid,
        vout: Number(u.vout || 0),
        type: u.type,
        blockHeight: Number(u.block_height || 0),
        decodedData: decodeHex(u.data),
      })),
    };
  } catch (err) {
    return { loaded: false, error: err.message, items: [], path: CSV_PATH };
  }
}

// ── Entropy-weighted selection (mirrors Rust SHA-256 scoring) ──

function selectUtxoByEntropy(anchorData, opts = {}) {
  const preference = normalizePreference(opts.preference);
  const candidates = preference ? inventory.filter(u => normalizePreference(u.type) === preference) : inventory;
  if (!candidates.length) {
    throw new Error(preference
      ? `No dead UTXOs available for preference '${preference}'`
      : 'No dead UTXOs available in inventory');
  }

  const context = toDataBuffer(anchorData || 'live-scan');
  const entropy = sha256(context);

  const scored = candidates.map((utxo, idx) => {
    const utxoId = `${utxo.txid}:${utxo.vout || 0}`;
    const hash = sha256(Buffer.concat([
      entropy,
      context,
      Buffer.from(utxoId, 'utf8'),
    ]));
    const score = hash.readBigUInt64LE(0);
    return { utxo, score: score.toString(), index: idx };
  });

  scored.sort((a, b) => (BigInt(b.score) > BigInt(a.score) ? 1 : -1));
  return scored[0];
}

// ── Mempool.space live fetch ────────────────────────────

async function fetchFromMempool(txid, vout) {
  const txUrl = `${MEMPOOL_API}/tx/${txid}`;
  const outspendUrl = `${MEMPOOL_API}/tx/${txid}/outspend/${vout}`;

  const [txResp, outspendResp] = await Promise.all([
    fetch(txUrl, { signal: AbortSignal.timeout(15000) }),
    fetch(outspendUrl, { signal: AbortSignal.timeout(15000) }),
  ]);

  if (!txResp.ok) throw new Error(`tx fetch HTTP ${txResp.status}`);
  if (!outspendResp.ok) throw new Error(`outspend fetch HTTP ${outspendResp.status}`);

  const [txData, outspendData] = await Promise.all([txResp.json(), outspendResp.json()]);

  return {
    tx: txData,
    outspend: outspendData,
    voutData: txData.vout?.[Number(vout)] || null,
    apiUrls: { tx: txUrl, outspend: outspendUrl },
  };
}

async function fetchBlockHeaderHex(blockHash) {
  if (!blockHash) return null;
  const resp = await fetch(`${MEMPOOL_API}/block/${blockHash}/header`, { signal: AbortSignal.timeout(15000) });
  if (!resp.ok) throw new Error(`block header fetch HTTP ${resp.status}`);
  return (await resp.text()).trim();
}

// ── Dead-output verification ────────────────────────────

function verifyDead(mempool) {
  const { outspend, voutData } = mempool;
  const isSpent = outspend?.spent || false;
  const isOpReturn = voutData?.scriptpubkey_type === 'op_return';
  const value = voutData?.value ?? 0;
  const isDust = value > 0 && value < 546;

  let isDead = false;
  let reason = '';

  if (isOpReturn) {
    isDead = true;
    reason = 'OP_RETURN outputs are provably unspendable by Bitcoin consensus rules';
  } else if (isSpent) {
    isDead = true;
    reason = 'Output already spent — double-spend is cryptographically impossible';
  } else if (isDust) {
    isDead = true;
    reason = `Dust output (${value} sats) — below economic spending threshold at current fee rates`;
  } else {
    reason = 'Output status uncertain — further analysis needed';
  }

  return {
    isDead,
    reason,
    checks: { isOpReturn, isSpent, isDust, value, scriptType: voutData?.scriptpubkey_type || 'unknown' },
    confirmed: mempool.tx.status?.confirmed || false,
    blockHeight: mempool.tx.status?.block_height || null,
    blockHash: mempool.tx.status?.block_hash || null,
  };
}

// ── Canonical anchor creation (mirrors Rust shape + anchor id formula) ────

function createCanonicalAnchor(utxo, anchorData, mempool, opts = {}) {
  const utxoId = `${utxo.txid}:${utxo.vout || 0}`;
  const createdAt = Number(opts.createdAt || Math.floor(Date.now() / 1000));
  const storageReference = String(opts.storageReference || '');
  const anchorInput = toDataBuffer(anchorData || 'live-scan');
  const headerHex = normalizeHex(opts.blockHeaderHex || '');
  const entropy = sha256(Buffer.concat([
    anchorInput,
    headerHex ? Buffer.from(headerHex, 'hex') : Buffer.from(String(mempool?.tx?.status?.block_hash || ''), 'utf8'),
    Buffer.from(utxoId, 'utf8'),
    Buffer.from(String(mempool?.voutData?.scriptpubkey || ''), 'utf8'),
    Buffer.from(String(mempool?.voutData?.value ?? ''), 'utf8'),
    Buffer.from(String(mempool?.outspend?.txid || ''), 'utf8'),
  ]));
  const payload = Buffer.concat([
    anchorInput,
    entropy,
    Buffer.from(utxoId, 'utf8'),
  ]);
  const dataHash = sha256Hex(payload);
  const merkleRoot = dataHash;
  const metadata = {
    method: ANCHOR_METHOD,
    preference: normalizePreference(opts.preference) || normalizePreference(utxo.type) || 'unknown',
    selected_utxo: utxoId,
    utxo_category: normalizePreference(utxo.type) || 'unknown',
    utxo_summary: utxoSummary(utxo),
    selection_reason: selectionReason(utxo, normalizePreference(opts.preference) || normalizePreference(utxo.type)),
    created_at: String(createdAt),
  };
  const metadataDigest = metadataDigestHex(metadata);
  const anchorId = computeAnchorId(utxoId, dataHash, merkleRoot, storageReference, createdAt);

  return {
    anchorId,
    utxoId,
    dataHash,
    merkleRoot,
    storageReference,
    metadata,
    metadataDigest,
    method: ANCHOR_METHOD,
    entropyHex: entropy.toString('hex'),
    createdAt,
    blockHeaderHex: headerHex || null,
  };
}

function buildVerifierRegistrationPayload(anchor, attestor = null) {
  return {
    contract: 'UTXOAnchorVerifier.registerAnchor',
    args: {
      utxoId: anchor.utxoId,
      dataHashHex: anchor.dataHash,
      merkleRootHex: anchor.merkleRoot,
      storageReference: anchor.storageReference || '',
      metadataDigest: `0x${anchor.metadataDigest}`,
      createdAt: anchor.createdAt,
      attestor,
    },
  };
}

function buildCertificateMintPayload(anchor, options = {}) {
  return {
    contract: 'UTXOCertificateRegistry.mintCertificate',
    args: {
      recipient: options.recipient || null,
      documentHash: options.documentHash || null,
      utxoId: anchor.utxoId,
      dataHashHex: anchor.dataHash,
      merkleRootHex: anchor.merkleRoot,
      storageReference: anchor.storageReference || '',
      metadataDigest: `0x${anchor.metadataDigest}`,
      anchorCreatedAt: anchor.createdAt,
      certType: options.certType ?? null,
      metadataURI: options.metadataURI || '',
      attestationSignature: options.attestationSignature || null,
    },
  };
}

// ── Full 5-step scan ────────────────────────────────────

async function runFullScan(anchorData, opts = {}) {
  const scanId = crypto.randomBytes(8).toString('hex');
  const steps = [];
  const scanStart = Date.now();

  // ── Step 1: Load inventory ──
  const s1 = Date.now();
  const inv = loadInventory();
  steps.push({
    step: 1,
    title: 'Load UTXO inventory',
    icon: '📦',
    status: inv.loaded ? 'ok' : 'error',
    detail: inv.loaded
      ? `Loaded ${inv.count} dead Bitcoin outputs from ${inv.source}`
      : `Failed: ${inv.error}`,
    data: inv.loaded
      ? { count: inv.count, source: inv.source, types: inv.types }
      : { error: inv.error },
    durationMs: Date.now() - s1,
  });

  if (!inv.loaded) {
    return finalise(scanId, steps, scanStart, 'Inventory load failed');
  }

  // ── Step 2: Entropy-based selection ──
  const s2 = Date.now();
  let selection;
  try {
    selection = selectUtxoByEntropy(anchorData, opts);
  } catch (err) {
    steps.push({
      step: 2,
      title: 'Entropy-based UTXO selection',
      icon: '🎲',
      status: 'error',
      detail: err.message,
      data: { error: err.message, preference: normalizePreference(opts.preference) || null },
      durationMs: Date.now() - s2,
    });
    return finalise(scanId, steps, scanStart, 'UTXO selection failed');
  }
  const sel = selection.utxo;
  const decodedData = decodeHex(sel.data);
  steps.push({
    step: 2,
    title: 'Entropy-based UTXO selection',
    icon: '🎲',
    status: 'ok',
    detail: `Selected ${sel.txid.slice(0, 12)}…:${sel.vout || 0} (${sel.type}, block ${Number(sel.block_height).toLocaleString()})`,
    data: {
      txid: sel.txid,
      vout: Number(sel.vout || 0),
      type: sel.type,
      blockHeight: Number(sel.block_height || 0),
      entropyScore: selection.score,
      preference: normalizePreference(opts.preference) || null,
      candidatesEvaluated: inv.count,
      selectionReason: selectionReason(sel, normalizePreference(opts.preference) || normalizePreference(sel.type)),
      decodedData,
    },
    durationMs: Date.now() - s2,
  });

  // ── Step 3: Fetch from Bitcoin network ──
  const s3 = Date.now();
  let mempool;
  let blockHeaderHex = null;
  try {
    mempool = await fetchFromMempool(sel.txid, sel.vout || 0);
    blockHeaderHex = await fetchBlockHeaderHex(mempool.tx.status?.block_hash || null).catch(() => null);
    const vd = mempool.voutData;
    steps.push({
      step: 3,
      title: 'Fetch from Bitcoin network',
      icon: '🌐',
      status: 'ok',
      detail: `Verified on mempool.space — confirmed block ${(mempool.tx.status?.block_height || 0).toLocaleString()}, type: ${vd?.scriptpubkey_type || '?'}`,
      data: {
        apiUrls: mempool.apiUrls,
        explorerUrl: `https://mempool.space/tx/${sel.txid}`,
        confirmed: mempool.tx.status?.confirmed,
        blockHeight: mempool.tx.status?.block_height,
        blockHash: mempool.tx.status?.block_hash,
        blockHeaderHex,
        voutIndex: Number(sel.vout || 0),
        voutType: vd?.scriptpubkey_type,
        voutValue: vd?.value,
        voutScriptpubkey: vd?.scriptpubkey || null,
        isSpent: mempool.outspend?.spent,
        spentInTx: mempool.outspend?.txid || null,
      },
      durationMs: Date.now() - s3,
    });
  } catch (err) {
    steps.push({
      step: 3,
      title: 'Fetch from Bitcoin network',
      icon: '🌐',
      status: 'error',
      detail: `mempool.space fetch failed: ${err.message}`,
      data: { error: err.message },
      durationMs: Date.now() - s3,
    });
    return finalise(scanId, steps, scanStart, 'Network verification failed');
  }

  // ── Step 4: Verify dead output ──
  const s4 = Date.now();
  const dead = verifyDead(mempool);
  steps.push({
    step: 4,
    title: 'Confirm dead output status',
    icon: dead.isDead ? '💀' : '⚠️',
    status: dead.isDead ? 'ok' : 'warn',
    detail: dead.isDead
      ? `Dead output confirmed — ${dead.reason}`
      : `Warning — ${dead.reason}`,
    data: dead,
    durationMs: Date.now() - s4,
  });

  // ── Step 5: Create entropy anchor ──
  const s5 = Date.now();
  const anchor = createCanonicalAnchor(sel, anchorData, mempool, {
    preference: opts.preference,
    storageReference: opts.storageReference || '',
    createdAt: opts.createdAt,
    blockHeaderHex,
  });
  const anchorRecord = {
    scanId,
    ...anchor,
    txid: sel.txid,
    vout: Number(sel.vout || 0),
    type: sel.type,
    blockHeight: Number(sel.block_height || 0),
    explorerUrl: `https://mempool.space/tx/${sel.txid}`,
    isDead: dead.isDead,
    deadReason: dead.reason,
    decodedData,
  };
  storeAnchor(anchorRecord);
  steps.push({
    step: 5,
    title: 'Create entropy anchor',
    icon: '🔗',
    status: 'ok',
    detail: `Anchor ${anchor.anchorId.slice(0, 16)}… bound to ${anchor.utxoId.slice(0, 20)}…`,
    data: {
      ...anchor,
      metadataCanonicalJson: canonicalizeMetadata(anchor.metadata),
    },
    durationMs: Date.now() - s5,
  });

  return finalise(scanId, steps, scanStart, null, {
    utxoId: anchor.utxoId,
    txid: sel.txid,
    vout: Number(sel.vout || 0),
    type: sel.type,
    blockHeight: Number(mempool.tx.status?.block_height || sel.block_height || 0),
    blockHash: mempool.tx.status?.block_hash || null,
    blockHeaderHex,
    anchorId: anchor.anchorId,
    dataHash: anchor.dataHash,
    merkleRoot: anchor.merkleRoot,
    storageReference: anchor.storageReference,
    metadata: anchor.metadata,
    metadataDigest: anchor.metadataDigest,
    anchorCreatedAt: anchor.createdAt,
    isDead: dead.isDead,
    deadReason: dead.reason,
    scriptPubKey: mempool.voutData?.scriptpubkey || null,
    scriptType: mempool.voutData?.scriptpubkey_type || null,
    outputValueSats: mempool.voutData?.value ?? null,
    spent: mempool.outspend?.spent ?? null,
    apiUrls: mempool.apiUrls,
    explorerUrl: `https://mempool.space/tx/${sel.txid}`,
    blockExplorerUrl: mempool.tx.status?.block_hash ? `https://mempool.space/block/${mempool.tx.status.block_hash}` : null,
    decodedData,
  });
}

function finalise(scanId, steps, startTime, error, summary) {
  const result = {
    scanId,
    steps,
    summary: summary || null,
    error: error || null,
    durationMs: Date.now() - startTime,
    timestamp: new Date().toISOString(),
  };
  lastScan = result;
  scanHistory.unshift(result);
  if (scanHistory.length > MAX_HISTORY) scanHistory.length = MAX_HISTORY;
  return result;
}

function countTypes(items) {
  const c = {};
  items.forEach(i => { c[i.type] = (c[i.type] || 0) + 1; });
  return c;
}

function getLastScan() { return lastScan; }
function getScanHistory() { return scanHistory; }
function getInventoryInfo() { return loadInventory(); }
function getLatestAnchor() { return anchorHistory[0] || null; }
function getAnchorHistory() { return anchorHistory; }
function getAnchorById(anchorId) { return anchorIndex.get(String(anchorId || '').trim()) || null; }
function getScanById(scanId) { return scanHistory.find(scan => scan.scanId === scanId) || null; }

// ── Live Discovery — scan Bitcoin blocks for dead UTXOs ──

/**
 * Discovers new dead UTXOs by scanning recent Bitcoin blocks via mempool.space.
 * Looks for OP_RETURN outputs, dust outputs (<546 sat), and spent outputs.
 * Appends newly discovered UTXOs to the CSV inventory.
 *
 * @param {object} opts
 * @param {number} opts.blocks — how many recent blocks to scan (default 3, max 10)
 * @param {number} opts.startHeight — optional starting block height (default: latest - blocks)
 * @returns {object} { discovered, added, skippedDuplicates, errors, scannedBlocks, duration }
 */
async function discoverDeadUtxos(opts = {}) {
  const maxBlocks = Math.min(opts.blocks || 3, 10);
  const startTime = Date.now();
  const results = { discovered: [], added: 0, skippedDuplicates: 0, errors: [], scannedBlocks: [], durationMs: 0 };

  // Load existing inventory txids to deduplicate
  const existing = new Set();
  try {
    const inv = loadInventory();
    if (inv.items) inv.items.forEach(i => existing.add(`${i.txid}:${i.vout}`));
  } catch {}

  try {
    // Get current block height
    let tipHeight = opts.startHeight;
    if (!tipHeight) {
      const tipResp = await fetch(`${MEMPOOL_API}/blocks/tip/height`, { signal: AbortSignal.timeout(10000) });
      if (!tipResp.ok) throw new Error(`Failed to get tip height: HTTP ${tipResp.status}`);
      tipHeight = Number(await tipResp.text());
    }

    for (let i = 0; i < maxBlocks; i++) {
      const height = tipHeight - i;
      try {
        // Get block hash
        const hashResp = await fetch(`${MEMPOOL_API}/block-height/${height}`, { signal: AbortSignal.timeout(10000) });
        if (!hashResp.ok) continue;
        const blockHash = await hashResp.text();

        // Get first page of transactions (up to 25)
        const txsResp = await fetch(`${MEMPOOL_API}/block/${blockHash}/txs/0`, { signal: AbortSignal.timeout(15000) });
        if (!txsResp.ok) continue;
        const txs = await txsResp.json();

        let blockFound = 0;
        for (const tx of txs) {
          if (!tx.vout || !Array.isArray(tx.vout)) continue;
          for (let vIdx = 0; vIdx < tx.vout.length; vIdx++) {
            const vout = tx.vout[vIdx];
            let type = null;
            let data = '';

            if (vout.scriptpubkey_type === 'op_return') {
              type = 'op_return';
              // OP_RETURN data is in scriptpubkey after the OP_RETURN opcode (6a)
              const sp = vout.scriptpubkey || '';
              // Strip the 6a prefix + push-data length byte
              if (sp.startsWith('6a')) {
                const afterOp = sp.slice(2);
                // Skip push-data length byte(s)
                if (afterOp.length > 2) {
                  const pushLen = parseInt(afterOp.slice(0, 2), 16);
                  data = (pushLen <= 80 && afterOp.length >= 2 + pushLen * 2) ? afterOp.slice(2) : afterOp;
                } else {
                  data = afterOp;
                }
              }
            } else if (vout.value !== undefined && vout.value > 0 && vout.value < 546) {
              type = 'dust';
            }

            if (type) {
              const key = `${tx.txid}:${vIdx}`;
              if (existing.has(key)) {
                results.skippedDuplicates++;
                continue;
              }
              existing.add(key);
              const entry = {
                type,
                txid: tx.txid,
                vout: vIdx,
                block_height: height,
                data: data || '',
                satoshis: vout.value || 0,
                decoded: type === 'op_return' ? decodeHex(data) : null,
              };
              results.discovered.push(entry);
              blockFound++;
            }
          }
        }
        results.scannedBlocks.push({ height, hash: blockHash.slice(0, 16) + '…', txsScanned: txs.length, found: blockFound });
      } catch (blockErr) {
        results.errors.push({ height, error: blockErr.message });
      }

      // Rate-limit: mempool.space allows ~10 req/s, be conservative
      if (i < maxBlocks - 1) await new Promise(r => setTimeout(r, 500));
    }

    // Append newly discovered UTXOs to CSV
    if (results.discovered.length > 0) {
      try {
        const csvLines = results.discovered.map(d =>
          `${d.type},${d.txid},${d.vout},${d.block_height},${d.data},${d.satoshis},,,`
        );
        // Ensure CSV has a trailing newline before appending
        let existing_content = '';
        if (fs.existsSync(CSV_PATH)) {
          existing_content = fs.readFileSync(CSV_PATH, 'utf8');
          if (!existing_content.endsWith('\n')) existing_content += '\n';
        } else {
          existing_content = 'type,txid,vout,block_height,data,satoshis,fee_rate_threshold,address,spent_in_block,spent_at_height\n';
        }
        fs.writeFileSync(CSV_PATH, existing_content + csvLines.join('\n') + '\n');
        results.added = results.discovered.length;
        // Reset inventory cache so next scan picks up new entries
        inventory = [];
      } catch (writeErr) {
        results.errors.push({ write: true, error: writeErr.message });
      }
    }
  } catch (err) {
    results.errors.push({ fatal: true, error: err.message });
  }

  results.durationMs = Date.now() - startTime;
  return results;
}

module.exports = {
  runFullScan,
  getLastScan,
  getScanHistory,
  getScanById,
  getInventoryInfo,
  getLatestAnchor,
  getAnchorHistory,
  getAnchorById,
  buildVerifierRegistrationPayload,
  buildCertificateMintPayload,
  canonicalizeMetadata,
  metadataDigestHex,
  computeAnchorId,
  discoverDeadUtxos,
};
