# L2 Mining Architecture

## Canonical flow

1. Miner fetches challenge from `TemporalGradientL2Beacon.getMiningChallenge()`.
2. Miner searches locally for a valid solution candidate.
3. Miner submits `submitMiningCommitment()` with EIP-712 signature.
4. Miner waits for `minCommitmentAge`.
5. Miner submits `revealMiningCommitment()`.
6. Beacon validates:
   - active commitment
   - commitment hash match
   - previous output exists in history
   - rate limits
   - difficulty target
   - output uniqueness
7. Beacon updates:
   - output history
   - bloom filter
   - epoch reward state
   - miner reward balances
8. Events are emitted for off-chain monitoring.

## Contract responsibilities

### TemporalGradientL2Beacon.sol
- L2 mining entrypoint
- rate limiting
- pool access
- reward minting
- output lifecycle

### MiningLib.sol
- reveal verification
- quantum-resistant hashing
- reward calculation
- commitment structure definitions

### GovernanceLib.sol
- pool creation/update
- bonus and commit/reveal parameter tuning

### TokenomicsLib.sol
- epoch transition
- halving/reward schedule

### RateTypes.sol
- user/global throttling primitives

## Rust responsibilities

### temporal_gradient_core
- shared miner config
- temporal seed encode/decode/generate
- commitment + reveal helper logic
- PQC-enhanced hashing hooks
- runtime worker management
- live challenge polling
- live commit/reveal submission path
- receipt reward extraction
- telemetry snapshots and graceful shutdown API

### rust/package
- installer/bootstrap CLI
- executable miner binary
- per-user config and log layout
- launch + doctor workflows

### Mining.rs
- challenge polling
- EIP-712 commitment creation
- local nonce search
- commit/reveal submission
- receipt parsing
- legacy reference implementation kept for comparison while the packaged runtime mirrors the same live flow

### memory.rs
- secure key handling

### cpu.rs
- optional hardware/thermal helpers

### nist_pqc.rs
- optional PQ support layer

## JS responsibilities

### RateMonitor.js / RateAnalyzer.ts / RateVisualizer.js
- miner analytics
- operator visibility
- difficulty and efficiency monitoring

## Current blockers

1. Difficulty weighting is still placeholder logic in Solidity.
2. The modular contracts compile, but module-isolated tests still need expansion.
3. The package bootstrap creates config/data folders, but a portable install step is still needed to place the standalone miner binary into the per-user bin directory.

## Immediate next steps

1. Freeze L2 mining protocol inputs.
2. Finalize difficulty weighting in Solidity.
3. Finish portable packaging so the installer can place the miner executable in the expected per-user bin path.
4. Add a full integrated modular-system test that wires core + mining + randomness + tokenomics together in one flow.
