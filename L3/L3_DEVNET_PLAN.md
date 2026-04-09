# Temporal Gradient L3 — Devnet Plan

Status: Draft v0  
Date: March 24, 2026

---

## 1. Purpose

This document defines the first devnet plan for the Temporal Gradient L3.

The goal is to create the smallest useful development environment that can validate the first L3 assumptions before any serious rollout.

The devnet is not just a deployment milestone.

It is the place where the project proves:

- chain topology assumptions,
- sequencer operations,
- contract migration order,
- fee behavior,
- off-chain service integration,
- and rollback safety.

---

## 2. Core devnet objective

The first devnet should prove one thing clearly:

> Temporal Gradient can settle its first-wave protocol flows on an Orbit-based L3 without breaking the current system.

That means the devnet should validate:

- first-wave L3 contracts,
- off-chain posting and indexing,
- epoch submission behavior,
- basic proof and certificate settlement,
- operator procedures.

It does **not** need to prove the entire future network.

---

## 3. Recommended devnet shape

The recommended first devnet is:

- **Arbitrum Orbit Rollup devnet**,
- **single operator sequencer**,
- **ETH gas**,
- **project-controlled RPC and services**,
- **hybrid integration with the current miner/epoch flow**.

This is enough for meaningful validation.

---

## 4. What the first devnet must include

### 4.1 Chain layer

The devnet must include:

- an Orbit-based L3 instance,
- sequencer access,
- deployer/operator keys,
- RPC access for contracts and services,
- block explorer or inspection tooling if practical.

### 4.2 Contract layer

The devnet must include the first-wave L3 contract set:

- epoch settlement contract/module,
- randomness proof settlement contract,
- certificate settlement contract,
- minimal payment/receipt/accounting helpers.

### 4.3 Service layer

The devnet must include:

- a settlement poster / transaction submitter,
- an L3-aware epoch worker or adapted builder,
- an indexing/query API,
- health/status reporting for the L3 app layer.

### 4.4 Integration layer

The devnet must include a clean way to feed the L3 from the current system:

- current miner runtime remains intact,
- current epoch formation logic is reused or lightly adapted,
- L3 becomes the settlement target for selected actions.

---

## 5. What the first devnet does not need

The first devnet does **not** need:

- TGBT native gas,
- decentralized sequencing,
- production bridge finality,
- full relay mesh,
- privacy stack,
- PQC enforcement,
- VDF lanes,
- mobile miner support,
- or complete marketplace feature richness.

Those are later-stage concerns.

---

## 6. Recommended build order

The devnet should be built in phases.

### Phase 1 — Documentation freeze

Before deployment work begins, the project should have:

- [L3/L3_SCOPE_V0.md](L3_SCOPE_V0.md)
- [L3/L3_ASSET_MODEL.md](L3_ASSET_MODEL.md)
- [L3/L3_SEQUENCER_MODEL.md](L3_SEQUENCER_MODEL.md)
- [L3/L3_CONTRACT_MIGRATION.md](L3_CONTRACT_MIGRATION.md)

Without these, devnet execution will drift.

### Phase 2 — Contract skeletons

Create the minimum L3 contract skeletons for:

- epoch settlement,
- proof purchase settlement,
- certificate issuance settlement,
- minimal payment/accounting utilities.

### Phase 3 — Service adaptation

Adapt existing off-chain tooling so the devnet can actually be exercised.

Most important source:

- [l2-mining/randomness-api/epoch-builder.js](../l2-mining/randomness-api/epoch-builder.js)

The devnet should reuse the existing epoch logic as much as possible.

### Phase 4 — End-to-end test loop

Run a full loop:

1. miner emits outputs,
2. epoch builder aggregates outputs,
3. L3 settlement transaction is posted,
4. epoch data becomes queryable on L3,
5. proof / certificate settlement paths are exercised,
6. operator verifies health and recovery behavior.

---

## 7. Devnet contract targets

The first devnet should focus on a narrow contract target list.

### Highest-priority contract reference

The existing batch settlement flow in [l2-mining/contracts/modules/BatchMiningModule.sol](../l2-mining/contracts/modules/BatchMiningModule.sol) is the strongest current reference for the first L3 settlement path.

### Recommended devnet contract outputs

The devnet should have:

1. **L3 epoch registry / settlement contract**
2. **L3 proof purchase settlement contract**
3. **L3 certificate issuance settlement contract**
4. **minimal treasury / accounting helpers**

This is enough to prove the concept.

---

## 8. Devnet service targets

The service layer should be kept simple.

### 8.1 Epoch settlement worker

Needed:

- read pending epoch data,
- submit the epoch settlement transaction to L3,
- track receipts,
- retry safely,
- expose status.

### 8.2 Indexing / query API

Needed:

- query settled epochs,
- query proof receipts,
- query certificate issuance state,
- expose health endpoints.

### 8.3 Operator scripts

Needed:

- deploy contracts,
- initialize system state,
- seed test assets/config where needed,
- run smoke tests,
- inspect receipts and balances.

---

## 9. Environment assumptions

The first devnet should assume:

- a project-controlled machine or environment,
- a single sequencer operator,
- one canonical RPC endpoint,
- one canonical deployment key set,
- explicit environment configuration checked into design docs but not with secrets.

This should be treated as a controlled systems test, not a public launch.

---

## 10. Suggested folder outputs for devnet work

The first devnet implementation will likely need new files such as:

- `L3/L3_SEQUENCER_MODEL.md`
- `L3/L3_CONTRACT_MIGRATION.md`
- `L3/L3_DEVNET_PLAN.md`
- `L3/contracts/` additions for first-wave L3 contracts
- `L3/devnet/` scripts or notes
- `L3/reference/` inputs already collected

Potential code outputs after this planning step may include:

- deployment scripts,
- contract initialization configs,
- service worker code,
- indexer or API skeletons.

---

## 11. Integration with the current system

The devnet should avoid forcing early changes into the live miner.

### Recommended rule

- keep the current miner/runtime unchanged,
- adapt the builder and settlement edge first,
- let the L3 absorb settlement responsibility gradually.

This means the current pipeline remains the upstream source of truth for generated outputs, while the devnet validates L3 settlement as a downstream target.

---

## 12. Devnet test scenarios

The devnet should validate a concrete list of scenarios.

### 12.1 Basic deployment

- contracts deploy successfully,
- initialization succeeds,
- RPC access works,
- operator accounts are funded and usable.

### 12.2 Epoch settlement flow

- outputs are collected,
- a root is formed,
- settlement is posted to L3,
- state becomes queryable,
- duplicate or invalid settlement attempts are rejected cleanly.

### 12.3 Proof purchase settlement

- a proof purchase flow succeeds,
- fee routing is visible,
- receipts are queryable.

### 12.4 Certificate issuance settlement

- certificate payment and issuance succeed,
- issuance records are visible,
- repeated or invalid operations fail correctly.

### 12.5 Failure handling

- posting failures are retried safely,
- duplicate submissions do not corrupt state,
- sequencer or RPC interruption is visible to operators,
- service restarts do not lose critical state.

---

## 13. Devnet success criteria

The first devnet is successful if all of the following are true:

1. the Orbit L3 runs reliably under the chosen operator model,
2. first-wave contracts deploy and initialize correctly,
3. the current epoch pipeline can be adapted to settle onto L3,
4. proof and certificate settlement work end to end,
5. operators can monitor and recover the system,
6. no risky canonical-token migration is required just to make the devnet work.

---

## 14. Explicit non-goals of the first devnet

The first devnet is **not** intended to prove:

- final bridge design,
- final token canonicality,
- public production readiness,
- decentralized sequencing,
- final protocol governance,
- or all future product features.

It is a validation environment for the first L3 wave.

---

## 15. Recommended first implementation tasks after this document

If this devnet plan is accepted, the next coding tasks should be:

1. create minimal L3 settlement contracts,
2. adapt or clone the epoch builder into an L3 settlement worker,
3. add deployment scripts for the devnet,
4. add an L3 query/indexing API,
5. define smoke-test scripts for end-to-end flow.

That is the shortest path to a working L3 prototype.

---

## 16. Recommended v0 decision

The recommended first devnet is:

- **Orbit Rollup devnet**,
- **single operator sequencer**,
- **ETH gas**,
- **current miner preserved**,
- **L3 used first for epoch, proof, and certificate settlement**,
- **no advanced roadmap features required for bring-up**.

This is the correct devnet shape for the current stage of the project.

---

## 17. Summary

The first devnet should prove the L3 as a settlement layer, not as a complete new universe.

If it can:

- deploy,
- accept first-wave settlement transactions,
- integrate with the current epoch flow,
- and survive basic operator failures,

then it has done its job.

That is the milestone the project needs next.
