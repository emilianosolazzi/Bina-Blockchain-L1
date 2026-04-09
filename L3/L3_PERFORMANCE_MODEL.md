# Temporal Gradient L3 — Performance Model

Status: Draft v0  
Date: March 25, 2026

---

## 1. Purpose

This document defines the first performance model for the Temporal Gradient L3.

The goal is to make the chain feel web2-fast where realistic, without pretending that true blockchain finality can always behave like an in-memory database.

---

## 2. Core performance rule

The first performance rule is to separate three latency classes:

1. **hot reads**
2. **accepted writes**
3. **finalized writes**

These should never be treated as the same metric.

---

## 3. Realistic target posture

The realistic target for the first L3 is:

- **3–10ms p95 hot reads** for cached API paths,
- **fast accepted-write acknowledgements** through the sequencer path,
- **slower asynchronous settlement/finality** handled explicitly.

This creates web2-like user experience without making false claims about finality.

---

## 4. Latency classes

### 4.1 Hot read latency

This is the latency for serving precomputed or cached data such as:

- latest randomness,
- recent epochs,
- proof receipt summaries,
- certificate summaries,
- health data.

This is the main candidate for single-digit millisecond p95.

### 4.2 Accepted write latency

This is the latency from request submission to operator/sequencer acceptance.

This should be exposed as a product state, not confused with final settlement.

### 4.3 Finalized write latency

This is the latency until the write is fully settled and safe under the chosen finality semantics.

This will be slower than hot reads and accepted acknowledgements.

---

## 5. Performance architecture principle

The chain should not rely on raw chain reads for most dapp traffic.

Instead it should use a layered data plane:

1. **sequencer / write path**
2. **materialized read model**
3. **hot cache layer**
4. **public read API**
5. **consumer SDK**

This is how the chain can feel faster than web2 applications that still hit slow backing systems.

---

## 6. Cache model

The first L3 should use multiple cache tiers.

### L1 — in-process memory

For ultra-hot values:

- latest randomness,
- latest finalized epoch,
- API health snapshot,
- recent receipt summaries.

### L2 — Redis or equivalent

For shared hot state across service instances:

- hot epoch summaries,
- recent proof receipts,
- recent certificate records,
- precomputed randomness bundles.

### L3 — materialized store

For indexed read models and replayable state:

- epoch history,
- proof receipt history,
- certificate history,
- trust labels and provenance flags.

---

## 7. Performance-critical endpoints

The first system should optimize aggressively for:

- `/api/health`
- `/api/randomness/latest`
- `/api/epochs?limit=N`
- recent proof receipt queries
- recent certificate queries

These are the paths that shape the user experience most strongly.

---

## 8. Precomputation strategy

The chain should precompute rather than derive expensive read responses on demand.

Precompute and cache:

- latest randomness bundle,
- trust label bundle,
- latest anchored-status bundle,
- epoch summary list,
- proof receipt summary list,
- certificate summary list.

This is especially important for randomness products, where proof context can otherwise become the latency bottleneck.

---

## 9. Write-path design principle

The hot write path should stay as small as possible.

Recommended rule:

- minimal state transition first,
- asynchronous enrichment later,
- proof-heavy work off the critical path,
- read model updated after acceptance.

This reduces user-facing latency while keeping the state model clear.

---

## 10. Fast randomness product model

For randomness consumers, the product should expose:

1. a fast latest-value path,
2. a proof-ready path,
3. an externally anchored path.

These paths should correspond to different latency and trust tradeoffs.

This is not a weakness.

It is a better product design than forcing every request through the slowest trust path.

---

## 11. Reliability-performance interaction

High speed without graceful degradation is not useful.

The performance model must include:

- backpressure,
- request prioritization,
- queue-depth control,
- shard failover,
- stale-data signaling,
- read-only degraded mode.

The reference design in [sharding/ShardManager.js](../sharding/ShardManager.js) is useful here.

---

## 12. Geographic and topology assumptions

To keep p95 low, the first deployment should prefer:

- single-region colocated services for the hot path,
- cache and API close to the sequencer and indexer,
- minimal synchronous cross-region dependencies,
- no cold storage dependency in the hot path.

---

## 13. Performance SLO categories

The chain should track at least:

- read p50 / p95 / p99,
- accepted write p50 / p95 / p99,
- materialized-view lag,
- cache hit rate,
- proof generation latency,
- queue depth,
- failover frequency,
- degraded-mode percentage.

---

## 14. Anti-patterns to avoid

The first L3 should avoid:

- forcing raw chain reads for every consumer request,
- synchronous proof generation in hot reads,
- pretending accepted writes are finalized writes,
- large write payloads on the latency-critical path,
- and cacheless public randomness endpoints.

---

## 15. Summary

The right performance target is not “3–10ms final blockchain settlement.”

The right performance target is:

- 3–10ms p95 hot reads,
- fast acknowledged writes,
- asynchronous finality,
- and predictable degraded behavior.

That is how the chain can feel faster than web2 for many dapp experiences.