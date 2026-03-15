# Batch Mining & Randomness API

Low-gas epoch-based mining for Temporal Gradient Beacon. Instead of one
on-chain transaction per solution, the miner accumulates N solutions
off-chain, builds a Merkle tree, and anchors **one root** on-chain per
epoch.

## Architecture

```
┌────────────────────────────────────────────────────────────┐
│  Rust Miner (existing)                                     │
│  Mines solutions continuously → telemetry.jsonl            │
└──────────────────┬─────────────────────────────────────────┘
                   │ watches file
┌──────────────────▼─────────────────────────────────────────┐
│  Epoch Builder  (epoch-builder.js)                          │
│  1. Reads new accepted solutions from telemetry             │
│  2. Accumulates SOLUTIONS_PER_EPOCH (default 50)            │
│  3. Builds Merkle tree of output hashes                     │
│  4. POSTs epoch+leaves to Randomness API                    │
│  5. Calls BatchMiningModule.commitEpochRoot() on-chain      │
│  6. After challenge window → finalizeEpoch() to claim TGBT  │
└──────────────────┬─────────────────────────────────────────┘
                   │
┌──────────────────▼─────────────────────────────────────────┐
│  Randomness API  (server.js)                                │
│  Serves randomness to consumers:                            │
│    GET /api/randomness/latest          latest output hash    │
│    GET /api/randomness/:hash/proof     Merkle proof          │
│    GET /api/epochs                     epoch list            │
│    GET /api/epochs/:id                 epoch detail           │
│    GET /api/health                     service health         │
└────────────────────────────────────────────────────────────┘
                   │
┌──────────────────▼─────────────────────────────────────────┐
│  BatchMiningModule.sol (on-chain)                           │
│  - commitEpochRoot()     one tx per epoch (not per solution)│
│  - finalizeEpoch()       mints TGBT for all solutions       │
│  - verifyRandomnessLeaf() anyone can verify a random output │
└────────────────────────────────────────────────────────────┘
```

## Gas Savings

| Mode | Tx per 50 solutions | Est. gas (Arbitrum One) |
|------|---------------------|------------------------|
| Per-solution (current) | 100 (commit+reveal each) | ~5,000,000 total |
| **Batch epoch** | **2** (commit + finalize) | **~200,000 total** |

**~25x reduction in gas cost.**

## Quick Start

### 1. Start the Randomness API
```bash
cd l2-mining/randomness-api
node server.js
# → http://127.0.0.1:4271
```

### 2. Run the Epoch Builder
```bash
# With on-chain anchoring:
MINER_PRIVATE_KEY=0xYOUR_KEY \
BATCH_CONTRACT=0xDEPLOYED_ADDRESS \
SOLUTIONS_PER_EPOCH=50 \
node epoch-builder.js

# Local-only (no chain tx, just builds epochs):
node epoch-builder.js
```

### 3. Query randomness
```bash
# Latest random output
curl http://127.0.0.1:4271/api/randomness/latest

# Merkle proof for a specific output
curl http://127.0.0.1:4271/api/randomness/0xABC.../proof

# Epoch list
curl http://127.0.0.1:4271/api/epochs
```

### 4. Verify on-chain
Any consumer can call `verifyRandomnessLeaf()` on `BatchMiningModule` to
confirm an output hash belongs to a finalized epoch, using the Merkle proof
returned by the API.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `RANDOMNESS_HOST` | `127.0.0.1` | API bind host |
| `RANDOMNESS_PORT` | `4271` | API port |
| `TELEMETRY_FILE` | `../rust/miner-telemetry.jsonl` | Miner telemetry path |
| `SOLUTIONS_PER_EPOCH` | `50` | Solutions per epoch batch |
| `RPC_URL` | Sepolia public node | Chain RPC |
| `BATCH_CONTRACT` | *(none)* | Deployed BatchMiningModule address |
| `MINER_PRIVATE_KEY` | *(none)* | Miner wallet private key |
| `POOL_ID` | `0` | Mining pool ID |
| `POLL_INTERVAL` | `30000` | Telemetry poll interval (ms) |
| `EPOCH_STORE` | `./epoch-store/` | Local epoch storage directory |

## Contract Deployment

Deploy `BatchMiningModule.sol` via Forge:

```bash
cd l2-mining
forge create contracts/modules/BatchMiningModule.sol:BatchMiningModule \
  --rpc-url $RPC_URL \
  --private-key $DEPLOYER_KEY

# Then call initialize:
cast send $BATCH_CONTRACT \
  "initialize(address,address)" \
  $CORE_ADDRESS $STAKE_TOKEN_ADDRESS \
  --rpc-url $RPC_URL \
  --private-key $DEPLOYER_KEY

# Register as the dedicated batch slot (keeps classic MiningModule active)
cast send $CORE_ADDRESS \
  "setModule(bytes32,address)" \
  $(cast keccak "BATCH_MINING_MODULE") $BATCH_CONTRACT \
  --rpc-url $RPC_URL \
  --private-key $DEPLOYER_KEY
```

With this setup, both modules coexist:
- `MINING_MODULE` → classic `MiningModule`
- `BATCH_MINING_MODULE` → `BatchMiningModule`

`TokenomicsModule` accepts rewards from either slot.

## Files

```
contracts/
  interfaces/IBatchMiningModule.sol   ← interface
  modules/BatchMiningModule.sol       ← on-chain epoch contract
randomness-api/
  server.js                           ← HTTP API for randomness consumers
  epoch-builder.js                    ← orchestrator: watches miner → builds epochs → commits on-chain
  epoch-store/                        ← local JSON epoch storage (auto-created)
  README.md                          ← this file
```
