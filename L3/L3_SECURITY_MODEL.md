# Temporal Gradient L3 — Security Model

Status: Draft v0  
Date: March 24, 2026

---

## 1. Purpose

This document defines the first security model for the Temporal Gradient L3.

The goal is not to claim perfect decentralization on day one.

The goal is to define the concrete safety posture required to make the chain:

- operationally safe,
- predictable for dapps,
- resilient under normal operator faults,
- and trustworthy as a randomness-focused execution environment.

---

## 2. Security objective

The first L3 security model must protect five things:

1. **chain liveness**,
2. **state integrity**,
3. **randomness integrity**,
4. **operator accountability**,
5. **consumer trust assumptions**.

The first version should prefer clear trust boundaries over vague decentralization claims.

---

## 3. Security design posture

The recommended v0 posture is:

- **single-operator sequencer**,
- **strict operator key separation**,
- **small public surface area**,
- **observable service health**,
- **defensive rate limiting and failover**, 
- **external entropy anchoring preserved**,
- **explicit incident procedures**.

This aligns with the current Orbit bootstrap plan.

---

## 4. Threat model categories

The first L3 should explicitly model these threat groups.

### 4.1 Chain-operation threats

- sequencer downtime,
- operator key compromise,
- broken deployment procedure,
- invalid config rollout,
- unsafe upgrades,
- broken RPC edge.

### 4.2 Application-layer threats

- abuse of public endpoints,
- proof replay or duplicate purchase attempts,
- excessive query load,
- malformed epoch payloads,
- stale read data,
- inconsistent receipt indexing.

### 4.3 Randomness-specific threats

- biased or unverifiable output presentation,
- misleading freshness claims,
- proof delivery failures,
- unanchored outputs being presented as anchored,
- confusion between chain-local and externally anchored entropy.

### 4.4 Governance and trust threats

- unclear pause authority,
- unclear upgrade authority,
- unclear treasury authority,
- unclear canonicality claims,
- and unclear recovery policy during faults.

---

## 5. Security boundaries

The first security rule is to keep boundaries clear.

### 5.1 The L3 secures execution

The L3 is responsible for:

- ordering transactions,
- preserving state transitions,
- recording protocol settlement,
- exposing deterministic on-chain results.

### 5.2 External systems secure entropy provenance

External systems remain responsible for:

- stale-block harvesting,
- dead-UTXO anchoring,
- Bitcoin provenance,
- off-chain epoch formation,
- storage and proof materialization.

This is a feature, not a weakness.

The randomness system is safer if the chain does **not** pretend to be the only source of truth.

---

## 6. Operator key model

The first L3 should use separate keys for separate duties.

Minimum recommended split:

1. **deployment key**
2. **sequencer/operator key**
3. **treasury/admin key**
4. **service signing key(s)** where needed

These should not collapse into one hot wallet.

### Rules

- deployment keys should be rotated out of daily use,
- admin keys should not double as public-service keys,
- service keys should be scoped to service actions,
- emergency authority should be documented before public dapp onboarding.

---

## 7. Upgrade and freeze policy

The first L3 should publish a conservative upgrade policy.

Recommended v0 rule:

- upgrades allowed during controlled bring-up,
- changes logged clearly,
- critical interfaces stabilized before dapp onboarding,
- freeze or slow-change policy once the first dapps depend on the chain.

Dapps should know whether they are building on:

- an experimental surface,
- a soft-stable surface,
- or a stability-committed surface.

---

## 8. RPC and API edge security

The first L3 should protect its read and write edges.

### Minimum requirements

- canonical RPC endpoints,
- canonical read API endpoint,
- rate limits,
- abuse throttling,
- request logging,
- health monitoring,
- degraded-mode responses.

The system should not fail open under traffic spikes.

This is where the sharding and backpressure reference in [sharding/ShardManager.js](../sharding/ShardManager.js) is useful.

---

## 9. Service resilience model

The first L3 should assume that services fail under load or configuration mistakes.

Recommended posture:

- backpressure before collapse,
- failover before silent data corruption,
- explicit health status before timeout ambiguity,
- read-only degradation before full outage when possible.

This especially matters for:

- randomness read APIs,
- proof retrieval endpoints,
- dapp SDK endpoints,
- and settlement workers.

---

## 10. Randomness integrity model

The chain should never oversell the randomness claim.

The project should explicitly distinguish between:

1. **chain-recorded randomness state**
2. **epoch-level proof state**
3. **externally anchored randomness provenance**

### Required rule

An output must not be described as stronger than the evidence attached to it.

Examples:

- if it only has an epoch root, call it epoch-settled,
- if it has proof material, call it proof-verifiable,
- if it has Bitcoin-linked provenance, call it externally anchored.

This honesty is part of the security model.

---

## 11. Incident model

The first L3 should publish operational responses for:

- sequencer downtime,
- RPC overload,
- bad deployment,
- stale read API data,
- treasury misconfiguration,
- proof-delivery outage,
- and suspected randomness-integrity events.

At minimum, operators should define:

- who decides incident severity,
- who pauses public onboarding,
- who communicates status,
- who rotates affected credentials,
- who validates recovery.

---

## 12. Dapp-facing trust model

For dapps to join safely, the project should state clearly:

- what the chain guarantees,
- what the randomness layer guarantees,
- what is externally anchored,
- what is operator-controlled,
- what is still experimental.

This is how a smaller chain can be more trustworthy than a larger but vaguer one.

---

## 13. Security controls required before real dapp onboarding

Before serious onboarding, the chain should have:

- stable endpoint inventory,
- documented operator roles,
- configuration management discipline,
- monitored RPC health,
- monitored read API health,
- request throttling,
- incident response notes,
- published trust model,
- published randomness guarantee language.

---

## 14. Why this chain can be safer for randomness-focused dapps

The security advantage is not only chain execution.

It is the combination of:

- dedicated blockspace,
- smaller and clearer trust surface,
- protocol-specific observability,
- externally anchored entropy pathways,
- and explicit proof language.

This creates a chain that can be safer for randomness consumers than general-purpose environments that treat randomness as a side feature.

---

## 15. Summary

The first L3 security model should optimize for:

- clarity,
- resilience,
- operator accountability,
- and honest randomness guarantees.

That is the right security foundation for a randomness-first chain.