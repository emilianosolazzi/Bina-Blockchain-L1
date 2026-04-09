# Temporal Gradient L3 — Stylus Readiness

Status: Draft v0  
Date: March 25, 2026

---

## 1. Purpose

This document defines what it means for the Temporal Gradient L3 to be Stylus-ready.

The goal is not only to support one extra language.

The goal is to prepare the L3 for a **multi-runtime contract model** where:

- Solidity contracts remain first-class,
- Stylus contracts can be introduced safely,
- ABI compatibility remains stable for dapps,
- and Rust/WASM becomes a practical path for selected modules.

---

## 2. Core readiness principle

Being Stylus-ready means the chain and developer stack are prepared for:

1. **WASM contract execution through Stylus**,
2. **Solidity ABI interoperability**,
3. **Rust-first Stylus development**,
4. **safe coexistence of EVM and Stylus contracts**,
5. **clear deployment, activation, and verification flow**.

---

## 3. Why Stylus matters for this L3

Stylus is a strong fit for Temporal Gradient because the project already values:

- specialized execution,
- proof-heavy logic,
- verification-oriented design,
- and future modules that may benefit from Rust or WASM ergonomics.

For this chain, Stylus is not a branding feature.

It is a path to safer and more expressive contract development for selected components.

---

## 4. Practical assumptions from Stylus docs

The practical preparation points are:

- use `cargo stylus` for project creation and validation,
- pin `wasm32-unknown-unknown` in the Rust toolchain,
- use `cargo stylus check` before deployment,
- export Solidity ABI with `cargo stylus export-abi`,
- keep contracts EVM-callable through standard tooling.

This means Stylus readiness is mainly a **tooling, interface, and operational discipline** problem.

---

## 5. Recommended v0 Stylus posture

The recommended first posture is:

- **Solidity remains fully supported**,
- **Rust is the first Stylus language target**,
- **ABI compatibility is mandatory**,
- **Stylus is introduced module by module**,
- **no requirement to rewrite existing Solidity surfaces immediately**.

This is the lowest-risk path.

---

## 6. Stylus-ready chain requirements

To be Stylus-ready, the L3 should prepare for:

### 6.1 Toolchain readiness

- Rust toolchain pinned,
- `wasm32-unknown-unknown` target pinned,
- `cargo stylus` supported in docs and dev workflow,
- ABI export workflow defined,
- contract artifact conventions defined.

### 6.2 Interface readiness

- stable contract interfaces,
- Solidity ABI as the main compatibility surface,
- language-neutral contract specs,
- shared event and error semantics where practical.

### 6.3 Operational readiness

- deploy vs activate flow documented,
- Stylus validation checks required before deploy,
- contract metadata records implementation runtime,
- monitoring distinguishes Solidity vs Stylus modules.

---

## 7. What the L3 should standardize now

The L3 should standardize these items before heavy Stylus adoption:

1. contract ABI conventions,
2. artifact naming,
3. activation procedure,
4. runtime labeling,
5. language admission policy,
6. verification and rollback expectations.

---

## 8. Recommended contract runtime labels

Each contract should be labeled by runtime type.

Recommended labels:

- `solidity-evm`
- `stylus-rust`
- future labels only after real need emerges

This helps operators and dapps understand what they are interacting with.

---

## 9. Best first Stylus candidates

The first Stylus modules should be narrow and high-fit.

Good candidates:

- verification helpers,
- proof-processing helpers,
- math-heavy utilities,
- randomness transformation logic,
- compression or parsing helpers.

Poor first candidates:

- every core contract at once,
- governance-critical modules with immature interfaces,
- large migrations done for style instead of need.

---

## 10. ABI rule

The most important Stylus readiness rule is:

> implementation language must not become a burden for dapp integration.

That means:

- ABI remains stable,
- clients use normal EVM tooling,
- exported ABI is treated as the contract surface,
- runtime differences remain behind the interface.

---

## 11. Developer readiness requirements

Before calling the L3 Stylus-ready for builders, the project should provide:

- one Stylus example project,
- a Rust toolchain file,
- instructions for `cargo stylus check`,
- instructions for ABI export,
- example EVM interaction using normal tools,
- clear statement that Rust is the first supported Stylus language path.

---

## 12. Security implications

Stylus readiness should not weaken the security model.

Required rules:

- Stylus contracts must pass validation checks,
- runtime-specific risk should be documented,
- language novelty should not bypass review discipline,
- ABI compatibility should not hide unsafe behavior changes.

---

## 13. Dapp implications

For dapps, Stylus readiness should feel simple:

- same ABI consumption model,
- same call pattern,
- same tooling compatibility,
- improved implementation flexibility under the hood.

If dapps need special handling for every Stylus contract, readiness is incomplete.

---

## 14. Summary

The right Stylus-ready posture is:

- Solidity-compatible,
- Rust-first for Stylus,
- ABI-stable,
- gradual,
- and operationally explicit.

That makes the L3 ready for different contract languages without fragmenting the developer experience.
