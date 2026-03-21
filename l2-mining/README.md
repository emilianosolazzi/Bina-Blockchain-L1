# L2 Mining Workspace

This folder isolates the files needed for the Temporal Gradient L2 mining path.

## Structure

- contracts/
  - TemporalGradientCore.sol
  - TemporalGradientL2Beacon.sol
  - modules/MiningModule.sol
  - modules/RandomnessModule.sol
  - modules/RateLimitModule.sol
  - modules/TokenomicsModule.sol
  - modules/ModuleBase.sol
  - MiningLib.sol
  - GovernanceLib.sol
  - TokenomicsLib.sol
  - RateTypes.sol
  - CoreUtilsLib.sol
  - StorageLib.sol
  - RandomnessLib.sol
  - MinerRewardsCalculator.sol
  - interfaces/ITGBT.sol
  - interfaces/IRandomnessModule.sol
- rust/
  - temporal_gradient_core/
  - memory.rs
  - cpu.rs
  - Cargo.toml
  - package/
- archive/
  - deprecated-rust/
- js/
  - RateMonitor.js
  - RateAnalyzer.ts
  - RateVisualizer.js
- docs/
  - L2_MINING_ARCHITECTURE.md
  - MODULAR_BEACON_REFACTOR.md

## Scope

This workspace is limited to L2 mining only:
- mining commitments
- mining reveals
- output verification
- reward issuance
- pool configuration
- rate limiting
- miner client runtime
- mining telemetry

Excluded for now:
- L1 beacon work
- generalized randomness consumers
- tokenization flows
- bridge integrations
- web backend features outside mining telemetry

## Important notes

1. This is an isolated mining subset created for focused work.
2. The original project files were left in place for safety.
3. The Rust miner and Solidity beacon are now aligned on the 8-byte temporal seed format.
4. Difficulty checks in the beacon now use the fixed pool target directly, with no per-miner weighting hook.
5. The packaged Rust runtime now lives in `rust/temporal_gradient_core` + `rust/package`.
6. The old `rust/Mining.rs` and `rust/nist_pqc.rs` files are archived under `archive/deprecated-rust/`.
7. For the active production/runtime file map, see [PRODUCTION_FILES.md](PRODUCTION_FILES.md).
8. Bloom-filter based uniqueness tracking has been removed from the active on-chain path; mining now relies on exact `usedOutputs` checks.

## Integration tests

Foundry-based mining integration tests now live in:

- [test/MiningModule.t.sol](test/MiningModule.t.sol)
- [test/RandomnessModule.t.sol](test/RandomnessModule.t.sol)
- [test/TokenomicsModule.t.sol](test/TokenomicsModule.t.sol)
- [test/mocks/MiningModuleHarness.sol](test/mocks/MiningModuleHarness.sol)
- [test/mocks/RandomnessModuleHarness.sol](test/mocks/RandomnessModuleHarness.sol)
- [test/mocks/MockProtocolToken.sol](test/mocks/MockProtocolToken.sol)
- [test/mocks/MockTokenomicsModule.sol](test/mocks/MockTokenomicsModule.sol)

The harness keeps the real commit/reveal flow, EIP-712 commitment signing, temporal seed validation, output history updates, and reward accounting, but swaps the hash function to a deterministic low output so tests can complete quickly.

Typical setup:

1. Install Foundry dependencies into `l2-mining/lib`:
  - `forge install OpenZeppelin/openzeppelin-contracts`
  - `forge install OpenZeppelin/openzeppelin-contracts-upgradeable`
  - `forge install foundry-rs/forge-std`
2. Run the suite from `l2-mining/`:
  - `forge test`

## Randomness UX helpers

The beacon now exposes wallet/app-friendly randomness helpers:

- `getRandomnessConfig()`
- `getRandomnessReceipt(requestId)`
- `getRandomnessContributionDetails(requestId)`

These make it easier to show request progress, remaining contributions, fee quote, final result, and the entropy inputs used for verification.

## Rust packaging starter

A Windows-friendly starter package now lives in `rust/package/`.

It currently provides:

- a small installer/bootstrap CLI
- a standalone miner runtime binary
- a default config template generator
- a simple health/diagnostic command
- a launcher command for the miner process
- a PowerShell installer for per-user setup
- a PowerShell build script for creating a portable zip bundle

The packaging layer is now wired to a dedicated Rust core library and a stable executable runtime, so end-user installation can bundle both the launcher and the miner binary.

Recent validation status:

- `forge build` succeeds for the modular Solidity split, including `TokenomicsModule`.
- `forge test --match-path test/MiningModule.t.sol` passes for isolated mining module coverage.
- `forge test --match-path test/RandomnessModule.t.sol` passes for isolated randomness module coverage.
- `forge test --match-path test/TokenomicsModule.t.sol` passes for isolated tokenomics module coverage.
- `cargo check --workspace` succeeds for the Rust workspace.
- `cargo test --workspace --lib` passes for the shared Rust core.
- The packaged miner launcher bootstrap (`tg-miner-installer init` / `doctor`) works on Windows.
- `cargo run -- install` now installs both binaries into the expected per-user bin directory.
- The `temporal-gradient-miner` executable starts successfully in simulated mode and writes telemetry.

The isolated module suites now cover:

- `MiningModule`: commitment submission, reveal flow, duplicate output rejection, pool updates, and rate-limit integration
- `RandomnessModule`: request state, contribution tracking, automatic fulfillment, emergency fulfillment, and expiry behavior
- `TokenomicsModule`: base rewards, bonus rewards, halving transitions, manual slashing, inactivity burns, and missed-contribution penalties

Start with docs/L2_MINING_ARCHITECTURE.md.

If you want the short list of active production files, read [PRODUCTION_FILES.md](PRODUCTION_FILES.md).

For the planned split of the large beacon into core + modules, see [docs/MODULAR_BEACON_REFACTOR.md](docs/MODULAR_BEACON_REFACTOR.md).
