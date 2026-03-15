# Production files

This file marks which Solidity and Rust files are production, active runtime, legacy, test-only, or utility-only.

## Solidity

### Current production-compatible mining contract path

Use these when you need the active contract surface that the Rust live miner can talk to today:

- [contracts/TemporalGradientL2Beacon.sol](contracts/TemporalGradientL2Beacon.sol)
  - Monolith beacon.
  - Still exposes the live mining ABI used by the Rust miner (`getMiningChallenge`, `submitMiningCommitment`, `revealMiningCommitment`, `nonces`, `minCommitmentAge`).
  - Treat this as the current production-compatible on-chain mining target.

- [contracts/MiningLib.sol](contracts/MiningLib.sol)
- [contracts/RandomnessLib.sol](contracts/RandomnessLib.sol)
- [contracts/TokenomicsLib.sol](contracts/TokenomicsLib.sol)
- [contracts/CoreUtilsLib.sol](contracts/CoreUtilsLib.sol)
- [contracts/BloomFilterLib.sol](contracts/BloomFilterLib.sol)
- [contracts/StorageLib.sol](contracts/StorageLib.sol)
- [contracts/GovernanceLib.sol](contracts/GovernanceLib.sol)
- [contracts/RateTypes.sol](contracts/RateTypes.sol)
  - Shared production libraries/types.

### Modular production path

These are the intended production contracts for the modular split:

- [contracts/TemporalGradientCore.sol](contracts/TemporalGradientCore.sol)
- [contracts/modules/MiningModule.sol](contracts/modules/MiningModule.sol)
- [contracts/modules/RandomnessModule.sol](contracts/modules/RandomnessModule.sol)
- [contracts/modules/TokenomicsModule.sol](contracts/modules/TokenomicsModule.sol)
- [contracts/modules/RateLimitModule.sol](contracts/modules/RateLimitModule.sol)
- [contracts/modules/ModuleBase.sol](contracts/modules/ModuleBase.sol)
- [contracts/interfaces/ITemporalGradientCore.sol](contracts/interfaces/ITemporalGradientCore.sol)
- [contracts/interfaces/IMiningModule.sol](contracts/interfaces/IMiningModule.sol)
- [contracts/interfaces/IRandomnessModule.sol](contracts/interfaces/IRandomnessModule.sol)
- [contracts/interfaces/ITokenomicsModule.sol](contracts/interfaces/ITokenomicsModule.sol)
- [contracts/interfaces/IRateLimitModule.sol](contracts/interfaces/IRateLimitModule.sol)
- [contracts/interfaces/ITGBT.sol](contracts/interfaces/ITGBT.sol)

Status:

- Production code, but not the current live-miner target.
- Use these for the modular architecture and module-level tests.

### Not production

- [contracts/mocks/LocalMiningSmokeBeacon.sol](contracts/mocks/LocalMiningSmokeBeacon.sol)
  - Local smoke-test contract only.

- Anything under [test](test)
  - Test-only.

- Anything under [script](script)
  - Deployment/dev scripts, not runtime contracts.

## Rust

### Production runtime

- [rust/temporal_gradient_core](rust/temporal_gradient_core)
  - Active production Rust core crate.
  - Contains the real miner runtime, live-chain client, hashing, config, telemetry, and seed handling.

- [rust/package/src/bin/temporal-gradient-miner.rs](rust/package/src/bin/temporal-gradient-miner.rs)
  - Active miner executable entrypoint.

- [rust/package/src/main.rs](rust/package/src/main.rs)
  - Installer/bootstrap CLI.

- [rust/Cargo.toml](rust/Cargo.toml)
  - Workspace manifest.

### Utility / not active miner runtime

- [rust/nist_pqc.rs](rust/nist_pqc.rs)
  - Standalone/support code, not the active runtime entrypoint.

- [rust/memory.rs](rust/memory.rs)
  - Utility file, not part of the Cargo workspace runtime path.

- [rust/cpu.rs](rust/cpu.rs)
  - Utility file, not part of the Cargo workspace runtime path.

### Legacy / do not use

- `rust/Mining.rs`
  - Deleted.
  - It was stale legacy code and is not part of the active miner.

## Quick rule

If you only want the active production path today, start here:

- [contracts/TemporalGradientL2Beacon.sol](contracts/TemporalGradientL2Beacon.sol)
- [rust/temporal_gradient_core](rust/temporal_gradient_core)
- [rust/package/src/bin/temporal-gradient-miner.rs](rust/package/src/bin/temporal-gradient-miner.rs)

If you want the modular future production path, start here:

- [contracts/TemporalGradientCore.sol](contracts/TemporalGradientCore.sol)
- [contracts/modules/MiningModule.sol](contracts/modules/MiningModule.sol)
- [contracts/modules/RandomnessModule.sol](contracts/modules/RandomnessModule.sol)
- [contracts/modules/TokenomicsModule.sol](contracts/modules/TokenomicsModule.sol)
- [contracts/modules/RateLimitModule.sol](contracts/modules/RateLimitModule.sol)