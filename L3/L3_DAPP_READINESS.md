# Temporal Gradient L3 — Dapp Readiness

Status: Draft v0  
Date: March 24, 2026

---

## 1. Purpose

This document defines what it means for the Temporal Gradient L3 to be ready for dapps.

The goal is not merely to have contracts deployed.

The goal is to make the chain usable, understandable, and trustworthy for external builders.

---

## 2. Core readiness principle

A chain is dapp-ready when an external builder can:

1. understand the trust model,
2. reach stable endpoints,
3. get predictable data,
4. verify randomness claims,
5. and integrate without special insider knowledge.

If any of those fail, the chain is not truly ready.

---

## 3. Readiness layers

The first L3 should measure readiness across five layers:

1. **chain availability**
2. **developer access**
3. **data access**
4. **randomness product usability**
5. **operational trust**

---

## 4. Chain availability requirements

Minimum requirements:

- stable RPC endpoint,
- published chain metadata,
- funded test flow or faucet path if needed,
- stable block explorer or equivalent read endpoint,
- known maintenance posture.

Builders should not need private instructions to connect.

---

## 5. Developer access requirements

Minimum requirements:

- SDK or client helper,
- endpoint reference,
- environment examples,
- one working integration example,
- clear error response format,
- versioned API behavior.

If a dapp team has to reverse engineer the system, onboarding is not ready.

---

## 6. Data access requirements

The first L3 should expose easy read paths for:

- epochs,
- proof receipts,
- certificate records,
- latest randomness summary,
- anchored randomness metadata when available,
- service health.

This is why a read API is a first-class requirement, not an optional nice-to-have.

---

## 7. Randomness-consumer readiness

Because this chain is randomness-focused, readiness must be evaluated through a consumer lens.

The first dapp-facing randomness surface should answer:

- what is the latest usable randomness value,
- what epoch did it come from,
- is it finalized,
- what proof material exists,
- is there external anchoring,
- and what trust label applies.

These answers should be available without forcing the consumer to read raw contract storage.

---

## 8. Trust labeling requirements

Every randomness response should be easy to classify.

Recommended labels:

- `epoch-settled`
- `proof-verifiable`
- `externally-anchored`
- `experimental`

This gives dapps a usable policy surface.

For example:

- games may require `proof-verifiable`,
- high-stakes draws may prefer `externally-anchored`,
- internal testing may allow `experimental`.

---

## 9. Reliability requirements

Before real onboarding, the chain should have:

- public health endpoint,
- degraded-mode behavior,
- request throttling,
- backpressure handling,
- clear retry guidance,
- observable service status.

The goal is not infinite scale on day one.

The goal is predictable behavior under load.

---

## 10. Dapp integration checklist

The first real onboarding checklist should include:

- chain ID and RPC info published,
- read API base URL published,
- JS/TS SDK available,
- proof response format documented,
- randomness freshness semantics documented,
- receipt semantics documented,
- sample app integration published,
- status page or health endpoint available.

---

## 11. Operational trust requirements

Dapps need to know:

- who operates the sequencer,
- what the upgrade posture is,
- whether the system is experimental,
- whether the chain can pause,
- how incidents are communicated.

Operational trust is part of product readiness.

---

## 12. Recommended first dapp personas

The first L3 should optimize for a narrow set of high-fit dapps.

Best early targets:

1. on-chain games,
2. raffles and lotteries,
3. provable draw systems,
4. consumer proof marketplaces,
5. provenance and certificate workflows.

These fit the chain's strongest differentiator.

---

## 13. What dapps should not need yet

Early dapps should not need:

- custom bridge complexity,
- bespoke indexing infrastructure,
- manual proof decoding,
- hidden operator contacts,
- or undocumented recovery assumptions.

If they do, readiness is incomplete.

---

## 14. Readiness milestones

### Milestone A — Internal-ready

- operators can use the system end to end,
- endpoints are stable for project-controlled apps,
- health monitoring exists.

### Milestone B — Partner-ready

- SDK exists,
- trust model is documented,
- read API is stable,
- one or two external teams can integrate.

### Milestone C — Public dapp-ready

- interfaces are stable,
- operational behavior is documented,
- support path exists,
- randomness labels and guarantees are consistent.

---

## 15. Summary

For this chain, dapp readiness is not mainly about having more contracts.

It is about making randomness:

- easy to consume,
- easy to verify,
- easy to trust,
- and easy to operationalize.

That is the readiness standard that matters.