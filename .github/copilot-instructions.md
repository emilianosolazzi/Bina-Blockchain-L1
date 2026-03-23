# Copilot Instructions — TGBT Miner Project

## Repository

- **Remote:** `https://github.com/emilianosolazzi/TGBT_Randomness`
- Both `origin` and `tgbt` remotes point here. Always push to `origin`.

## Project Layout

```
Randomness_Entropy/                      ← workspace root
├── l2-mining/
│   ├── rust/                            ← Cargo workspace root
│   │   ├── Cargo.toml                   ← [workspace] members = ["temporal_gradient_core", "package"]
│   │   ├── temporal_gradient_core/      ← main miner crate
│   │   │   └── src/
│   │   │       ├── runtime.rs           ← mining loop, stale-block WS, harvest pipeline
│   │   │       ├── chain.rs             ← on-chain commit-reveal
│   │   │       ├── config.rs            ← MinerConfig, StaleBlockConfig
│   │   │       ├── stale_block_miner.rs ← stale block types, proof gen, tracker
│   │   │       ├── pending.rs           ← pending commitment persistence
│   │   │       ├── cpu.rs               ← CPU feature detection
│   │   │       ├── memory.rs            ← memory hardening
│   │   │       └── tg_output_filter.rs  ← Bloom filter output dedup
│   │   ├── package/                     ← installer/packaging crate
│   │   └── target/release/              ← ⚠️ BUILD OUTPUT lives here (workspace-level)
│   │       └── temporal-gradient-miner.exe
│   ├── js/                              ← beacon-api-server (port 3100)
│   ├── miner-dashboard/                 ← Vite dashboard (port 4173)
│   ├── randomness-api/                  ← randomness API (port 4271)
│   └── security/                        ← heartbeat sidecar (port 4380)
├── start-all.ps1                        ← starts all 7 services
├── stop-all.ps1                         ← stops all services
└── .runtime-logs/stack/                 ← service logs (miner.out.log, miner.err.log, etc.)
```

## Build Commands

**IMPORTANT:** The Cargo workspace is at `l2-mining/rust/`, NOT at `l2-mining/rust/temporal_gradient_core/`.
Binary output is at `l2-mining/rust/target/release/temporal-gradient-miner.exe`.

```powershell
# Build miner (from workspace root or l2-mining/rust/)
cd l2-mining/rust
cargo build --release --features stale-mining

# Binary output:
#   l2-mining/rust/target/release/temporal-gradient-miner.exe
```

## Deploy Paths (Windows)

| What            | Path |
|-----------------|------|
| Build binary    | `l2-mining/rust/target/release/temporal-gradient-miner.exe` |
| Deploy binary   | `%LOCALAPPDATA%\entropy\TemporalGradientMiner\data\bin\temporal-gradient-miner.exe` |
| Config          | `%APPDATA%\entropy\TemporalGradientMiner\config\miner-config.json` |
| Telemetry       | `%LOCALAPPDATA%\entropy\TemporalGradientMiner\data\logs\telemetry.jsonl` |
| Pending commit  | `l2-mining/rust/keys/miner.pending.json` |
| Private key     | `l2-mining/rust/keys/miner.key` |
| Service logs    | `.runtime-logs/stack/*.log` |

## Deploy Workflow

1. Build: `cargo build --release --features stale-mining` (from `l2-mining/rust/`)
2. Stop: `.\stop-all.ps1`  (from workspace root)
3. Copy: build binary → deploy binary (start-all.ps1 does this automatically when build is newer)
4. Start: `.\start-all.ps1` (from workspace root)

Or just run `stop-all.ps1` then `start-all.ps1` — it auto-builds if sources are newer and syncs the binary.

## Services & Ports

| # | Service           | Port | Process        | Log files                        |
|---|-------------------|------|----------------|----------------------------------|
| 1 | Redis             | 6379 | redis-server   | (system)                         |
| 2 | PostgreSQL        | 5432 | (service)      | (system)                         |
| 3 | Beacon API        | 3100 | node           | beacon-api.out.log, .err.log     |
| 4 | Randomness API    | 4271 | node           | randomness-api.out.log, .err.log |
| 5 | Heartbeat Sidecar | 4380 | node           | heartbeat-sidecar.out.log, .err.log |
| 6 | Miner Dashboard   | 4173 | node (Vite)    | miner-dashboard.out.log, .err.log |
| 7 | Miner             | —    | temporal-gradient-miner.exe | miner.out.log, miner.err.log |

All log files are in `.runtime-logs/stack/`. The miner logs to **miner.out.log** (NOT miner.log).

## Key Config Fields (miner-config.json)

- `rpc_api_key`: NativeBTC API key (e.g. `fp_2d93df5e...`), also inherited by `stale_block.api_key` if absent
- `stale_block.enabled`: must be `true` for stale-block mining
- `stale_block.bitcoin_api_url`: `https://api.nativebtc.org`
- `stale_block.api_key`: optional, falls back to top-level `rpc_api_key`
- Blockchain: Arbitrum (chain ID 42161)
- Wallet: `0x5cB4D906f0464b34c44d6555A770BF6aF4A2cEfe`, Pool ID: 3

## Config Inheritance (config.rs)

`MinerConfig::normalize()` handles fallback logic:
- `rpc_api_key` → `stale_block.api_key` (when stale_block.api_key is absent)
- `StaleBlockConfig` (JSON-facing, in config.rs) converts to `StaleBlockMinerConfig` (runtime, in stale_block_miner.rs) via `to_miner_config()`
- Config loaded in `load_or_create_config()` → `load_from_path()` → `normalize()`

## NativeBTC WebSocket API

- URL: `wss://api.nativebtc.org/v1/mempool/stream?key=<api_key>`
- Valid commands: `subscribe:stats`, `subscribe:txs`, `filter:address:<addr>`
- NO `subscribe:blocks` (does not exist, causes close frame)

## Feature Flags

- `stale-mining`: enables stale-block Bitcoin orphan mining (WS stream + REST harvest)
- Always build with `--features stale-mining` for production

## Common Gotchas

1. **Binary is at workspace level** — `l2-mining/rust/target/release/`, NOT `l2-mining/rust/temporal_gradient_core/target/release/`
2. **Running process locks the binary** — must `stop-all.ps1` before copying a new build to AppData
3. **Miner runs from AppData** — the deploy path (`%LOCALAPPDATA%\entropy\...`), not the build dir. If you build manually, you must sync to AppData or the old binary keeps running.
4. **Arbitrum TX receipts can be None** — chain.rs polls `get_transaction_receipt()` with retries. Don't treat None as failure.
5. **Log file names** — it's `miner.out.log` and `miner.err.log` (NOT `miner.log`)
6. **start-all.ps1 auto-syncs** — when the build binary is newer than the deploy binary, it copies automatically (even without rebuilding)

## Testing

```powershell
# Run all tests (from l2-mining/rust/)
cargo test --features stale-mining

# Integration tests against live NativeBTC API
cargo test --features stale-mining --test nativebtc_api_test -- --nocapture
```
