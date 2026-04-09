# Temporal Gradient L3 — Precompile Address Map

Status: Draft v0  
Date: March 25, 2026

---

## 1. Purpose

This document defines the first reserved address strategy for custom precompiles on the Temporal Gradient L3.

The goal is to reserve a clear, stable, and documented address range for future chain-level cryptographic acceleration.

---

## 2. Core rule

Precompile addresses must be:

- explicit,
- documented,
- stable once adopted,
- and easy to wrap from Solidity and Stylus.

---

## 3. Recommended strategy

The L3 should reserve a dedicated custom range for Temporal Gradient-specific precompiles rather than scattering addresses ad hoc.

Recommended policy:

- keep standard/common precompile expectations intact where applicable,
- reserve a separate custom range for L3-native extensions,
- document unused reserved slots before implementation.

---

## 4. Recommended reserved range

Recommended custom range for Temporal Gradient L3-native precompiles:

- `0x0000000000000000000000000000000000000100`
- through
- `0x00000000000000000000000000000000000001FF`

This gives a clean extension band for future accelerated primitives.

---

## 5. Proposed initial reservation map

### Signature verification

- `0x0000000000000000000000000000000000000100` → `ed25519_verify`

### Pairing and curve operations

- `0x0000000000000000000000000000000000000110` → `bls12_381_pairing`
- `0x0000000000000000000000000000000000000111` → `bls12_381_g1_add`
- `0x0000000000000000000000000000000000000112` → `bls12_381_g2_add`

### zk verification

- `0x0000000000000000000000000000000000000120` → `groth16_verify`
- `0x0000000000000000000000000000000000000121` → `plonk_verify`

---

## 6. Reserved but unassigned zones

To avoid future collisions, the map should reserve zones by category.

Suggested zones:

- `0x0100–0x010F` signature primitives
- `0x0110–0x011F` curve and pairing primitives
- `0x0120–0x012F` zk verification primitives
- `0x0130–0x01FF` future chain-native cryptographic extensions

---

## 7. Wrapper requirement

Every adopted address should eventually have:

- Solidity wrapper library support,
- Stylus Rust wrapper support,
- ABI/input-output documentation,
- gas model documentation.

The address map alone is not enough.

---

## 8. Stability rule

Once a precompile address is activated for a primitive, it should not be reassigned casually.

Address stability is part of developer trust.

---

## 9. Metadata recommendation

The chain should maintain a machine-readable registry later containing:

- address,
- name,
- status (`reserved`, `active`, `deprecated`),
- gas model reference,
- wrapper library reference,
- runtime implementation reference.

---

## 10. Summary

The L3 should reserve a clean custom precompile range now and expand within it carefully.

Recommended first mapping:

- `0x0100` → `ed25519_verify`
- `0x0110` → `bls12_381_pairing`
- `0x0111` → `bls12_381_g1_add`
- `0x0112` → `bls12_381_g2_add`
- `0x0120` → `groth16_verify`
- `0x0121` → `plonk_verify`

That gives the chain a disciplined foundation for future crypto acceleration.
