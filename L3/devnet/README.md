# Temporal Gradient L3 Devnet Scaffold

This folder contains the first working scaffold for the L3 devnet described in:

- `L3/TG_L3_ORBIT_PLAN.md`
- `L3/L3_SEQUENCER_MODEL.md`
- `L3/L3_CONTRACT_MIGRATION.md`
- `L3/L3_DEVNET_PLAN.md`

## What was scaffolded

### Contracts

- `L3/contracts/TGL3Treasury.sol`
- `L3/contracts/TGL3EpochSettlement.sol`
- `L3/contracts/TGL3ProofMarket.sol`
- `L3/contracts/TGL3CertificateRegistry.sol`

### Devnet helpers

- `l2-mining/script/DeployL3SettlementScaffold.s.sol`
- `L3/devnet/.env.example`
- `L3/devnet/epoch-settlement-worker.js`

## Practical Orbit notes used for this scaffold

The Arbitrum Orbit docs at https://docs.arbitrum.foundation/new-arb-chains were useful here.

The most relevant points for this scaffold are:

1. Orbit is the right path for a sovereign application chain.
2. Dedicated blockspace is the main architectural win.
3. A custom gas token is possible, but not required at bring-up.
4. Starting simple is better than combining chain bring-up with token-economics complexity.

That is why this scaffold keeps:

- a single-operator bring-up model,
- ETH gas assumptions for chain operation,
- TGBT-style settlement at the application layer,
- and small, isolated first-wave contracts.

## What these contracts do

### `TGL3Treasury`

Reusable payment splitter for first-wave settlement apps.

### `TGL3EpochSettlement`

Minimal epoch commit/finalize contract for posting epoch roots to the L3.

### `TGL3ProofMarket`

Receipts and fee settlement for proof purchases tied to settled epochs.

### `TGL3CertificateRegistry`

Minimal certificate issuance registry with fee settlement and issuer allowlisting.

## What this scaffold does not do yet

This is not production-ready and intentionally leaves out:

- bridge logic,
- canonical token migration,
- decentralized sequencing,
- NFT-based certificate design,
- advanced proof verification,
- advanced governance,
- and full indexer/query services.

## Suggested next steps

1. Wire the worker to the current epoch-builder output shape.
2. Add a read API for epochs, proof receipts, and certificate records.
3. Add Foundry tests for commit/finalize, proof purchases, and certificate issuance.
4. Add a sample epoch JSON payload and smoke test flow.
5. Decide whether the next step belongs in `L3/` or should migrate into `l2-mining/contracts/` once stabilized.