# Temporal Gradient L3 — Multi-Language Contract Model

Status: Draft v0  
Date: March 25, 2026

---

## 1. Purpose

This document defines the first multi-language contract model for the Temporal Gradient L3.

The goal is to allow different implementation languages without turning the chain into a fragmented developer environment.

---

## 2. Core model

The recommended model is:

- **one chain**,
- **one ABI-centric contract surface**,
- **multiple implementation runtimes**,
- **strict interface discipline**.

This means the language is an implementation detail unless a contract explicitly declares otherwise.

---

## 3. Runtime layers

The first model should distinguish between:

1. **Solidity / EVM contracts**
2. **Stylus / Rust contracts**

Future runtimes should not be assumed until they are actually supported safely.

---

## 4. Why ABI matters more than language

Dapps integrate through:

- ABI,
- events,
- errors,
- transaction semantics,
- read/write behavior.

They do not benefit if a chain advertises many languages but breaks consistency.

So the L3 should be **ABI-first, language-second**.

---

## 5. Recommended v0 language policy

### Supported now

- Solidity
- Stylus Rust

### Not yet first-class

- any other language claim that lacks a real toolchain and validation path

This keeps the project honest and practical.

---

## 6. Contract classification model

Each deployed contract should be classified by:

- `interfaceId` or logical contract role,
- `runtimeType`,
- ABI version,
- deployment artifact reference,
- activation / verification status where relevant.

Recommended runtime values:

- `solidity-evm`
- `stylus-rust`

---

## 7. Interface discipline

Before a contract is implemented in a different runtime, the project should define:

1. method signatures,
2. event signatures,
3. error behavior,
4. expected storage semantics where externally relevant,
5. compatibility expectations for clients.

Only then should runtime-specific implementation begin.

---

## 8. Migration model

The chain should not migrate modules to Stylus just because it can.

Recommended rule:

- keep stable Solidity surfaces where appropriate,
- add Stylus where it brings clear implementation value,
- preserve client-facing ABI continuity.

---

## 9. Best use of different languages

### Solidity best fit

- widely integrated settlement modules,
- straightforward application contracts,
- ecosystem-standard interfaces.

### Stylus Rust best fit

- logic that benefits from Rust ergonomics,
- verification-heavy utilities,
- computation-heavy modules,
- specialized helper contracts.

---

## 10. Operational model

Operators should track language/runtime differences in deployment metadata.

Recommended metadata per contract:

- name
- role
- address
- runtime type
- ABI path
- source repo path
- deployment tx hash
- activation status for Stylus

---

## 11. Dapp compatibility model

Dapps should not need to care about implementation language in most cases.

The default expectation should be:

- same EVM-style addressing,
- same ABI consumption pattern,
- same normal tooling,
- same event indexing flow.

If runtime-specific behavior matters, it must be documented explicitly.

---

## 12. Risk controls

A multi-language contract model needs extra discipline.

Required controls:

- no runtime-specific hidden behavior changes,
- no undocumented ABI divergence,
- no skipping verification because the runtime is newer,
- no automatic assumption that Stylus means better for every module.

---

## 13. Admission rule for new runtime types

New runtime types should be admitted only if they have:

- real toolchain support,
- repeatable validation,
- ABI export path,
- operator documentation,
- and clear value for the protocol.

---

## 14. Summary

The right multi-language model is not “support everything.”

It is:

- one stable contract surface,
- multiple carefully admitted runtimes,
- and strict compatibility discipline.

That is how the L3 can accept different languages without harming reliability or dapp usability.
