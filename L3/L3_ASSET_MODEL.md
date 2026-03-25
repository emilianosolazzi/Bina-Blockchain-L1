# Temporal Gradient L3 — Asset Model

Status: Draft v0  
Date: March 24, 2026

---

## 1. Purpose

This document defines the first asset-model assumptions for the Temporal Gradient L3.

The goal is to answer the hardest early economic design question:

> How should `TGBT`, fees, settlement, and gas behave across the current environment and the future Orbit L3?

This is a design document, not a final governance commitment.

---

## 2. Primary design objective

The L3 asset model must do four things at once:

1. preserve the credibility of the current `TGBT` economic model,
2. avoid accidental fragmentation of token meaning across chains,
3. allow the L3 to bootstrap safely before full sovereignty,
4. create a clear path toward future TGBT-native execution.

---

## 3. Baseline recommendation

The recommended first asset model is:

- **ETH as the bootstrap gas token for the Orbit L3**,
- **TGBT as the protocol economic asset from day one**,
- **canonical TGBT supply logic preserved outside ad hoc L3 redefinition**, 
- **staged migration toward TGBT-native gas only after the L3 is stable**.

This is the safest first design.

---

## 4. Asset roles

The protocol should distinguish clearly between **gas assets** and **protocol assets**.

### 4.1 Gas asset

The gas asset is the token used to pay for execution on the chain.

For the first L3 phase, this should be:

- `ETH`

Reason:

- easiest tooling support,
- easiest wallet support,
- simplest sequencer operations,
- and least risk during chain bring-up.

### 4.2 Protocol asset

The protocol asset is the token that expresses protocol-specific economic value.

That asset remains:

- `TGBT`

TGBT should remain the asset for:

- mining reward economics,
- stale-block reward economics,
- proof marketplace payments,
- certificate issuance payments,
- future sponsor and settlement flows,
- future staking or bonded service roles,
- and later, potentially gas.

---

## 5. Canonical TGBT principle

The most important rule is this:

> `TGBT` must have one coherent canonical economic meaning.

The protocol should not casually create multiple equally authoritative supply domains.

That means the design must preserve:

- one hard cap model,
- one authoritative issuance logic,
- one coherent reward accounting model,
- one defensible canonical supply story.

---

## 6. Recommended first canonicality assumption

For the initial L3 design phase, the recommended assumption is:

- treat the **current Arbitrum-centered TGBT system as canonical first**,
- and let the Orbit L3 consume a bridged / represented form of TGBT during the bootstrap phase.

This avoids premature supply migration risk.

### Why this is safer

Because the current protocol already has:

- live tokenomics assumptions,
- current on-chain addresses and module relationships,
- existing miner-facing economic flows,
- and a known Arbitrum-based operating context.

Moving canonicality too early would multiply risk across:

- supply accounting,
- bridge assumptions,
- mint authority,
- marketplace settlement,
- and governance migration.

---

## 7. Phased asset model

The L3 asset model should be phased.

### Phase A — Bootstrap phase

- Orbit L3 uses `ETH` for gas.
- `TGBT` remains the protocol asset.
- Canonical TGBT remains tied to the current environment.
- L3 consumes a bridged or chain-local representation for app settlement.

### Phase B — Growth phase

- More protocol settlement moves onto L3.
- Marketplace and certificate flows become L3-native.
- Bridge logic becomes more important.
- Canonicality assumptions are revisited with more real operational data.

### Phase C — Sovereign phase

- Temporal Gradient may choose to make the L3 the dominant execution home of protocol economics.
- TGBT may become the native gas token.
- Canonicality may migrate if governance, bridge security, and settlement design are strong enough.

This should be a later decision, not an early assumption.

---

## 8. What TGBT must represent on L3

Regardless of where canonicality sits initially, TGBT on the L3 should preserve the same meaning it has in the broader protocol.

On L3, TGBT should represent:

- payment for randomness proofs,
- payment for certificate issuance,
- settlement unit for protocol services,
- future unit for sponsor reimbursements or service accounting,
- and eventually the likely candidate for gas if the L3 matures into the sovereign execution layer.

The L3 should therefore treat TGBT as a **core settlement asset**, even before it becomes a gas asset.

---

## 9. Bridge-model questions that must be answered

Before the L3 asset model becomes implementation work, the following questions must be resolved.

### 9.1 Canonical origin

- Where does canonical `TGBT` live during bootstrap?
- Does that change later?

### 9.2 Representation on L3

- Is L3 TGBT bridged from the current chain?
- Is it minted as a canonical wrapped representation?
- What authority controls that representation?

### 9.3 Supply integrity

- How is total supply integrity preserved across environments?
- How are bridge mint / burn / escrow semantics enforced?
- How is double-accounting prevented?

### 9.4 Settlement domains

- Which payments settle entirely on L3?
- Which values must still reconcile with parent-side state?
- Which reward or marketplace paths remain hybrid for a while?

### 9.5 Upgrade path

- What exact event would justify migrating from ETH gas to TGBT gas?
- What exact event would justify changing canonical token placement?

---

## 10. Recommended bootstrap bridge posture

The first bridge posture should be conservative.

Recommended assumptions:

- use the bridge to make TGBT economically available on L3,
- do not redesign tokenomics around the bridge initially,
- do not move issuance logic blindly into the L3 first wave,
- do not create multiple uncontrolled mint domains,
- and do not tie chain bring-up to the final bridge model immediately.

This keeps the L3 bootstrappable.

---

## 11. Relationship between tokenomics and the L3

A critical distinction must be preserved:

> the L3 is an execution environment; tokenomics is an economic truth layer.

In the bootstrap phase, the L3 should not casually alter:

- the hard cap,
- mining allocation logic,
- stale-block allocation logic,
- or core reward semantics.

Instead, the L3 should execute application settlement in a way that remains consistent with those truths.

This matters for credibility.

---

## 12. TGBT as gas — future conditions

The protocol may eventually move to `TGBT` as the native gas token.

That should only happen after all of the following are true:

1. Orbit chain operations are stable.
2. Sequencer behavior is well understood.
3. Wallet / tooling impact is acceptable.
4. Bridge logic is mature.
5. Protocol service demand in TGBT is already real.
6. Governance is comfortable that the token can safely play both roles:
   - protocol settlement asset,
   - chain execution asset.

Until then, ETH gas remains the safer choice.

---

## 13. Why ETH-first is not a contradiction

Using ETH for gas first does **not** weaken the long-term L3 vision.

It is a sequencing choice.

It means:

- the chain can be validated without token-gas complexity,
- economic logic stays readable,
- migration risk is reduced,
- and the protocol keeps a clean path toward future sovereignty.

The point is not to avoid TGBT gas forever.

The point is to adopt it only when the L3 is ready.

---

## 14. Marketplace and certificate settlement model

The strongest first use of TGBT on L3 is likely **application settlement**, not gas.

That means the first L3 should prioritize TGBT for:

- randomness proof purchases,
- premium proof tiers,
- certificate minting,
- provenance-related economic flows,
- protocol settlement logic where TGBT already has product meaning.

This makes L3 TGBT useful immediately without forcing it into every chain-level role at once.

---

## 15. Miner and sponsor implications

The asset model also affects miners and sponsors.

### For miners

The model should preserve:

- reward integrity,
- clear token meaning,
- a path from current economic participation to L3 participation,
- and no confusion about what TGBT on L3 actually represents.

### For sponsors

The model should allow future sponsor systems to understand clearly:

- what value they reimburse,
- which fees are gas fees,
- which fees are protocol service fees,
- and which flows are chain-execution costs versus protocol-economy costs.

This separation is important for future `UniversalMinerGasPool`-style integrations.

---

## 16. Risks to avoid

The L3 asset model must avoid the following mistakes.

### 16.1 Premature gas-token migration

Do not force TGBT to become gas before:

- chain stability,
- bridge maturity,
- and clear operational readiness.

### 16.2 Supply ambiguity

Do not create a situation where users cannot answer:

- what the real TGBT supply is,
- which chain is canonical,
- or how bridged value maps to protocol truth.

### 16.3 Hidden hybrid complexity

Do not leave parent/L3 responsibility ambiguous.

Hybrid periods are acceptable.

Unclear hybrid periods are not.

### 16.4 Overloading the first wave

Do not force the first L3 deployment to solve:

- final bridge architecture,
- final governance architecture,
- final gas architecture,
- and final settlement architecture

all at once.

---

## 17. Recommended v0 asset-model statement

The recommended current position is:

- `TGBT` remains the protocol's canonical economic asset.
- The first Orbit L3 should use `ETH` for gas.
- The L3 should use `TGBT` for protocol settlement from the start.
- Canonical supply logic should remain anchored to the current environment during bootstrap.
- A bridge should provide L3 economic usability without redefining token truth casually.
- Migration to TGBT-native gas should be a later phase after successful L3 stabilization.

---

## 18. What needs to be specified next

After this document, the next asset-related files or sections should specify:

1. exact canonical TGBT assumption,
2. initial bridge architecture,
3. escrow / mint / burn flow model,
4. how L3 marketplace payments settle,
5. how certificate payments settle,
6. whether any reward logic should be mirrored on L3 in the first wave,
7. what event triggers evaluation of TGBT-native gas.

---

## 19. Conclusion

The first L3 asset model should be conservative and legible.

That means:

- ETH gas first,
- TGBT settlement from day one,
- canonical TGBT logic preserved,
- bridge model used carefully,
- TGBT-native gas later.

This gives Temporal Gradient a realistic path toward sovereign execution without putting the token model at risk during the first migration phase.
