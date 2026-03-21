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

let inventory = [];
let scanHistory = [];
let lastScan = null;

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

function selectUtxoByEntropy(anchorData) {
  const seed = crypto.createHash('sha256')
    .update(anchorData ? Buffer.from(anchorData) : crypto.randomBytes(32))
    .update(Buffer.from(Date.now().toString()))
    .digest();

  const scored = inventory.map((utxo, idx) => {
    const hash = crypto.createHash('sha256')
      .update(seed)
      .update(Buffer.from(`${utxo.txid}:${utxo.vout || 0}`))
      .digest();
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

// ── Anchor creation (mirrors Rust entropy_anchor_v1) ────

function createAnchor(utxo, anchorData) {
  const utxoId = `${utxo.txid}:${utxo.vout || 0}`;
  const entropy = crypto.randomBytes(32);
  const payload = Buffer.concat([
    Buffer.from(anchorData || 'live-scan'),
    entropy,
    Buffer.from(utxoId),
  ]);
  const dataHash = crypto.createHash('sha256').update(payload).digest('hex');
  const anchorId = crypto.createHash('sha256')
    .update(Buffer.from(utxoId))
    .update(Buffer.from(dataHash))
    .update(Buffer.from(Date.now().toString()))
    .digest('hex');

  return {
    anchorId,
    utxoId,
    dataHash,
    method: 'entropy_anchor_v1',
    entropyHex: entropy.toString('hex'),
    createdAt: Math.floor(Date.now() / 1000),
  };
}

// ── Full 5-step scan ────────────────────────────────────

async function runFullScan(anchorData) {
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
  const selection = selectUtxoByEntropy(anchorData);
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
      candidatesEvaluated: inv.count,
      decodedData,
    },
    durationMs: Date.now() - s2,
  });

  // ── Step 3: Fetch from Bitcoin network ──
  const s3 = Date.now();
  let mempool;
  try {
    mempool = await fetchFromMempool(sel.txid, sel.vout || 0);
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
  const anchor = createAnchor(sel, anchorData);
  steps.push({
    step: 5,
    title: 'Create entropy anchor',
    icon: '🔗',
    status: 'ok',
    detail: `Anchor ${anchor.anchorId.slice(0, 16)}… bound to ${anchor.utxoId.slice(0, 20)}…`,
    data: anchor,
    durationMs: Date.now() - s5,
  });

  return finalise(scanId, steps, scanStart, null, {
    utxoId: anchor.utxoId,
    txid: sel.txid,
    vout: Number(sel.vout || 0),
    type: sel.type,
    blockHeight: Number(sel.block_height || 0),
    anchorId: anchor.anchorId,
    dataHash: anchor.dataHash,
    isDead: dead.isDead,
    deadReason: dead.reason,
    explorerUrl: `https://mempool.space/tx/${sel.txid}`,
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

module.exports = { runFullScan, getLastScan, getScanHistory, getInventoryInfo };
