# Epoch Rules & Behaviour

## Overview

Epochs are the batch-mining unit. The miner accumulates solutions off-chain, builds a Merkle tree, and commits the root on-chain via `BatchMiningModule`. After a challenge window the operator finalises the epoch to mint TGBT.

## Lifecycle

```
Miner (Rust)            epoch-builder.js           BatchMiningModule (Arbitrum)
─────────────           ────────────────           ────────────────────────────
solutions → telemetry   poll telemetry.jsonl
                        accumulate leaves
                        build Merkle tree
                        EIP-712 sign root
                                ──commitEpochRoot()──▶  store root, start challenge
                          … 28 800 L1 blocks (~96 h) …
                                ──finalizeEpoch()────▶  mint TGBT, record output
                                ──recordStorageAttestation()──▶  mark attested
```

## On-Chain Constants

| Constant | Value | Meaning |
|----------|-------|---------|
| `EPOCH_COOLDOWN_BLOCKS` | 50 | Min L1 blocks between commits from same operator |
| `CHALLENGE_WINDOW` | 28 800 | L1 blocks before epoch can be finalised (~96 h) |
| `MAX_LEAVES_PER_EPOCH` | 10 000 | Max solutions per epoch |
| `REWARD_PER_SOLUTION` | 1.375 TGBT | Minted per leaf on finalisation |

## Key Rules

1. **Sequential IDs** — `epochId` must equal `_nextEpochId`. Out-of-order commits revert with `EpochNotFound`.
2. **EIP-712 signature** — `commitEpochRoot` requires a typed-data signature from `msg.sender`.
3. **Cooldown** — same operator cannot commit two epochs within 50 L1 blocks.
4. **Challenge window** — `finalizeEpoch` reverts with `CooldownNotElapsed` until `block.number >= startBlock + 28800`.
5. **Operator-only finalise** — only the address that committed an epoch can finalise it.
6. **Reward** — `leafCount × 1.375 TGBT`, minted via `TokenomicsModule.onBlockMined()`.
7. **Storage attestation** — optional post-finalization step; operator records an attestation hash (IPFS/storage proof).
8. **No poolId validation** — the contract stores whichever `poolId` is passed; it does not verify against MiningModule pools.

## Block Number Semantics

On Arbitrum, Solidity `block.number` returns the **L1 Ethereum mainnet** block number, not the L2 number. The challenge window is therefore ~96 hours (28 800 × 12 s L1 block time). The epoch-builder fetches the L1 number via `eth_getBlockByNumber → l1BlockNumber` to track readiness.

## epoch-builder.js Behaviour

- **Polls** `telemetry.jsonl` every 30 s using byte-offset tracking (avoids loading the full file).
- **Accumulates** accepted solutions in `pendingLeaves` (persisted in `epoch-state.json`).
- **Commits** when `pendingLeaves.length >= SOLUTIONS_PER_EPOCH` (default 10).
- **Finalise sweep** — each poll also iterates all committed epochs and finalises any whose challenge window has passed.
- **Self-correction** — if local `nextEpochId` is ahead of on-chain `_nextEpochId`, it resets and re-queues leaves.
- **Cooldown check** — if the last commit was < 50 L1 blocks ago, it defers and re-queues.

### State File (`epoch-state.json`)

```json
{
  "nextEpochId": 90,
  "processedBytes": 781921213,
  "processedLines": 30896,
  "pendingLeaves": [],
  "lastAcceptedCount": 18
}
```

- `processedBytes` — file offset for incremental reads (avoids V8 string limit on large files).
- `lastAcceptedCount` — last seen `accepted_submissions` value to detect new solutions.

### Config (`.env`)

| Var | Purpose |
|-----|---------|
| `TELEMETRY_FILE` | Absolute path to `telemetry.jsonl` |
| `SOLUTIONS_PER_EPOCH` | Leaves per epoch (default 10) |
| `POOL_ID` | Passed to `commitEpochRoot` (must match active pool) |
| `BATCH_CONTRACT` | BatchMiningModule address |
| `MINER_PRIVATE_KEY` | Hex private key for signing + sending |
| `RPC_URL` | Arbitrum RPC (must include API key) |

## Error Conditions

| Error | Cause |
|-------|-------|
| `EpochNotFound(id)` | `epochId != _nextEpochId` or epoch doesn't exist |
| `EpochAlreadyExists(id)` | Root already committed for this epoch |
| `CooldownNotElapsed` | Deadline passed, cooldown active, or challenge window not met |
| `InvalidLeafCount` | `leafCount == 0` or `> MAX_LEAVES_PER_EPOCH` |
| `InvalidMerkleProof` | EIP-712 signature doesn't match `msg.sender` |
| `NotEpochOperator` | Caller is not the epoch's committing operator |
| `EpochAlreadyFinalized` | Epoch already finalised |
| `EpochNotFinalized` | Storage attestation attempted before finalisation |
| `INSUFFICIENT_FUNDS` | Wallet lacks ETH for gas |

## Current Status (April 2026)

- 90 epochs committed (0–89).
- 57 finalised, 33 remaining (28 ready, 5 in challenge window).
- Remaining finalizations blocked on wallet gas (~0.025 ETH needed).
- Epoch-builder is live with byte-offset telemetry reading and explicit gas limits (500k finalize, 300k attest).
