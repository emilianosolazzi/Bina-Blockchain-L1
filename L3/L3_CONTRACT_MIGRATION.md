# Temporal Gradient L3 — Contract Migration

Status: Draft v0  
Date: March 24, 2026

---

## 1. Purpose

This document defines the first contract-migration strategy for the Temporal Gradient L3.

Its purpose is not to list every contract in the repository mechanically.

Its purpose is to answer the practical question:

> Which contracts should move first, which should remain outside the first L3 wave, and in what order should migration happen?

This is a migration-discipline document.

---

## 2. Core migration rule

The first L3 should migrate **execution surfaces**, not every historical component.

The migration order must follow three principles:

1. move what is most valuable for settlement first,
2. keep existing live paths stable during the transition,
3. avoid migrating old experimental or reference components as production assumptions.

---

## 3. Migration categories

For v0, contracts and contract-adjacent systems should be grouped into four categories.

### Category A — Move first

Contracts or modules that belong in the first L3 wave because they define core protocol settlement.

### Category B — Move later

Contracts that are likely to become L3-native eventually, but should not block the first migration.

### Category C — Keep external / hybrid initially

Contracts or functions that should remain outside the first L3 wave during bootstrap, even if they later integrate tightly.

### Category D — Reference only

Legacy designs, exploratory contracts, or concept references that should inform architecture but must not be treated as direct migration targets.

---

## 4. Category A — Move first

The first L3 wave should focus on the smallest useful settlement surface.

### 4.1 Epoch settlement path

Highest-priority migration target:

- the epoch / batch settlement flow currently represented by [l2-mining/contracts/modules/BatchMiningModule.sol](../l2-mining/contracts/modules/BatchMiningModule.sol)

Why it should move first:

- it is already a clean settlement-oriented contract surface,
- it maps naturally to L3-native batching economics,
- it matches the current off-chain builder flow in [l2-mining/randomness-api/epoch-builder.js](../l2-mining/randomness-api/epoch-builder.js),
- it is the clearest way to make the L3 immediately useful.

### 4.2 Randomness settlement / proof purchase surface

Move early:

- the proof marketplace settlement surface represented conceptually by `RandomnessShop`

Why:

- it is a clean consumer-facing economic sink,
- it benefits from lower-cost dedicated execution,
- it helps anchor `TGBT` utility on L3.

### 4.3 Certificate issuance settlement

Move early:

- the certificate mint / issuance settlement flow tied to provenance products

Why:

- certificates are settlement-heavy and product-facing,
- the L3 can provide cleaner execution and fee behavior for this than the current mixed environment.

### 4.4 Minimal L3-native accounting helpers

Move early only if required by the above:

- fee routing helpers,
- payment/accounting helpers,
- receipt/proof registry helpers,
- settlement-facing storage needed by the first-wave apps.

The rule is to move only what the first-wave apps actually need.

---

## 5. Category B — Move later

These are likely L3 candidates, but should follow only after the first settlement wave works.

### 5.1 More complete mining-related settlement logic

Move later:

- broader mining-accounting surfaces,
- more complex reimbursement or gas-sponsorship machinery,
- any logic that tightly couples miner runtime assumptions to chain migration.

### 5.2 Broader tokenomics execution

Move later:

- any tokenomics logic that changes canonical supply placement,
- deeper reward issuance surfaces that require stronger bridge or governance decisions.

### 5.3 More sophisticated attestation and storage settlement

Move later:

- richer storage attestation paths,
- advanced provenance settlement paths,
- anything that depends on first-wave marketplace and certificate behavior already being proven.

---

## 6. Category C — Keep external or hybrid initially

These should remain outside the first L3 wave, or remain hybrid, during bootstrap.

### 6.1 Miner runtime

Keep outside the first L3 wave:

- the active Rust miner runtime under [l2-mining/rust/temporal_gradient_core](../l2-mining/rust/temporal_gradient_core)

Why:

- it is already live and operationally sensitive,
- moving runtime behavior and chain settlement at the same time creates unnecessary risk,
- the L3 should first become a settlement destination, not a reason to rewrite the miner.

### 6.2 Bitcoin-dependent entropy paths

Keep external / hybrid initially:

- stale-block harvesting,
- dead-UTXO anchoring,
- Bitcoin provenance flows.

Why:

- these are external truth layers,
- they do not need to become Orbit-native to benefit the protocol,
- they should feed the L3, not be redefined by it.

### 6.3 Bridge canonicality logic

Keep conservative at first:

- canonical token placement,
- issuance migration,
- bridge authority placement.

Why:

- these are high-risk economic decisions,
- they should follow successful L3 settlement validation, not precede it.

### 6.4 Governance migration

Keep partial or external initially:

- ownership migration,
- module-governance migration,
- chain-sovereignty governance changes.

These should be staged carefully.

---

## 7. Category D — Reference only

The following retained items are useful architecture references but should **not** be treated as direct production migration targets.

### 7.1 Legacy bridge/export concepts

Reference only:

- [L3/contracts/BeaconExporter.sol](contracts/BeaconExporter.sol)

Use for:

- bridge/export concepts,
- quotas,
- fee/burn design ideas.

Do not use as-is.

### 7.2 Privacy / zk entropy concepts

Reference only:

- [L3/contracts/ZKEntropyVerifier.sol](contracts/ZKEntropyVerifier.sol)

Use for:

- future privacy lanes,
- proof-flow ideas,
- verifier design ideas.

Do not treat as a first-wave L3 contract.

### 7.3 Delay / VDF concepts

Reference only:

- [L3/contracts/VDFVerifier.sol](contracts/VDFVerifier.sol)

Use for:

- reveal-hardening ideas,
- anti-manipulation time-delay concepts.

Not for first-wave deployment.

### 7.4 SDK / service / scaling references

Reference only:

- [L3/reference/sdk-bridge-protocol.js](reference/sdk-bridge-protocol.js)
- [L3/reference/TemporalGradient_SDK.py](reference/TemporalGradient_SDK.py)
- [L3/reference/sharding/ShardManager.js](reference/sharding/ShardManager.js)
- [L3/reference/bridges/CrossChainEntropy.ts](reference/bridges/CrossChainEntropy.ts)
- [L3/reference/bridges/ChainProtectionBridge.rs](reference/bridges/ChainProtectionBridge.rs)

These should inform service architecture, not be dropped directly into the first L3 codebase.

---

## 8. Recommended migration order

The first migration should happen in explicit phases.

### Phase 0 — Design freeze for v0 scope

Before coding:

- finalize sequencer model,
- finalize contract migration boundaries,
- finalize devnet plan,
- freeze which first-wave contracts exist.

### Phase 1 — Minimal L3 settlement contracts

Build or adapt the smallest possible set for:

- epoch registry / batch settlement,
- proof purchase settlement,
- certificate settlement,
- fee routing / receipts needed for those flows.

### Phase 2 — Off-chain submission and indexing layer

Build the service path that talks to those contracts:

- batch poster,
- L3 indexer/API,
- proof and epoch query surface,
- devnet operator scripts.

### Phase 3 — Hybrid rollout

Keep current live system running while:

- selected settlement actions are exercised against L3,
- outputs and epochs are mirrored or routed cleanly,
- operator workflows are tested.

### Phase 4 — Expand only after validation

Only after the first settlement surfaces are stable should the project consider:

- more tokenomics movement,
- more mining-path migration,
- broader governance movement,
- deeper asset canonicality shifts.

---

## 9. What should not migrate in the first wave

The following should explicitly **not** be first-wave migration targets:

- legacy monoliths just because they exist,
- archived experimental Rust or Solidity references,
- advanced privacy-verifier logic,
- VDF or anti-manipulation modules,
- PQC enforcement layers,
- service sharding and relay mesh logic,
- bridge-complete final token canonicality logic.

This is critical to avoid turning the v0 L3 into a repository dump.

---

## 10. Contract rewrite posture

The first L3 should assume that many existing contracts are **concept sources**, not deployable migration artifacts.

Recommended rule:

- preserve ideas,
- rewrite cleanly for Orbit where needed,
- avoid dragging old upgrade patterns, stale dependencies, and mixed responsibilities into the new code.

Especially for L3:

- prefer smaller modules,
- separate accounting from policy,
- separate receipts from bridges,
- separate settlement from auxiliary verification logic.

---

## 11. Current files that most directly inform migration

The most important current files for migration planning are:

- [l2-mining/contracts/modules/BatchMiningModule.sol](../l2-mining/contracts/modules/BatchMiningModule.sol)
- [l2-mining/randomness-api/epoch-builder.js](../l2-mining/randomness-api/epoch-builder.js)
- [L3/L3_SCOPE_V0.md](L3_SCOPE_V0.md)
- [L3/L3_ASSET_MODEL.md](L3_ASSET_MODEL.md)
- [L3/L3_SEQUENCER_MODEL.md](L3_SEQUENCER_MODEL.md)

These define the real shape of first-wave movement more than the older archived files do.

---

## 12. Minimal first-wave contract set

If forced to choose the narrowest useful first-wave L3 contract set, it should be:

1. **Epoch settlement contract or module**
2. **Randomness proof settlement contract**
3. **Certificate issuance settlement contract**
4. **Small payment / accounting helpers needed by those**

Nothing else is required to prove the L3 concept.

---

## 13. Migration success criteria

The contract migration is successful when the project can demonstrate:

1. first-wave settlement contracts deploy and operate on the L3,
2. off-chain services can post and query state cleanly,
3. current miner and epoch workflows remain operational during migration,
4. fees and receipts are understandable,
5. no premature canonical token breakage is introduced.

---

## 14. Recommended v0 decision

The recommended v0 migration decision is:

- move **epoch settlement first**,
- move **proof marketplace settlement second**,
- move **certificate settlement with the same wave if possible**,
- keep miner runtime, Bitcoin truth layers, and canonical token migration outside the first wave,
- treat retained legacy contracts as reference only.

This is the cleanest migration path.

---

## 15. Summary

The L3 should not begin by migrating everything.

It should begin by migrating the contract surfaces that:

- are most settlement-centric,
- are easiest to reason about,
- and create immediate value for the protocol.

That means:

- epoch settlement,
- proof settlement,
- certificate settlement,
- and only the minimal helpers around them.

Everything else should wait until the first L3 wave proves itself.
