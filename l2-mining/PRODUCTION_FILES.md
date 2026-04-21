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
- [contracts/StorageLib.sol](contracts/StorageLib.sol)
- [contracts/GovernanceLib.sol](contracts/GovernanceLib.sol)
- [contracts/RateTypes.sol](contracts/RateTypes.sol)
  - Shared production libraries/types.

Archived reference:

- [archive/deprecated-bloom/BloomFilterLib.sol](archive/deprecated-bloom/BloomFilterLib.sol)
  - Archived only.
  - Removed from the active mining system to eliminate bloom-filter false positives and extra maintenance.

### Modular production path

These are the intended production contracts for the modular split:

- [contracts/TemporalGradientCore.sol](contracts/TemporalGradientCore.sol)
- [contracts/modules/MiningModule.sol](contracts/modules/MiningModule.sol)
- [contracts/modules/RandomnessModule.sol](contracts/modules/RandomnessModule.sol)
- [contracts/modules/TokenomicsModule.sol](contracts/modules/TokenomicsModule.sol)
- [contracts/modules/RateLimitModule.sol](contracts/modules/RateLimitModule.sol)
- [contracts/StaleBlockOracle.sol](contracts/StaleBlockOracle.sol)
- [contracts/modules/ModuleBase.sol](contracts/modules/ModuleBase.sol)
- [contracts/interfaces/ITemporalGradientCore.sol](contracts/interfaces/ITemporalGradientCore.sol)
- [contracts/interfaces/IMiningModule.sol](contracts/interfaces/IMiningModule.sol)
- [contracts/interfaces/IRandomnessModule.sol](contracts/interfaces/IRandomnessModule.sol)
- [contracts/interfaces/ITokenomicsModule.sol](contracts/interfaces/ITokenomicsModule.sol)
- [contracts/interfaces/IRateLimitModule.sol](contracts/interfaces/IRateLimitModule.sol)
- [contracts/interfaces/ITGBT.sol](contracts/interfaces/ITGBT.sol)

Status:

- Production code — now the **current live-miner target** on Arbitrum.
- The modular mining path uses exact `usedOutputs` tracking for uniqueness.

Arbitrum deployment addresses:

| Module | Address | Live status | Explorer source verification | Deploy tx |
|---|---|---|---|---|
| TemporalGradientCore | `0xF6556DDC7CdD3635A05428BD85BCf33A09F752e6` | Live | Verified | not recorded here |
| TGBT Token | `0x31228eE520e895DA19f728DE5459b1b317d9b8D8` | Live | Verified | not recorded here |
| MINING_MODULE | `0xb2b3d9bC63993b725Aea36aC90601c22292F3171` | Live | **Verified 2026-04-21** | deploy tx `0x0f54cba023b83a586ba78c9c1b62761c4a9c6ba609009ece19f83c0345d1f107` |
| BATCH_MINING_MODULE | `0xAf07E37D104E9be17639FE7a51B36972D4738651` | Live | Not verified on explorer as of 2026-04-20 | `0x18bdeffae0a3b02016f54a5ef02074425be8e3418004659f53cb5af965d1b44d` |
| RANDOMNESS_MODULE | `0x583863CFC5EFc0106886BA485e1b67F0966584f9` | Live | Not verified on explorer as of 2026-04-20 | `0x546404da42b698c90bb5551312f7fef1bd9a710a59e3b1802d75478cbddd36d2` |
| TOKENOMICS_MODULE (V2) | `0x7B871bdeDdED0064C34e22902181A9a983C9E2ab` | Live | Verified on 2026-04-20 | `0x0d0c857b7d01600b5e40f98c4ebd6b199dd3cd6b39f6ccbea88d174def0c20c8` |
| RATE_LIMIT_MODULE | `0x61dEEEf2B2956db3AD291c639939669cD5399c1B` | Live | Verified | not recorded here |
| STALE_BLOCK_MODULE | `0xdc4eDF632187d05da50393Af87D19A08f6986517` | Live | Verified | not recorded here |

Additional known deployed addresses tied to the live system:

| Contract | Address | Status |
|---|---|---|
| TokenomicsModule V1 | `0xF6069614FE09B91e5B00DA0a13A11B2BFcCabC36` | Deauthorized, not live |
| TokenomicsModule V0 | `0xA9f684d709bB46155A252b260dDDE4cb2a37a0E3` | Deauthorized, not live |
| MiningModule hot wallet / operator | `0x5cB4D906f0464b34c44d6555A770BF6aF4A2cEfe` | Live operator wallet |
| Ledger governance / owner wallet | `0xd28E6a7AD806E85BD0544ed443D25E48f52c06c3` | Live governance wallet |

Verification notes:

- `TemporalGradientCore`, `TGBT`, `RateLimitModule`, `StaleBlockOracle`, and `TokenomicsModuleV2` are verified on Arbiscan/Etherscan as of 2026-04-20.
- `TokenomicsModuleV2` was successfully verified on 2026-04-20 using `solc 0.8.28`, optimizer enabled, `optimizer_runs = 1`, and `via_ir = true`.
- `MiningModule` was redeployed on 2026-04-21 to `0xb2b3d9bC63993b725Aea36aC90601c22292F3171` and is now **verified on Arbiscan**. The old address `0x97A88f7ed5e7D8EEd442f6979aC66bBb599ff595` is deregistered (read-only, not in Core registry).
- `BatchMiningModule` and `RandomnessModule` are verified as of 2026-04-21.
- The repo now keeps the Etherscan API key in `l2-mining/.env` (gitignored) and Foundry verifier configuration in `l2-mining/foundry.toml`.

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

- [rust/self-miner/src/main.rs](rust/self-miner/src/main.rs)
  - Self-miner standalone runtime entry.
  - Uses live `MiningModule` `0xb2b3d9bC63993b725Aea36aC90601c22292F3171` as `DEFAULT_CONTRACT` (redeployed + verified 2026-04-21).
  - `sanitize_dev_values()` auto-migrates old configs that still point at Core `0xF6556DDC7CdD3635A05428BD85BCf33A09F752e6` so mining calls land on `MiningModule`.

- [rust/package/src/bin/temporal-gradient-miner.rs](rust/package/src/bin/temporal-gradient-miner.rs)
  - Active miner executable entrypoint.

- [rust/package/src/main.rs](rust/package/src/main.rs)
  - Installer/bootstrap CLI.

- [rust/Cargo.toml](rust/Cargo.toml)
  - Workspace manifest.

### Utility / not active miner runtime

- [rust/memory.rs](rust/memory.rs)
  - Utility file, not part of the Cargo workspace runtime path.

- [rust/cpu.rs](rust/cpu.rs)
  - Utility file, not part of the Cargo workspace runtime path.

### Legacy / do not use

- Legacy standalone Rust references were removed from the `l2-mining` working set.
- If needed for future design work, use the L3 reference set under [../L3/reference/rust](../L3/reference/rust).

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