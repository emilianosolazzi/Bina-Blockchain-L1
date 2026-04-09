# Temporal Gradient L3 — Reliability SLO

Status: Draft v0  
Date: March 25, 2026

---

## 1. Purpose

This document defines the first reliability objectives for the Temporal Gradient L3.

The goal is to make the chain predictable for dapps even before it is fully decentralized.

---

## 2. Reliability principle

Reliability means more than uptime.

The system must be:

- available,
- observable,
- degradation-aware,
- and honest about service state.

---

## 3. Reliability surfaces

The first L3 should define SLOs for:

1. **RPC availability**
2. **read API availability**
3. **randomness freshness**
4. **materialized-store lag**
5. **accepted write acknowledgement**
6. **proof and certificate read consistency**

---

## 4. Suggested initial SLO targets

These are recommended bootstrap targets, not final production promises.

### 4.1 Read API availability

- target: **99.9% monthly availability** for public read endpoints

### 4.2 Health endpoint availability

- target: **99.95% monthly availability** for `/api/health`

### 4.3 Hot read latency

- target: **p95 3–10ms** for cache-served hot endpoints under normal load

### 4.4 Accepted write acknowledgement

- target: **fast acknowledgement under normal operating load**, tracked separately from finality

### 4.5 Materialized-view lag

- target: **small and explicitly measured lag**, with visibility when breached

### 4.6 Randomness freshness

- target: latest randomness endpoint should expose freshness metadata and stale-state flags

---

## 5. Reliability states

The first L3 should publish explicit service states.

Recommended states:

- `healthy`
- `degraded-read`
- `degraded-write`
- `stale-data`
- `recovery`
- `critical`

This is better than vague success/failure reporting.

---

## 6. Error budget concept

The system should operate with an explicit error budget.

Examples:

- if read availability drops below target, feature rollout slows,
- if materialized lag exceeds target too often, cache policy is adjusted,
- if failovers spike, onboarding should pause until stability improves.

---

## 7. Required telemetry

The reliability model requires:

- latency histograms,
- queue depth,
- cache hit rate,
- RPC upstream health,
- shard health,
- failover count,
- store sync lag,
- stale randomness age,
- acknowledged-vs-finalized write gap.

---

## 8. Public SLO candidates

Before real dapp onboarding, the system should be able to publish:

- public read API availability target,
- health endpoint target,
- freshness semantics for latest randomness,
- response classifications during degraded mode.

---

## 9. Internal-only SLO candidates

Initially, some reliability metrics may stay internal:

- sequencer infrastructure failure counts,
- exact queue thresholds,
- detailed shard balancing data,
- internal operator response-time targets.

---

## 10. Degraded-mode expectations

The chain should degrade gracefully.

Recommended order:

1. throttle non-critical traffic,
2. serve cached data where safe,
3. mark freshness loss explicitly,
4. preserve health/status visibility,
5. avoid silent corruption.

---

## 11. Reliability gates before dapp onboarding

Before broader dapp onboarding, the chain should have:

- stable public health endpoint,
- stable hot-read latency profile,
- cache hit rates consistently high on hot paths,
- explicit stale-data signaling,
- repeatable failover behavior,
- clear incident communication path.

---

## 12. Summary

The first reliability promise should be modest but strong:

- fast reads,
- predictable acknowledgements,
- visible freshness,
- graceful degradation,
- and honest system state.

That is enough to make the chain dependable for early dapps.