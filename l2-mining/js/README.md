# Temporal Gradient Beacon API

Production randomness API serving verifiable on-chain randomness to dApp customers.

## Architecture

```
Client dApp → beacon-api-server → Redis (cache + rate limits)
                                → PostgreSQL (usage logs)
                                → Sepolia contracts (randomness source)
```

Supports **two mining stacks**:
- **Classic commit-reveal** (MiningModule + TemporalGradientCore) — currently live
- **Batch epoch-based** (BatchMiningModule) — optional

## Quick Start

```bash
# 1. Install dependencies
cd l2-mining/js
npm install

# 2. Set up environment
cp .env.example .env
# Edit .env with your PostgreSQL, Redis, and JWT secret

# 3. Generate JWT secret
node scripts/generate-jwt-secret.js
# Copy output to .env JWT_SECRET

# 4. Run database migrations
node db/migrate.js

# 5. Start the server
npm start
```

## Prerequisites

- **Node.js** ≥ 18
- **PostgreSQL** (any version with JSONB support)
- **Redis** (for rate limiting and caching)

## Endpoints

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/healthz` | None | Health check (DB, Redis, chain) |
| GET | `/api/v1/status` | None | Full beacon status with economics |
| GET | `/api/v1/latest` | None | Latest randomness output |
| GET | `/api/v1/output-history` | None | Full 32-slot output ring buffer |
| GET | `/api/v1/economics` | None | Mining rewards, epoch, halving info |
| GET | `/api/v1/pools` | None | Active mining pools |
| GET | `/api/v1/supply` | None | TGBT token total supply |
| POST | `/api/v1/randomness` | None | Generate random words from beacon |
| POST | `/api/v1/physical-randomness` | None | OS CSPRNG random bytes |
| POST | `/api/v1/verify` | None | Verify Merkle proof (batch mining) |
| POST | `/api/v1/slot-spin` | Required | Beacon-derived slot machine spin |
| POST | `/api/v1/request-onchain-randomness` | Required | Submit on-chain randomness request |
| GET | `/api/v1/get-onchain-result/:id` | None | Poll on-chain result |
| POST | `/api/v1/auth/token` | API Key | Issue JWT for session auth |
| GET | `/api-docs` | None | Swagger UI |

## Authentication

Three strategies (fail-closed):

1. **API Key** — `x-api-key` header (server-to-server)
2. **JWT Bearer** — `Authorization: Bearer <token>` (session-based)
3. **EIP-191 Signature** — `x-signature` + `x-signer-address` + `x-timestamp` (Web3 native)

## Contract Addresses (Sepolia)

| Contract | Address |
|----------|---------|
| Core | `0xa1fB393D33819C4ef85f3457FCC339BF56f8AF1F` |
| MiningModule | `0x99B9f244104cbE8653274a3936580D29B5E74871` |
| TokenomicsModule | `0x305393D146e958cbDFda5830506e468984259F28` |
| TGBT Token | `0x496598fDeab78fb2986e89d396249779595418E9` |
| BatchMiningModule | `0xd52467e0C442c0817665fdB11f86FC47dC56ef3E` |

## Rate Limits

- **Global**: 500,000 req/min
- **Per-user**: 100 req/min (keyed by API key or IP)
- **Strict** (on-chain ops): 20 req/min
