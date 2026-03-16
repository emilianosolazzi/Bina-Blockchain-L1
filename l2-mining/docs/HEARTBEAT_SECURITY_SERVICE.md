# Heartbeat Security Sidecar

This sidecar runs in parallel with the miner and converts the continuous mining stream into a security heartbeat.

## What it does

The service watches the miner telemetry JSONL stream and derives:

- telemetry freshness
- heartbeat continuity
- solution-gap alerts
- hashrate collapse alerts
- temperature alerts
- submission rejection alerts

This gives you a local proof-of-presence signal without changing the mining loop.

## Why this matches the Temporal Gradient model

Traditional zero-trust trusts a static certificate.

The sidecar trusts continuous work:

- if the miner is active and telemetry stays fresh, the device is alive
- if the accepted-solution heartbeat stops, the device may be disrupted
- if hashrate collapses or rejections spike, the node becomes suspicious

## Run it

From the l2-mining folder:

```powershell
node security/heartbeat-sidecar.js
```

Optional environment variables:

- `TELEMETRY_FILE`
- `HEARTBEAT_PORT`
- `HEARTBEAT_HOST`
- `HEARTBEAT_MINER_NAME`
- `HEARTBEAT_MINER_REGION`
- `HEARTBEAT_MINER_OPERATOR`
- `HEARTBEAT_TELEMETRY_STALE_MS`
- `HEARTBEAT_MIN_SOLUTION_GAP_MS`
- `HEARTBEAT_HASHRATE_DROP_RATIO`
- `HEARTBEAT_TEMPERATURE_WARN_C`

Example:

```powershell
$env:HEARTBEAT_MINER_NAME='Miner A'
$env:HEARTBEAT_MINER_REGION='Arlington'
$env:HEARTBEAT_MINER_OPERATOR='0x3058bd411b9ec0dF6C7d0b04914C9bd2934b7fb3'
node security/heartbeat-sidecar.js
```

## API

### `GET /api/health`
Service health.

### `GET /api/heartbeat/status`
Current heartbeat and security posture.

### `GET /api/heartbeat/alerts?all=1`
Active alerts and alert history.

### `GET /events`
Server-sent events stream for status and alert updates.

## Current scope

Implemented now:

- single-miner local heartbeat monitoring
- sidecar deployment with no miner runtime changes
- alerting from the telemetry stream

Not yet implemented:

- multi-region fleet correlation
- latency-map censorship detection
- relay routing or firewall-bypass flows
- hardware fingerprint anchoring in the sidecar payload

Those can be layered on top by feeding multiple miners into the same alert plane.
