# L3 Contract Triage

Date: March 24, 2026

## Moved into L3

### BeaconExporter.sol

Moved to:
- [L3/contracts/BeaconExporter.sol](contracts/BeaconExporter.sol)

Why it was kept:
- conceptually relevant for future cross-chain export / bridge-facing L3 settlement design,
- useful as a legacy reference for quotas, export accounting, burn hooks, and subscription-style export surfaces.

Why it needs refactor:
- upgradeable pattern is heavier than what the current L3 plan likely needs,
- mixes too many concerns in one contract,
- includes unresolved / legacy dependencies and event assumptions,
- not aligned yet with the current Orbit-first architecture.

Recommended L3 use:
- reference only,
- extract ideas, not direct deployment.

### ZKEntropyVerifier.sol

Moved to:
- [L3/contracts/ZKEntropyVerifier.sol](contracts/ZKEntropyVerifier.sol)

Why it was kept:
- conceptually relevant if the L3 later supports privacy-preserving entropy submissions,
- useful as a legacy reference for zk-proof-gated entropy scoring, staking/slashing, and manual fallback verification,
- may inform a future specialized verifier lane or coprocessor-backed proof pipeline.

Why it needs heavy refactor:
- it is not aligned with the current Orbit-first v0 scope,
- it mixes verifier management, staking, manual approvals, and scoring in one contract,
- it carries legacy upgradeable/admin complexity,
- parts of the implementation are not production-ready and should be treated as design input only.

Recommended L3 use:
- reference only,
- mine it for privacy / proof-flow ideas later,
- not part of first-wave settlement rollout.

### VDFVerifier.sol

Moved to:
- [L3/contracts/VDFVerifier.sol](contracts/VDFVerifier.sol)

Why it was kept:
- conceptually relevant for delayed-finality / anti-manipulation entropy mechanisms,
- useful as a legacy reference for challenge-response timing, entropy delay windows, and cleanup patterns,
- may inform a future time-delay or reveal-hardening module on L3.

Why it needs refactor:
- the current implementation is a simplified legacy design, not a production-grade VDF system,
- it relies on timestamp-driven assumptions that should be revisited for Orbit,
- it should be redesigned around the actual L3 sequencing/finality model rather than reused directly.

Recommended L3 use:
- reference only,
- extract timing and reveal-hardening concepts,
- not direct deployment code.

## Not moved into L3

### EntropyChainConnector.sol

Why it was not moved:
- it is built around custom native precompiles,
- it targets an "Entropy chain" model rather than Arbitrum Orbit,
- those assumptions do not fit the current L3 plan.

Conclusion:
- keep it out of the L3 working set for now.
- if needed later, only reuse high-level interface ideas, not the contract itself.
