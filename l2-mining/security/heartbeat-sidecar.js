#!/usr/bin/env node
'use strict';

const fs = require('fs');
const http = require('http');
const path = require('path');
const crypto = require('crypto');

const ROOT = __dirname;
const TELEMETRY_FILE = process.env.TELEMETRY_FILE
  ? path.resolve(process.cwd(), process.env.TELEMETRY_FILE)
  : path.resolve(ROOT, '..', 'rust', 'miner-telemetry.jsonl');
const HOST = process.env.HEARTBEAT_HOST || '127.0.0.1';
const PORT = Number(process.env.HEARTBEAT_PORT || 4380);
const POLL_INTERVAL_MS = Number(process.env.HEARTBEAT_POLL_INTERVAL_MS || 2000);
const TELEMETRY_STALE_MS = Number(process.env.HEARTBEAT_TELEMETRY_STALE_MS || 15000);
const MIN_SOLUTION_GAP_MS = Number(process.env.HEARTBEAT_MIN_SOLUTION_GAP_MS || 30000);
const MAX_TAIL_BYTES = Number(process.env.HEARTBEAT_MAX_TAIL_BYTES || 1024 * 512);
const MAX_HISTORY_LINES = Number(process.env.HEARTBEAT_MAX_HISTORY_LINES || 720);
const HASHRATE_BASELINE_SAMPLES = Number(process.env.HEARTBEAT_HASHRATE_BASELINE_SAMPLES || 60);
const HASHRATE_DROP_RATIO = Number(process.env.HEARTBEAT_HASHRATE_DROP_RATIO || 0.45);
const TEMPERATURE_WARN_C = Number(process.env.HEARTBEAT_TEMPERATURE_WARN_C || 82);
const REJECT_BURST_THRESHOLD = Number(process.env.HEARTBEAT_REJECT_BURST_THRESHOLD || 1);
const MINER_NAME = process.env.HEARTBEAT_MINER_NAME || process.env.MINER_NAME || 'default-miner';
const MINER_REGION = process.env.HEARTBEAT_MINER_REGION || 'unknown';
const MINER_OPERATOR = process.env.HEARTBEAT_MINER_OPERATOR || null;
const MAX_ALERT_HISTORY = Number(process.env.HEARTBEAT_MAX_ALERT_HISTORY || 200);

const sseClients = new Set();
const activeAlerts = new Map();
const alertHistory = [];
let status = buildEmptyStatus('Waiting for telemetry…');
let lastPublishedDigest = '';
let nextAlertId = 1;

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

  if (!latest) {
    return buildEmptyStatus('Telemetry file exists but no valid snapshots were parsed.');
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

  if (solutionGapMs != null && solutionGapMs > derivedGapMs) {
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

  if (hashrateRatio != null && baselineHashrateHs > 0 && hashrateRatio < HASHRATE_DROP_RATIO) {
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

  if (Number(latest.temperature_c || 0) >= TEMPERATURE_WARN_C) {
    candidateAlerts.push({
      type: 'temperature_high',
      severity: Number(latest.temperature_c || 0) >= TEMPERATURE_WARN_C + 8 ? 'high' : 'medium',
      message: `Miner temperature is ${latest.temperature_c}°C.`,
      details: { temperatureC: Number(latest.temperature_c || 0), thresholdC: TEMPERATURE_WARN_C },
    });
  }

  if (rejectedBurst >= REJECT_BURST_THRESHOLD || currentRejected > 0) {
    candidateAlerts.push({
      type: 'submission_rejections',
      severity: rejectedBurst >= Math.max(3, REJECT_BURST_THRESHOLD * 2) ? 'high' : 'medium',
      message: `Rejected submissions increased by ${Math.max(rejectedBurst, currentRejected)}.`,
      details: { currentRejected, rejectedBurst, threshold: REJECT_BURST_THRESHOLD },
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
  const suspicious = intrusionScore >= 25;

  return {
    service: 'heartbeat-sidecar',
    status: telemetryOnline ? 'ok' : suspicious ? 'alert' : 'degraded',
    message: telemetryOnline ? 'Mining heartbeat sidecar is monitoring telemetry.' : 'Mining heartbeat sidecar detected degraded continuity.',
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
      narrative: describeNarrative({ activeAlerts: activeAlertList }),
      activeAlerts: activeAlertList,
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

function sendJson(res, statusCode, payload) {
  res.writeHead(statusCode, {
    'Content-Type': 'application/json; charset=utf-8',
    'Cache-Control': 'no-store',
    'Access-Control-Allow-Origin': '*',
  });
  res.end(JSON.stringify(payload, null, 2));
}

function handleRequest(req, res) {
  const url = new URL(req.url, `http://${HOST}:${PORT}`);

  if (req.method === 'GET' && url.pathname === '/api/health') {
    return sendJson(res, 200, {
      ok: status.status !== 'error',
      service: status.service,
      generatedAt: status.generatedAt,
      telemetryFile: TELEMETRY_FILE,
      pollIntervalMs: POLL_INTERVAL_MS,
    });
  }

  if (req.method === 'GET' && url.pathname === '/api/heartbeat/status') {
    return sendJson(res, 200, status);
  }

  if (req.method === 'GET' && url.pathname === '/api/heartbeat/alerts') {
    const includeResolved = url.searchParams.get('all') === '1';
    return sendJson(res, 200, {
      active: [...activeAlerts.values()],
      history: includeResolved ? alertHistory : alertHistory.filter(item => item.status === 'active'),
    });
  }

  if (req.method === 'GET' && url.pathname === '/events') {
    res.writeHead(200, {
      'Content-Type': 'text/event-stream',
      'Cache-Control': 'no-cache',
      Connection: 'keep-alive',
      'Access-Control-Allow-Origin': '*',
    });
    res.write(': heartbeat-sidecar connected\n\n');
    sseClients.add(res);
    emitSse('status', status);
    req.on('close', () => sseClients.delete(res));
    return;
  }

  sendJson(res, 404, {
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
