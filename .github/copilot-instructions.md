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
│   │   ├── keys/                        ← miner.key (private key), miner.pending.json
│   │   └── target/release/              ← ⚠️ BUILD OUTPUT lives here (workspace-level)
│   │       └── temporal-gradient-miner.exe
│   ├── js/                              ← beacon-api-server (port 3100)
│   ├── miner-dashboard/                 ← Vite dashboard (port 4173)
│   ├── randomness-api/                  ← randomness API (port 4271)
│   │   ├── epoch-builder.js             ← batch epoch orchestrator (service #8)
│   │   ├── bitcoin-anchor.js            ← Bitcoin OP_RETURN anchoring
│   │   ├── .env                         ← epoch-builder config (NOT committed — secrets)
│   │   ├── epoch-state.json             ← epoch-builder persistent state
│   │   └── epoch-store/                 ← local epoch JSON files
│   ├── security/                        ← heartbeat sidecar (port 4380)
│   └── contracts/modules/
│       └── BatchMiningModule.sol        ← on-chain epoch commit/finalize contract
├── start-all.ps1                        ← starts all 8 services
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
| Epoch state     | `l2-mining/randomness-api/epoch-state.json` |
| Epoch .env      | `l2-mining/randomness-api/.env` (secrets — not committed) |

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
| 8 | Epoch Builder     | —    | node           | epoch-builder.out.log, .err.log  |

All log files are in `.runtime-logs/stack/`. The miner logs to **miner.out.log** (NOT miner.log).

## Epoch Pipeline

The epoch pipeline turns raw miner solutions into on-chain commitments:

1. **Miner** writes solutions to `telemetry.jsonl` (one JSON line per solution)
2. **Epoch Builder** (`epoch-builder.js`) polls `telemetry.jsonl` every 30 s
3. Accumulates solutions in `pendingLeaves` (stored in `epoch-state.json`)
4. At `SOLUTIONS_PER_EPOCH` (default 10) leaves → builds Merkle tree
5. Pushes epoch data to randomness API → commits `epochRoot` on-chain via `BatchMiningModule.commitEpochRoot()` with EIP-712 signature
6. After 28,800 L1-block challenge window (~96 hours) → `finalizeEpoch()` called automatically
7. Optionally anchors finalized epoch root to Bitcoin via OP_RETURN

### Key Files

| File | Purpose |
|------|---------|
| `l2-mining/randomness-api/epoch-builder.js` | Orchestrator (579 lines) |
| `l2-mining/randomness-api/epoch-state.json` | Persistent state: `{nextEpochId, processedLines, pendingLeaves}` |
| `l2-mining/randomness-api/.env` | Config (secrets — NOT committed) |
| `l2-mining/randomness-api/epoch-store/` | Local epoch JSON snapshots |
| `l2-mining/contracts/modules/BatchMiningModule.sol` | On-chain contract (279 lines) |

### Epoch Builder .env (l2-mining/randomness-api/.env)

```env
# Arbitrum production (chain ID 42161)
RPC_URL=https://api.nativebtc.org/v1/arb?key=<rpc_api_key>   # ⚠️ MUST include API key
CHAIN_ID=42161
POOL_ID=3
MINER_PRIVATE_KEY=<hex_private_key>

# Contracts
BATCH_CONTRACT=0x6eb6D03A8E98c79E89B98ce19AcAefB865817Db2
CORE_CONTRACT=0xF6556DDC7CdD3635A05428BD85BCf33A09F752e6
TGBT_TOKEN=0x31228eE520e895DA19f728DE5459b1b317d9b8D8
TOKENOMICS_CONTRACT=0xA9f684d709bB46155A252b260dDDE4cb2a37a0E3

# Tuning
SOLUTIONS_PER_EPOCH=10
POLL_INTERVAL=30000          # ms between telemetry scans
PORT=4271
```

### BatchMiningModule.sol Constraints

- **Sequential epoch IDs:** `epochId != _nextEpochId` → `revert EpochNotFound(epochId)`. Epochs must be committed in strict order (0, 1, 2, …).
- **Challenge window:** 28,800 **L1 Ethereum** blocks (~96 hours) must pass between `commitEpochRoot()` and `finalizeEpoch()`. Premature finalize → `CooldownNotElapsed()`. Note: Arbitrum Solidity's `block.number` returns the L1 mainnet block number, NOT the L2 number.
- **Cooldown:** `EPOCH_COOLDOWN_BLOCKS` enforced between commits from same operator.
- **Leaf cap:** `MAX_LEAVES_PER_EPOCH` limits solutions per epoch.

## Key Config Fields (miner-config.json)

- `rpc_api_key`: NativeBTC API key (e.g. `fp_2d93df5e...`), also inherited by `stale_block.api_key` if absent
- `stale_block.enabled`: must be `true` for stale-block mining
- `stale_block.bitcoin_api_url`: `https://api.nativebtc.org`
- `stale_block.api_key`: optional, falls back to top-level `rpc_api_key`
- Blockchain: Arbitrum (chain ID 42161, hex `0xa4b1`)

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

## Smart Contracts (Arbitrum)

| Contract | Address |
|----------|---------|
| Core (TemporalGradientBeacon) | `0xF6556DDC7CdD3635A05428BD85BCf33A09F752e6` |
| BatchMiningModule | `0x6eb6D03A8E98c79E89B98ce19AcAefB865817Db2` |
| TGBT Token | `0x31228eE520e895DA19f728DE5459b1b317d9b8D8` |
| Tokenomics | `0xA9f684d709bB46155A252b260dDDE4cb2a37a0E3` |

Wallet: `0x5cB4D906f0464b34c44d6555A770BF6aF4A2cEfe`, Pool ID: 3

## Common Gotchas

1. **Binary is at workspace level** — `l2-mining/rust/target/release/`, NOT `l2-mining/rust/temporal_gradient_core/target/release/`
2. **Running process locks the binary** — must `stop-all.ps1` before copying a new build to AppData
3. **Miner runs from AppData** — the deploy path (`%LOCALAPPDATA%\entropy\...`), not the build dir. If you build manually, you must sync to AppData or the old binary keeps running.
4. **Arbitrum TX receipts can be None** — chain.rs polls `get_transaction_receipt()` with retries. Don't treat None as failure.
5. **Log file names** — it's `miner.out.log` and `miner.err.log` (NOT `miner.log`)
6. **start-all.ps1 auto-syncs** — when the build binary is newer than the deploy binary, it copies automatically (even without rebuilding)
7. **Epoch IDs are strictly sequential** — the contract enforces `epochId == _nextEpochId`. If `epoch-state.json` gets out of sync with the on-chain counter, reset `nextEpochId` to match `_nextEpochId` on-chain.
8. **RPC URL needs API key** — NativeBTC RPC requires `?key=<api_key>` query parameter. Without it → `NETWORK_ERROR: could not detect network`.
9. **Epoch Builder has no port** — unlike other services, it uses a PID file (`.runtime-logs/stack/epoch-builder.pid`) for process tracking, not a TCP port.
10. **Challenge window** — `finalizeEpoch()` will revert with `CooldownNotElapsed()` until 28,800 **L1 Ethereum** blocks (~96 hours) have passed since `commitEpochRoot()`. Arbitrum's Solidity `block.number` returns the L1 mainnet block number, not L2. The epoch-builder uses `l1BlockNumber` from the raw Arbitrum block to track progress.

## Testing

```powershell
# Run all tests (from l2-mining/rust/)
cargo test --features stale-mining

# Integration tests against live NativeBTC API
cargo test --features stale-mining --test nativebtc_api_test -- --nocapture
```
