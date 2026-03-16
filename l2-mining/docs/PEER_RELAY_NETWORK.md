# Peer Relay Network Roadmap

This repository now exposes the minimum identity and continuity signals needed for a verified miner relay network:

- miner identity bound to an operator wallet
- continuous heartbeat monitoring from live mining telemetry
- signed proof-of-presence from the latest randomness output
- relay profile export from the dashboard

## What is implemented now

Current dashboard endpoint:

- `GET /api/security/relay-profile`

Current relay profile includes:

- operator address
- miner region/name
- heartbeat health
- intrusion score
- latest signed proof-of-presence
- relay readiness flag

This is enough to prove a miner is a live, healthy, signed endpoint.

## What the next build needs

### 1. Peer discovery

Add a signed peer directory where miners publish their relay profile.

Suggested payload:

- relay profile JSON
- timestamp
- expiry
- signature by miner key

### 2. Session keys

Add ephemeral X25519 or similar transport keys so relay traffic is encrypted independently of the miner wallet key.

### 3. Circuit establishment

Add a lightweight control plane:

- client requests relay session
- relay verifies miner is healthy
- relay returns ephemeral endpoint and session material

### 4. Traffic forwarding

Start with simple egress forwarding:

- HTTPS CONNECT proxy
- DNS-over-HTTPS relay
- later multi-hop routing

### 5. Policy enforcement

Before any relay traffic is allowed, require:

- heartbeat online
- telemetry fresh
- intrusion score below threshold
- latest proof-of-presence signature available

## Practical product path

### Stage A — Personal threat dashboard

Already underway in the dashboard:

- continuity window
- gap detection
- alert feed
- proof-of-presence

### Stage B — Verified relay node

Expose relay profile to trusted peers only.

### Stage C — Miner mesh

Allow miners to relay each other’s traffic through verified healthy nodes.

### Stage D — Privacy network

Add multi-region path selection and anti-correlation routing.

## Key rule

The miner wallet proves identity.
The heartbeat proves liveness.
The relay plane should only trust miners that have both.
