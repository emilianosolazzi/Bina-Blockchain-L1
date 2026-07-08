# Bina Chain L1

> A next-generation Layer 1 blockchain with post-quantum security, Bitcoin-anchored randomness, deterministic consensus, and EVM oracle integration.

[![License: TGBT](https://img.shields.io/badge/License-TGBT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-blue.svg)](https://www.rust-lang.org/)
[![Solidity](https://img.shields.io/badge/solidity-0.8.30-blue.svg)](https://soliditylang.org/)

---

## Overview

Bina Chain is a novel L1 blockchain featuring:

- **Hybrid Post-Quantum Cryptography** — Ed25519 (classical) + Falcon-512 (post-quantum) signatures
- **Bitcoin-Anchored Entropy** — Live Bitcoin chain state feeds into block randomness
- **Stale Block Harvesting** — Extracts entropy from Bitcoin orphan blocks
- **Deterministic Consensus** — Objective work + election scores, not arrival order
- **Dynamic Difficulty** — 40ms target block time with automatic adjustment
- **Built-in Reward Ledger** — 2 billion BINA hard cap with halving schedule
- **EVM Oracle Integration** — Consume BINA randomness on any EVM chain
- **Proof of BINA Work** — Access control gated by valid BINA mining

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           Bina Chain L1 Node                                │
├─────────────────────────────────────────────────────────────────────────────┤
│  ┌─────────────┐   ┌──────────────┐   ┌──────────────────────┐             │
│  │   Crypto    │   │   Bitcoin    │   │  Stale Block Miner   │             │
│  │  Ed25519 +  │   │   Entropy    │   │  (orphan harvester)  │             │
│  │  Falcon-512 │   │   Fetcher    │   │                      │             │
│  └─────────────┘   └──────────────┘   └──────────────────────┘             │
│                                                                             │
│  ┌─────────────┐   ┌──────────────┐   ┌──────────────────────┐             │
│  │    PoW      │   │  Difficulty  │   │  Randomness Output   │             │
│  │   Mining    │◄──┤  Adjuster    │   │  + Nullifier System  │             │
│  └─────────────┘   └──────────────┘   └──────────────────────┘             │
│                                                                             │
│  ┌─────────────┐   ┌──────────────┐   ┌──────────────────────┐             │
│  │   Reward    │   │ Transaction  │   │  P2P Gossip +        │             │
│  │   Ledger    │   │   Mempool    │   │  Block Store (SQLite) │             │
│  └─────────────┘   └──────────────┘   └──────────────────────┘             │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                           EVM Oracle Layer                                  │
├─────────────────────────────────────────────────────────────────────────────┤
│  ┌─────────────┐   ┌──────────────┐   ┌──────────────────────┐             │
│  │ BinaOracle  │   │  Proof of    │   │  Utility Helpers     │             │
│  │   (Base)    │   │  BINA Work   │   │  (shuffle, batch)    │             │
│  └─────────────┘   └──────────────┘   └──────────────────────┘             │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## Key Features

### 1. Post-Quantum Security

Bina Chain uses mandatory hybrid signatures:

| Algorithm   | Public Key | Secret Key | Signature |
|-------------|-----------|-----------|-----------|
| Ed25519     | 32 B      | 32 B      | 64 B      |
| Falcon-512  | 897 B     | 1,281 B   | ≤666 B    |
| **Hybrid**  | **929 B** | **2,210 B** | **≤732 B** |

Both signatures must verify for any transaction or block claim to be accepted. Compromise of one algorithm still leaves the wallet protected by the other.

### 2. Bitcoin-Anchored Randomness

Each block produces deterministic randomness derived from:

- The block hash (unpredictable until mined)
- Live Bitcoin chain state (fetched from `mempool.space` + `blockstream.info`)
- The winning miner's nonce

**Properties:**

| Property | Description |
|----------|-------------|
| Unpredictable | No one knows the valid nonce before the block is found |
| Unbiasable | Miner must discard entire block and re-mine to change output |
| Unique | Height is monotonically increasing |
| Non-replayable | Nullifier system prevents double-spend |

**Bitcoin Entropy Seed derivation:**

```
bitcoin_seed_hash = blake3("BINA-BTC-v1" || tip_hash || utxo_entropy || stale_xor_pool)
```

- `tip_hash` — current Bitcoin canonical chain tip
- `utxo_entropy` — coinbase script of the tip block
- `stale_xor_pool` — XOR of provider tips when they diverge

### 3. Stale Block Harvesting

Bitcoin's orphan blocks become entropy sources:

- Parses 80-byte Bitcoin block headers
- Extracts entropy from stale (non-canonical) blocks
- Produces `StaleWorkProof` submissions
- Tracks reorg depth and frequency
- Quality-scored entropy reports (0–100)

### 4. Deterministic Consensus

Block claims are ranked by:

1. **Work** — Leading zero bits in the block hash
2. **Election Score** — Deterministic `BLAKE3` hash of `(height || prev_hash || block_hash || miner_address)`

This makes the winner independent of network arrival order.

### 5. Dynamic Difficulty

| Parameter | Value |
|-----------|-------|
| Target block time | 40ms |
| Epoch size | 20 blocks |
| Difficulty range | 25–45 leading zero bits |
| Max adjustment | ±3 bits per epoch |

---

## Technical Specifications

| Component | Details |
|-----------|---------|
| Consensus | BLAKE3 PoW |
| Block time | 40ms target |
| Max supply | 2,000,000,000 BINA |
| Initial reward | 50 BINA/block |
| Halving interval | 1,576,800,000 blocks (~2 years) |
| Difficulty range | 25–45 leading zero bits |
| Address size | 20 bytes |
| Genesis timestamp | 2025-06-30 00:00:00 UTC |
| PQ security | 128 bits |

**Wallet Address Derivation:**

```
Address = blake3("BINA-ADDR-v1" || ed25519_pk_bytes || falcon_pk_bytes)[..20]
```

---

## Getting Started

### Prerequisites

- Rust 1.70+
- SQLite
- *(Optional)* Bitcoin RPC/API access for entropy fetching

### Installation

```bash
# Clone the repository
git clone https://github.com/your-org/bina-chain
cd bina-chain

# Build the node
cargo build -p l1-node --release

# Build the wallet CLI
cargo build -p l1-wallet --release
```

### Generate a Wallet

```bash
# Generate a new hybrid keypair
./target/release/l1-wallet generate

# Show wallet details
./target/release/l1-wallet show

# Get just the address
./target/release/l1-wallet address

# Get public key hex only
./target/release/l1-wallet public-key
```

The wallet is stored at `~/.bina/wallet.json`:

```json
{
  "version": 1,
  "address": "<40-char hex>",
  "public_key": "<1858-char hex>",
  "secret_key": "<4420-char hex>"
}
```

### Run a Node

```bash
# Start with default settings
./target/release/l1-node

# Override HTTP port
BINA_HTTP_PORT=8282 ./target/release/l1-node

# Use a custom data directory
BINA_DATA_DIR=/path/to/data ./target/release/l1-node

# Connect to specific seed peers
BINA_SEEDS="144.126.157.197:8181,192.168.1.100:8181" ./target/release/l1-node
```

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `BINA_HTTP_PORT` | HTTP API port | `8181` |
| `BINA_DATA_DIR` | Data directory | `data/` |
| `BINA_P2P_LISTEN_ADDR` | P2P listen address | `127.0.0.1:8181` |
| `BINA_SEEDS` | Comma-separated seed peers | `144.126.157.197:8181` |

---

## Wallet Commands

```bash
# Sign a message
./target/release/l1-wallet sign "hello world"

# Verify a message
./target/release/l1-wallet verify "hello world" <sig_hex>

# Sign a BINA transaction
./target/release/l1-wallet sign-tx <to_address> <amount> <nonce> <fee>

# Verify a signed transaction
./target/release/l1-wallet verify-tx <signed_tx_json_or_file>

# Verify a public key + signature
./target/release/l1-wallet verify-public <message> <public_key_hex> <sig_hex>
```

---

## API Reference

### Read-Only Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/` | GET | Node status + economics |
| `/chain/status` | GET | Same as `/` |
| `/chain/supply` | GET | Supply, reward, difficulty info |
| `/chain/latest` | GET | Latest mined block |
| `/chain/blocks` | GET | Last 20 blocks |
| `/chain/headers` | GET | Block header sync (pagination) |
| `/block/{height}` | GET | Block by height |
| `/randomness/latest` | GET | Latest randomness output |
| `/randomness/{height}` | GET | Randomness at a specific height |
| `/wallet/{address}/balance` | GET | BINA balance + next nonce |
| `/p2p/peers` | GET | Known peer list |

### Mutating Endpoints

> All mutating endpoints are rate-limited to **200 requests per 10 seconds per IP** with a **64KB body size limit**.

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/chain/submit` | POST | Submit a signed block claim |
| `/tx/submit` | POST | Submit a signed BINA transfer |
| `/wallet/send` | POST | Sign and submit from local wallet |
| `/p2p/message` | POST | Receive gossip messages |
| `/p2p/hello` | POST | Peer introduction |
| `/p2p/connect` | POST | Connect to a peer |

### Example: Submit a Transaction

```bash
# Sign a transaction using the local wallet
./target/release/l1-wallet sign-tx \
  "3054ac8bc5c9b358e270e17183851201d0bc6b69" \
  100 \
  0 \
  1

# Submit to the node
curl -X POST http://127.0.0.1:8181/tx/submit \
  -H "Content-Type: application/json" \
  -d '{
    "from": "your_address_hex",
    "to": "recipient_address_hex",
    "amount": 100,
    "nonce": 0,
    "fee": 1,
    "public_key": "...",
    "signature": "..."
  }'
```

---

## EVM Oracle Integration

### Solidity Interface

```solidity
interface IBinaOracle {
    function getLatestSeed(bytes32 purpose)
        external view returns (bytes32 seed, uint64 height, uint64 btcHeight, bytes32 blockHash);

    function deriveWord(bytes32 purpose, bytes32 salt, address consumer)
        external view returns (bytes32);

    function randomUintFor(bytes32 purpose, bytes32 salt, address consumer, uint256 upperBound)
        external view returns (uint256);

    function isPQResistant() external pure returns (bool);
    function pqSecurityBits() external pure returns (uint8);
    function signingScheme() external pure returns (string);
}
```

### TypeScript Client

```typescript
import { createOracle, latestRandomness, deriveRandomWord, randomNumber } from 'bina-chain';

const config = {
  rpcUrl: 'https://your-evm-rpc.com',
  oracleAddress: '0x...',
  chainId: 1n,
};

// Get latest randomness seed
const oracle = createOracle(config);
const proof = await latestRandomness(oracle, 'BINA_GENERIC_UTILITY');

// Derive a random word for a specific consumer
const randomWord = await deriveRandomWord(
  oracle,
  'BINA_VALIDATOR_SELECTION',
  'my-salt',
  'consumer-id'
);

// Get a bounded random number (0–99)
const bounded = await randomNumber(
  oracle,
  'BINA_DEFI',
  'position-123',
  'defi-protocol',
  100
);

// Deterministic shuffle using BINA randomness
import { deterministicShuffle } from 'bina-chain';
const shuffled = deterministicShuffle(validators, proof.seed, 'round-1');
```

### Trust Model

| Property | Detail |
|----------|--------|
| Publisher set | Permissioned by owner via `setPublisher` |
| Quorum threshold | Default 1; configurable for multi-publisher agreement |
| Publisher bonding | Economic liveness guarantees |
| Future commitments | Slashable delivery guarantees |

---

## Proof of BINA Work

Gate any contract feature behind valid BINA mining:

```solidity
import { ProofOfBinaWork } from "bina-chain/contracts/ProofOfBinaWork.sol";

contract MyMinerGatedFeature is ProofOfBinaWork {
    constructor(address oracle) ProofOfBinaWork(oracle, 22, false) {}

    function doSomething(bytes32 blockHash, bytes20 binaMiner)
        external
        requiresBinaWork(blockHash, binaMiner)
    {
        // Only valid BINA miners can execute this
    }
}
```

**Included example contracts:**

| Contract | Description |
|----------|-------------|
| `BinaMinerToken.sol` | ERC20 that only mints for valid BINA miners |
| `BinaMinerLottery.sol` | Lottery where entry requires a BINA mining proof |
| `BinaMinerRegistry.sol` | On-chain reputation system for BINA miners |

---

## Project Structure

```
bina-chain/
├── l1-core/                    # Core blockchain logic
│   └── src/
│       ├── crypto.rs           # Ed25519 + Falcon-512 hybrid
│       ├── block.rs            # Block header + genesis
│       ├── claims.rs           # Signed claims + election
│       ├── pow.rs              # Mining engine
│       ├── difficulty.rs       # Dynamic difficulty adjuster
│       ├── rewards.rs          # Emission + reward ledger
│       ├── transaction.rs      # BINA transfers
│       ├── randomness.rs       # Randomness output + nullifiers
│       ├── bitcoin_entropy.rs  # Bitcoin state fetcher
│       ├── stale_block_miner.rs # Orphan block harvester
│       ├── secure_memory.rs    # Secret memory protection
│       └── cpu.rs              # CPU detection + telemetry
│
├── l1-node/                    # Full node implementation
│   └── src/
│       ├── main.rs             # HTTP API + mining loop
│       ├── gossip.rs           # P2P message relay
│       ├── peers.rs            # Peer management
│       ├── store.rs            # SQLite block store
│       └── envelope.rs         # P2P message types
│
├── l1-wallet/                  # Wallet CLI
│   └── src/main.rs
│
├── contracts/                  # EVM smart contracts
│   ├── BinaOracle.sol          # Main oracle contract
│   ├── ProofOfBinaWork.sol     # Access control base
│   ├── BinaMinerToken.sol      # Miner-gated ERC20
│   ├── BinaMinerLottery.sol    # Miner lottery
│   └── BinaMinerRegistry.sol   # Miner reputation
│
├── typescript/                 # TypeScript client library
│   └── src/
│       ├── index.ts            # Main exports
│       ├── oracle.ts           # Oracle client
│       └── utils.ts            # Helpers
│
├── Cargo.toml                  # Rust workspace manifest
└── README.md
```

---

## Security

### Secure Memory

The `secure_memory` module provides:

- Memory locking (`mlock` / `VirtualLock`) to prevent swapping to disk
- Guard pages around secrets
- Zeroization on drop
- Debugger detection
- Platform-specific hardening (`MADV_DONTDUMP`, `MADV_DONTFORK`)

### Signature Requirements

| Context | Requirement |
|---------|-------------|
| Block claims | Hybrid (Ed25519 + Falcon-512) |
| Transactions | Hybrid or Ed25519-only (provisional wallets) |

---

## Development

### Running Tests

```bash
# Core library tests
cargo test -p l1-core

# Node integration tests
cargo test -p l1-node -- --ignored

# Full workspace
cargo test --workspace
```

### Integration Tests

The integration test suite spawns real processes and mines actual blocks:

```bash
# Build first
cargo build -p l1-node

# Run integration tests
cargo test -p l1-node -- --ignored \
  integration_tests::fresh_node_resyncs_from_genesis_against_a_running_peer
```

These tests verify:

- Persistent block store survives restarts
- Fresh nodes sync from genesis via P2P
- Deterministic consensus across nodes

### Coding Standards

| Language | Tools |
|----------|-------|
| Rust | `cargo fmt`, `cargo clippy` |
| Solidity | Ethereum Style Guide |
| TypeScript | ESLint + Prettier |

---

## Roadmap

- [x] Core L1 implementation
- [x] Hybrid Ed25519 + Falcon-512 signatures
- [x] Bitcoin entropy anchoring
- [x] Stale block harvesting
- [x] Dynamic difficulty
- [x] Reward ledger + emission schedule
- [x] EVM oracle integration
- [x] Proof of BINA Work pattern
- [x] Mempool gossip propagation
- [ ] Light client support (sparse Merkle trie)
- [ ] Fully decentralized publisher set
- [ ] Cross-chain messaging

---

## Contributing

1. Fork the repository
2. Create your feature branch: `git checkout -b feature/amazing`
3. Commit your changes: `git commit -m 'Add amazing feature'`
4. Push to the branch: `git push origin feature/amazing`
5. Open a Pull Request

---

## License

**TGB (Temporal Gradient Beacon)** — Proprietary until further notice.
