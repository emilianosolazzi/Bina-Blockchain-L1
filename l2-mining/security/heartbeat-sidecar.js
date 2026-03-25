#!/usr/bin/env node
'use strict';

const fs = require('fs');
const http = require('http');
const path = require('path');
const crypto = require('crypto');
const { URL } = require('url');

const ROOT = __dirname;

function readBoundedNumber(name, fallback, min, max) {
  const raw = process.env[name];
  if (raw == null || raw === '') return fallback;
  const value = Number(raw);
  if (!Number.isFinite(value)) return fallback;
  const normalized = Math.trunc(value);
  if (normalized < min || normalized > max) return fallback;
  return normalized;
}

const TELEMETRY_FILE = process.env.TELEMETRY_FILE
  ? path.resolve(process.cwd(), process.env.TELEMETRY_FILE)
  : (process.env.LOCALAPPDATA
    ? path.join(process.env.LOCALAPPDATA, 'entropy', 'TemporalGradientMiner', 'data', 'logs', 'telemetry.jsonl')
    : path.resolve(ROOT, '..', 'rust', 'miner-telemetry.jsonl'));
const HOST = process.env.HEARTBEAT_HOST || '127.0.0.1';
const PORT = readBoundedNumber('HEARTBEAT_PORT', 4380, 1, 65535);
const POLL_INTERVAL_MS = readBoundedNumber('HEARTBEAT_POLL_INTERVAL_MS', 2000, 250, 300000);
const TELEMETRY_STALE_MS = readBoundedNumber('HEARTBEAT_TELEMETRY_STALE_MS', 15000, 1000, 3600000);
const MIN_SOLUTION_GAP_MS = readBoundedNumber('HEARTBEAT_MIN_SOLUTION_GAP_MS', 30000, 1000, 86400000);
const MAX_TAIL_BYTES = readBoundedNumber('HEARTBEAT_MAX_TAIL_BYTES', 1024 * 512, 4096, 8 * 1024 * 1024);
const MAX_HISTORY_LINES = readBoundedNumber('HEARTBEAT_MAX_HISTORY_LINES', 720, 10, 10000);
const HASHRATE_BASELINE_SAMPLES = readBoundedNumber('HEARTBEAT_HASHRATE_BASELINE_SAMPLES', 60, 5, 1000);
const HASHRATE_DROP_RATIO = Number(process.env.HEARTBEAT_HASHRATE_DROP_RATIO || 0.45);
const TEMPERATURE_WARN_C = readBoundedNumber('HEARTBEAT_TEMPERATURE_WARN_C', 82, 20, 150);
const REJECT_BURST_THRESHOLD = readBoundedNumber('HEARTBEAT_REJECT_BURST_THRESHOLD', 1, 1, 1000);
const MINER_NAME = process.env.HEARTBEAT_MINER_NAME || process.env.MINER_NAME || 'default-miner';
const MINER_REGION = process.env.HEARTBEAT_MINER_REGION || 'unknown';
const MINER_OPERATOR = process.env.HEARTBEAT_MINER_OPERATOR || null;
const MAX_ALERT_HISTORY = readBoundedNumber('HEARTBEAT_MAX_ALERT_HISTORY', 200, 10, 5000);
const RANSOMWARE_STATUS_FILE = process.env.RANSOMWARE_STATUS_FILE
  ? path.resolve(process.cwd(), process.env.RANSOMWARE_STATUS_FILE)
  : path.join(path.dirname(TELEMETRY_FILE), 'ransomware-status.json');
const RANSOMWARE_EVIDENCE_DIR = process.env.RANSOMWARE_EVIDENCE_DIR
  ? path.resolve(process.cwd(), process.env.RANSOMWARE_EVIDENCE_DIR)
  : path.join(path.dirname(TELEMETRY_FILE), 'ransomware-evidence');
const RANSOMWARE_MAX_SCAN_FILES = readBoundedNumber('RANSOMWARE_MAX_SCAN_FILES', 1024, 32, 10000);
const RANSOMWARE_MAX_SCAN_DEPTH = readBoundedNumber('RANSOMWARE_MAX_SCAN_DEPTH', 4, 1, 16);
const RANSOMWARE_ENCRYPTED_FILE_THRESHOLD = readBoundedNumber('RANSOMWARE_ENCRYPTED_FILE_THRESHOLD', 3, 1, 1000);
const MINER_DATA_ROOT = process.env.LOCALAPPDATA
  ? path.join(process.env.LOCALAPPDATA, 'entropy', 'TemporalGradientMiner', 'data')
  : path.dirname(TELEMETRY_FILE);
const MINER_CONFIG_ROOT = process.env.APPDATA
  ? path.join(process.env.APPDATA, 'entropy', 'TemporalGradientMiner', 'config')
  : null;

const RANSOM_NOTE_PATTERNS = [
  /readme/i,
  /decrypt/i,
  /recover/i,
  /restore/i,
  /ransom/i,
  /how_to/i,
  /payment/i,
];

const RANSOMWARE_EXTENSIONS = new Set([
  '.encrypted', '.encrypt', '.crypted', '.crypt', '.enc', '.lockbit', '.locked',
  '.ryk', '.conti', '.zepto', '.wnry', '.cerber', '.clop', '.akira', '.pay',
]);

const RANSOMWARE_IGNORED_FILES = new Set([
  'telemetry.jsonl',
  'miner-control.json',
  'miner-trust-seal.json',
  'ransomware-status.json',
  'heartbeat-sidecar.out.log',
  'heartbeat-sidecar.err.log',
]);

const RANSOMWARE_IGNORED_DIRS = new Set([
  'ransomware-evidence',
  'node_modules',
  '.git',
  'target',
]);

const RANSOMWARE_IGNORED_EXTENSIONS = new Set([
  '.log',
  '.jsonl',
  '.tmp',
  '.part',
]);

const PROTECTED_ASSET_BASENAMES = new Set([
  'miner-config.json',
  'temporal-gradient-miner.exe',
  'miner.key',
  'telemetry.jsonl',
  'miner-control.json',
  'miner-trust-seal.json',
]);
const MAX_RANSOMWARE_EVENTS = readBoundedNumber('RANSOMWARE_MAX_EVENTS', 20, 5, 500);

const sseClients = new Set();
const activeAlerts = new Map();
const alertHistory = [];
let status = buildEmptyStatus('Waiting for telemetry…');
let lastPublishedDigest = '';
let nextAlertId = 1;
let ransomwareBaseline = new Map();
let lastRansomwareFingerprint = '';
let lastRansomwareEvidencePath = null;
let lastRansomwareSignalFingerprint = '';
const ransomwareEventHistory = [];

function nowIso() {
  return new Date().toISOString();
}

function buildEmptyStatus(message) {
  return {
    service: 'heartbeat-sidecar',
    status: 'starting',
    message,
    generatedAt: nowIso(),
    miner: {
      name: MINER_NAME,
      region: MINER_REGION,
      operator: MINER_OPERATOR,
      telemetryFile: TELEMETRY_FILE,
    },
    heartbeat: {
      online: false,
      telemetryFresh: false,
      lastTelemetryAt: null,
      telemetryAgeMs: null,
      lastSolutionAt: null,
      solutionGapMs: null,
      targetGapMs: MIN_SOLUTION_GAP_MS,
      averageSolutionIntervalMs: null,
      acceptedSubmissions: 0,
      rejectedSubmissions: 0,
      hashrateHs: 0,
      baselineHashrateHs: 0,
      hashrateRatio: null,
      workerCount: 0,
      temperatureC: null,
      state: null,
      phase: null,
      lastSolutionHash: null,
      fingerprint: null,
    },
    security: {
      suspicious: false,
      intrusionScore: 0,
      narrative: 'No telemetry yet.',
      activeAlerts: [],
      ransomware: {
        active: false,
        status: 'clear',
        reason: null,
        detectedAtUnixMs: null,
        detectedAt: null,
        evidencePath: null,
        indicators: [],
        scannedFiles: 0,
        protectedRoots: [],
        protectedAssets: [...PROTECTED_ASSET_BASENAMES],
        truncated: false,
        recentEvents: [],
      },
    },
    telemetry: {
      snapshotsAnalyzed: 0,
      fileExists: false,
      fileSizeBytes: 0,
      lastReadAt: nowIso(),
    },
  };
}

function safeJsonParse(line) {
  try {
    return JSON.parse(line);
  } catch {
    return null;
  }
}

function readTailLines(filePath, maxBytes, maxLines) {
  if (!fs.existsSync(filePath)) {
    return { lines: [], size: 0 };
  }

  const stats = fs.statSync(filePath);
  const size = stats.size;
  const readBytes = Math.min(size, maxBytes);
  if (readBytes <= 0) {
    return { lines: [], size };
  }

  const fd = fs.openSync(filePath, 'r');
  try {
    const buffer = Buffer.alloc(readBytes);
    fs.readSync(fd, buffer, 0, readBytes, size - readBytes);
    const text = buffer.toString('utf8');
    const lines = text
      .split(/\r?\n/)
      .map(line => line.trim())
      .filter(Boolean)
      .slice(-maxLines);
    return { lines, size };
  } finally {
    fs.closeSync(fd);
  }
}

function uniquePaths(values) {
  return [...new Set(values.filter(Boolean).map(value => path.resolve(value)))];
}

function protectedRoots() {
  return uniquePaths([
    MINER_CONFIG_ROOT,
    path.join(MINER_DATA_ROOT, 'bin'),
    path.join(MINER_DATA_ROOT, 'keys'),
    path.dirname(TELEMETRY_FILE),
  ]).filter(root => fs.existsSync(root));
}

function shouldIgnoreProtectedPath(filePath) {
  const base = path.basename(filePath).toLowerCase();
  const ext = path.extname(base).toLowerCase();
  if (RANSOMWARE_IGNORED_FILES.has(base)) return true;
  if (RANSOMWARE_IGNORED_EXTENSIONS.has(ext)) return true;
  return false;
}

function walkProtectedFiles(root, files, state, depth = 0) {
  if (!root || state.truncated || depth > RANSOMWARE_MAX_SCAN_DEPTH) return;
  let entries = [];
  try {
    entries = fs.readdirSync(root, { withFileTypes: true });
  } catch {
    return;
  }

  for (const entry of entries) {
    if (state.truncated) break;
    const fullPath = path.join(root, entry.name);
    const lowerName = entry.name.toLowerCase();
    if (entry.isDirectory()) {
      if (RANSOMWARE_IGNORED_DIRS.has(lowerName)) continue;
      walkProtectedFiles(fullPath, files, state, depth + 1);
      continue;
    }
    if (!entry.isFile()) continue;
    if (shouldIgnoreProtectedPath(fullPath)) continue;
    files.push(fullPath);
    if (files.length >= RANSOMWARE_MAX_SCAN_FILES) {
      state.truncated = true;
      break;
    }
  }
}

function snapshotProtectedFiles() {
  const files = [];
  const roots = protectedRoots();
  const state = { truncated: false };
  for (const root of roots) {
    walkProtectedFiles(root, files, state, 0);
    if (state.truncated) break;
  }

  const snapshot = new Map();
  for (const filePath of files) {
    try {
      const stats = fs.statSync(filePath);
      snapshot.set(path.resolve(filePath), {
        path: path.resolve(filePath),
        base: path.basename(filePath),
        ext: path.extname(filePath).toLowerCase(),
        size: stats.size,
        mtimeMs: Math.trunc(stats.mtimeMs),
      });
    } catch {
    }
  }

  return {
    snapshot,
    roots,
    scannedFiles: snapshot.size,
    truncated: state.truncated,
  };
}

function isRansomNoteName(fileName) {
  return RANSOM_NOTE_PATTERNS.some(pattern => pattern.test(fileName));
}

function buildRansomwareReason(indicators) {
  if (!indicators.length) return null;
  const note = indicators.find(item => item.type === 'ransom_note');
  if (note) {
    return `Ransom note pattern detected at ${note.relativePath}`;
  }
  return `${indicators.length} suspicious encrypted files appeared inside protected miner paths`;
}

function isProtectedAssetName(fileName) {
  return PROTECTED_ASSET_BASENAMES.has(String(fileName || '').toLowerCase());
}

function summarizeIndicators(indicators) {
  return (Array.isArray(indicators) ? indicators : []).slice(0, 6).map(item => ({
    type: item.type,
    fileName: item.fileName,
    relativePath: item.relativePath,
  }));
}

function rememberRansomwareObservation({ active, reason, indicators, evidencePath }) {
  const normalizedIndicators = summarizeIndicators(indicators);
  const fingerprint = normalizedIndicators.length
    ? crypto.createHash('sha1').update(JSON.stringify({ active, reason, normalizedIndicators })).digest('hex')
    : '';
  if (!fingerprint || fingerprint === lastRansomwareSignalFingerprint) {
    return;
  }
  lastRansomwareSignalFingerprint = fingerprint;
  ransomwareEventHistory.unshift({
    timestamp: nowIso(),
    active,
    escalation: active ? 'escalated' : 'observed_only',
    reason: reason || (active ? 'Compound ransomware signal detected.' : 'Observed suspicious pattern did not meet escalation threshold.'),
    evidencePath: evidencePath || null,
    indicators: normalizedIndicators,
  });
  if (ransomwareEventHistory.length > MAX_RANSOMWARE_EVENTS) {
    ransomwareEventHistory.length = MAX_RANSOMWARE_EVENTS;
  }
}

function isEncryptedProtectedAsset(meta) {
  const base = String(meta?.base || '');
  const ext = String(meta?.ext || '').toLowerCase();
  if (!RANSOMWARE_EXTENSIONS.has(ext)) return false;
  const lower = base.toLowerCase();
  return [...PROTECTED_ASSET_BASENAMES].some(asset => lower === `${asset}${ext}` || lower.startsWith(`${asset}.`));
}

function writeRansomwareEvidence(result) {
  try {
    fs.mkdirSync(RANSOMWARE_EVIDENCE_DIR, { recursive: true });
    const evidencePath = path.join(RANSOMWARE_EVIDENCE_DIR, `ransomware-${result.detectedAtUnixMs}.json`);
    fs.writeFileSync(evidencePath, JSON.stringify({
      version: 1,
      createdAt: nowIso(),
      detector: 'heartbeat-sidecar',
      ransomware: result,
    }, null, 2));
    return evidencePath;
  } catch {
    return null;
  }
}

function persistRansomwareStatus(result) {
  try {
    fs.mkdirSync(path.dirname(RANSOMWARE_STATUS_FILE), { recursive: true });
    fs.writeFileSync(RANSOMWARE_STATUS_FILE, JSON.stringify(result, null, 2));
  } catch {
  }
}

function evaluateRansomwareSignals() {
  const observedAtUnixMs = Date.now();
  const { snapshot, roots, scannedFiles, truncated } = snapshotProtectedFiles();
  const previousSnapshot = ransomwareBaseline;
  const indicators = [];
  const noteIndicators = [];
  const encryptedProtectedIndicators = [];

  for (const [filePath, meta] of snapshot.entries()) {
    const baseLower = String(meta.base || '').toLowerCase();
    if (isRansomNoteName(baseLower)) {
      const indicator = {
        type: 'ransom_note',
        path: filePath,
        relativePath: path.relative(path.dirname(TELEMETRY_FILE), filePath) || meta.base,
        fileName: meta.base,
      };
      indicators.push(indicator);
      noteIndicators.push(indicator);
      continue;
    }

    if (isEncryptedProtectedAsset(meta)) {
      const indicator = {
        type: 'encrypted_file',
        path: filePath,
        relativePath: path.relative(path.dirname(TELEMETRY_FILE), filePath) || meta.base,
        fileName: meta.base,
      };
      indicators.push(indicator);
      encryptedProtectedIndicators.push(indicator);
    }
  }

  const missingProtectedIndicators = [];
  for (const [filePath, meta] of previousSnapshot.entries()) {
    if (!isProtectedAssetName(meta.base)) continue;
    if (snapshot.has(filePath)) continue;
    missingProtectedIndicators.push({
      type: 'protected_asset_missing',
      path: filePath,
      relativePath: path.relative(path.dirname(TELEMETRY_FILE), filePath) || meta.base,
      fileName: meta.base,
    });
  }
  indicators.push(...missingProtectedIndicators);

  ransomwareBaseline = snapshot;

  const active = noteIndicators.length > 0
    && (encryptedProtectedIndicators.length > 0 || missingProtectedIndicators.length > 0);
  const reason = active ? buildRansomwareReason(indicators) : null;
  const fingerprint = active
    ? crypto.createHash('sha1').update(JSON.stringify({ reason, indicators: indicators.map(item => `${item.type}:${item.path}`) })).digest('hex')
    : '';

  let evidencePath = null;
  if (active) {
    if (fingerprint === lastRansomwareFingerprint && lastRansomwareEvidencePath) {
      evidencePath = lastRansomwareEvidencePath;
    } else {
      evidencePath = writeRansomwareEvidence({
        active: true,
        status: 'ransomware_detected',
        reason,
        detectedAtUnixMs: observedAtUnixMs,
        detectedAt: new Date(observedAtUnixMs).toISOString(),
        evidencePath: null,
        indicators: indicators.slice(0, 16),
        scannedFiles,
        protectedRoots: roots,
        truncated,
      });
      lastRansomwareFingerprint = fingerprint;
      lastRansomwareEvidencePath = evidencePath;
    }
  } else {
    lastRansomwareFingerprint = '';
    lastRansomwareEvidencePath = null;
  }

  const result = {
    active,
    status: active ? 'ransomware_detected' : 'clear',
    reason,
    detectedAtUnixMs: active ? observedAtUnixMs : null,
    detectedAt: active ? new Date(observedAtUnixMs).toISOString() : null,
    evidencePath,
    indicators: active ? indicators.slice(0, 16) : [],
    scannedFiles,
    protectedRoots: roots,
    protectedAssets: [...PROTECTED_ASSET_BASENAMES],
    truncated,
    recentEvents: ransomwareEventHistory.slice(0, 10),
  };

  if (indicators.length > 0) {
    rememberRansomwareObservation({
      active,
      reason,
      indicators,
      evidencePath,
    });
    result.recentEvents = ransomwareEventHistory.slice(0, 10);
  }

  persistRansomwareStatus(result);
  return result;
}

function deriveSolutionTransitions(snapshots) {
  const transitions = [];
  let prevAccepted = null;
  let prevNonce = null;

  for (const snap of snapshots) {
    const accepted = Number(snap.accepted_submissions || 0);
    const nonce = snap.last_solution_nonce == null ? null : Number(snap.last_solution_nonce);
    const changed = prevAccepted !== null && (accepted > prevAccepted || (nonce != null && nonce !== prevNonce));
    if (changed || (prevAccepted === null && accepted > 0 && nonce != null)) {
      transitions.push({
        timestampMs: Number(snap.timestamp_unix_ms || 0),
        acceptedSubmissions: accepted,
        nonce,
        hash: snap.last_output_hash_hex || snap.last_solution_hash_hex || null,
      });
    }
    prevAccepted = accepted;
    prevNonce = nonce;
  }

  const intervals = [];
  for (let i = 1; i < transitions.length; i += 1) {
    const delta = transitions[i].timestampMs - transitions[i - 1].timestampMs;
    if (delta > 0) intervals.push(delta);
  }

  return { transitions, intervals };
}

function average(values) {
  if (!values.length) return null;
  return values.reduce((sum, value) => sum + value, 0) / values.length;
}

function median(values) {
  if (!values.length) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? (sorted[mid - 1] + sorted[mid]) / 2
    : sorted[mid];
}

function describeNarrative(evaluation) {
  const alerts = evaluation.activeAlerts;
  if (!alerts.length) {
    return 'Mining heartbeat is continuous. Device appears present, active, and connected.';
  }

  const severe = alerts.filter(alert => alert.severity === 'critical' || alert.severity === 'high');
  if (severe.length) {
    return `Heartbeat anomalies detected: ${severe.map(alert => alert.type).join(', ')}.`;
  }

  return `Heartbeat degraded: ${alerts.map(alert => alert.type).join(', ')}.`;
}

function scoreAlerts(alerts) {
  const weights = { low: 12, medium: 25, high: 45, critical: 70 };
  return Math.min(100, alerts.reduce((sum, alert) => sum + (weights[alert.severity] || 10), 0));
}

function emitSse(event, payload) {
  const encoded = `event: ${event}\ndata: ${JSON.stringify(payload)}\n\n`;
  for (const client of sseClients) {
    client.write(encoded);
  }
}

function isLoopbackOrigin(origin) {
  if (!origin) return true;
  try {
    const parsed = new URL(origin);
    return parsed.protocol === 'http:' && (
      parsed.hostname === 'localhost' ||
      parsed.hostname === '127.0.0.1' ||
      parsed.hostname === '[::1]' ||
      parsed.hostname === '::1'
    );
  } catch {
    return false;
  }
}

function corsHeaders(origin) {
  const headers = { Vary: 'Origin' };
  if (origin && isLoopbackOrigin(origin)) {
    headers['Access-Control-Allow-Origin'] = origin;
  }
  return headers;
}

function registerAlert(type, severity, message, details, observedAt) {
  const current = activeAlerts.get(type);
  if (current) {
    current.lastSeenAt = observedAt;
    current.message = message;
    current.details = details;
    current.severity = severity;
    return current;
  }

  const alert = {
    id: nextAlertId++,
    type,
    severity,
    status: 'active',
    message,
    details,
    since: observedAt,
    lastSeenAt: observedAt,
  };
  activeAlerts.set(type, alert);
  alertHistory.unshift({ ...alert, event: 'opened' });
  if (alertHistory.length > MAX_ALERT_HISTORY) alertHistory.length = MAX_ALERT_HISTORY;
  emitSse('alert', { event: 'opened', alert });
  return alert;
}

function resolveAlert(type, observedAt) {
  const current = activeAlerts.get(type);
  if (!current) return;
  activeAlerts.delete(type);
  const resolved = {
    ...current,
    status: 'resolved',
    resolvedAt: observedAt,
  };
  alertHistory.unshift({ ...resolved, event: 'resolved' });
  if (alertHistory.length > MAX_ALERT_HISTORY) alertHistory.length = MAX_ALERT_HISTORY;
  emitSse('alert', { event: 'resolved', alert: resolved });
}

function evaluateSnapshots(snapshots, fileSize) {
  const latest = snapshots[snapshots.length - 1] || null;
  const generatedAt = nowIso();
  const ransomware = evaluateRansomwareSignals();

  if (!latest) {
    const empty = buildEmptyStatus('Telemetry file exists but no valid snapshots were parsed.');
    empty.security.ransomware = ransomware;
    return empty;
  }

  const nowMs = Date.now();
  const telemetryTs = Number(latest.timestamp_unix_ms || 0);
  const telemetryAgeMs = telemetryTs > 0 ? Math.max(0, nowMs - telemetryTs) : null;
  const telemetryFresh = telemetryAgeMs != null && telemetryAgeMs <= TELEMETRY_STALE_MS;
  const telemetryOnline = latest.state === 'running' && telemetryFresh;

  const recentHashrates = snapshots
    .slice(-HASHRATE_BASELINE_SAMPLES)
    .map(snap => Number(snap.hashrate_hs || 0))
    .filter(value => Number.isFinite(value) && value > 0);
  const baselineHashrateHs = median(recentHashrates) || 0;
  const currentHashrateHs = Number(latest.hashrate_hs || 0);
  const hashrateRatio = baselineHashrateHs > 0 ? currentHashrateHs / baselineHashrateHs : null;

  const { transitions, intervals } = deriveSolutionTransitions(snapshots);
  const lastTransition = transitions[transitions.length - 1] || null;
  const averageSolutionIntervalMs = average(intervals.slice(-20));
  const derivedGapMs = averageSolutionIntervalMs ? Math.max(MIN_SOLUTION_GAP_MS, averageSolutionIntervalMs * 6) : MIN_SOLUTION_GAP_MS;
  const lastSolutionAtMs = lastTransition ? lastTransition.timestampMs : null;
  const solutionGapMs = lastSolutionAtMs ? Math.max(0, nowMs - lastSolutionAtMs) : null;

  const currentRejected = Number(latest.rejected_submissions || 0);
  const recentRejectedCounts = snapshots.slice(-5).map(snap => Number(snap.rejected_submissions || 0));
  const rejectedBurst = recentRejectedCounts.length >= 2
    ? recentRejectedCounts[recentRejectedCounts.length - 1] - recentRejectedCounts[0]
    : 0;

  const candidateAlerts = [];
  if (!telemetryFresh) {
    candidateAlerts.push({
      type: 'telemetry_stale',
      severity: 'critical',
      message: `No fresh telemetry heartbeat for ${telemetryAgeMs} ms.`,
      details: { telemetryAgeMs, thresholdMs: TELEMETRY_STALE_MS },
    });
  }

  if (latest.state && latest.state !== 'running') {
    candidateAlerts.push({
      type: 'miner_not_running',
      severity: 'high',
      message: `Miner state is ${latest.state}.`,
      details: { state: latest.state },
    });
  }

  // Suppress gap & hashrate alerts during phases where the miner is alive but
  // intentionally idle (commit-reveal cycle: waiting for clearance, locked, etc.)
  const currentPhase = latest.mining_phase || null;
  const waitingPhases = ['waiting_for_clearance', 'commitment_locked', 'committing', 'revealing'];
  const isWaitingPhase = currentPhase && waitingPhases.includes(currentPhase);

  // Operator paused mining via dashboard — intentional idle, not an anomaly
  const isPaused = latest.mining_paused === true;

  // Stale-block / UTXO entropy sources can keep the miner "alive" even at 0 PoW hashrate.
  // If stale_block_count increased in the last N snapshots, the miner is doing useful work.
  const recentStale = snapshots.slice(-HASHRATE_BASELINE_SAMPLES).map(s => Number(s.stale_block_count || 0));
  const staleGrowing = recentStale.length >= 2 && recentStale[recentStale.length - 1] > recentStale[0];
  const isEntropyActive = staleGrowing;

  // Combined: suppress hashrate/gap alerts when any of these is true
  const suppressHashrateAlerts = isWaitingPhase || isPaused || isEntropyActive;

  if (solutionGapMs != null && solutionGapMs > derivedGapMs) {
    if (!suppressHashrateAlerts) {
      candidateAlerts.push({
        type: 'heartbeat_gap',
        severity: solutionGapMs > derivedGapMs * 2 ? 'critical' : 'high',
        message: `No new mining heartbeat for ${solutionGapMs} ms.`,
        details: {
          lastSolutionAt: new Date(lastSolutionAtMs).toISOString(),
          solutionGapMs,
          targetGapMs: derivedGapMs,
          averageSolutionIntervalMs,
        },
      });
    }
  }

  if (hashrateRatio != null && baselineHashrateHs > 0 && hashrateRatio < HASHRATE_DROP_RATIO) {
    // 0 H/s during waiting/paused/entropy-active phases is expected — not anomalous
    if (!suppressHashrateAlerts) {
      candidateAlerts.push({
        type: 'hashrate_drop',
        severity: hashrateRatio < HASHRATE_DROP_RATIO / 2 ? 'high' : 'medium',
        message: `Hashrate dropped to ${(hashrateRatio * 100).toFixed(1)}% of baseline.`,
        details: {
          currentHashrateHs,
          baselineHashrateHs,
          hashrateRatio,
        },
      });
    }
  }

  if (Number(latest.temperature_c || 0) >= TEMPERATURE_WARN_C) {
    candidateAlerts.push({
      type: 'temperature_high',
      severity: Number(latest.temperature_c || 0) >= TEMPERATURE_WARN_C + 8 ? 'high' : 'medium',
      message: `Miner temperature is ${latest.temperature_c}°C.`,
      details: { temperatureC: Number(latest.temperature_c || 0), thresholdC: TEMPERATURE_WARN_C },
    });
  }

  if (rejectedBurst >= REJECT_BURST_THRESHOLD) {
    candidateAlerts.push({
      type: 'submission_rejections',
      severity: rejectedBurst >= Math.max(3, REJECT_BURST_THRESHOLD * 2) ? 'high' : 'medium',
      message: `Rejected submissions increased by ${rejectedBurst} in the last ${recentRejectedCounts.length} snapshots.`,
      details: { currentRejected, rejectedBurst, threshold: REJECT_BURST_THRESHOLD },
    });
  }

  if (ransomware.active) {
    candidateAlerts.push({
      type: 'ransomware_detected',
      severity: 'critical',
      message: ransomware.reason || 'Suspicious ransomware-like activity detected in protected miner paths.',
      details: {
        evidencePath: ransomware.evidencePath,
        scannedFiles: ransomware.scannedFiles,
        indicators: ransomware.indicators,
      },
    });
  }

  const observedAt = generatedAt;
  const activeTypes = new Set(candidateAlerts.map(alert => alert.type));
  for (const alert of candidateAlerts) {
    registerAlert(alert.type, alert.severity, alert.message, alert.details, observedAt);
  }
  for (const type of [...activeAlerts.keys()]) {
    if (!activeTypes.has(type)) {
      resolveAlert(type, observedAt);
    }
  }

  const activeAlertList = [...activeAlerts.values()].sort((a, b) => a.id - b.id);
  const intrusionScore = scoreAlerts(activeAlertList);
  const suspicious = ransomware.active || intrusionScore >= 25;

  return {
    service: 'heartbeat-sidecar',
    status: ransomware.active ? 'alert' : telemetryOnline ? 'ok' : suspicious ? 'alert' : 'degraded',
    message: ransomware.active
      ? 'Mining heartbeat sidecar detected ransomware-like activity in protected miner paths.'
      : telemetryOnline ? 'Mining heartbeat sidecar is monitoring telemetry.' : 'Mining heartbeat sidecar detected degraded continuity.',
    generatedAt,
    miner: {
      name: MINER_NAME,
      region: MINER_REGION,
      operator: MINER_OPERATOR,
      telemetryFile: TELEMETRY_FILE,
    },
    heartbeat: {
      online: telemetryOnline,
      telemetryFresh,
      lastTelemetryAt: telemetryTs ? new Date(telemetryTs).toISOString() : null,
      telemetryAgeMs,
      lastSolutionAt: lastSolutionAtMs ? new Date(lastSolutionAtMs).toISOString() : null,
      solutionGapMs,
      targetGapMs: derivedGapMs,
      averageSolutionIntervalMs,
      acceptedSubmissions: Number(latest.accepted_submissions || 0),
      rejectedSubmissions: currentRejected,
      hashrateHs: currentHashrateHs,
      baselineHashrateHs,
      hashrateRatio,
      workerCount: Number(latest.worker_count || 0),
      temperatureC: latest.temperature_c == null ? null : Number(latest.temperature_c),
      state: latest.state || null,
      phase: latest.mining_phase || (latest.state === 'running' ? 'searching' : null),
      lastSolutionHash: latest.last_output_hash_hex || latest.last_solution_hash_hex || null,
      fingerprint: latest.cpu_fingerprint || latest.hardware_fingerprint || null,
    },
    security: {
      suspicious,
      intrusionScore,
      narrative: ransomware.active
        ? (ransomware.reason || 'Ransomware-like activity detected in protected miner paths.')
        : describeNarrative({ activeAlerts: activeAlertList }),
      activeAlerts: activeAlertList,
      ransomware,
    },
    telemetry: {
      snapshotsAnalyzed: snapshots.length,
      fileExists: true,
      fileSizeBytes: fileSize,
      lastReadAt: generatedAt,
    },
  };
}

function publishStatus(nextStatus) {
  status = nextStatus;
  const digest = crypto.createHash('sha1').update(JSON.stringify({
    status: nextStatus.status,
    generatedAt: nextStatus.generatedAt,
    heartbeat: nextStatus.heartbeat,
    security: nextStatus.security,
  })).digest('hex');

  if (digest !== lastPublishedDigest) {
    lastPublishedDigest = digest;
    emitSse('status', status);
  }
}

function pollTelemetry() {
  try {
    const { lines, size } = readTailLines(TELEMETRY_FILE, MAX_TAIL_BYTES, MAX_HISTORY_LINES);
    const snapshots = lines.map(safeJsonParse).filter(Boolean);
    publishStatus(evaluateSnapshots(snapshots, size));
  } catch (error) {
    publishStatus({
      ...buildEmptyStatus(`Heartbeat sidecar failed to read telemetry: ${error.message}`),
      status: 'error',
      telemetry: {
        snapshotsAnalyzed: 0,
        fileExists: fs.existsSync(TELEMETRY_FILE),
        fileSizeBytes: fs.existsSync(TELEMETRY_FILE) ? fs.statSync(TELEMETRY_FILE).size : 0,
        lastReadAt: nowIso(),
      },
      security: {
        suspicious: true,
        intrusionScore: 40,
        narrative: `Heartbeat monitoring degraded: ${error.message}`,
        activeAlerts: [...activeAlerts.values()],
      },
    });
  }
}

function sendJson(req, res, statusCode, payload) {
  res.writeHead(statusCode, {
    'Content-Type': 'application/json; charset=utf-8',
    'Cache-Control': 'no-store',
    'X-Content-Type-Options': 'nosniff',
    ...corsHeaders(req.headers.origin),
  });
  res.end(JSON.stringify(payload, null, 2));
}

function handleRequest(req, res) {
  let url;
  try {
    url = new URL(req.url, `http://${HOST}:${PORT}`);
  } catch {
    return sendJson(req, res, 400, { error: 'Invalid request URL' });
  }

  if (req.headers.origin && !isLoopbackOrigin(req.headers.origin)) {
    return sendJson(req, res, 403, { error: 'Forbidden origin' });
  }

  if (req.method === 'OPTIONS') {
    res.writeHead(204, {
      'Access-Control-Allow-Methods': 'GET, OPTIONS',
      'Access-Control-Allow-Headers': 'Content-Type',
      'Cache-Control': 'no-store',
      'X-Content-Type-Options': 'nosniff',
      ...corsHeaders(req.headers.origin),
    });
    return res.end();
  }

  if (req.method !== 'GET' && req.method !== 'HEAD') {
    return sendJson(req, res, 405, { error: 'Method not allowed' });
  }

  if (req.method === 'GET' && url.pathname === '/api/health') {
    return sendJson(req, res, 200, {
      ok: status.status !== 'error',
      status: status.status,
      message: status.message,
      service: status.service,
      generatedAt: status.generatedAt,
      heartbeatOnline: status.heartbeat?.online ?? false,
      telemetryFresh: status.heartbeat?.telemetryFresh ?? false,
      ransomwareActive: status.security?.ransomware?.active ?? false,
      telemetryFile: TELEMETRY_FILE,
      pollIntervalMs: POLL_INTERVAL_MS,
    });
  }

  if (req.method === 'GET' && url.pathname === '/api/heartbeat/status') {
    return sendJson(req, res, 200, status);
  }

  if (req.method === 'GET' && url.pathname === '/api/heartbeat/alerts') {
    const includeResolved = url.searchParams.get('all') === '1';
    return sendJson(req, res, 200, {
      active: [...activeAlerts.values()],
      history: includeResolved ? alertHistory : alertHistory.filter(item => item.status === 'active'),
    });
  }

  if (req.method === 'GET' && url.pathname === '/events') {
    res.writeHead(200, {
      'Content-Type': 'text/event-stream',
      'Cache-Control': 'no-cache',
      Connection: 'keep-alive',
      ...corsHeaders(req.headers.origin),
    });
    res.write(': heartbeat-sidecar connected\n\n');
    sseClients.add(res);
    emitSse('status', status);
    req.on('close', () => sseClients.delete(res));
    return;
  }

  sendJson(req, res, 404, {
    error: 'Not found',
    routes: [
      '/api/health',
      '/api/heartbeat/status',
      '/api/heartbeat/alerts?all=1',
      '/events',
    ],
  });
}

pollTelemetry();
setInterval(pollTelemetry, POLL_INTERVAL_MS).unref();

const server = http.createServer(handleRequest);
server.listen(PORT, HOST, () => {
  console.log(`[HeartbeatSidecar] listening on http://${HOST}:${PORT}`);
  console.log(`[HeartbeatSidecar] telemetry: ${TELEMETRY_FILE}`);
  console.log(`[HeartbeatSidecar] miner: ${MINER_NAME} (${MINER_REGION})`);
});

function shutdown(signal) {
  console.log(`[HeartbeatSidecar] shutting down on ${signal}`);
  server.close(() => process.exit(0));
  setTimeout(() => process.exit(1), 5000).unref();
}

process.on('SIGINT', () => shutdown('SIGINT'));
process.on('SIGTERM', () => shutdown('SIGTERM'));
