# Temporal Gradient L3 — Scope v0

Status: Draft v0  
Date: March 24, 2026

---

## 1. Purpose

This document defines the **first implementation scope** for the Temporal Gradient L3.

It exists to prevent the project from trying to move the full roadmap into the first Orbit deployment.

The purpose of v0 is to answer one question clearly:

> What is the minimum useful Temporal Gradient L3 that should be designed and validated first?

This scope is intentionally narrow.

---

## 2. Core objective of v0

The first L3 should prove that Temporal Gradient can run its **core protocol settlement layer** inside a sovereign execution environment without breaking the current architecture.

That means v0 is about:

- protocol execution,
- settlement correctness,
- contract migration discipline,
- fee-path validation,
- and operator workflow validation.

It is **not** about shipping the entire future network.

---

## 3. What v0 must achieve

The v0 L3 should demonstrate all of the following:

1. A Temporal Gradient-specific execution environment can run under Arbitrum Orbit.
2. Core protocol contracts can be deployed and exercised there safely.
3. The first-wave economic flows can settle correctly.
4. The protocol can preserve compatibility with the current Arbitrum-centered architecture during migration.
5. The L3 can become the future home of more protocol functionality without forcing that migration all at once.

---

## 4. In-scope for v0

The following items are **in scope** for the first L3 version.

### 4.1 L3 base chain design

In scope:

- Arbitrum Orbit Rollup design,
- parent-chain assumption: Arbitrum One,
- ETH gas bootstrap assumption,
- future TGBT gas migration planning,
- initial sequencer model,
- fee destination planning,
- settlement topology documentation.

### 4.2 Core protocol settlement contracts

In scope:

- epoch / batch settlement logic,
- randomness proof marketplace settlement,
- certificate issuance settlement,
- bridge/export architecture references where relevant,
- L3-facing accounting surfaces for protocol payments.

### 4.3 Devnet-first validation

In scope:

- local or private Orbit devnet plan,
- first-wave deployment order,
- end-to-end contract validation goals,
- fee behavior checks,
- operator workflow testing.

### 4.4 Asset and bridge design

In scope:

- canonical TGBT model definition,
- early bridge assumptions,
- parent-vs-L3 settlement responsibility,
- staged gas-token transition planning,
- identifying which assets or economic surfaces remain parent-side initially.

### 4.5 Migration planning

In scope:

- contract migration order,
- explicit first-wave contract list,
- explicit contracts deferred from migration,
- hybrid parent/L3 period planning,
- rollback-friendly design assumptions.

---

## 5. First-wave application scope

The first-wave L3 application scope should be limited to the following protocol domains.

### 5.1 Epoch settlement

This is in scope because it is a core protocol execution surface.

Includes:

- epoch root commit/finalize logic,
- settlement ordering assumptions,
- proof verification support required for core app flows,
- compatibility planning with existing batch mining logic.

### 5.2 Randomness marketplace settlement

This is in scope because it is one of the clearest utility sinks for TGBT and one of the cleanest candidates for L3-native execution.

Includes:

- proof purchase settlement,
- marketplace fee logic,
- treasury / burn / routing assumptions,
- future buyer flow compatibility.

### 5.3 Certificate issuance settlement

This is in scope because certificate minting is already part of the protocol's economic and provenance design.

Includes:

- certificate payment logic,
- mint settlement,
- attestor-facing settlement assumptions,
- L3-native provenance product execution.

### 5.4 Miner-facing protocol accounting hooks

This is in scope, but narrowly.

Includes:

- miner payment-related contract assumptions,
- future reimbursement integration points,
- accounting paths that affect protocol execution.

Does **not** mean full mining runtime migration in v0.

---

## 6. Explicitly out of scope for v0

The following items are **not** part of the first L3 implementation scope.

### 6.1 Relay mesh

Out of scope:

- packet forwarding plane,
- multi-hop relay routing,
- verified egress implementation,
- relay session management,
- onion routing,
- cover traffic generation.

### 6.2 Messaging and privacy stack

Out of scope:

- end-to-end private messaging,
- whistleblower dead drops,
- censorship-resistant publishing,
- mixnet batching.

### 6.3 Advanced network infrastructure

Out of scope:

- decentralized CDN,
- decentralized DNS,
- RPC mesh,
- oracle relay network,
- load balancing network.

### 6.4 Advanced service-bonding systems

Out of scope:

- full staking system,
- attestor bonding economics,
- relay operator staking,
- service slashing design.

### 6.5 Advanced compute transport

Out of scope:

- MPC transport,
- verifiable computation relay,
- consensus gossip layer,
- appchain transport role.

### 6.6 Mobile mining integration

Out of scope:

- mobile miner runtime support,
- battery-aware mining execution,
- mobile miner settlement integration as a first-wave dependency.

### 6.7 Full migration of the current ecosystem

Out of scope:

- forcing all current contracts to move at once,
- immediate replacement of all current Arbitrum-side flows,
- all-at-once chain sovereignty.

---

## 7. What stays outside the first L3 wave

The first L3 should assume that some protocol responsibilities remain outside the new chain during the initial phase.

This may include:

- some bridge logic,
- some governance transition logic,
- some reimbursement logic,
- some legacy compatibility layers,
- some parent-chain verification surfaces,
- some off-chain service orchestration.

This is acceptable.

The first goal is correct execution, not total migration purity.

---

## 8. v0 contract-classification model

For v0 planning, every protocol contract or subsystem should fall into one of four categories.

### Category A — Move first

These are contracts or modules that should be part of the first L3 wave because they are central to application execution.

Examples:

- epoch settlement,
- proof marketplace,
- certificate settlement.

### Category B — Reference only / refactor candidate

These are useful as design references but should not be deployed directly without redesign.

Example:

- `BeaconExporter.sol` in its current legacy form.

### Category C — Keep parent-side initially

These remain outside the first L3 migration wave.

Examples may include:

- legacy compatibility surfaces,
- some governance controls,
- some asset canonicality logic,
- some bridge authorities.

### Category D — Not relevant to current Orbit-first path

These are not useful for the present L3 direction.

Example:

- contracts built around custom chain precompile assumptions rather than Orbit.

---

## 9. Minimal success criteria

The v0 L3 scope is successful if it can demonstrate the following on a devnet or private test environment:

1. Orbit chain configuration is stable.
2. First-wave contracts deploy cleanly.
3. Epoch-related settlement flows execute correctly.
4. Marketplace payment flows execute correctly.
5. Certificate mint/payment flows execute correctly.
6. Fee routing is observable and coherent.
7. Parent/L3 responsibility boundaries are documented clearly.
8. The system is ready for the next design layer without re-architecting everything again.

---

## 10. Non-goals

To avoid confusion, these are **non-goals** of v0:

- proving full decentralization,
- replacing the full current protocol immediately,
- shipping a public production chain,
- launching TGBT-native gas immediately,
- solving all cross-chain design questions at once,
- delivering the full relay roadmap,
- shipping the final economic architecture.

v0 is a disciplined first execution step.

---

## 11. Dependencies for v0

Before v0 can move into serious implementation, the following documents should exist:

- `TG_L3_ORBIT_PLAN.md`
- `L3_ASSET_MODEL.md`
- `L3_SEQUENCER_MODEL.md`
- `L3_CONTRACT_MIGRATION.md`
- `L3_DEVNET_PLAN.md`

This scope document does not replace those. It limits them.

---

## 12. Immediate next decisions

The next decisions after this scope should be:

1. confirm the first-wave contract list,
2. define canonical TGBT placement,
3. define initial bridge assumptions,
4. define sequencer fee flow,
5. decide which payment paths must be L3-native in wave one,
6. define the devnet milestone.

---

## 13. Working summary

The first L3 version should be understood as:

- an **Orbit-based execution foundation**,
- focused on **core settlement**,
- limited to **epoch, marketplace, and certificate flows**,
- with **ETH gas first**,
- **TGBT gas later**,
- and with the broader relay/privacy/network roadmap explicitly postponed.

That is the right size for v0.

---

## 14. Conclusion

v0 should be narrow on purpose.

If the first L3 tries to include too much, it will slow the protocol down and blur the architecture.

If the first L3 focuses on core execution and settlement, it creates a real foundation for the later phases.

That is the correct first scope for Temporal Gradient L3.
