# Modular Beacon Refactor

## Recommendation

Yes.

But the better pattern is not just "Beacon + modules".

The better pattern is:

- **Core** owns only shared state and shared authority
- **Feature modules** own feature state and feature logic
- **Cross-module interactions** happen only through explicit interfaces
- **Users call modules directly**
- **No `delegatecall` router unless strictly needed**

## Why the current file should be split

The current beacon mixes:

- mining lifecycle
- randomness request flow
- tokenomics and reward issuance
- governance setters
- rate limiting
- L1/L2 entropy bridging
- user penalty logic

That makes [l2-mining/contracts/TemporalGradientL2Beacon.sol](l2-mining/contracts/TemporalGradientL2Beacon.sol) hard to:

- audit
- upgrade safely
- test in isolation
- reason about for storage changes

## Better target layout

```text
TemporalGradientCore
  ├── shared output history
  ├── shared module registry
  ├── shared pause/roles
  ├── shared global events
  └── shared write gates

MiningModule
  ├── commitments
  ├── pools
  ├── uniqueness tracking
  └── mining reveal path

RandomnessModule
  ├── randomness requests
  ├── contribution tracking
  ├── fulfillment
  └── emergency fulfill path

TokenomicsModule
  ├── epochs
  ├── halving schedule
  ├── reward minting
  ├── reputation/accounting hooks
  └── mining economics views

RateLimitModule
  ├── user buckets
  ├── global window
  ├── thresholds
  └── throttle decisions

GovernanceModule
  ├── module wiring
  ├── pool admin operations
  ├── parameter updates
  └── emergency controls
```

## Key improvement over the proposed sketch

### 1. Core should own only **shared** mutable state

Good candidates for core ownership:

- output history
- current output index
- pause state
- module registry
- role checks
- maybe L1/L2 bridge anchor state

Do **not** put feature state in core if only one module uses it.

### 2. Modules should not try to read storage references from core

This part of the sketch is not workable as written:

```solidity
function getOutputHistory() internal view returns (bytes32[32] storage)
```

A module cannot obtain another contract's storage pointer.

Use:

- explicit memory-returning getters
- explicit write entrypoints on core
- explicit cross-module interfaces

### 3. Do not make the beacon emit every feature event

Let modules emit module events directly.

Keep only a small set of cross-system events in core if needed, such as:

- `ModuleUpdated`
- `CoreOutputRecorded`
- `SystemPaused`

This reduces coupling.

### 4. Rate limiting should be a first-class shared service

Mining and randomness both need throttling.

So rate limiting should be an independent module that exposes a narrow API like:

- `consumeOrRevert(user, cost, operation)`

### 5. Reward issuance should not stay inside mining

Mining should determine that a valid output exists.

Tokenomics should decide:

- current epoch reward
- bonus multiplier
- remaining allocation
- mint/slash/burn side effects

That keeps economics isolated from proof validation.

## Suggested contract foundation

Starter interfaces and base module were added:

- [l2-mining/contracts/interfaces/ITemporalGradientCore.sol](l2-mining/contracts/interfaces/ITemporalGradientCore.sol)
- [l2-mining/contracts/interfaces/IRateLimitModule.sol](l2-mining/contracts/interfaces/IRateLimitModule.sol)
- [l2-mining/contracts/interfaces/ITokenomicsModule.sol](l2-mining/contracts/interfaces/ITokenomicsModule.sol)
- [l2-mining/contracts/modules/ModuleBase.sol](l2-mining/contracts/modules/ModuleBase.sol)

First-pass extraction scaffolds now also exist:

- [l2-mining/contracts/TemporalGradientCore.sol](l2-mining/contracts/TemporalGradientCore.sol)
- [l2-mining/contracts/modules/RateLimitModule.sol](l2-mining/contracts/modules/RateLimitModule.sol)
- [l2-mining/contracts/modules/MiningModule.sol](l2-mining/contracts/modules/MiningModule.sol)
- [l2-mining/contracts/modules/RandomnessModule.sol](l2-mining/contracts/modules/RandomnessModule.sol)
- [l2-mining/contracts/modules/TokenomicsModule.sol](l2-mining/contracts/modules/TokenomicsModule.sol)

These provide the beginning of:

- shared core reads
- shared module auth
- system pause enforcement
- explicit cross-module boundaries

The current module set now covers:

- mining validation in [l2-mining/contracts/modules/MiningModule.sol](l2-mining/contracts/modules/MiningModule.sol)
- randomness flow in [l2-mining/contracts/modules/RandomnessModule.sol](l2-mining/contracts/modules/RandomnessModule.sol)
- rate throttling in [l2-mining/contracts/modules/RateLimitModule.sol](l2-mining/contracts/modules/RateLimitModule.sol)
- reward issuance and penalty hooks in [l2-mining/contracts/modules/TokenomicsModule.sol](l2-mining/contracts/modules/TokenomicsModule.sol)

## Recommended user call flow

### Mining

```text
user
  -> MiningModule.submitMiningCommitment()
  -> MiningModule.revealMiningCommitment()
      -> RateLimitModule.consumeOrRevert()
      -> TokenomicsModule.onBlockMined()
      -> Core.recordMinedOutput()
```

### Randomness

```text
user
  -> RandomnessModule.requestRandomness()
  -> RandomnessModule.contributeEntropy()
      -> RateLimitModule.consumeOrRevert()
      -> Core.getOutputHistory()
```

## Migration path

### Phase 1

Extract without changing behavior:

1. `RateLimitModule`
2. `RandomnessModule`
3. `TokenomicsModule`
4. `MiningModule`

### Phase 2

Shrink the current beacon into `TemporalGradientCore`:

- shared state only
- module registry only
- explicit write gates only

### Phase 3

Replace direct internal coupling with interfaces:

- mining -> tokenomics
- mining/randomness -> rate limit
- modules -> core

### Phase 4

Move tests to module isolation:

- mining tests only touch mining + mocked core/tokenomics/rate limit
- randomness tests only touch randomness + mocked core/rate limit

## Practical split for the current file

### Move out first

From [l2-mining/contracts/TemporalGradientL2Beacon.sol](l2-mining/contracts/TemporalGradientL2Beacon.sol):

- `submitMiningCommitment()`
- `revealMiningCommitment()`
- `_processMiningReveal()`
- `batchSubmitCommitments()`
- pool views and pool updates

into `MiningModule`.

Then move:

- `requestRandomness()`
- `contributeEntropy()`
- `getRandomResult()`
- `emergencyRandomnessFulfill()`
- randomness getters

into `RandomnessModule`.

Then move:

- `_checkRateLimit()`
- `updateRateLimitThresholds()`
- rate stats views

into `RateLimitModule`.

Then move:

- epoch and reward admin
- slashing/burning hooks
- economics getters

into `TokenomicsModule`.

## Recommendation in one line

Yes: build **Core + Direct-Call Modules + Explicit Interfaces**, not a giant router and not a diamond.

## Immediate implementation status

The monolith has not been removed yet, but the split foundation now exists.

The next practical extractions are:

1. `TokenomicsModule.sol`
2. replacing monolith writes with `TemporalGradientCore.recordMinedOutput()`
3. switching tests to target the new modules directly
4. deprecating randomness paths in the monolith after module cutover
