# Temporal Gradient L3 — Precompile Strategy

Status: Draft v0  
Date: March 25, 2026

---

## 1. Purpose

This document defines the first precompile strategy for the Temporal Gradient L3.

The goal is to prepare the chain for high-value native cryptographic acceleration without turning the execution environment into an uncontrolled collection of special cases.

---

## 2. Core strategy rule

The first precompile rule is:

> use precompiles for shared, high-cost, high-value primitives that benefit all contract runtimes.

That means precompiles should exist for chain-wide cryptographic acceleration, not for arbitrary application logic.

---

## 3. Why precompiles matter for this L3

This L3 is moving toward:

- Solidity support,
- Stylus Rust support,
- proof-heavy workflows,
- verification-oriented dapps,
- and stronger randomness/security guarantees.

Precompiles are the right place for expensive cryptographic primitives because they provide:

- lower execution overhead,
- shared access for Solidity and Stylus,
- more predictable gas metering,
- and a language-neutral acceleration layer.

---

## 4. Architectural placement

The L3 should use a three-layer execution model:

1. **contracts** — Solidity and Stylus ABI-facing logic
2. **precompiles** — shared native cryptographic primitives
3. **node/runtime implementation** — chain-level implementation of those primitives

This keeps application logic out of the precompile layer while still enabling fast verification and signature operations.

---

## 5. Selection criteria

A primitive should only become a precompile if it meets most of these criteria:

- high computational cost in contract code,
- reused across multiple apps or modules,
- strong fit for chain-level acceleration,
- stable enough semantics to standardize,
- meaningful value to randomness, verification, or identity flows,
- feasible gas metering and abuse control.

---

## 6. Recommended v0 precompile posture

The recommended first posture is:

- **define the strategy before implementing many precompiles**,
- **reserve address space early**,
- **standardize call conventions**,
- **introduce only a small number of high-value primitives first**,
- **expose wrappers for Solidity and Stylus later**.

This is safer than implementing a large suite immediately.

---

## 7. Best first candidate families

The best candidate families for this chain are:

### 7.1 Signature verification

- `ed25519_verify`
- future BLS-related verification helpers

### 7.2 Pairing and curve operations

- `bls12_381_pairing`
- `bls12_381_g1_add`
- `bls12_381_g2_add`
- related multi-scalar or helper operations only if justified

### 7.3 zk verification

- `groth16_verify`
- `plonk_verify`

These fit the chain's direction better than a broad precompile grab-bag.

---

## 8. What not to precompile early

The chain should avoid early precompiles for:

- application-specific business logic,
- features without repeatable ABI/use patterns,
- primitives with unclear gas economics,
- and primitives added only for marketing breadth.

---

## 9. ABI and calling model

Precompiles should be callable from both Solidity and Stylus through a stable interface model.

Recommended rule:

- fixed precompile addresses,
- stable input encoding,
- stable output encoding,
- clear success/failure semantics,
- wrapper libraries for each contract runtime.

The precompile surface must be easier to consume than re-implementing the primitive in contract code.

---

## 10. Gas and abuse model

Every precompile needs a gas strategy before production use.

That means defining:

- base cost,
- size-dependent cost if relevant,
- failure-cost semantics,
- DOS considerations,
- and throughput impact on the chain.

A fast primitive with poor metering becomes a denial-of-service risk.

---

## 11. Runtime implementation posture

The implementation language in the node/runtime layer matters less than the chain-level contract it exposes.

If the runtime implementation is done in Go or another native layer, the important rule is:

- the contract-facing interface must remain stable,
- the address map must remain explicit,
- and wrappers must preserve compatibility across runtimes.

---

## 12. Relationship to Stylus and multi-language support

Precompiles complement Stylus.

They do not replace it.

The right model is:

- Solidity for broad EVM compatibility,
- Stylus Rust for richer WASM contracts,
- precompiles for shared heavy cryptographic acceleration.

That makes the L3 stronger as a multi-runtime chain.

---

## 13. Rollout plan

Recommended rollout:

### Phase 1

- strategy doc
- address reservation
- wrapper design
- gas model design

### Phase 2

- implement one or two highest-value precompiles
- add Solidity wrapper
- add Stylus wrapper
- benchmark and meter

### Phase 3

- expand only after real usage and performance data

---

## 14. Summary

The right precompile strategy is not “support every cryptographic primitive.”

It is:

- reserve the right primitives,
- standardize the interface,
- meter them safely,
- and use them as a shared acceleration layer for Solidity and Stylus.

That is the correct chain-level crypto strategy for this L3.
