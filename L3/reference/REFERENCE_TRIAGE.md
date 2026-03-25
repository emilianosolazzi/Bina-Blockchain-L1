# L3 Reference Triage

Date: March 24, 2026

## SDK / API References Kept

### sharding/ShardManager.js

Location:
- [L3/reference/sharding/ShardManager.js](sharding/ShardManager.js)

Why it was kept:
- useful as a legacy reference for request-routing, backpressure, failover, and shard health management,
- conceptually relevant to future L3 service topology, especially if the L3 grows a sequencer-adjacent API tier, relayer tier, or horizontally scaled randomness service,
- provides reusable ideas for overload thresholds, recovery windows, and shard selection policy.

Why it was copied instead of moved:
- it is still actively used by [services/RandomnessService.js](../../services/RandomnessService.js),
- tests and the current service layer still depend on the live file in [sharding/ShardManager.js](../../sharding/ShardManager.js).

Recommended L3 use:
- reference only,
- extract service-scaling ideas,
- do not treat it as first-wave on-chain L3 logic.

### sdk-bridge-protocol.js

Location:
- [L3/reference/sdk-bridge-protocol.js](sdk-bridge-protocol.js)

Why it was kept:
- useful as a legacy reference for client/service bridge patterns,
- contains ideas around fallback routing, entropy pooling, shard-aware behavior, and request orchestration.

Recommended L3 use:
- reference only,
- extract SDK and service concepts during later client redesign.

### TemporalGradient_SDK.py

Location:
- [L3/reference/TemporalGradient_SDK.py](TemporalGradient_SDK.py)

Why it was kept:
- useful as a legacy reference for a Python client surface,
- contains request/poll/reveal interaction patterns for beacon and mining flows,
- may help shape future L3 Python SDK ergonomics and API coverage.

Why it needs refactor:
- the ABI is explicitly placeholder / incomplete,
- it reflects older contract assumptions,
- it is not aligned to the current L3 Orbit-first architecture,
- it should not be treated as production SDK code.

Recommended L3 use:
- reference only,
- mine it for client API ideas,
- rebuild cleanly against the eventual L3 contract and API surface.

## Rust Reference Utilities Kept

### EntropyQualityLib.rs

Location:
- [L3/reference/rust/EntropyQualityLib.rs](rust/EntropyQualityLib.rs)

Why it was kept:
- useful as a legacy reference for entropy scoring heuristics and zk-proof preparation logic,
- conceptually aligned with the retained [L3/contracts/ZKEntropyVerifier.sol](../contracts/ZKEntropyVerifier.sol),
- may help shape a future off-chain verifier/coprocessor or SDK-side quality analysis pipeline.

Why it needs refactor:
- it appears standalone rather than actively integrated into the current miner/runtime path,
- the repo already has a separate active scorer implementation in [l2-mining/rust/temporal_gradient_core/src/entropy_quality_scorer.rs](../../l2-mining/rust/temporal_gradient_core/src/entropy_quality_scorer.rs),
- it mixes simulation, test harness, JSON export, and scoring logic in one file,
- it is not part of the current Orbit-first L3 v0 scope.

Recommended L3 use:
- reference only,
- extract scoring and proof-prep ideas,
- do not treat as deployable or production runtime code.

### nist_pqc.rs

Location:
- [L3/reference/rust/nist_pqc.rs](rust/nist_pqc.rs)

Why it was kept:
- useful as a legacy reference for post-quantum hardening concepts around Kyber/Dilithium style key exchange and signatures,
- relevant to a future L3 security roadmap if the protocol later adds PQC-aware operator identity, relay authentication, or off-chain proof transport,
- provides design ideas for hybrid classical+PQC hashing and signed entropy flows.

Why it needs refactor:
- it is an archived standalone module, not part of the active runtime,
- the current miner/runtime already replaced it with a newer active implementation in [l2-mining/rust/temporal_gradient_core/src/pqc.rs](../../l2-mining/rust/temporal_gradient_core/src/pqc.rs),
- PQC resistance for an Orbit L3 is mostly an off-chain / operational / bridge / signer concern at this stage, not a direct first-wave settlement-contract concern,
- it should be mined for architecture ideas rather than reused directly.

Recommended L3 use:
- reference only,
- use it to inform a future PQC roadmap,
- not direct production code for L3 v0.

### pqc.rs

Location:
- [L3/reference/rust/pqc.rs](rust/pqc.rs)

Why it was kept:
- useful as a compact legacy reference for a simple classical-vs-enhanced PQC hashing mode,
- may help shape a lightweight L3-side hashing compatibility toggle or migration path,
- easier to mine for ideas than the larger archived PQC module.

Why it needs refactor:
- it is a deprecated standalone helper, not part of the active miner/runtime path,
- it is intentionally narrow and does not provide a full PQC system by itself,
- any L3 use would need to be redesigned around the actual signer, bridge, and settlement threat model.

Recommended L3 use:
- reference only,
- extract compatibility-mode ideas,
- not production runtime code.
