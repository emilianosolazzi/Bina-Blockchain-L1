# Temporal Gradient L3 Stylus Devnet Scaffold

This folder prepares the L3 devnet for Stylus-compatible contracts.

## Goals

- keep the L3 ABI-first and EVM-compatible
- prepare a Rust-first Stylus workflow
- make it easy to validate WASM contracts before any deployment
- keep Solidity and Stylus contracts compatible for dapps

## Included

- `rust-toolchain.toml` — pins stable Rust and the `wasm32-unknown-unknown` target
- `example-counter/` — minimal Stylus Rust example scaffold

## Recommended local setup

From the Stylus quickstart, the practical requirements are:

- Rust toolchain
- Docker
- Foundry `cast`
- `cargo stylus`
- a Nitro devnode when actually testing deployment/activation

## Typical workflow

1. Install Stylus tooling
2. Build the example contract for WASM
3. Run `cargo stylus check`
4. Export ABI with `cargo stylus export-abi`
5. Interact with the contract through normal EVM tooling

## Important rule

The L3 should treat Stylus as:

- Rust-first initially
- ABI-compatible always
- gradual in adoption
- module-specific, not chain-wide all at once

## Example future uses

Good future Stylus candidates:

- verification helpers
- proof-processing helpers
- randomness transformation modules
- math-heavy helper contracts
