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
│   │   │       ├── runtime.rs           ← mining loop, stale-block WS, harvest pipeline, pause/power control, fork event submission
│   │   │       ├── chain.rs             ← on-chain commit-reveal, stale proof submit, fork event recording
│   │   │       ├── config.rs            ← MinerConfig, StaleBlockConfig, control_file_path()
│   │   │       ├── telemetry.rs         ← TelemetrySnapshot, MiningControl (pause/power), PhaseTracker
│   │   │       ├── stale_block_miner.rs ← stale block types, proof gen, tracker, LoserChainStats, pending_fork_events queue
│   │   │       ├── pending.rs           ← pending commitment persistence
│   │   │       ├── cpu.rs               ← CPU feature detection
│   │   │       ├── memory.rs            ← memory hardening
│   │   │       ├── tg_output_filter.rs  ← Bloom filter output dedup
│   │   │       ├── utxo_fetcher.rs      ← UTXO fetcher, entropy anchoring, 4 dead UTXO types (916 lines)
│   │   │       └── bitcoin_dead_utxo_anchor.rs ← DeadUTXOAnchorDB, anchor create/verify (396 lines)
│   │   ├── package/                     ← installer/packaging crate
│   │   ├── keys/                        ← miner.key (private key), miner.pending.json
│   │   └── target/release/              ← ⚠️ BUILD OUTPUT lives here (workspace-level)
│   │       └── temporal-gradient-miner.exe
│   ├── js/                              ← beacon-api-server (port 3100)
│   ├── miner-dashboard/                 ← Vite dashboard (port 4173)
│   │   ├── index.html                   ← single-file SPA (~4800+ lines, all HTML/CSS/JS)
│   │   └── server.js                    ← Node.js dashboard server (proxies to APIs, mining control)
│   ├── randomness-api/                  ← randomness API (port 4271)
│   │   ├── server.js                    ← HTTP server, all API routes
│   │   ├── epoch-builder.js             ← batch epoch orchestrator (service #8)
│   │   ├── bitcoin-anchor.js            ← Bitcoin OP_RETURN anchoring
│   │   ├── utxo-scanner.js              ← UTXO scanner: 5-step pipeline + live discovery (450+ lines)
│   │   ├── test-dead-utxos.csv          ← dead UTXO inventory (grows via /api/utxo/discover)
│   │   ├── storage-attestation.js       ← epoch storage verification
│   │   ├── .env                         ← epoch-builder config (NOT committed — secrets)
│   │   ├── epoch-state.json             ← epoch-builder persistent state
│   │   └── epoch-store/                 ← local epoch JSON files
│   ├── security/                        ← heartbeat sidecar (port 4380)
│   └── contracts/
│       ├── StaleBlockOracle.sol         ← stale block proof submission, reward claiming, fork events (452 lines)
│       ├── interfaces/IStaleBlockOracle.sol ← oracle interface
│       ├── interfaces/ITokenomicsModule.sol ← tokenomics interface
│       └── modules/
│           ├── BatchMiningModule.sol        ← on-chain epoch commit/finalize contract
│           ├── RandomnessShop.sol           ← sells randomness proofs, not tokens
│           └── UniversalMinerGasPool.sol    ← Arbitrum ETH gas reimbursement pool with attestor-signed epochs
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
| Mining control   | `%LOCALAPPDATA%\entropy\TemporalGradientMiner\data\logs\miner-control.json` |
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
BATCH_CONTRACT=0xAf07E37D104E9be17639FE7a51B36972D4738651
CORE_CONTRACT=0xF6556DDC7CdD3635A05428BD85BCf33A09F752e6
TGBT_TOKEN=0x31228eE520e895DA19f728DE5459b1b317d9b8D8
TOKENOMICS_CONTRACT=0xF6069614FE09B91e5B00DA0a13A11B2BFcCabC36

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

| Contract | Address | Status |
|----------|--------|--------|
| Core (TemporalGradientBeacon) | `0xF6556DDC7CdD3635A05428BD85BCf33A09F752e6` | ✅ Live |
| BatchMiningModule | `0xAf07E37D104E9be17639FE7a51B36972D4738651` | ✅ Live |
| StaleBlockOracle | `0xdc4eDF632187d05da50393Af87D19A08f6986517` | ✅ Live + Initialized (v2, LE zero-bit fix) |
| TGBT Token | `0x31228eE520e895DA19f728DE5459b1b317d9b8D8` | ✅ Live |
| TokenomicsModule (active) | `0xF6069614FE09B91e5B00DA0a13A11B2BFcCabC36` | ✅ Live |
| TokenomicsModule (old, deauthorized) | `0xA9f684d709bB46155A252b260dDDE4cb2a37a0E3` | ❌ Deauthorized |

### Wallet & Module Registry

Wallet (hot): `0x5cB4D906f0464b34c44d6555A770BF6aF4A2cEfe` — Pool ID: 3
- Roles: `GOVERNANCE_ROLE` on Core
- Registered modules: `FORK_RELAY` (for `recordForkEvent()` on StaleBlockOracle)
- `core.isModule(hotWallet)` → `true` (verified on-chain)

Wallet (Ledger): `0xd28E6a7AD806E85BD0544ed443D25E48f52c06c3` (Core owner/DEFAULT_ADMIN_ROLE, TGBT governance, HD path `m/44'/60'/1'/0/0`)

### Module IDs (keccak256 hashes)

| Module Name | keccak256 Hash |
|-------------|---------------|
| `FORK_RELAY` | `0xd574db746a15bbfe83a62e38e86f4862b6a9d1be2d7d6d5444ff766ff3a35413` |
| `STALE_BLOCK_MODULE` | `0xb7cd39e77ac6ec57f4274a1e6593d3e120545cc97868f8010faa93e0c47e299f` |
| `TOKENOMICS_MODULE` | `keccak256("TOKENOMICS_MODULE")` |

### Module Registration (Important)

`core.setModule(moduleId, address)` registers any address (including EOAs) as a module. There is NO `isContract()` check. This was used to register the hot wallet EOA as `FORK_RELAY` so it can call `recordForkEvent()` directly.

- `onlyCoreOrModule` modifier (ModuleBase.sol line 30): `if (msg.sender != address(core) && !core.isModule(msg.sender)) revert OnlyCoreOrModule();`
- `core.isModule(addr)` checks `moduleRefCount[addr] != 0` — only set via `setModule()`
- `core.modulesLocked()` → `false` (modules can still be registered/changed)
- Once locked, module config is permanently frozen (Bitcoin-style ossification)

## StaleBlockOracle Contract (`l2-mining/contracts/StaleBlockOracle.sol`)

The `StaleBlockOracle` manages stale (orphaned) Bitcoin block proofs on Arbitrum. It is **LIVE and INITIALIZED**.

### On-Chain Parameters (verified)

| Parameter | Value | Meaning |
|-----------|-------|--------|
| `baseReward` | 50 TGBT (50e18 wei) | Base reward per stale block proof |
| `minLeadingZeros` | 32 | Minimum PoW difficulty for valid proofs |
| `maxReorgDepth` | 100 | Maximum reorg depth accepted |
| `maxStaleAgeSecs` | 604,800 (1 week) | Maximum age of stale block |

### Lifecycle: Detect → Submit → Claim → Fork Event

1. **Miner detects** orphan block via WebSocket/REST from NativeBTC API
2. **`submitStaleBlock()`** — miner submits proof (blockHash, header, coinbase, leading zeros, Merkle proof, reorg depth)
3. **`claimReward(blockHash)`** — miner claims TGBT reward for accepted proof
   - Calls `_tokenomics().onStaleBlockReward(msg.sender, requestedReward)`
   - TokenomicsModule.onStaleBlockReward() → `tgbtToken.mint()` (**DOES mint TGBT**)
4. **`recordForkEvent(forkHeight, winnerHash, loserHashes, reorgDepth)`** — records multi-loser fork events
   - Gated by `onlyCoreOrModule` — requires caller to be registered module
   - Hot wallet registered as `FORK_RELAY` module for this purpose

### Reward Formula

```
reward = baseReward(50) × qualityScore(0-100) × min(reorgDepth + 1, 7) / 100
```

- **Range:** 0 to 350 TGBT per orphan block
- **Quality score components:** PoW difficulty (0-30), reorg depth (0-25), freshness (0-20), timestamp divergence (0-25)
- **Allocation:** 75,000,000 TGBT (separate from PoW mining's 1,900,000,000)
- **Cap:** TokenomicsModule.onStaleBlockReward() caps against `STALE_BLOCK_ALLOCATION` and remaining supply

### Key Functions

| Function | Access | Purpose |
|----------|--------|---------|
| `submitStaleBlock()` | Any miner | Submit orphan block proof |
| `claimReward(blockHash)` | Proof submitter only | Claim TGBT reward → mints via TokenomicsModule |
| `pendingReward(blockHash)` | View | Calculate pending reward amount |
| `recordForkEvent()` | `onlyCoreOrModule` | Record multi-block fork events |
| `forkEventsAtHeight(height)` | View | Count fork events at a Bitcoin height |
| `initialize()` | Owner (once) | Set baseReward, minLeadingZeros, maxReorgDepth, maxStaleAgeSecs |

### Past Issue: Oracle deployed but NEVER INITIALIZED

The StaleBlockOracle was deployed days before being initialized. All params (baseReward, minLeadingZeros, etc.) were 0, meaning no proofs would be accepted and no rewards would be paid. **Fixed** by calling `initialize()` via `cast send`.

## TGBT Reward System — Two Active Minting Paths

### 1. PoW Mining (MiningModule → TokenomicsModule)

| Aspect | Detail |
|--------|--------|
| Trigger | `MiningModule.revealMiningCommitment()` → `TokenomicsModule.onBlockMined()` |
| Allocation | 1,900,000,000 TGBT (95% of mining pool) |
| Formula | Epoch halving (~12.5 TGBT base), bonus 1.25× for exceptional difficulty |
| Frequency | Every solution (~minutes) |

### 2. Stale Block Mining (StaleBlockOracle → TokenomicsModule)

| Aspect | Detail |
|--------|--------|
| Trigger | `StaleBlockOracle.claimReward()` → `TokenomicsModule.onStaleBlockReward()` |
| Allocation | 75,000,000 TGBT (dedicated stale pool) |
| Formula | `baseReward(50) × quality(0-100) × min(depth+1, 7) / 100` → 0-350 TGBT |
| Frequency | Rare (1-2 orphans/day globally), higher value per event |

### 3. Dead UTXO Anchoring (NO token reward)

| Aspect | Detail |
|--------|--------|
| Purpose | Entropy contribution + Bitcoin-grade timestamps |
| Reward | None — no on-chain mint mechanism |
| Value | Anchoring provenance, not token incentive |

## Fork Event Pipeline (Rust → On-Chain)

The miner automatically submits fork events to the StaleBlockOracle when orphan blocks are detected.

### Architecture

```
stale_block_miner.rs              chain.rs                     StaleBlockOracle (Arbitrum)
┌──────────────────────┐   ┌──────────────────────────┐   ┌─────────────────────────┐
│ process_tips()       │──▶│ record_fork_event_onchain│──▶│ recordForkEvent()       │
│ queues fork events   │   │ checks forkEventsAtHeight│   │ onlyCoreOrModule gate   │
│ pending_fork_events  │   │ skips duplicates         │   │ emits ForkEventRecorded  │
└──────────────────────┘   │ submits tx               │   └─────────────────────────┘
                           └──────────────────────────┘

runtime.rs: submit_pending_fork_events()
  Called at all 3 harvest sites:
  1. WS + poll harvest
  2. WS stream harvest  
  3. HTTP fallback harvest
  Permanent errors (reverted, OnlyCoreOrModule) → drop event
  Transient errors → requeue for retry
```

### Key Rust Types & Functions

| Location | Item | Purpose |
|----------|------|---------|
| `stale_block_miner.rs` | `pending_fork_events: VecDeque<ChainForkEvent>` | Queue of fork events awaiting on-chain submission |
| `stale_block_miner.rs` | `drain_pending_fork_events()` | Take all pending events for batch submission |
| `stale_block_miner.rs` | `requeue_fork_events(events)` | Put failed events back at front of queue |
| `chain.rs` | `record_fork_event_onchain()` | Submit one fork event, with duplicate check via `forkEventsAtHeight` |
| `chain.rs` | `StaleBlockOracleContract` ABI | Includes `recordForkEvent`, `forkEventsAtHeight`, `submitStaleBlock`, `claimReward` |
| `runtime.rs` | `submit_pending_fork_events()` | Batch submission loop, mirrors `submit_pending_proofs` pattern |

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
11. **End-to-end verification** — When fixing data display issues, always trace the full pipeline: Rust miner → telemetry.jsonl fields → dashboard server.js → dashboard index.html JS rendering. Fixing one layer (e.g. miner writes correct data) doesn't help if a downstream layer (e.g. dashboard JS) overwrites it with fake/computed values. Always verify the final rendered output matches the source data.
12. **Dashboard browser cache** — After editing `index.html`, users must hard-refresh (**Ctrl+Shift+R**) or the browser serves the old cached version. The Node.js dashboard server (`server.js`) serves the raw file with no cache-busting.
13. **Mining control file** — `miner-control.json` sits next to `telemetry.jsonl` in AppData. Dashboard writes it via `POST /api/miner/control`, miner reads it each cycle. If missing, defaults to `{paused:false, power_pct:100}`. Power values are clamped to `[25, 50, 75, 100]`.
14. **StaleBlockOracle must be initialized** — Deploying the contract is NOT enough. Must call `initialize(core, minLeadingZeros, maxReorgDepth, maxStaleAgeSecs, baseReward)` once. Without it, all params are 0 and no proofs/rewards work.
15. **`cast send` returns null on Arbitrum** — `cast send` often shows "server returned a null response" but the tx succeeds. Always verify with a follow-up `cast call` to read state.
16. **EOAs can be registered as modules** — `core.setModule()` has NO `isContract()` check. Any address (including EOAs) can be registered as a module and pass the `onlyCoreOrModule` gate. This is by design.
17. **Stale block rewards DO mint TGBT** — `StaleBlockOracle.claimReward()` calls `_tokenomics().onStaleBlockReward()` which calls `tgbtToken.mint()`. The old dashboard comments saying "no mint" / "TODO" were wrong.
18. **Foundry `cast` for on-chain queries** — Use `cast call` for reads, `cast send` for writes. Available at v1.5.0-dev. Private key from `l2-mining/rust/keys/miner.key` (64 hex chars, no 0x prefix). RPC: `https://api.nativebtc.org/v1/arb?key=<api_key>`.
19. **NativeBTC RPC strips revert data from eth_estimateGas** — When ethers-rs calls `eth_estimateGas` via the NativeBTC RPC proxy, contract reverts may return `data: 0x` (empty) instead of the actual custom error selector. This caused `submitStaleBlock()` to appear as if the function didn't exist. **Fix**: chain.rs now (a) does a pre-flight `eth_call` to get detailed error data before sending, and (b) sets a manual gas limit (500k) to skip `eth_estimateGas`. The runtime.rs error handler now treats `data: 0x` reverts as transient (retryable) instead of permanent drops.
20. **Dashboard hero TGBT includes stale rewards** — The "TOTAL TGBT EARNED" hero display now includes `entropyState.tgbt.stale` (stale block rewards) alongside PoW rewards. Previously it only showed `effectiveRewards(snap).total` which is PoW-only from `total_rewards_estimate`.
21. **StaleBlockOracle requires reorgDepth >= 1** — The contract reverts with `ReorgTooDeep(0, maxReorgDepth)` if `reorgDepth == 0`. The harvest function floors `branchLen` to 1 via `.max(1)`, but the `stale_fork_depth` telemetry field tracks the max across all detected blocks (may be 0 if only REST-harvested tips with branchLen=1 are found).
22. **Dashboard staleMiningEnabled defaults to TRUE** — `staleMiningEnabled = localStorage.getItem(STALE_MINING_KEY) !== 'false'` defaults to `true` on first visit (localStorage returns `null`, `null !== 'false'` is `true`). Additionally, both `initEntropyPipeline()` and `tickEntropyPipeline()` auto-enable the toggle when `stale_block_count > 0`. The toggle is purely **client-side localStorage** — it has NO connection to the Rust miner's `stale_block.enabled` config. Previously defaulted to `false` (used `=== 'true'`) which caused the stale card to show "Waiting for stale blocks…" even with active harvesting.
23. **Dashboard tick refreshes stale data from telemetry** — `tickEntropyPipeline()` re-reads `snap.stale_block_count`, `snap.stale_quality`, `snap.stale_xor_hex`, etc. from the latest telemetry snapshot on every 3-second tick. This ensures new stale blocks detected during runtime are reflected without page reload.
24. **Stale developer-proof endpoint uses hash-order fallback** — telemetry exposes the stale block hash in display order, but `StaleBlockOracle.getStaleProof(bytes32)` may require the reversed `bytes32` form that appears in `StaleBlockSubmitted` event logs. The dashboard server now tries both forms before concluding the proof is missing on-chain.

## Deployment Topology — Server vs Personal Miner

Currently all 8 services run on a single machine via `start-all.ps1` (single-operator setup). The architecture is designed to split into two tiers:

### Personal Miner (each participant runs on their own machine)

| Component | What it does | Files / paths |
|-----------|-------------|---------------|
| **Miner binary** | Core PoW computation, stale-block mining, entropy generation, on-chain commit-reveal | `temporal-gradient-miner.exe` → `%LOCALAPPDATA%\entropy\…\bin\` |
| **Miner Dashboard** (port 4173) | Personal monitoring UI — solution history, entropy pipeline, system health, UTXO scan panel | `l2-mining/miner-dashboard/` |
| **Heartbeat Sidecar** (port 4380) | Monitors local miner health, tamper detection, threat profiling | `l2-mining/security/` |
| **Redis** (port 6379) | Local caching — recent solutions, dedup state | System service |
| **Config & keys** | Private key, miner config, pending commitments | `miner-config.json`, `miner.key`, `miner.pending.json` |
| **Telemetry output** | Solutions written by the miner, consumed by the epoch builder | `telemetry.jsonl` (in AppData logs dir) |
| **UTXO scanner** | Bitcoin anchor scanning for the local dashboard (5-step pipeline) | `utxo-scanner.js`, `test-dead-utxos.csv` |

**What stays private:** Private key (`miner.key`), local threat profile, heartbeat alerts, solution telemetry before epoch submission.

### Central Server (shared project infrastructure)

| Component | What it does | Files / paths |
|-----------|-------------|---------------|
| **Beacon API** (port 3100) | Public randomness beacon — serves verified random outputs to consumers | `l2-mining/js/beacon-api-server.js` |
| **Randomness API** (port 4271) | Epoch storage, Merkle proofs, UTXO inventory, public randomness queries | `l2-mining/randomness-api/server.js` |
| **Epoch Builder** | Aggregates miner solutions → Merkle tree → on-chain `commitEpochRoot()` → `finalizeEpoch()` | `l2-mining/randomness-api/epoch-builder.js` |
| **PostgreSQL** (port 5432) | Epoch storage, solution aggregation, historical data | System service |
| **Epoch store** | Local epoch JSON snapshots + persistent state | `epoch-store/`, `epoch-state.json` |
| **Bitcoin anchoring** | OP_RETURN anchoring of finalized epoch roots to Bitcoin | `bitcoin-anchor.js` |
| **Storage attestation** | Verifies epoch data availability (IPFS/Arweave) | `storage-attestation.js` |

**What's public:** Beacon outputs, epoch Merkle roots, randomness proofs, UTXO anchor certificates.

### On-Chain (Arbitrum — neither server nor miner)

| Component | Purpose | Mints TGBT? |
|-----------|--------|-------------|
| `TemporalGradientCore` | Core beacon contract, module registry | No |
| `BatchMiningModule` | Epoch commit/finalize with challenge window | No |
| `StaleBlockOracle` | Stale block proof submission, reward claiming, fork events | **Yes** (via TokenomicsModule) |
| `RandomnessShop` | Proof marketplace; sells randomness proofs and anchor receipts | No (burns TGBT) |
| `UniversalMinerGasPool` | ETH reimbursement vault for miner gas sponsorship on Arbitrum | No |
| `TGBT Token` | Mining reward token (ERC-20) | N/A (is the token) |
| `TokenomicsModule` | Reward distribution, pool management, actual `mint()` caller | **Yes** (sole minter) |
| `MiningModule` | Individual commit-reveal mining | **Yes** (via TokenomicsModule) |
| `StaleBlockModule` | Legacy — replaced by StaleBlockOracle | Deprecated |

### Current vs Future Architecture

```
CURRENT (single-operator, one machine):
┌─────────────────────────────────────────────┐
│  start-all.ps1 runs ALL 8 services locally  │
│  Miner + Dashboard + Heartbeat + Redis      │
│  + Beacon API + Randomness API + Epoch      │
│  Builder + PostgreSQL                        │
│  Epoch Builder reads local telemetry.jsonl   │
└─────────────────────────────────────────────┘

FUTURE (multi-miner network):
┌──────────────────────┐     ┌──────────────────────────┐
│  PERSONAL MINER (×N) │     │   CENTRAL SERVER         │
│  • Miner binary      │────▶│  • Beacon API            │
│  • Dashboard         │     │  • Randomness API        │
│  • Heartbeat         │     │  • Epoch Builder         │
│  • Redis             │     │  • PostgreSQL            │
│  • UTXO scanner      │     │  • Bitcoin anchoring     │
│  • telemetry.jsonl   │     │  • Storage attestation   │
│  • miner.key         │     └──────────┬───────────────┘
└──────────────────────┘                │
                                        ▼
                              ┌──────────────────┐
                              │  ARBITRUM CHAIN   │
                              │  Smart contracts  │
                              │  TGBT token       │
                              └──────────────────┘
```

### Migration Notes

- **Epoch Builder connection:** Currently reads local `telemetry.jsonl`. Multi-miner requires miners to POST solutions to the central Randomness API instead.
- **Dashboard API URLs:** Dashboard currently connects to `127.0.0.1:4271`. In multi-miner mode, each dashboard would point to the central server's public URL.
- **UTXO scanner:** Can run on either tier — the inventory (`test-dead-utxos.csv`) and scan pipeline are self-contained. Central server could share a larger inventory across all miners.
- **Config split:** `miner-config.json` stays personal. `.env` (epoch builder secrets, contract addresses) stays on the server.
- **Heartbeat sidecar:** Always personal — monitors the local miner process and hardware.

## Gas Sponsorship & Marketplace Contracts

### RandomnessShop (`l2-mining/contracts/modules/RandomnessShop.sol`)

The redesigned `RandomnessShop` is a **proof marketplace**, not a token vending machine.

- Sells **randomness proofs** in 3 tiers: `Standard`, `Anchored`, `Enterprise`
- Accepts **TGBT directly** for proof receipts
- Records on-chain `ProofReceipt` entries: buyer, tier, beacon output, proof hash, anchor ID, fee, block number, timestamp
- Splits revenue into:
  - **miner share** → burned immediately to support TGBT scarcity
  - **protocol share** → sent to treasury
- **Does not mint TGBT**
- Exists specifically to avoid the old inflationary design where anyone could buy freshly minted TGBT

Design intent:

- sell the **service** (verifiable randomness / anchored proofs) and require TGBT as the payment asset
- route economic value back toward miners through direct token burn without creating an admin mint backdoor
- keep the contract simple, non-upgradeable, and auditable

### UniversalMinerGasPool (`l2-mining/contracts/modules/UniversalMinerGasPool.sol`)

The `UniversalMinerGasPool` is a **secure ETH reimbursement vault** for Arbitrum miners.

It is intentionally **not** a naive wrapper that forwards mining calls, because the current mining contracts authenticate the miner via `msg.sender`. A forwarding contract would break miner identity and introduce authorization bugs.

Instead, the design is:

1. **Sponsors deposit ETH** into a shared pool and receive proportional pool shares
2. A decentralized **attestor set** observes real mining transactions on Arbitrum
3. Attestors publish a **threshold-signed reimbursement epoch** with:
   - Merkle root
   - ETH budget
   - claim deadline
4. A miner proves inclusion in that epoch and calls `claimRefund()`
5. The contract reimburses ETH directly to the miner
6. Optional `TGBT` fees paid by miners are distributed pro-rata to sponsors

### Why this design is safer

- **No arbitrary external call forwarding** from the gas pool
- **No broken `msg.sender` semantics** for `MiningModule`, `BatchMiningModule`, or `StaleBlockOracle`
- **Threshold attestors** reduce trust in a single sponsor/operator
- **Merkle epochs** make reimbursements auditable and batch-efficient
- **Reserved ETH accounting** prevents sponsors from withdrawing funds already promised to miners
- **Lockable attestor set** and **lockable target/selector allowlists** support Bitcoin-style ossification once the configuration is stable
- Built for **Arbitrum L2 reality**: miners still submit their own transactions, then claim reimbursement safely

### Supported sponsored actions

The gas pool is intended for audited mining actions only:

- `submitMiningCommitment()`
- `revealMiningCommitment()`
- `commitEpochRoot()`
- `finalizeEpoch()`
- `recordStorageAttestation()`
- `submitStaleBlock()`
- `claimReward()`

### Important limitation

This contract is a **reimbursement system**, not a full ERC-4337 paymaster.

- It works with today's mining contracts without changing miner identity semantics
- It does **not** pay EOA gas upfront inside the same transaction
- A future account-abstraction / paymaster layer could be built later, but this contract is the safe universal primitive for the current Arbitrum architecture

## Testing

```powershell
# Run all tests (from l2-mining/rust/)
cargo test --features stale-mining

# Integration tests against live NativeBTC API
cargo test --features stale-mining --test nativebtc_api_test -- --nocapture
```

## API Endpoints

### Randomness API (port 4271) — `l2-mining/randomness-api/server.js`

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/randomness/latest` | Latest random output + signature |
| `GET` | `/api/randomness/:outputHash/proof` | Merkle proof for a specific output hash |
| `GET` | `/api/epochs` | List epochs (paginated, `?limit=N`) |
| `GET` | `/api/epochs/:epochId` | Single epoch detail |
| `POST` | `/api/epochs` | Epoch-builder pushes a new epoch |
| `POST` | `/api/epochs/:epochId/verify-storage` | Verify epoch storage attestation |
| `POST` | `/api/epochs/:epochId/attestation-onchain` | Record on-chain attestation |
| `POST` | `/api/epochs/:epochId/bitcoin-anchor` | Record Bitcoin OP_RETURN anchor |
| `GET` | `/api/health` | Service health check |
| `GET` | `/api/utxo/scan` | Run 5-step UTXO scan pipeline (live mempool.space fetch) |
| `GET` | `/api/utxo/latest` | Last scan result (404 if none) |
| `GET` | `/api/utxo/inventory` | Dead UTXO inventory from CSV |
| `GET` | `/api/utxo/history` | All scan results this session (`{ scans: [...] }`) |
| `GET` | `/api/utxo/discover?blocks=N` | Discover new dead UTXOs from N recent Bitcoin blocks (1-10) |

### Dashboard API (port 4173) — `l2-mining/miner-dashboard/server.js`

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/history?limit=N` | Telemetry solution history |
| `GET` | `/api/latest` | Latest miner telemetry |
| `GET` | `/api/system/status` | System status (miner, Redis, PostgreSQL, APIs) |
| `GET` | `/api/security/heartbeat/status` | Heartbeat sidecar status (proxied) |
| `GET` | `/api/security/heartbeat/alerts` | Heartbeat alerts (proxied) |
| `GET` | `/api/security/threat-profile` | Security threat analysis |
| `GET` | `/api/security/relay-profile` | RPC relay profile |
| `GET` | `/api/security/relay-status` | RPC relay health |
| `GET` | `/api/miner/control` | Read mining control state (pause + power) |
| `POST` | `/api/miner/control` | Write mining control state (`{paused, power_pct}`) |
| `GET` | `/api/network/health` | Network health (randomness API) |
| `GET` | `/api/network/randomness/latest` | Proxied randomness latest |
| `GET` | `/api/network/randomness/:hash/proof` | Proxied Merkle proof |
| `GET` | `/api/network/epochs` | Proxied epoch list |
| `GET` | `/api/network/epochs/:epochId` | Proxied epoch detail |
| `POST` | `/api/network/epochs/:epochId/verify-storage` | Proxied storage verification |

## UTXO Scanner & Bitcoin Anchoring

### Architecture

The UTXO system anchors miner entropy to Bitcoin's immutable chain. It uses dead (permanently unspendable) Bitcoin outputs as tamper-proof timestamps.

```
utxo_fetcher.rs (Rust)          ← 4 dead UTXO types, entropy-weighted selection, live fetch
bitcoin_dead_utxo_anchor.rs     ← DeadUTXOAnchorDB, anchor creation/verification
utxo-scanner.js (Node.js)       ← 5-step pipeline, dashboard API, live discovery
test-dead-utxos.csv             ← inventory (OP_RETURN txids, grows via discovery)
```

### 5-Step Scan Pipeline (`utxo-scanner.js`)

1. **Load inventory** — parse dead UTXOs from `test-dead-utxos.csv`
2. **Entropy-based selection** — SHA-256 scoring (mirrors Rust `selectUtxoByEntropy`)
3. **Fetch from Bitcoin** — live mempool.space API (`/tx/{txid}` + `/tx/{txid}/outspend/{vout}`)
4. **Verify dead** — confirm OP_RETURN / spent / dust (<546 sat)
5. **Create anchor** — `entropy_anchor_v1` with SHA-256(anchor_data + entropy + utxo_id)

### Scan Result Shape

```json
{
  "scanId": "hex",
  "steps": [{ "step": 1, "title": "...", "status": "ok|error|warn", "data": {...}, "durationMs": 42 }],
  "summary": {
    "utxoId": "txid:vout", "txid": "...", "vout": 0, "type": "op_return",
    "blockHeight": 800000, "anchorId": "hex", "dataHash": "hex",
    "isDead": true, "deadReason": "OP_RETURN outputs are provably unspendable...",
    "explorerUrl": "https://mempool.space/tx/...", "decodedData": { "decoded": "..." }
  },
  "durationMs": 1234, "timestamp": "ISO"
}
```

### Discovery (`/api/utxo/discover?blocks=N`)

Scans N recent Bitcoin blocks (max 10) via mempool.space API. Finds OP_RETURN and dust outputs. Appends to `test-dead-utxos.csv`. Rate-limited (500ms between blocks). Returns:

```json
{
  "discovered": [{ "type": "op_return", "txid": "...", "vout": 0, "block_height": 940000, "data": "hex", "decoded": {...} }],
  "added": 5, "skippedDuplicates": 2,
  "scannedBlocks": [{ "height": 940000, "txsScanned": 25, "found": 3 }],
  "errors": [], "durationMs": 4500
}
```

### Inventory File (`test-dead-utxos.csv`)

```csv
type,txid,vout,block_height,data,satoshis,fee_rate_threshold,address,spent_in_block,spent_at_height
op_return,6033990087599ce3cc6fd6f90694736fb9d7912bf5b2eec973389adf29066634,0,800000,68747470733a2f...,0,,,,
```

Grows dynamically via `/api/utxo/discover`. The scanner re-reads it on each scan.

### 4 Dead UTXO Types (from `utxo_fetcher.rs`)

| Type | Why it's dead | Anchor quality |
|------|--------------|----------------|
| `op_return` | Bitcoin consensus makes OP_RETURN permanently unspendable | Highest — provably dead |
| `spent` | Already spent — double-spend is cryptographically impossible | High |
| `dust` | Below 546 sat minimum relay — uneconomic to spend at any fee rate | Medium |
| `burn` | Sent to known burn address (e.g. `1111111111111111114oLvT2`) | High |

### Rust UTXO Fetcher (`utxo_fetcher.rs`, 916 lines)

Key public API:
- `fetch_utxo(txid, vout)` — live fetch from mempool.space/blockstream.info with LRU cache
- `fetch_batch(queries)` — batch fetch
- `search_utxos(query)` — search by address/value/confirmation/block/type
- `find_anchoring_utxos(preference, count)` — find best dead UTXOs by preference
- `find_entropy_anchoring_utxos(preference, count, anchor_data)` — entropy-weighted selection
- `create_entropy_anchor(data, preference)` — create anchor bound to best scoring UTXO
- `create_entropy_anchor_with_reference(data, preference, storage_ref)` — with IPFS/storage ref
- `preview_entropy_anchor(preference, data)` — preview without creating
- `verify_anchor(anchor_id)` — verify stored anchor

## Dashboard (`l2-mining/miner-dashboard/index.html`)

Single-file SPA (~4300+ lines). All HTML, CSS, and JS in one file.

### Key Sections

| Section | Feature | Collapsible |
|---------|---------|-------------|
| Unified Entropy Pipeline | 3 entropy source cards (PoW, Dead UTXO, Stale Block) | Yes (`<details open>`) |
| UTXO Scan Panel | 4-tab panel (Live Scan, History, Inventory, Use Cases) | Via close button |
| Solution Storage | Telemetry history table with filters | Yes (`<details open>`) |
| Personal Threat Dashboard | Security threat analysis | Yes (`<details open>`) |
| Relay Profile | RPC relay analysis | Yes (`<details open>`) |
| System & Chain Status | Service health, chain info | Yes (`<details open>`) |
| Epoch Explorer | On-chain epoch browser | Yes (`<details open>`) |

### UTXO Panel Tabs

| Tab | Content |
|-----|---------|
| ⚡ Live Scan | 5-step pipeline with animated steps, raw data toggle, scan summary, anchor certificate |
| 📜 History | All scans this session, clickable to replay details |
| 📦 Inventory | Dead UTXO browser with type badges + "Discover more" button (1-10 blocks) |
| 💡 Use Cases | 8 collapsible real-world scenarios with data flows and API examples |

### Use Cases (in Use Cases tab)

1. **Document Notarisation** — contracts, NDAs, wills, IP disclosure (Ready now)
2. **Supply Chain & Inventory Tracking** — cold chain, warehouse counts, luxury goods (Ready now)
3. **Legal Evidence Chain** — screenshots, whistleblower protection, IP infringement (Ready now)
4. **Carbon Credit & ESG Provenance** — IoT sensors, solar readings, carbon offsets (Ready now)
5. **Academic Priority & Research** — pre-prints, dataset versioning, lab notebooks (Ready now)
6. **Software Build Provenance** — release signing, CVE dating, firmware, Docker images (API)
7. **Financial Audit Trail** — daily NAV, tax records, compliance reports (API)
8. **Decentralised Identity & PKI** — mining wallet as device identity (Coming soon)

### UTXO Auto-Scan Behaviour

- Auto-scans 5s after page load if no previous scan exists
- Re-scans every 5 minutes automatically
- Accumulates anchor count across scans (shown in header)
- Freshness indicator decays from "Just scanned" → timestamp after 30s
- Certificate shows full anchor proof, copyable as JSON

### Dashboard JS Constants

```javascript
const RANDOMNESS_API = 'http://127.0.0.1:4271';
const UTXO_RESCAN_INTERVAL = 300000;   // 5 min periodic re-scan
const UTXO_AUTO_SCAN_DELAY = 5000;     // 5s initial auto-scan
```

### Key Dashboard Functions (UTXO)

| Function | Purpose |
|----------|---------|
| `runUtxoScan(opts)` | Main scan — calls `/api/utxo/scan`, renders steps, certificate |
| `utxoSwitchTab(name)` | Switch between scan/history/inventory/usecases tabs |
| `utxoLoadInventory()` | Fetch `/api/utxo/inventory`, render typed grid |
| `utxoLoadHistory()` | Merge local + server history, render clickable list |
| `utxoDiscover()` | Fetch `/api/utxo/discover?blocks=N`, add to inventory |
| `utxoRenderCertificate(scan)` | Render 12-field anchor certificate |
| `utxoCopyProof(scan)` | Copy full anchor proof JSON to clipboard |
| `utxoUpdateCard(scan)` | Update entropy card metrics + entropyState |
| `utxoShowHistoryScan(idx)` | Replay a historical scan's steps in the scan tab |

### Key Dashboard Functions (Mining Controls)

| Function | Purpose |
|----------|---------|
| `fetchMiningControl()` | `GET /api/miner/control` → update `miningCtrlState`, sync UI |
| `postMiningControl(data)` | `POST /api/miner/control` → write new state, sync UI |
| `toggleMiningPause()` | Toggle `paused` flag |
| `setMiningPower(pct)` | Set power to 25/50/75/100% |
| `syncMiningControlUI()` | Update pause button text/color, power button active states, status label |

## Mining Control System

### Architecture

The miner (Rust) and dashboard (Node.js) are **separate processes** that communicate via a shared JSON file.

```
Dashboard (port 4173)                    Miner binary (AppData)
  ┌──────────────┐                       ┌─────────────────────┐
  │ Pause button  │──POST /api/miner/──▶│  miner-control.json  │
  │ Power buttons │    control           │  (next to telemetry) │
  └──────┬───────┘                       └──────────┬──────────┘
         │ GET /api/miner/control                    │ read each cycle
         ▼                                           ▼
  ┌──────────────┐                       ┌─────────────────────┐
  │ UI syncs     │◀──────────────────────│ runtime.rs loop     │
  │ every 5s     │   telemetry.jsonl     │ paused → sleep 2s   │
  └──────────────┘   (mining_paused,     │ power → fewer workers│
                      mining_power_pct)   └─────────────────────┘
```

### Control File (`miner-control.json`)

```json
{"paused": false, "power_pct": 100}
```

- Lives next to `telemetry.jsonl` in `%LOCALAPPDATA%\entropy\TemporalGradientMiner\data\logs\`
- Dashboard `server.js` reads/writes it via `GET/POST /api/miner/control`
- Miner reads it at the top of each mining cycle in `run_live_runtime()`
- If missing or unreadable → defaults to `{paused: false, power_pct: 100}`
- Power values clamped to `[25, 50, 75, 100]` via `MiningControl::normalized_power_pct()`
- Worker count: `effective_workers = (max_threads * power_pct / 100).max(1)`

### Rust Types (`telemetry.rs`)

```rust
pub struct MiningControl {
    pub paused: bool,      // default false
    pub power_pct: u8,     // default 100, clamped to 25/50/75/100
}
// TelemetrySnapshot includes:
//   mining_paused: Option<bool>
//   mining_power_pct: Option<u8>
```

### Pause Behavior

- When `paused=true`, the miner enters a tight 2-second sleep loop
- Phase stays as `Searching` (idle indicator)
- Stale block harvesting also pauses (since it's part of the same runtime)
- Telemetry still emits snapshots (with `mining_paused: true`)
- Heartbeat sidecar suppresses `hashrate_drop` and `heartbeat_gap` alerts when paused

## Heartbeat Sidecar Alert Suppression

`l2-mining/security/heartbeat-sidecar.js` suppresses `hashrate_drop` and `heartbeat_gap` anomalies when:

| Condition | Variable | Meaning |
|-----------|----------|----------|  
| Waiting phase | `isWaitingPhase` | Commit-reveal idle (clearance, locked, committing, revealing) |
| Paused | `isPaused` | Operator paused via dashboard (`mining_paused === true` in telemetry) |
| Entropy-active | `isEntropyActive` | `stale_block_count` increasing in recent snapshots |

Combined: `suppressHashrateAlerts = isWaitingPhase \|\| isPaused \|\| isEntropyActive`

## Stale Block Telemetry Data Flow

**CRITICAL:** Always verify the FULL pipeline when debugging stale block display issues.

```
StaleBlockMiner.stats()          → LoserChainStats (Rust struct)
  ├── total_stale_blocks         → TelemetrySnapshot.stale_block_count
  ├── max_reorg_depth            → TelemetrySnapshot.stale_fork_depth
  ├── max_leading_zeros          → TelemetrySnapshot.stale_zero_bits
  ├── average_quality_score      → TelemetrySnapshot.stale_quality
  └── cumulative_entropy_hex     → TelemetrySnapshot.stale_xor_hex

telemetry.jsonl (JSON lines)
  └── Dashboard server.js reads via /api/latest, /api/history

Dashboard index.html (JS)
  ├── entropyState.stale.count       ← snap.stale_block_count
  ├── entropyState.stale.forkDepth   ← snap.stale_fork_depth
  ├── entropyState.stale.zeroBits    ← snap.stale_zero_bits
  ├── entropyState.stale.realQuality ← snap.stale_quality    ⚠️ MUST USE REAL DATA
  ├── entropyState.stale.realXorHex  ← snap.stale_xor_hex   ⚠️ MUST USE REAL DATA
  └── entropyState.stale.realTipHeight ← snap.bitcoin_tip_height
```

**Past bug:** Dashboard JS was overwriting real quality/XOR from telemetry with fake `pseudoHash()` values every tick. Fixed by reading `snap.stale_quality` and `snap.stale_xor_hex` directly.

## Dashboard Stale Proof Developer View

- The Rust miner keeps the full stale proof in memory as `StaleWorkProof` in `l2-mining/rust/temporal_gradient_core/src/stale_block_miner.rs`.
- `StaleWorkProof` includes `proof_id`, `raw_header`, `block_hash`, `height`, `canonical_hash`, `leading_zeros`, `reorg_depth`, `entropy`, `quality_score`, `submitter`, and `created_at`.
- Telemetry now exports the **latest pending** stale proof fields in addition to the aggregate counters: `stale_proof_id`, `stale_raw_header_hex`, `stale_block_hash_hex`, `stale_canonical_hash`, `stale_entropy_digest`, `stale_submitter`, and `stale_created_at`.
- This lets the dashboard show a developer-facing pending stale proof JSON without asking the miner to re-fetch header data.
- Transaction hashes (`submitTxHash`, `claimTxHash`) are still **not** mirrored into miner telemetry, but the dashboard now backfills them from live `StaleBlockSubmitted` and `StaleRewardClaimed` event logs.
- The dashboard endpoint `/api/stale/developer-proof` now returns a compact factual payload with three sections: `proof`, `tx`, and `onChain`.
- The endpoint also handles the stale-proof hash byte-order mismatch by trying both the telemetry/display hash and the reversed `bytes32` form stored by the oracle.

## Dashboard Solution Store Dedup

- `l2-mining/miner-dashboard/solution-store.js` dedupes persisted solution rows on load and on insert.
- Stale entries are deduped by `stale:<hash>` so the same XOR/stale summary does not get inserted repeatedly for hours.
- Accepted rows are deduped by `accepted:<nonce>:<outputHash>`.
- Rejected rows are deduped by `rejected:<nonce>:<phase>:<hash>`.

## Dashboard Entropy Pipeline — Reward Computation (JS)

The dashboard computes TGBT rewards client-side for display. This mirrors the on-chain formulas.

### entropyState.tgbt Object

```javascript
entropyState.tgbt = {
  pow: 0,          // From effectiveRewards(snap) — real on-chain or estimated
  powOnChain: false, // true if reward confirmed on-chain (not estimated)
  utxo: 0,         // Always 0 — no on-chain UTXO reward mechanism
  stale: 0,        // Computed: 50 × quality × min(depth+1, 7) / 100
  total: 0,        // pow + utxo + stale
};
```

### Stale Reward JS Formula

```javascript
const sq = entropyState.stale.quality || 0;   // 0-100 from real telemetry
const sd = entropyState.stale.forkDepth || 0;  // from real telemetry
entropyState.tgbt.stale = sq > 0 ? (50 * sq * Math.min(sd + 1, 7)) / 100 : 0;
```

This matches the Solidity `_calculateReward()` in StaleBlockOracle:
`baseReward × qualityScore × min(reorgDepth + 1, MAX_DEPTH_MULTIPLIER) / 100`

### Display Text States

| State | Card Display | Breakdown Display |
|-------|-------------|------------------|
| Disabled | `'Disabled'` | `'Disabled'` |
| Active (has blocks) | `'{amount} TGBT'` | `'{amount} TGBT ✓'` |
| Scanning (no blocks yet) | `'0.000 TGBT (scanning)'` | `'0.000 TGBT (scanning)'` |

### Total Label Logic

- Both PoW + Stale active → `'{total} TGBT (PoW + Stale)'`
- Only PoW → `'{total} TGBT (PoW)'`
- Neither → `'{total} TGBT'`

### Block Log Entries

Stale block log entries show `+{reward} TGBT` in gold (was previously muted + "(pending)"). Reward is now live via `claimReward()`.

## TelemetrySnapshot — Complete Field Reference (telemetry.rs)

The Rust struct `TelemetrySnapshot` is the **single source of truth** for all miner data. It is serialized as compact JSON (one line per snapshot) to `telemetry.jsonl`. Fields with `#[serde(skip_serializing_if = "Option::is_none")]` are **omitted** from JSON when `None`.

### Always-present fields

| JSON field | Type | Description |
|------------|------|-------------|
| `timestamp_unix_ms` | u128 | Unix epoch in milliseconds |
| `state` | enum | `"starting"` / `"running"` / `"stopping"` / `"stopped"` |
| `uptime_seconds` | u64 | Seconds since miner started |
| `worker_count` | usize | Active mining threads |
| `hashes` | u64 | Total nonces computed |
| `hashrate_hs` | f64 | Current hashes/second |
| `solutions` | u64 | Total solutions found |
| `accepted_submissions` | u64 | On-chain accepted commits |
| `rejected_submissions` | u64 | Rejected submissions |
| `total_rewards_estimate` | f64 | On-chain PoW TGBT total |
| `output_count` | u64 | Bloom filter outputs |
| `last_solution_nonce` | Option<u64> | Last solution's nonce |
| `last_solution_hash_hex` | Option<String> | Last solution's hash |
| `temperature_c` | Option<f32> | CPU temperature |

### Conditionally-present fields (omitted when None/empty)

| JSON field | Type | Description |
|------------|------|-------------|
| `last_commit_hash_hex` | String | Last on-chain commit hash |
| `last_output_hash_hex` | String | Last beacon output hash |
| `filter_fp_rate` | f64 | Bloom filter false-positive rate |
| `filter_memory_kb` | u64 | Bloom filter memory usage |
| `epoch_stats` | HashMap<u64,u64> | Epoch → solution count map |
| `mining_phase` | enum | `"searching"` / `"solution_found"` / `"waiting_for_clearance"` / `"committing"` / `"commitment_locked"` / `"revealing"` / `"reward_received"` |
| `phase_blocks_remaining` | u64 | Blocks until phase change |
| `phase_eta_seconds` | u64 | Estimated time until phase change |
| `mining_paused` | bool | True if operator paused mining |
| `mining_power_pct` | u8 | Current power level (25/50/75/100) |
| `stale_block_count` | u64 | Total orphan blocks detected |
| `stale_fork_depth` | u32 | Max reorg depth seen |
| `stale_zero_bits` | u32 | Max leading zero bits in orphan hash |
| `stale_quality` | u32 | Average quality score (0-100) |
| `stale_xor_hex` | String | Cumulative XOR of all orphan hashes |
| `bitcoin_tip_height` | u64 | Latest Bitcoin block height |
| `stale_pending_proofs` | u64 | Proofs queued for submission |

### MiningControl struct

```rust
pub struct MiningControl {
    pub paused: bool,      // default false
    pub power_pct: u8,     // default 100, snapped to 25/50/75/100
}
```
- File: `miner-control.json` (next to `telemetry.jsonl`)
- `normalized_power_pct()` snaps to nearest of [25, 50, 75, 100]
- `effective_workers(max)` = `(max * pct / 100).max(1)`

## Dashboard `server.js` — API Response Shapes

The dashboard server at `l2-mining/miner-dashboard/server.js` reads `telemetry.jsonl` directly with **no field transformation**. Raw Rust snapshot JSON passes through to the dashboard.

### `readSnapshots(limit)` — Core reader

Reads entire `telemetry.jsonl`, splits by newlines, takes last `limit` lines, JSON-parses each. Returns array of raw snapshot objects.

### Response shapes

```javascript
// GET /api/latest
{ telemetryPath: "<path>", latest: <snapshot or null> }

// GET /api/history?limit=N  (default 120, max 500)
{ telemetryPath: "<path>", latest: <snapshot or null>, history: [<snapshots>] }

// GET /api/miner/control
{ paused: false, power_pct: 100 }  // or current values from file

// POST /api/miner/control  body: {"paused": true, "power_pct": 50}
// Validates: power_pct in [25,50,75,100], paused coerced to boolean
// Writes to miner-control.json, returns sanitized object

// GET /api/stale/developer-proof
// Returns compact stale-proof data from telemetry + live oracle state + event-derived tx hashes
// Top-level keys: source, status, proof, tx, onChain
```

## Dashboard `entropyState` — Complete Object Structure

Defined at line ~3744 of `index.html`. This is the **client-side state** that drives all entropy card rendering.

```javascript
const entropyState = {
  initialized: false,
  tick: 0,
  btcTipHeight: 941529,
  pow:      { hashrate: 0, nonces: 0, solutions: 0, accepted: 0, quality: 0, hash: '', phase: 'searching', diffBits: 11 },
  utxo:     { anchored: 0, scanned: 0, height: 0, quality: 0, hash: '', fresh: false, status: 'offline' },
  stale:    { count: 0, forkDepth: 0, zeroBits: 0, quality: 0, hash: '', xorPool: '', blocks: [], status: 'offline' },
  combined: { hash: '', quality: 0, divergence: 0, mixRounds: 8 },
  tgbt:     { pow: 0, utxo: 0, stale: 0, total: 0, powOnChain: false },
};
```

Runtime-only stale fields (set by `initEntropyPipeline()` / `tickEntropyPipeline()`):
- `stale.realQuality` — raw `snap.stale_quality` (u32 or null)
- `stale.realXorHex` — raw `snap.stale_xor_hex` (string or null)
- `stale.realTipHeight` — raw `snap.bitcoin_tip_height` (u64 or 0)

## Dashboard Stale Mining Toggle — CRITICAL

The stale block card has a **client-side toggle** that controls whether stale data is displayed. This is independent of the Rust miner's `stale_block.enabled` config.

### Toggle mechanics

```javascript
const STALE_MINING_KEY = 'tgbt-stale-mining-enabled';
let staleMiningEnabled = localStorage.getItem(STALE_MINING_KEY) !== 'false';
// Default: TRUE (unless user has explicitly toggled it off)
```

### Auto-detection

When telemetry reports `stale_block_count > 0` and the toggle is off, the dashboard auto-enables it:
```javascript
if (entropyState.stale.count > 0 && !staleMiningEnabled) {
  staleMiningEnabled = true;
  localStorage.setItem(STALE_MINING_KEY, 'true');
  syncStaleToggleUI();
}
```
This happens in both `initEntropyPipeline()` and `tickEntropyPipeline()`.

### Rendering gate

```javascript
const staleActive = staleMiningEnabled && ep.stale.count > 0;
const staleDisabled = !staleMiningEnabled;
```

When `staleDisabled=true`: card shows "Disabled", all metrics show "—", hash shows "Stale mining disabled".
When `staleActive=true`: card shows real values, reward shows `"33.50 TGBT"`.
When `staleMiningEnabled=true` but `count=0`: card shows "0.000 TGBT (scanning)".

### Past bug

`staleMiningEnabled` previously defaulted to `false` (used `=== 'true'` check). This meant the stale card showed "Waiting for stale blocks…" even when the miner was actively harvesting. Fixed by changing to `!== 'false'` (defaults true) + auto-detection from telemetry.

## Dashboard Polling Architecture

The dashboard uses **multiple independent loops**, NOT a single refresh function:

| What | Method | Interval | Init |
|------|--------|----------|------|
| Telemetry | SSE (`EventSource('/events')`) | Real-time push | `loadHistory().then(connect)` at startup |
| Mining control | `fetchMiningControl()` | 5 s | Immediate |
| Entropy pipeline | `tickEntropyPipeline()` | 3 s | 1.5 s delay → `initEntropyPipeline()` |
| Solutions table | `loadSolutions()` | 10 s | At startup |
| System/Security/Network/Epochs | `Promise.allSettled(...)` | 15 s | At startup |
| UTXO re-scan | `runUtxoScan({ auto: true })` | 5 min | 5 s delay (if no anchors) |

### Startup sequence (no DOMContentLoaded — inline script)

1. `els` object built from all `[id]` elements via `Object.fromEntries([...document.querySelectorAll('[id]')].map(el => [el.id, el]))`
2. `fetchMiningControl()` + 5s interval
3. 1.5s timeout → `initEntropyPipeline()` → 3s interval `tickEntropyPipeline()`
4. UTXO: load inventory + history, auto-scan at 5s if no anchors, rescan every 5 min
5. `loadHistory().then(connect)` — loads 120 snapshots, then SSE at `/events`
6. `loadSolutions()` + 10s interval
7. `Promise.allSettled([loadSecurity, loadSystemStatus, loadNetworkLatest, loadEpochs])` + 15s interval

### Key helper functions

| Function | Purpose |
|----------|---------|
| `effectiveRewards(snap)` | Returns `{total, perSolution, estimated}` from `total_rewards_estimate` or fallback estimate |
| `fmtTGBT(v)` | Format TGBT with 2-4 decimals, returns `'0.00'` for null/zero |
| `fmtNum(n)` | Thousands-separated number formatting |
| `fmtHashrate(h)` | Human-readable hashrate (H/s, kH/s, MH/s) |
| `normalizedPhase(snap)` | Extracts `mining_phase` from snapshot, returns snake_case string |
| `syncStaleToggleUI()` | Updates toggle visuals, card grayed state, status text |
| `setBadge(el, text, cls)` | Sets badge element text + className (`ok`, `warn`, `fail`) |

### Hero TGBT display

```javascript
const rewards = effectiveRewards(snap);
const staleReward = entropyState.tgbt.stale;
const combinedTotal = rewards.total + staleReward;
const sourceLabel = staleReward > 0 ? ' (PoW + Stale)' : '';
els.tgbtTotal.innerHTML = `${fmtTGBT(combinedTotal)} <span class="unit">TGBT${estLabel}${sourceLabel}</span>`;
```

### Dashboard element IDs — Entropy Pipeline

| Element ID | Card | Shows |
|------------|------|-------|
| `epHashrate` | PoW | Hashrate (H/s) |
| `epNonces` | PoW | Total nonces |
| `epSolutions` | PoW | Solutions found |
| `epCommitReveal` | PoW | Mining phase text |
| `epDifficulty` | PoW | Difficulty bits |
| `epPowHash` | PoW | Entropy hash |
| `epPowQuality` | PoW | Quality bar fill |
| `epPowQualityLabel` | PoW | "X/100" |
| `epPowTgbt` | PoW | PoW TGBT reward |
| `powStatus` | PoW | Badge (Active/Idle) |
| `epUtxoCount` | UTXO | Anchored count |
| `epUtxoScanned` | UTXO | Scanned count |
| `epUtxoHeight` | UTXO | Bitcoin height |
| `epUtxoEnterprise` | UTXO | Enterprise status |
| `epUtxoFreshness` | UTXO | Freshness badge |
| `epUtxoHash` | UTXO | Entropy hash |
| `epUtxoQuality` | UTXO | Quality bar |
| `epUtxoQualityLabel` | UTXO | "X/100" |
| `epUtxoTgbt` | UTXO | Anchor info text |
| `utxoStatus` | UTXO | Badge (Active/Offline) |
| `epStaleCount` | Stale | Orphan blocks found |
| `epStaleForkDepth` | Stale | Fork depth |
| `epStaleZeroBits` | Stale | Leading zero bits |
| `epStaleXorStatus` | Stale | XOR pool status |
| `epStaleTipHeight` | Stale | Bitcoin tip height |
| `epStaleHash` | Stale | XOR pool hash |
| `epStaleQuality` | Stale | Quality bar |
| `epStaleQualityLabel` | Stale | "X/100" |
| `epStaleTgbt` | Stale | **TGBT reward value** |
| `staleStatus` | Stale | Badge (Active/Scanning/Disabled) |
| `staleSourceCard` | Stale | Card container (toggle inactive class) |
| `epStaleLog` | Stale | Recent blocks log entries |
| `tgbtTotal` | Hero | Total TGBT earned (PoW + Stale) |
| `entropyPipelineStatus` | Pipeline | Sources active badge |
| `entropyLastUpdate` | Pipeline | Last update timestamp |

### localStorage keys

| Key | Default | Purpose |
|-----|---------|---------|
| `tgbt-stale-mining-enabled` | `true` (unless explicitly 'false') | Stale mining toggle state |

No other localStorage keys are used by the dashboard.

## On-Chain Proof Submission — chain.rs Architecture

### Pre-flight pattern (added to bypass NativeBTC RPC bug)

NativeBTC's RPC proxy strips revert data from `eth_estimateGas`. This means `ethers.send()` fails with `data: 0x` (empty) on contract reverts, hiding the actual error reason.

**Fix pattern used in chain.rs:**
1. **Pre-flight `eth_call`** — simulate the transaction to get full revert data
2. **Explicit gas limit** — skip `eth_estimateGas` entirely
3. **Detailed error logging** — parse the pre-flight revert for actual error selector

```
submit_stale_proof():
  1. Resolve StaleBlockOracle address via Core → moduleAddress(STALE_BLOCK_MODULE)
  2. Build submitStaleBlock() calldata
  3. Pre-flight eth_call → if reverts, log actual error and return Err
  4. send() with gas_limit=500_000 → polls for receipt
  5. On success: immediately call claimReward() with gas_limit=300_000

record_fork_event_onchain():
  1. Check forkEventsAtHeight() to skip duplicates
  2. If count == 0 → call recordForkEvent() (requires onlyCoreOrModule gate)
```

### Error classification (runtime.rs)

When `submit_pending_proofs()` encounters a revert:
- **Permanent drop** (don't retry): revert with non-empty data containing specific error strings (`STALE_BLOCK_MODULE`, `address(0)`, `module not registered`, `Pre-flight rejected`)
- **Transient retry** (requeue): `data: 0x` (empty revert), network errors, timeout errors
- This prevents valid proofs from being permanently dropped due to RPC proxy quirks

The project uses Foundry's `cast` CLI for reading/writing Arbitrum state.

### Setup Variables (PowerShell)

```powershell
$RPC = "https://api.nativebtc.org/v1/arb?key=fp_2d93df5e6cebe485b69c363a62e237fc9d0f88b9"
$CORE = "0xF6556DDC7CdD3635A05428BD85BCf33A09F752e6"
$STALE_ORACLE = "0xdc4eDF632187d05da50393Af87D19A08f6986517"
$HOT_WALLET = "0x5cB4D906f0464b34c44d6555A770BF6aF4A2cEfe"
$PRIVATE_KEY = Get-Content "l2-mining/rust/keys/miner.key" -Raw  # 64 hex chars, no 0x
```

### Common Queries

```powershell
# Check if address is registered module
cast call $CORE "isModule(address)(bool)" $HOT_WALLET --rpc-url $RPC

# Check modules locked status
cast call $CORE "modulesLocked()(bool)" --rpc-url $RPC

# Read oracle parameters
cast call $STALE_ORACLE "baseReward()(uint256)" --rpc-url $RPC | ForEach-Object { cast to-unit $_ ether }
cast call $STALE_ORACLE "minLeadingZeros()(uint32)" --rpc-url $RPC
cast call $STALE_ORACLE "maxReorgDepth()(uint32)" --rpc-url $RPC
cast call $STALE_ORACLE "maxStaleAgeSecs()(uint64)" --rpc-url $RPC

# Register a module (requires owner/governance)
cast send $CORE "setModule(bytes32,address)" $(cast keccak "FORK_RELAY") $HOT_WALLET --rpc-url $RPC --private-key $PRIVATE_KEY

# Get module address by ID
cast call $CORE "moduleAddress(bytes32)(address)" $(cast keccak "FORK_RELAY") --rpc-url $RPC
```

### Arbitrum-Specific Notes

- `cast send` frequently returns "null response" — always verify with `cast call`
- Use `cast to-unit <wei> ether` to convert uint256 to human-readable TGBT amounts
- `cast keccak "STRING"` computes the keccak256 hash used for module IDs
