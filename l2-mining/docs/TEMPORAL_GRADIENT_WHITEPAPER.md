# Temporal Gradient Whitepaper

## Continuous Mining as a Security Primitive

Version 0.3  
March 23, 2026

---

## Abstract

No institution today can continuously prove that its critical devices are still alive, still authentic, and still connected — without trusting a central observer.

Certificates get stolen. Machines get cloned. Agents get disabled. Logs get tampered with. Monitoring servers fail. Every static credential is a snapshot of trust that was true once, at some earlier point in time. None of them answer the question that actually matters: **is this device alive and behaving consistently right now?**

This is not a theoretical gap. It is the structural weakness beneath every major breach, every supply-chain compromise, and every undetected infrastructure outage. The missing capability is not more logs — it is stronger proof.

Temporal Gradient proposes a different trust primitive: continuous computational work.

In the Temporal Gradient model, mining is not only a way to generate randomness or secure token issuance. Mining acts as a continuous cryptographic heartbeat. Every legitimate miner continuously performs CPU work, emits signed outputs, contributes to Merkle-rooted epochs, and anchors proofs to chain. Over time, that stream becomes a tamper-evident record of machine liveness, continuity, and identity.

The system also harvests entropy from Bitcoin itself — capturing the proof-of-work embedded in orphaned and stale blocks that Bitcoin discards. This turns wasted computation from the most secure network on Earth into a scarce, high-quality entropy source anchored on-chain.

This makes mining useful beyond incentives. It turns miners into sensors, witnesses, and trust anchors. A gap in work becomes a signal. A mismatch in identity becomes a signal. Large-scale silence across regions becomes a signal. The same infrastructure that secures randomness can become the basis for device attestation, passive intrusion detection, continuity assurance, and eventually a decentralized verified egress network.

The central claim of this paper is simple:

**Bitcoin made computation prove economic security. Temporal Gradient makes computation prove operational reality.**

---

## 1. The Problem

Modern institutions depend on centralized trust systems:

- certificate authorities
- VPN concentrators
- centralized identity providers
- endpoint agents
- SIEM pipelines
- centralized monitoring servers
- cloud logging stacks

These systems are useful, but they share a structural weakness:

**they are static, central, and easy to blind.**

A certificate can be stolen.  
A machine can be cloned.  
An agent can be disabled.  
A log collector can be tampered with.  
A monitoring server can fail.  
A central VPN can become the single point of attack.

Institutions therefore suffer from a missing capability:

> They cannot continuously prove that critical devices are still alive, still authentic, and still connected without trusting a central observer.

Temporal Gradient addresses that gap.

---

## 2. Core Thesis

Temporal Gradient starts from a mining network, but extends the meaning of mining.

In this system, mining is simultaneously:

1. a source of randomness,
2. an incentive mechanism,
3. a cryptographic heartbeat,
4. a continuity proof,
5. a basis for decentralized security telemetry.

A valid mining solution is not just an economic event. It is also evidence that:

- a machine was active,
- the machine possessed the expected key material,
- CPU work was performed in real time,
- the node was connected to the broader network,
- the node preserved continuity over time.

This shifts trust from possession of a static secret to **continuous fresh work**.

That is the essential innovation.

---

## 3. Mining and Security Are the Same Loop

Traditional security tools and mining systems are usually separate.

Temporal Gradient unifies them.

### 3.1 Mining loop

A miner continuously:

- searches for valid solutions,
- derives output hashes,
- emits telemetry snapshots,
- records solution metadata,
- batches leaves into Merkle epochs,
- optionally signs outputs,
- anchors epochs and attestations on-chain.

### 3.2 Security loop

That same process creates a security signal:

- if mining continues, the node is alive,
- if telemetry remains fresh, the node is connected,
- if signed outputs remain consistent, the node identity persists,
- if epochs remain verifiable, historical continuity is preserved,
- if the node goes silent, that absence is measurable.

The mining loop and the security loop are therefore the same loop observed from two different angles.

---

## 4. System Architecture

Temporal Gradient consists of several layers.

### 4.1 Miner runtime

The miner runtime continuously performs CPU work and emits telemetry snapshots, including:

- timestamp,
- state,
- uptime,
- hashrate,
- accepted submissions,
- rejected submissions,
- last solution nonce,
- last solution hash,
- optional commit/output hashes,
- temperature,
- mining phase.

This telemetry is the raw heartbeat stream.

### 4.2 Randomness API

Accepted solutions are accumulated into epochs.

Each epoch contains:

- epoch ID,
- Merkle root,
- leaf count,
- leaf proofs,
- timestamps,
- nonces,
- optional signed outputs,
- storage verification metadata,
- on-chain attestation metadata.

This makes the mining record queryable and auditable.

### 4.3 On-chain anchoring

Epoch roots are anchored on-chain. That gives the system:

- immutable root references,
- on-chain proof verification,
- epoch finalization,
- reward settlement,
- on-chain storage attestation recording.

### 4.4 Heartbeat sidecar

A parallel heartbeat sidecar monitors the miner telemetry stream and derives:

- heartbeat continuity,
- stale telemetry alerts,
- solution-gap alerts,
- hashrate collapse alerts,
- rejection-burst alerts,
- temperature alerts,
- intrusion score and threat posture.

### 4.5 Dashboard

The dashboard exposes:

- live miner metrics,
- latest randomness and proofs,
- epoch explorer,
- storage verification state,
- on-chain attestation state,
- personal threat dashboard,
- verified egress readiness profile.

---

## 5. Cryptographic Heartbeats

The heartbeat concept is central.

A heartbeat in Temporal Gradient is not a simple “ping.” It is a cryptographic, computational, and economic event.

A valid heartbeat can include:

- a mining solution or accepted submission,
- an output hash,
- an optional miner signature over that output,
- epoch inclusion through a Merkle proof,
- eventual epoch finalization on-chain,
- optional storage attestation for epoch persistence.

This gives stronger evidence than a conventional liveness check.

A server ping proves that something answered.
A Temporal Gradient heartbeat proves that computational work occurred under a persistent identity and was recorded into an auditable chain of evidence.

---

## 6. Why Mining Matters

Mining is not incidental in this design. It is the source of security value.

### 6.1 Mining forces freshness

An attacker can replay a certificate.  
An attacker cannot cheaply replay continuous fresh CPU work over time.

### 6.2 Mining forces continuity

Mining is not a one-time proof. It is ongoing. That means trust becomes temporal, not static.

### 6.3 Mining is measurable

The system can observe:

- solution frequency,
- output timing,
- hashrate stability,
- nonce evolution,
- continuity gaps,
- multi-region silence.

These measurements become a passive security dataset.

### 6.4 Mining provides incentives

Unlike ordinary monitoring systems, miners are economically rewarded to remain online and behave consistently. The security layer is therefore incentive-aligned.

---

## 7. Bitcoin Entropy Harvesting

Temporal Gradient does not only generate entropy internally. It harvests entropy from Bitcoin — the most computationally secured network in existence.

### 7.1 The insight: wasted proof-of-work

Bitcoin occasionally produces stale blocks — valid blocks with real proof-of-work that lose the chain-tip race to a competing block at the same height. From Bitcoin's perspective, this work is "wasted." From Temporal Gradient's perspective, it is an exceptionally high-quality entropy source.

Every stale block contains:

- an unpredictable block hash (valid PoW, but not on the canonical chain),
- a nonce that diverges from the canonical block,
- a Merkle root over a different transaction set,
- a timestamp that differs from the winner,
- the outcome of a propagation race that is itself random.

Stale blocks are rare (roughly 1–2 per day on Bitcoin mainnet), making them scarce. They cannot be manufactured — producing one requires real SHA-256 work at Bitcoin-level difficulty. And the divergence between the stale block and the canonical winner at the same height is fundamentally unpredictable.

### 7.2 Stale block mining

The miner includes a stale-block harvesting sidecar that:

1. **Monitors Bitcoin chain tips** for forks and reorganizations by polling mempool and block explorer APIs.
2. **Detects stale blocks** when a competing chain tip loses — the orphaned block becomes harvestable.
3. **Extracts entropy from every field** of the 80-byte stale block header: version, previous block hash, Merkle root, timestamp, difficulty bits, and nonce. These are mixed through a domain-tagged SHA-256 to produce a 32-byte entropy digest.
4. **Builds a StaleWorkProof** containing the raw header, block hash, canonical hash, reorg depth, leading zero count, entropy quality score, and submitter address.
5. **Submits the proof on-chain** to a StaleBlockOracle contract, which verifies the header, confirms it differs from the canonical chain, and triggers a TGBT reward through the TokenomicsModule.

### 7.3 Quality scoring

Not all stale blocks carry equal entropy value. The system assigns a quality score (0–100) based on:

- **Leading zero bits**: more PoW = higher quality. A stale block must meet a minimum of 32 leading zero bits (standard Bitcoin difficulty) to qualify.
- **Reorg depth**: deeper reorganizations (where multiple blocks were orphaned) produce richer fork-divergence entropy.
- **Timestamp divergence**: greater difference between the stale and canonical timestamps increases unpredictability.
- **Header field divergence**: differing Merkle roots, nonces, and version fields each contribute additional entropy dimensions.

### 7.4 Fork divergence entropy

When a fork resolves, the system also extracts **fork divergence entropy** — the XOR of the stale block hash with the canonical block hash at the same height. This captures the pure randomness of the propagation race outcome: which miner's block reached enough of the network first.

This three-layer entropy extraction (primary header entropy, secondary field-mix entropy, fork divergence entropy) makes stale blocks one of the highest-quality external entropy sources available to any on-chain system.

### 7.5 Why this matters

Most randomness systems rely entirely on internal computation or on-chain state. Temporal Gradient is, to our knowledge, the first system to systematically harvest the entropy embedded in Bitcoin's orphaned proof-of-work and anchor it to a separate chain.

This means every miner is not only generating local entropy through CPU work — it is also acting as a bridge, importing Bitcoin-grade computational entropy into the Temporal Gradient beacon. The two entropy streams (internal mining and external Bitcoin harvesting) are independent, which makes the combined output strictly stronger than either source alone.

---

## 8. TGBT Tokenomics

TGBT (Temporal Gradient Beacon Token) is the native ERC-20 token that incentivizes mining, rewards entropy contributions, and aligns economic behavior with network security.

### 8.1 Supply structure

| Parameter | Value |
|---|---|
| **Hard cap** | 2,000,000,000 TGBT |
| **Mining allocation** | 700,000,000 TGBT (35%) |
| **Stale block allocation** | 25,000,000 TGBT (1.25%) |
| **Token standard** | ERC-20 (immutable, no proxy, no pause) |

The token contract enforces the hard cap at the protocol level. No admin mint exists. Once the module set is finalized, governance permissions can be permanently locked — irreversible, Bitcoin-style ossification.

### 8.2 Mining pools

Mining rewards are distributed through discrete **mining pools**, each with:

- a **target difficulty** that determines how hard a valid solution must be,
- an **emission bucket** — the total TGBT allocated to that pool,
- a **total mined** counter that tracks how much has been paid out,
- an **active** flag.

Pools are created by governance and are immutable after creation (no parameter changes — Bitcoin-style). When a pool's emission bucket is exhausted, miners must switch to another pool or await new pool creation.

At genesis, Pool 0 is created with the initial difficulty and emission. Additional pools (e.g., Pool 3, the current canonical mining pool) can target different difficulty levels and emission sizes, allowing the system to segment miners by capability.

### 8.3 Emission schedule and halving

Block rewards follow a **deterministic, block-number-anchored emission schedule**:

- Rewards are set at initialization (e.g., a base reward per accepted solution).
- After a fixed **halving interval** (measured in L2 blocks), the reward reduces by 35% (multiplied by 65/100).
- Halvings repeat until the reward reaches a protocol minimum floor (1e-12 TGBT), after which it remains constant.
- The halving interval supports up to ~5 years on Arbitrum (~630M blocks at 0.25s block time).

The emission is fully deterministic from the initialization block — no governance intervention, no manual adjustment.

### 8.4 Bonus rewards

Solutions that significantly exceed the pool's target difficulty receive a bonus:

- If a solution's effective difficulty exceeds `bonusThreshold × targetDifficulty`, the reward is multiplied by `bonusMultiplier / 100` (default: 125%, i.e., a 25% bonus).
- This rewards miners who find exceptionally strong solutions, incentivizing honest high-throughput work.

### 8.5 Stale block rewards

The stale-block allocation (25M TGBT) is a separate budget managed by the TokenomicsModule. When a miner submits a valid StaleWorkProof, the StaleBlockOracle requests a reward from the TokenomicsModule, which mints TGBT from the stale allocation — capped by both the stale budget and the global supply cap.

### 8.6 Commit-reveal mining

Mining uses a two-phase **commit-reveal** scheme to prevent front-running:

1. **Commit**: the miner submits a hash commitment (binding the solution, pool, nonce, and deadline) with an EIP-712 signature. The commitment is locked on-chain.
2. **Maturation**: the commitment must age at least `minCommitmentAge` blocks (currently 2) before it can be revealed. This prevents same-block front-running.
3. **Reveal**: the miner reveals the full solution (previous output, temporal seed, nonce, signature, secret value). The contract verifies the commitment hash matches, checks difficulty, and if valid, triggers the TokenomicsModule to mint the reward.

Commitments expire after `maxCommitmentAge` blocks (currently 500). A `minBlockInterval` cooldown (currently 1 block) prevents rapid sequential submissions.

### 8.7 Economic alignment

The tokenomics structure aligns miner incentives with network goals:

- **Uptime is rewarded**: miners must be continuously running to find solutions and submit before commitments expire.
- **Difficulty is self-selecting**: pools with harder targets attract stronger miners; pools with easier targets allow broader participation.
- **Stale harvesting is separately funded**: Bitcoin entropy collection does not compete with regular mining rewards.
- **No hold requirement**: genesis miners can mine with zero TGBT balance, removing the bootstrap problem.

---

## 9. Security Properties Provided Today

The current architecture already provides real security value.

### 7.1 Device liveness proof

Continuous mining telemetry demonstrates that the node is active.

### 7.2 Continuity monitoring

The system detects:

- stale telemetry,
- missing heartbeat gaps,
- hashrate collapse,
- rejection bursts,
- overheating.

### 7.3 Identity-bound signed outputs

When configured with the miner key, latest randomness outputs can be signed by the miner wallet identity. This creates a direct trust link between off-chain output and on-chain operator identity.

### 7.4 Merkle proof auditability

Outputs are included in Merkle epochs and can be proven against anchored roots.

### 7.5 Storage verification and attestation

Epoch files can be verified for presence and their attestation mirrored on-chain.

### 7.6 Personal threat dashboard

A miner can see:

- continuous verified runtime,
- current gap,
- longest gap,
- gap count,
- intrusion score,
- active alerts,
- signed proof-of-presence,
- verified egress readiness.

### 7.7 Runtime memory hardening and encrypted state persistence

Miner secret material — private keys, reveal signatures, and HMAC secrets — never exists in ordinary process memory.

The miner runtime holds all sensitive values inside a **SecureBuffer**: a hardened, OS-locked memory region with the following protections:

- **VirtualLock / mlock**: the buffer is pinned in physical RAM so that the operating system cannot swap it to disk, where it could be recovered forensically.
- **Guard-byte sentinels**: 8-byte canary regions are placed immediately before and after the user data. Any heap overflow or underflow that corrupts these sentinels is detected on every integrity check, turning a silent corruption into a loud failure.
- **Address-seeded canary**: a per-allocation canary derived from the buffer's own memory address makes it infeasible to forge a valid canary value from a different process or allocation.
- **Anti-debug gating**: every read access checks for an attached debugger. If one is detected, the buffer refuses to expose its contents. The check is rate-limited (one syscall per second via a monotonic-clock cache) so that it adds no measurable overhead to mining throughput.
- **RAII scoped access**: callers obtain a `ScopedRead` guard that dereferences to the data and issues a sequential memory fence (`fence(SeqCst)`) on drop. This prevents the compiler or CPU from caching or reordering the sensitive pointer across scope boundaries.
- **Three-pass paranoid wipe**: when a buffer is released, `paranoid_wipe` writes `0xFF`, then `0xAA`, then `0x00` with compiler fences between each pass, followed by a `zeroize`-crate zero pass. This exceeds DoD 5220.22-M overwrite requirements.
- **Pool recycling**: wiped allocations are returned to a lock-free pool so the next buffer reuse avoids a fresh VirtualLock syscall without ever leaking prior contents.

Beyond runtime memory, the miner also protects **at-rest state**. Pending commitment files — which contain the reveal signature and secret value needed to complete a commit-reveal cycle — are encrypted on disk using a blake3-derived keystream seeded from the miner's private key file. The encryption is transparent: `save()` encrypts before writing; `load()` decrypts after reading; legacy plaintext files are accepted on read and silently re-encrypted on the next write. When a pending file is no longer needed, it is overwritten with zeros before deletion to prevent forensic recovery.

These protections mean that even if an attacker obtains a memory dump, a disk image, or attaches a debugger to the running process, the sensitive values required to impersonate the miner or front-run a reveal are not recoverable. The commit-reveal scheme's economic security is therefore defended at every layer: protocol, network, memory, and disk.

---

## 10. Security Properties Not Yet Fully Implemented

Temporal Gradient is not yet a complete replacement for traditional network security or privacy tooling.

It does **not yet** provide:

- encrypted tunnel transport between relay nodes,
- packet forwarding plane for multi-hop routing,
- multi-hop onion-routed privacy circuits,
- full peer discovery and signed node directory,
- relay admission control and session management,
- mixnet batching and cover traffic generation,
- private messaging or censorship-resistant publishing,
- private transaction relay or MEV-protected order flow,
- cross-chain atomic swap negotiation transport,
- dVPN verified egress through healthy nodes,
- RPC load balancing and failover mesh,
- oracle data relay through the miner network,
- decentralized CDN / edge caching,
- decentralized key escrow with Shamir shares,
- verifiable computation delegation through the mesh,
- MPC transport with per-hop integrity attestation,
- consensus gossip layer for L3 / appchain,
- strong hardware attestation via TPM/TEE,
- large-scale fleet correlation across institutions,
- malware prevention or endpoint hardening.

These capabilities are future layers built on top of the current trust foundation. The relay-readiness profile and heartbeat attestation infrastructure already in place provide the primitives needed to implement each of them.

---

## 11. Zero-Trust Device Attestation

Traditional zero-trust trusts a certificate.

Temporal Gradient trusts continuous work.

This distinction matters.

A stolen certificate still authenticates.  
A copied configuration still authenticates.  
A cloned VM can still appear legitimate.  
A device running Temporal Gradient must continue to produce fresh work, maintain continuity, and preserve identity consistency over time.

Moreover, the miner's key material is protected by runtime memory hardening (locked pages, guard-byte sentinels, anti-debug gating, and RAII scoped access) and encrypted disk persistence — so even physical access to the device does not trivially yield the secrets needed to impersonate it. Attestation is therefore defended at the credential level, the runtime level, and the continuity level simultaneously.

That makes device attestation significantly stronger in dynamic environments.

Potential institutional use cases include:

- branch office continuity,
- industrial controller presence assurance,
- edge device liveness,
- remote workforce device continuity,
- critical infrastructure verification.

---

## 12. Passive Intrusion Detection

The system does not need to inspect payloads to be useful.

It can infer compromise or disruption from continuity changes.

Examples:

- miner process suspended → heartbeat gap,
- device degraded → hashrate collapse,
- hostile interference → rejection spikes,
- network isolation → telemetry staleness,
- thermal abuse or overload → temperature alerts.

This creates a passive intrusion and disruption detection surface that is orthogonal to signature-based endpoint tools.

---

## 13. National and Regional Continuity Sensing

As node count grows, the network becomes a distributed continuity map.

With sufficient geographic diversity, the network can detect:

- internet shutdowns,
- regional censorship,
- routing degradation,
- ISP-specific anomalies,
- clustered silence across cities or sectors.

At that point, the network becomes a national resilience helper.

The critical factor is not only total node count, but distribution across:

- geography,
- operators,
- ISPs,
- institutions,
- device classes.

A dense and diverse miner network can become a passive sensor grid without centralized ownership.

---

## 14. Verified Egress and Peer-to-Peer Relay Networks

A future extension of Temporal Gradient is a verified egress network built on top of the existing miner mesh.

In that model, healthy miners serve as forwarding nodes for other participants. Every relay node is continuously proving:

- it is alive,
- it is healthy,
- it is identity-bound,
- it is producing fresh work,
- it remains below a threat threshold.

This produces a fundamentally different trust model from any existing relay, VPN, or mixnet.

A traditional VPN asks the user to trust a server operator. Tor asks the user to trust that enough relay operators are honest. Neither system offers continuous, on-chain, independently verifiable proof of relay integrity.

Temporal Gradient relay nodes **cannot fake liveness**. The heartbeat chain and intrusion scoring are independently verifiable by anyone. That is the structural advantage.

The current system already exposes a relay-readiness profile per miner. The forwarding plane itself is the next major development phase.

### 14.1 Planned capabilities

The relay mesh is designed to support multiple capability layers, each building on the trust foundation of attested miners:

**Core transport**: encrypted tunnel transport with Double Ratchet key rotation, multi-hop onion-routed packet forwarding, peer discovery via on-chain signed node directory, relay admission control based on intrusion score and uptime thresholds.

**Privacy and communication**: end-to-end encrypted private messaging, mixnet batching using natural heartbeat cadence windows, whistleblower dead drops with Merkle-proven integrity, censorship-resistant publishing and data pinning.

**Financial services**: private transaction relay (IP-unlinkable tx submission), MEV-protected order flow through attested multi-hop paths, cross-chain atomic swap negotiation, entropy-as-a-service marketplace with TGBT payment channels.

**Infrastructure**: decentralized VPN (dVPN) verified egress, RPC load balancing and failover mesh, oracle data relay (building on the existing Bitcoin header fetching), decentralized CDN and edge caching, decentralized DNS resolution via DHT.

**Security and monitoring**: passive threat sensing grid (distributed IDS), canary network with on-chain alerting, decentralized key escrow with Shamir secret shares, proof-of-presence attestation for compliance and SLA enforcement.

**Compute and coordination**: verifiable computation relay (VDF, ZK delegation), MPC transport with per-hop integrity attestation, consensus gossip layer for future L3/appchain.

*A detailed technical specification for Phase 4 relay capabilities will be published separately.*

### 14.2 The structural differentiator

Every capability above exists in isolation in other projects. What makes Temporal Gradient relay unique is the trust foundation beneath it:

> Every relay node is continuously proving it is alive, healthy, identity-bound, and untampered — on-chain, independently verifiable.

No other relay network, VPN, mixnet, or mesh has this property. Tor nodes can be malicious. VPN operators can lie. CDN providers can be compromised. Temporal Gradient relay nodes cannot fake liveness because the heartbeat chain and intrusion scoring are independently verifiable by any observer.

---

## 15. Institutional Value Proposition

For institutions, the missing capability is not more logs. It is stronger proof.

Temporal Gradient offers the possibility of:

- continuous infrastructure attestation,
- independent continuity proof,
- passive disruption detection,
- decentralized resilience sensing,
- tamper-evident operational records,
- future verified egress through healthy nodes.

The strongest framing is:

**Temporal Gradient turns device uptime into cryptographic truth.**

That is valuable to:

- utilities,
- telecoms,
- hospitals,
- banks,
- logistics operators,
- industrial networks,
- governments,
- critical infrastructure providers.

---

## 16. Current Implementation Status

The current system includes:

- miner runtime with live telemetry and commit-reveal mining,
- deterministic block-anchored tokenomics with halving and bonus rewards,
- multi-pool mining with governance-created immutable pools,
- Bitcoin stale-block entropy harvesting sidecar,
- on-chain StaleBlockOracle with separate reward allocation,
- solution batching into Merkle epochs,
- randomness API,
- proof inspection,
- signed latest randomness outputs,
- EIP-712 commitment signatures with replay protection,
- on-chain epoch anchoring and finalization,
- TGBT token with immutable hard cap and permission ossification,
- storage verification and on-chain attestation recording,
- heartbeat sidecar for personal threat monitoring,
- dashboard support for threat posture and verified egress readiness,
- runtime memory hardening (VirtualLock, guard-byte sentinels, anti-debug gating, RAII scoped access, paranoid wipe),
- encrypted at-rest persistence for pending commitments (blake3-derived keystream, zero-scrub on delete).

This means the whitepaper is not purely aspirational. Core building blocks already exist in working form.

However, the following remain roadmap items:

- encrypted tunnel transport and packet forwarding plane,
- peer discovery and signed node directory,
- relay admission control and session management,
- private messaging and mixnet batching,
- private transaction relay and MEV-protected order flow,
- dVPN verified egress through healthy nodes,
- RPC load balancing, oracle relay, and decentralized CDN,
- decentralized key escrow and verifiable computation relay,
- MPC transport and consensus gossip layer,
- multi-miner institutional fleet view,
- multi-region anomaly aggregation,
- strong hardware-bound attestations,
- production policy engine for relay admission.

---

## 17. Roadmap

### Phase 1 — Mining-backed trust foundation *(current)*

- commit-reveal mining with EIP-712 replay protection,
- deterministic tokenomics with halving and bonus rewards (TGBT),
- multi-pool mining with governance-created immutable pools,
- Bitcoin stale-block entropy harvesting sidecar,
- on-chain StaleBlockOracle with dedicated reward allocation,
- Merkle epochs, proofs, and on-chain anchoring,
- randomness API and signed output identity binding,
- storage verification and on-chain attestation,
- heartbeat sidecar and threat dashboard,
- TGBT token with immutable hard cap and permission ossification.

### Phase 2 — Institutional continuity product

- fleet-wide heartbeat management,
- policy thresholds,
- institution-specific alerting,
- regional visibility,
- API integrations.

### Phase 3 — National continuity layer

- multi-region aggregation,
- censorship/outage inference,
- infrastructure silence clustering,
- distributed continuity maps.

### Phase 4 — Verified egress mesh

**4a — Core transport**

- peer discovery and signed on-chain node directory,
- encrypted tunnel transport with Double Ratchet key rotation,
- relay admission control (intrusion score, uptime, heartbeat freshness thresholds),
- packet forwarding plane with multi-hop onion routing,
- cover traffic generation and mixnet batching.

**4b — Privacy and communication**

- end-to-end encrypted private messaging through relay mesh,
- whistleblower dead drops with Merkle-proven integrity,
- censorship-resistant publishing and data pinning.

**4c — Financial services**

- private transaction relay (IP-unlinkable tx submission),
- MEV-protected order flow through attested multi-hop relay,
- cross-chain atomic swap negotiation transport,
- entropy-as-a-service marketplace with TGBT payment channels.

**4d — Infrastructure**

- decentralized VPN (dVPN) verified egress,
- RPC load balancing and failover mesh,
- oracle data relay through miner network,
- decentralized CDN / edge caching for proofs and epoch data,
- decentralized DNS resolution via DHT.

**4e — Advanced capabilities**

- passive threat sensing grid (distributed IDS),
- canary network with on-chain alerting,
- decentralized key escrow with Shamir secret shares,
- proof-of-presence attestation service for compliance/SLA,
- verifiable computation relay (VDF, ZK delegation),
- MPC transport with per-hop integrity attestation,
- consensus gossip layer for future L3/appchain.

---

## 18. Conclusion

Temporal Gradient begins as a mining system, but mining is only the surface.

Its deeper significance is that it turns continuous computational work into a decentralized security signal.

That changes what trust means.

Instead of trusting a static credential, we trust continuity.  
Instead of trusting a central observer, we trust distributed proof.  
Instead of asking whether a device was once approved, we ask whether it is alive and behaving consistently now.

This is why Temporal Gradient is more than a randomness network and more than a miner.

It is the foundation of a new class of security infrastructure:

- distributed,
- continuous,
- cryptographic,
- auditable,
- incentive-aligned.

**Every miner becomes a sensor. Every solution becomes a heartbeat. Every epoch becomes evidence.**

That is the core of the system, and the reason mining and security belong in the same architecture.

---

## Appendix A — Plain Language Summary

Temporal Gradient uses mining to do two things at once:

1. generate valuable randomness and rewards,
2. prove that machines are continuously alive and connected.

If many machines do this across many locations, the resulting network can become a decentralized layer of operational truth. Institutions can use it to verify critical devices, detect outages, and notice disruptions.

Beyond monitoring, healthy miners can become relay nodes — forwarding encrypted traffic, private messages, transactions, oracle data, and computation results through a mesh where every hop is a continuously attested, identity-bound node. This creates a verified egress network, decentralized VPN, privacy layer, and infrastructure mesh that no existing system can match, because every relay node is provably alive and untampered on-chain.

The system is therefore not only a mining network.
It is a mining-powered security and relay network.
