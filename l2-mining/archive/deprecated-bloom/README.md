# Deprecated bloom-filter components

Archived on 2026-03-21.

Reason:
- bloom-filter based output uniqueness tracking was removed from the active L2 mining path
- exact `usedOutputs` tracking is now the canonical mechanism
- bloom-filter false positives and extra operational complexity were intentionally removed from the on-chain system

Archived source snapshots:
- `BloomFilterLib.sol` moved here from the active contracts path

Removed active dependencies:
- `contracts/BloomFilterLib.sol`
- `contracts/FilterManager.sol`
- legacy bloom-filter test harness files tied to the old monolith path

Notes:
- the modular mining path now relies on exact duplicate detection via `usedOutputs`
- this archive is for reference only and is not part of the active build
