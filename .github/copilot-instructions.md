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
│   │   │       ├── tg_output_filter.rs  ← Bloom filter output dedup
│   │   │       ├── utxo_fetcher.rs      ← UTXO fetcher, entropy anchoring, 4 dead UTXO types (916 lines)
│   │   │       └── bitcoin_dead_utxo_anchor.rs ← DeadUTXOAnchorDB, anchor create/verify (396 lines)
│   │   ├── package/                     ← installer/packaging crate
│   │   ├── keys/                        ← miner.key (private key), miner.pending.json
│   │   └── target/release/              ← ⚠️ BUILD OUTPUT lives here (workspace-level)
│   │       └── temporal-gradient-miner.exe
│   ├── js/                              ← beacon-api-server (port 3100)
│   ├── miner-dashboard/                 ← Vite dashboard (port 4173)
│   │   ├── index.html                   ← single-file SPA (~4300+ lines, all HTML/CSS/JS)
│   │   └── server.js                    ← Node.js dashboard server (proxies to APIs)
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
│   └── contracts/modules/
│       ├── BatchMiningModule.sol        ← on-chain epoch commit/finalize contract
│       ├── RandomnessShop.sol           ← sells randomness proofs, not tokens
│       └── UniversalMinerGasPool.sol    ← Arbitrum ETH gas reimbursement pool with attestor-signed epochs
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

| Contract | Address |
|----------|---------|
| Core (TemporalGradientBeacon) | `0xF6556DDC7CdD3635A05428BD85BCf33A09F752e6` |
| BatchMiningModule | `0xAf07E37D104E9be17639FE7a51B36972D4738651` |
| TGBT Token | `0x31228eE520e895DA19f728DE5459b1b317d9b8D8` |
| TokenomicsModule (active) | `0xF6069614FE09B91e5B00DA0a13A11B2BFcCabC36` |
| TokenomicsModule (old, deauthorized) | `0xA9f684d709bB46155A252b260dDDE4cb2a37a0E3` |

Wallet (hot): `0x5cB4D906f0464b34c44d6555A770BF6aF4A2cEfe` (has GOVERNANCE_ROLE on Core), Pool ID: 3
Wallet (Ledger): `0xd28E6a7AD806E85BD0544ed443D25E48f52c06c3` (Core owner/DEFAULT_ADMIN_ROLE, TGBT governance, HD path `m/44'/60'/1'/0/0`)

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

| Component | Purpose |
|-----------|---------|
| `TemporalGradientCore` | Core beacon contract, module registry |
| `BatchMiningModule` | Epoch commit/finalize with challenge window |
| `RandomnessShop` | Proof marketplace; sells randomness proofs and anchor receipts |
| `UniversalMinerGasPool` | ETH reimbursement vault for miner gas sponsorship on Arbitrum |
| `TGBT Token` | Mining reward token (ERC-20) |
| `TokenomicsModule` | Reward distribution, pool management |
| `MiningModule` | Individual commit-reveal mining |
| `StaleBlockModule` | Stale block proof verification |

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
