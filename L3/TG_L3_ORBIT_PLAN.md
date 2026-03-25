# Temporal Gradient L3 — Arbitrum Orbit Plan

Status: Draft v0  
Date: March 24, 2026

---

## 1. Goal

This document defines the first design pass for a sovereign Temporal Gradient L3 built with Arbitrum Orbit.

The immediate goal is not to migrate the entire protocol at once.

The immediate goal is to:

- define the right L3 base architecture,
- identify what should move first,
- keep migration risk low,
- preserve compatibility with the current Arbitrum-based deployment,
- and create a phased path toward a TGBT-native execution layer.

---

## 2. Recommended starting point

### Chosen baseline

The recommended first path is:

- **Arbitrum Orbit Rollup**
- **Parent chain: Arbitrum One**
- **Bootstrap gas token: ETH**
- **Future native gas token: TGBT**

### Why this is the right first step

This path is the lowest-friction evolution of the current system because:

- the live protocol is already centered on Arbitrum,
- current smart contract and miner assumptions already fit the Arbitrum ecosystem,
- Orbit gives a practical path to sovereign execution without redesigning everything at once,
- and ETH gas at bootstrap avoids unnecessary early complexity while the chain architecture is still being validated.

The protocol should not begin with custom gas-token behavior on day one of the L3. That can come after the chain is stable.

---

## 3. Design principles

The first L3 design should follow these principles:

1. **Minimize migration surface area**  
   Move only the most important protocol execution first.

2. **Preserve current economic logic**  
   TGBT supply, reward allocation, and core issuance constraints should remain consistent.

3. **Do not over-couple the first version**  
   Relay mesh, privacy transport, and advanced marketplace behavior should not be first-wave requirements.

4. **Keep Bitcoin integrations independent**  
   Bitcoin stale-block entropy and dead-UTXO anchoring should remain external truth layers, not Orbit-specific assumptions.

5. **Treat L3 as an execution upgrade, not a whitepaper slogan**  
   The first deliverable is architecture clarity, not premature deployment.

---

## 4. What the first L3 should include

### v0 scope

The first L3 should focus on the protocol components that benefit most from a dedicated execution layer:

- epoch settlement,
- proof marketplace settlement,
- certificate issuance settlement,
- miner-facing protocol payments,
- future gas reimbursement and sponsor accounting hooks.

### v0 should not include yet

The first L3 should **not** attempt to ship all roadmap items at once.

Explicitly out of initial scope:

- full verified egress mesh,
- private messaging,
- onion routing,
- decentralized CDN,
- relay admission engine,
- consensus gossip layer,
- mobile mining execution integration,
- advanced MPC / delegated compute transport.

Those can be layered on after the execution environment itself is stable.

---

## 5. Recommended chain topology

### Target topology

- **Ethereum** → ultimate trust and settlement anchor
- **Arbitrum One** → Orbit parent chain
- **Temporal Gradient L3 (Orbit)** → protocol execution chain
- **Bitcoin** → external entropy and provenance anchor

### Why Arbitrum One should be the parent

Using Arbitrum One as the parent chain keeps the protocol closest to its current live architecture.

Benefits:

- easiest migration path for existing contracts and operators,
- easier tooling continuity,
- faster design iteration,
- lower conceptual complexity than jumping directly to a more exotic stack,
- and good alignment with the protocol's current operational base.

---

## 6. Rollup vs AnyTrust

### Recommendation

Start with **Orbit Rollup**.

### Reasoning

Rollup mode is the better first fit because it maximizes trust minimization while the protocol is still defining its long-term security posture.

AnyTrust may later become interesting if:

- costs become a dominant constraint,
- transaction volume grows sharply,
- or some lower-cost service layer is needed.

But for the first serious L3 design, Rollup is the cleaner choice.

---

## 7. Gas token strategy

### Phase 1: ETH gas

At bootstrap, the L3 should use ETH for gas.

Reason:

- easier tooling,
- easier wallet compatibility,
- easier sequencer operations,
- easier debugging,
- and avoids mixing chain bring-up risk with token-economics risk.

### Phase 2: TGBT-native gas

After the Orbit chain is stable, the protocol can evaluate migration to **TGBT as the native gas token**.

That later step should only happen after:

- the sequencer path is stable,
- bridge behavior is understood,
- protocol contracts are fully tested on Orbit,
- and the token's settlement role is mature enough to support fee-denominated execution.

---

## 8. Canonical asset model

A clean asset model is required before writing chain-specific code.

### Initial recommendation

- Maintain a clear canonical definition of `TGBT`.
- Treat the first Orbit deployment as consuming a bridged or chain-local representation rather than redefining supply logic casually.
- Preserve the hard cap and issuance logic as protocol truths, not per-chain improvisations.

### Core question to answer early

The team must decide:

- whether canonical `TGBT` remains anchored to the current Arbitrum-side deployment initially,
- or whether the L3 eventually becomes the canonical execution home of the token economy.

This decision affects:

- bridge design,
- mint authority placement,
- marketplace settlement,
- and long-term chain sovereignty.

---

## 9. What should move first

### First-wave contracts

The first-wave contract set should likely include:

- a batch / epoch settlement module,
- the randomness proof marketplace,
- certificate registry contracts,
- and any L3-native settlement/accounting helpers needed for miner-facing flows.

### Contracts that may remain outside initially

At least in the first design phase, some logic may remain on the parent side or be mirrored conservatively:

- legacy compatibility surfaces,
- transitional bridge-aware token controls,
- some reimbursement logic,
- some governance transition logic.

---

## 10. Bridge and settlement questions

Before implementation, the L3 design should answer the following clearly:

1. Where is canonical `TGBT` defined?
2. How does value move between the current environment and the Orbit L3?
3. Which fees are paid locally on L3 vs settled elsewhere?
4. Which proofs must remain readable from parent-chain context?
5. Which contracts must be bridge-aware from day one?

These questions should be resolved in design documents before deployment work begins.

---

## 11. Sequencer model

The first L3 design also needs a clear sequencer model.

### Questions to define

- Who operates the sequencer initially?
- Is the first phase single-operator sequenced?
- How are sequencer fees routed?
- Is there a treasury sink?
- Is there future decentralization of sequencing?
- How does sequencing interact with sponsor economics and future TGBT fee design?

### Initial recommendation

Start simple:

- single controlled sequencer for dev/test and early private rollout,
- explicit accounting for sequencer fee destination,
- later design for decentralization once application behavior is stable.

---

## 12. Migration philosophy

The protocol should not attempt a "big bang" cutover.

Recommended migration shape:

### Step 1

Deploy a minimal Orbit dev environment.

### Step 2

Deploy a reduced contract suite that proves:

- epoch settlement works,
- marketplace settlement works,
- certificate settlement works,
- and fee accounting behaves as expected.

### Step 3

Test end-to-end operator flows:

- miner-related protocol actions,
- proof purchases,
- certificate minting,
- sponsor / reimbursement interactions where relevant.

### Step 4

Only after those flows are stable, expand the design to:

- broader economic migration,
- bridge finalization,
- gas-token transition,
- relay settlement,
- and deeper protocol sovereignty.

---

## 13. Proposed first practical build target

The first engineering milestone should be:

### Local / private Orbit devnet

Use it to validate:

- deployment flow,
- contract compatibility,
- fee model,
- settlement ordering,
- and the developer/operator workflow.

### Minimal L3 test deployment should prove

1. Epoch contract deployment and basic commit/finalize path
2. Proof marketplace deployment and payment path
3. Certificate registry deployment and mint path
4. TGBT-linked accounting assumptions
5. Sequencer / fee behavior under expected usage

This is enough for a real first milestone.

---

## 14. Suggested design documents to create next

After this draft, the next documents should be created one by one.

### Recommended next files

1. **L3_ARCHITECTURE.md**  
   High-level chain architecture and responsibilities

2. **L3_SCOPE_V0.md**  
   Exact first-wave scope and explicit exclusions

3. **L3_ASSET_MODEL.md**  
   Canonical TGBT, bridge logic, gas-token transition

4. **L3_SEQUENCER_MODEL.md**  
   Sequencer ownership, fees, and decentralization path

5. **L3_CONTRACT_MIGRATION.md**  
   Which contracts move first and how

6. **L3_DEVNET_PLAN.md**  
   Local devnet goals, tasks, and validation milestones

---

## 15. Immediate recommendation

The correct next step is **not** to deploy Orbit immediately.

The correct next step is to formalize the architecture in documents and then make decisions file by file.

Recommended order:

1. approve the high-level Orbit direction,
2. define v0 scope,
3. define the asset model,
4. define sequencer behavior,
5. define migration order,
6. then begin devnet setup.

---

## 16. Current working recommendation summary

If the protocol begins L3 design now, the recommended first answer is:

- **Arbitrum Orbit Rollup**
- **Parent: Arbitrum One**
- **Bootstrap gas: ETH**
- **Future gas: TGBT**
- **Move core settlement first**
- **Do not move relay/privacy stack first**
- **Design bridge and token model before deployment**
- **Validate on a private/local Orbit devnet first**

---

## 17. Open decisions

The following decisions still need explicit answers:

- Is the L3 initially private, partner-only, or public test-first?
- Does canonical `TGBT` remain where it is first, or migrate later?
- Which contracts remain parent-side initially?
- Does `UniversalMinerGasPool` stay parent-side, L3-side, or hybrid?
- When should TGBT replace ETH as gas?
- What exact fees should the sequencer collect and where should they flow?
- Which parts of the proof and certificate economy must be native to L3 from day one?

---

## 18. Conclusion

Yes, the L3 design should start now.

But it should start as a disciplined architecture phase, not as uncontrolled deployment work.

Arbitrum Orbit is the right base.

The right first version is conservative:

- Orbit Rollup,
- Arbitrum One parent,
- ETH gas first,
- TGBT gas later,
- and only core protocol settlement moving in the first wave.

This gives Temporal Gradient a realistic path from an application running on another execution layer to a sovereign execution environment designed for:

- heartbeat-backed security,
- entropy markets,
- proof settlement,
- certificate issuance,
- and eventually relay-native infrastructure.
