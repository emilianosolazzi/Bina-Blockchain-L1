# Temporal Gradient L3 — Sequencer Model

Status: Draft v0  
Date: March 24, 2026

---

## 1. Purpose

This document defines the first sequencer model for the Temporal Gradient L3.

The goal is not to design the final decentralized sequencing system immediately.

The goal is to answer one narrower question first:

> What sequencing model is sufficient to bring up the L3 safely, validate protocol settlement, and keep migration risk low?

This document therefore focuses on a pragmatic bootstrap model.

---

## 2. Design objective

The first sequencer model must satisfy five requirements:

1. allow the Orbit L3 to run reliably in devnet and early controlled environments,
2. preserve a clean upgrade path toward stronger decentralization later,
3. keep operational responsibility explicit,
4. avoid coupling early chain bring-up to advanced governance design,
5. and fit the current Temporal Gradient architecture, where most complexity still lives above the chain rather than inside the sequencer itself.

---

## 3. Recommended first model

The recommended first sequencing model is:

- **single operator sequencer**,
- **operator-controlled posting / batch submission flow**,
- **clear treasury accounting for fee destination**,
- **explicit future path to a multi-operator or governed sequencer set**.

This is the correct v0 posture.

---

## 4. Why single-operator sequencing is the right start

A single operator sequencer is appropriate for v0 because the project is still validating:

- contract placement,
- epoch settlement flow,
- fee surfaces,
- bridge assumptions,
- devnet operating procedures,
- and parent/L3 responsibilities.

Adding sequencer decentralization too early would mix together too many unsolved domains:

- chain operations,
- governance,
- bridge security,
- treasury routing,
- and operator incentives.

That would slow the project down and create unnecessary failure modes.

---

## 5. Sequencer responsibilities in v0

The v0 sequencer is responsible for chain ordering and normal Orbit execution, but the protocol must clearly define which application-layer tasks remain outside it.

### 5.1 Sequencer responsibilities

In scope for the sequencer:

- ordering L3 transactions,
- posting batches to the parent path required by Orbit,
- collecting execution fees,
- maintaining chain liveness for the v0 environment,
- exposing reliable RPC / submission access for protocol services.

### 5.2 Not sequencer responsibilities

Not in scope for the sequencer itself:

- building miner epochs,
- deciding protocol rewards,
- attesting Bitcoin anchors,
- running marketplace business logic,
- deciding governance policy,
- or validating off-chain entropy truth.

Those belong to protocol contracts and external services.

---

## 6. Recommended operator model

### v0 operator assumption

The first sequencer should be operated by the project team or a clearly designated operator domain.

This means:

- one accountable operator,
- one explicit deployment owner for devnet and early private rollout,
- one monitored sequencer stack,
- one controlled RPC surface.

### Why this matters

The first L3 needs operational clarity more than ideological decentralization.

The project should be able to answer immediately:

- who runs it,
- who rotates keys,
- who monitors health,
- who pays posting costs,
- who pauses or recovers during faults.

---

## 7. Sequencer fee routing

The first sequencer model should make fee routing explicit.

### Recommended v0 rule

Sequencer fee flows should be separated conceptually into:

1. **chain execution fees**
2. **protocol fees**

These are not the same thing.

### 7.1 Chain execution fees

These are the fees paid for L3 execution itself.

In v0:

- paid in `ETH`,
- retained in the sequencer / operator / treasury path defined by the Orbit deployment,
- accounted for separately from protocol revenue.

### 7.2 Protocol fees

These are fees paid by the application for protocol services, such as:

- randomness proof purchases,
- certificate issuance,
- future sponsor flows,
- future settlement service flows.

These should route through protocol contracts, not be confused with sequencer fee capture.

---

## 8. Relationship between sequencer and protocol treasury

The protocol should not assume, in v0, that all sequencer revenue automatically belongs to the same treasury bucket as protocol application revenue.

The clean first assumption is:

- sequencer operations have their own operational accounting,
- protocol contracts have their own economic accounting,
- any treasury unification can come later once real fee behavior is understood.

This is important because the chain operator may initially carry infrastructure cost that should be measured cleanly.

---

## 9. Sequencer and gas-token assumptions

The sequencing model must match the asset model.

For v0:

- gas is paid in `ETH`,
- sequencer accounting therefore naturally starts in `ETH`,
- `TGBT` remains the protocol settlement asset,
- future TGBT gas should be treated as a later sequencer-model change, not a bootstrap assumption.

This avoids introducing gas-token complexity before the execution path is proven.

---

## 10. RPC and access posture

The first L3 should expose a controlled RPC posture.

### Recommended v0 RPC posture

- one canonical public or semi-public RPC endpoint,
- one internal operator RPC endpoint if needed,
- one monitoring path for liveness, latency, and posting health,
- explicit rate limiting and authentication at the service edge when necessary.

### Reason

The L3 will likely first be consumed by:

- project-controlled services,
- settlement workers,
- indexers,
- development tooling,
- early operator flows.

This does not require a fully open infrastructure mesh on day one.

---

## 11. Interaction with the epoch pipeline

The sequencer model should be designed around the current off-chain epoch builder rather than forcing a redesign of the miner immediately.

The most important current integration path is the existing epoch builder in [l2-mining/randomness-api/epoch-builder.js](../l2-mining/randomness-api/epoch-builder.js).

### Recommended v0 flow

1. miners continue producing outputs through the current runtime,
2. the epoch builder continues aggregating outputs,
3. the builder or its L3 successor submits settlement transactions to the L3,
4. the sequencer orders those settlement transactions,
5. final protocol state becomes L3-native for the migrated surfaces.

This keeps sequencing simple and keeps the miner stable.

---

## 12. Sequencer interaction with migrated contracts

The v0 sequencer does not need custom application logic, but the protocol should document which contract actions are expected to dominate early L3 traffic.

Likely early traffic classes:

- epoch commit transactions,
- epoch finalization transactions,
- marketplace purchase settlement,
- certificate mint settlement,
- attestation recording,
- future sponsor accounting updates.

This suggests the sequencer must be optimized first for reliability and low operational surprise, not exotic throughput tricks.

---

## 13. Availability and failure assumptions

The first sequencer model must define what happens operationally when the sequencer is degraded.

### v0 assumptions

If the sequencer is degraded:

- miner execution does not need to stop immediately,
- off-chain aggregation may continue temporarily,
- settlement submission may queue,
- protocol services should surface delayed settlement state clearly,
- operator alerts must fire quickly.

### Important implication

The project should design the v0 application layer to tolerate short sequencer interruptions better than it tolerates state inconsistency.

Correctness matters more than aggressive liveness.

---

## 14. Recovery model

The first recovery model should be operationally simple.

Recommended assumptions:

- sequencer keys and deployment ownership are tightly controlled,
- health monitoring is explicit,
- restart / recovery runbooks are written before production rollout,
- service-side submission workers are idempotent where possible,
- settlement submissions are safe to retry.

The devnet must validate this recovery story.

---

## 15. Future decentralization path

The v0 design should not pretend single-operator sequencing is the end state.

A future path may include:

- governed sequencer rotation,
- sequencer committee or operator set,
- economic participation tied to `TGBT`,
- stronger fee-sharing rules,
- or a more explicit decentralization roadmap.

But that later work should happen after the following are proven first:

1. migrated contracts behave correctly,
2. settlement flows are economically coherent,
3. devnet and early test deployments are operationally stable,
4. bridge and asset assumptions are clearer.

---

## 16. What v0 explicitly does not require

The first sequencer model does **not** require:

- decentralized sequencing on day one,
- TGBT gas on day one,
- relay mesh integration,
- protocol-level private mempool design,
- MEV-minimization research as a precondition for L3 bring-up,
- validator-style staking economics,
- or a public permissionless sequencer market.

Those are future questions.

---

## 17. Required implementation outputs from this model

If this sequencer model is accepted, the next concrete outputs should be:

1. Orbit devnet operator topology,
2. sequencer key / treasury / posting account layout,
3. RPC endpoint plan,
4. submission-worker assumptions for migrated contracts,
5. health monitoring checklist,
6. failure and restart runbook.

These belong especially in the devnet plan.

---

## 18. Recommended v0 decision

The recommended v0 decision is:

- **single operator sequencer**,
- **ETH gas**,
- **clear operational accounting**,
- **protocol fees separated from sequencer fees**,
- **no premature decentralization dependency**,
- **design now for later evolution, but build the simplest reliable path first**.

This is the right sequencing model for the first real Temporal Gradient L3 iteration.

---

## 19. Summary

The L3 should begin with a simple, explicit, operator-controlled sequencer model.

That model is sufficient to:

- validate settlement migration,
- prove Orbit integration,
- test fee behavior,
- and keep the rest of the protocol stable while the L3 grows.

The first job of the sequencer is not ideological purity.

The first job of the sequencer is to make the L3 usable, testable, and migration-safe.
