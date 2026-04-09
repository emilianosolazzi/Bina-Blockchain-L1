# Temporal Gradient L3 — Crypto Acceleration Plan

Status: Draft v0  
Date: March 25, 2026

---

## 1. Purpose

This document defines the first crypto acceleration plan for the Temporal Gradient L3.

The goal is to identify which cryptographic workloads should be accelerated at the chain level and in what order.

---

## 2. Core objective

The L3 should accelerate cryptographic operations that are:

- expensive in contract execution,
- broadly useful to the protocol or dapps,
- and aligned with the chain's specialization around randomness, verification, provenance, and high-trust application flows.

---

## 3. Acceleration layers

The chain should think about crypto acceleration in layers:

1. **contract-level logic**
2. **Stylus/WASM implementation**
3. **native precompiles**

The fastest and most reusable primitives belong in the precompile layer.

---

## 4. Highest-priority areas

### 4.1 Signature verification

Priority candidate:

- `ed25519_verify`

Why:

- useful for identity and signing flows,
- useful for cross-system attestations,
- attractive for apps that want signature schemes beyond typical EVM-native assumptions.

### 4.2 Pairing operations

Priority candidate:

- `bls12_381_pairing`

Why:

- useful for proof systems,
- useful for aggregation-oriented cryptography,
- useful for future verification-heavy workflows.

### 4.3 zk proof verification

Priority candidates:

- `groth16_verify`
- `plonk_verify`

Why:

- directly aligned with proof-oriented dapps,
- useful for future verification modules,
- useful if the L3 wants to support stronger verification ecosystems.

---

## 5. Suggested rollout order

Recommended order:

1. `ed25519_verify`
2. `bls12_381_pairing`
3. `groth16_verify`
4. `plonk_verify`

This order provides fast practical value while building toward heavier proof support.

---

## 6. Why not accelerate everything at once

A broad acceleration surface increases:

- maintenance cost,
- gas-design risk,
- DOS risk,
- implementation complexity,
- and audit complexity.

The chain should grow this surface only where it has clear product value.

---

## 7. Contract-facing model

Every accelerated primitive should be consumable through:

- Solidity wrappers,
- Stylus Rust wrappers,
- stable ABI-oriented helper surfaces.

Dapps should not need to understand runtime internals to benefit.

---

## 8. Operational requirements

Before production use, each accelerated primitive should have:

- benchmark data,
- gas model,
- failure semantics,
- wrapper libraries,
- abuse analysis,
- integration examples.

---

## 9. Product fit for this L3

Crypto acceleration strengthens the chain's positioning in:

- verifiable randomness,
- proof-bearing applications,
- signature-oriented identity flows,
- provenance and certificate systems,
- future privacy-aware or zk-aware modules.

This is especially valuable for a specialized L3 rather than a generic chain.

---

## 10. Summary

The crypto acceleration plan should focus on a narrow and high-value set of primitives first.

For this L3, that means:

- signature verification,
- pairing support,
- and zk verification,

implemented as shared native acceleration instead of repeated contract-level heavy computation.
