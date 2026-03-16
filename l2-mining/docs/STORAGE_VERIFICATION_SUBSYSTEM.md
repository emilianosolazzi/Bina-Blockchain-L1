# Storage Verification Subsystem

This is a real subsystem worth building, not R&D to archive.

## Why it matters

The strongest product value is provider reputation and slashing input.

The network has three actors that need trust signals:

- miners — are they producing valid randomness?
- storage providers — are they actually storing epoch data?
- randomness consumers — are they using outputs correctly?

The protocol already has miner slashing paths in tokenomics. It does not have an equivalent storage-verification path. This subsystem fills that gap.

## Build priority

### 1. Randomness archive verification

This is the foundation.

Flow:

1. epoch finalized on-chain
2. epoch data pinned to IPFS or other supported storage
3. storage verifier challenges the pin
4. provider proves retrieval and integrity
5. verifier emits or submits an attestation for settlement

If the archive cannot be proven retrievable, downstream reputation and settlement logic should not proceed.

### 2. Provider reputation and slashing input

This is the best immediate economic offering.

- failed proof → reputation score drops
- missed challenge → recommend TGBT slash
- high uptime / repeated success → improve reputation and eligibility for bonuses

This creates a direct incentive to keep epoch data durable and retrievable.

### 3. Off-chain validation before on-chain settlement

Before `finalizeEpoch()` or equivalent reward settlement:

- verify epoch data is actually stored
- verify Merkle proofs check out
- produce an attestation
- only then allow downstream minting or settlement

This is the trust guarantee that makes the randomness API credible for institutional users.

## What to skip for now

- Bloom filter layer — premature optimization
- temporal consistency extras — useful later, not core for first release
- full multi-provider integration scaffolding — build one production path first

## MVP output

The one-line pitch to dApp developers:

> Every randomness output we sell you is provably stored, retrievable, and integrity-verified before the reward was minted. Here is the on-chain attestation.

## Recommended implementation shape

- Rust verifier inside the active `temporal_gradient_core` crate
- challenge generation tied to beacon output
- verification result persisted off-chain
- provider reputation tracked off-chain first
- slash recommendation generated off-chain
- compact attestation submitted on-chain later

## Current wiring decision

The active implementation lives in the Rust runtime crate at [l2-mining/rust/temporal_gradient_core/src/storage_verification.rs](l2-mining/rust/temporal_gradient_core/src/storage_verification.rs).

Initial scope intentionally prioritizes:

- archive verification
- provider reputation/slash recommendation
- settlement gating

and intentionally defers:

- bloom-filter indexing
- broad storage-provider feature expansion
- advanced temporal heuristics