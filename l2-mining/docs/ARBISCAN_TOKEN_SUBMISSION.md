# Arbiscan Token Information Resubmission — TGBT

**Submission date:** 2026-04-24 (resubmission following 2026-04-09 review)
**Submitted by:** Emiliano Solazzi, project founder and protocol operator
**Sender email:** emiliano@nativebtc.org *(project-domain address — Cloudflare Email Routing, MX/SPF verifiable on the `nativebtc.org` zone)*
**Project website:** https://tgbt.nativebtc.org/ (GitHub Pages, CNAME verified; TLS certificate provisioning via GitHub automatic — HTTP is served 200 OK while Let's Encrypt completes)
**Scope of this document:** non-website criteria (project identity, sender authority, token facts, anti-misrepresentation, non-infringement). Website is live and referenced in §1 of the Arbiscan form.

This document is the single source of truth for Arbiscan's review. Every claim here can be verified on-chain against the contracts listed in the appendix.

---

## 1. Token Summary

| Field | Value |
|---|---|
| **Token name** | Temporal Gradient Beacon Token |
| **Symbol** | TGBT |
| **Standard** | ERC-20 (immutable, non-upgradeable, non-pausable) |
| **Decimals** | 18 |
| **Hard cap** | 2,000,000,000 TGBT (enforced at the token contract) |
| **Network** | Arbitrum One (chain ID 42161) |
| **Token contract** | `0x31228eE520e895DA19f728DE5459b1b317d9b8D8` (verified on Arbiscan) |
| **Minter** | Single authorized module: `TokenomicsModuleV2` — `0x7B871bdeDdED0064C34e22902181A9a983C9E2ab` (verified on Arbiscan) |
| **Admin mint** | None. No owner-mint, no proxy, no pause. |

The `TGBT` contract enforces the 2B cap at the protocol level. Only the currently-authorized `TokenomicsModuleV2` can call `mint()`, and it is constrained by two immutable budgets encoded in the module:

- **Mining allocation:** 1,900,000,000 TGBT (95% of cap), paid only through accepted commit-reveal mining solutions.
- **Stale-block allocation:** 75,000,000 TGBT (3.75%), paid only through validated Bitcoin stale-block proofs.
- **Unallocated cap headroom:** 25,000,000 TGBT (1.25%) — not assignable to any reward path under the current module set.

Prior tokenomics modules V0 (`0xA9f684d709bB46155A252b260dDDE4cb2a37a0E3`) and V1 (`0xF6069614FE09B91e5B00DA0a13A11B2BFcCabC36`) were explicitly **deauthorized** from minting TGBT; only V2 remains authorized.

---

## 2. Project Summary (plain, factual, non-promotional)

Temporal Gradient is a proof-of-work randomness and anchoring protocol deployed on Arbitrum One. Independent miners perform CPU-bound work, submit commit-reveal solutions to an on-chain `MiningModule`, and are rewarded in TGBT when their solutions are validated. Accepted solutions feed a verifiable randomness beacon and a batch-epoch Merkle commitment system. The protocol additionally harvests entropy from Bitcoin stale blocks and creates Bitcoin dead-UTXO anchors for provenance records.

TGBT's role is narrow and defined:

1. Reward asset for accepted mining solutions (`MiningModule` → `TokenomicsModuleV2.onBlockMined()`).
2. Reward asset for validated Bitcoin stale-block proofs (`StaleBlockOracle.claimReward()` → `TokenomicsModuleV2.onStaleBlockReward()`).
3. Payment asset for the proof marketplace and the dead-UTXO certificate registry (burn mechanics route miner share back to supply reduction).

There is **no token sale, no airdrop, no presale, and no public ICO**. Every TGBT currently in existence was either (a) minted as a mining reward to miners who did the work on-chain, or (b) seeded through governance-recorded initial balances documented in the deployment record referenced in the appendix.

---

## 3. Current On-Chain Supply Facts (verifiable)

As of this submission:

| Metric | Source | Value |
|---|---|---|
| `TGBT.totalSupply()` | token contract | ~13,460.375 TGBT |
| `TokenomicsModuleV2.totalMined()` | module | 8,812.5 TGBT (PoW mining + batch epochs) |
| `TokenomicsModuleV2.totalStaleRewards()` | module | 84.0 TGBT (Bitcoin stale-block claims) |
| Other seeded / governance balances | total minus mined minus stale | ~4,563.875 TGBT |
| `TokenomicsModuleV2.isAuthorized()` → true | module | Only V2 can mint |

Note that `totalSupply` is several orders of magnitude below the 2B hard cap. The token is early-stage and the vast majority of the allocation is still unminted and only reachable through verifiable on-chain mining work.

---

## 4. Governance and Control Disclosure

- **Token owner / admin on `TGBT`:** Ledger hardware wallet `0xd28E6a7AD806E85BD0544ed443D25E48f52c06c3`. Used only for permission management (authorize / revoke minter modules).
- **Protocol operator hot wallet:** `0x5cB4D906f0464b34c44d6555A770BF6aF4A2cEfe`. Used for module registration and live mining operations. Has no direct mint capability.
- **Minting authority:** Restricted to `TokenomicsModuleV2` at `0x7B871bdeDdED0064C34e22902181A9a983C9E2ab`. No EOA can mint TGBT.
- **Upgradability:** The `TGBT` token itself is non-proxy, non-upgradeable, non-pausable. Modules are registered through `TemporalGradientCore`; module permissions are currently unlocked (`permissionsLocked=false`) and are designed to be permanently locked ("Bitcoin-style ossification") once the module set is finalized.

No hidden admin mint function exists in `TGBT`. This is verifiable from the verified source code on Arbiscan.

---

## 5. Name and Symbol Non-Infringement Statement

To the knowledge of the project operator, **"Temporal Gradient Beacon Token" and the ticker "TGBT" do not infringe any existing trademark, registered mark, or well-known brand**. "TGBT" is derived from the first letters of the full token name and is used specifically within this protocol's context (a randomness and entropy-anchoring beacon on Arbitrum). The project is not associated with, and does not imply any endorsement by, any third-party brand, exchange, financial institution, or foundation.

If Arbiscan identifies a conflict with an existing project or trademark holder, the project is willing to:

- update its public-facing name / symbol representation on Arbiscan's token page, and/or
- attach a clarifying note distinguishing it from any unrelated project with a similar name,
- at Arbiscan's discretion.

---

## 6. Sender Authority

The submission email `emiliano@nativebtc.org` uses the project's official domain (`nativebtc.org`), matching the project website at `tgbt.nativebtc.org`. Mail is delivered via Cloudflare Email Routing on the same Cloudflare-managed DNS zone that publishes the `tgbt` CNAME used by the live website — so operational control of the zone, the website, and the sender address is the same party.

Additional proof paths remain available on request:

1. **On-chain signed message** from the protocol operator wallet `0x5cB4D906f0464b34c44d6555A770BF6aF4A2cEfe` or the Ledger governance wallet `0xd28E6a7AD806E85BD0544ed443D25E48f52c06c3`, signing a challenge phrase provided by Arbiscan.
2. **Transaction-based authority proof**: a zero-value Arbitrum transaction from either of the two wallets above carrying an Arbiscan-provided calldata memo.
3. **Verified-contract authorship proof**: source for `TGBT`, `TemporalGradientCore`, `TokenomicsModuleV2`, `MiningModule`, `BatchMiningModule`, `RandomnessModule`, `RateLimitModule`, and `StaleBlockOracle` is verified on Arbiscan against the deployer addresses in the appendix.

---

## 7. Founder / Team Transparency

- **Founder and protocol operator:** Emiliano Solazzi
- **LinkedIn:** <https://www.linkedin.com/in/emiliano-germ%C3%A1n-solazzi-griminger-936717210/> (profile lists TGBT / Temporal Gradient experience)
- **Official project website:** <https://tgbt.nativebtc.org/>
- **Code repository:** <https://github.com/emilianosolazzi/TGBT_Token>
- **Whitepaper:** <https://github.com/emilianosolazzi/Temporal-Gradient-whitepaper/blob/main/Temporal%20Gradient%20Whitepaper.md>
- **Role:** sole maintainer of the public code repository, protocol operator of the live Arbitrum deployment, and the party responsible for this submission.
- **Jurisdiction statement:** the project is a developer-led open-source protocol; it is not a registered investment product, is not marketed as one, and makes no forward-looking financial promises anywhere in its materials.

---

## 8. Anti-Misrepresentation Declarations

The project attests the following explicitly, to prevent any reviewer concern:

1. **No false claims of partnership.** TGBT is not endorsed by, affiliated with, or partnered with Bitcoin, Arbitrum Foundation, Offchain Labs, Chainlink, OpenZeppelin, or any exchange. The protocol *uses* Bitcoin public data (stale blocks, dead UTXOs) as a verifiable external data source; it does not speak for Bitcoin.
2. **No false claims of security or audit.** TGBT has not undergone a third-party security audit. Source code is public and verified on Arbiscan; reviewers are welcome to inspect it. The project does not use the word "audited" in any user-facing material.
3. **No investment solicitation.** TGBT is described exclusively as a mining reward asset and an in-protocol payment asset. No marketing copy describes it as an investment product, a yield vehicle, or a store of value.
4. **No misrepresentation of technology.** All technical claims in public documentation (whitepaper, README, self-miner dashboard, dashboard UI copy) correspond to behaviour that is actually implemented in the verified on-chain contracts and the open-source runtime. Where a feature is planned or forward-looking (e.g. DAO governance, staking layer), it is explicitly labelled as such.
5. **No misrepresentation of supply.** The current totalSupply is a small fraction of the 2B hard cap. The project does not claim a circulating supply larger than what on-chain reads confirm.

---

## 9. Contract and Transaction Appendix (single-page reference)

**Core protocol on Arbitrum One (chain ID 42161):**

| Contract | Address | Arbiscan status |
|---|---|---|
| `TGBT` token | `0x31228eE520e895DA19f728DE5459b1b317d9b8D8` | Verified |
| `TemporalGradientCore` | `0xF6556DDC7CdD3635A05428BD85BCf33A09F752e6` | Verified |
| `MiningModule` | `0xb2b3d9bC63993b725Aea36aC90601c22292F3171` | Verified (redeployed 2026-04-21) |
| `BatchMiningModule` | `0xAf07E37D104E9be17639FE7a51B36972D4738651` | Verified |
| `RandomnessModule` | `0x583863CFC5EFc0106886BA485e1b67F0966584f9` | Verified |
| `TokenomicsModuleV2` | `0x7B871bdeDdED0064C34e22902181A9a983C9E2ab` | Verified |
| `RateLimitModule` | `0x61dEEEf2B2956db3AD291c639939669cD5399c1B` | Verified |
| `StaleBlockOracle` | `0xdc4eDF632187d05da50393Af87D19A08f6986517` | Verified |

**Deauthorized / historical (not live):**

| Contract | Address | Status |
|---|---|---|
| `TokenomicsModule` V1 | `0xF6069614FE09B91e5B00DA0a13A11B2BFcCabC36` | Deauthorized from minting |
| `TokenomicsModule` V0 | `0xA9f684d709bB46155A252b260dDDE4cb2a37a0E3` | Deauthorized from minting |
| `MiningModule` (prior) | `0x97A88f7ed5e7D8EEd442f6979aC66bBb599ff595` | Deregistered — not in Core module registry |

**Deployment / authority transaction references:**

| Action | Transaction |
|---|---|
| `TokenomicsModuleV2` deploy | `0x0d0c857b7d01600b5e40f98c4ebd6b199dd3cd6b39f6ccbea88d174def0c20c8` |
| Grant V2 minter auth on TGBT (Ledger) | `0xcea9ba003cc635b6ff10a37bc6dcbc4793856ba8e1c130f15de7d40547ec9f56` |
| Register V2 in Core `setModule(TOKENOMICS_MODULE)` | `0x44157e00c7578234295e1adcb902399bb804e860bc222c112cb4554d494ae4c0` |
| Revoke V1 minter auth on TGBT (Ledger) | `0x808e0cfb1d1f7fb2524eec6c742927053120a9e1f776488ed2f541746240e1a5` |
| `MiningModule` (current) deploy | `0x0f54cba023b83a586ba78c9c1b62761c4a9c6ba609009ece19f83c0345d1f107` |
| `BatchMiningModule` deploy | `0x18bdeffae0a3b02016f54a5ef02074425be8e3418004659f53cb5af965d1b44d` |
| `RandomnessModule` deploy | `0x546404da42b698c90bb5551312f7fef1bd9a710a59e3b1802d75478cbddd36d2` |

**Controlling wallets:**

| Role | Address |
|---|---|
| Ledger governance / token admin | `0xd28E6a7AD806E85BD0544ed443D25E48f52c06c3` |
| Protocol operator (hot) | `0x5cB4D906f0464b34c44d6555A770BF6aF4A2cEfe` |

---

## 10. What We Are Asking Arbiscan To Do

1. Approve token information for `TGBT` at `0x31228eE520e895DA19f728DE5459b1b317d9b8D8` using the facts in §1–§4 of this document.
2. Accept the sender-authority mitigation in §6 (on-chain signed message from the governance or operator wallet) in lieu of a domain-matched email, or advise which alternative verification path Arbiscan prefers.
3. Flag any name / symbol concern described in §5 so the project can respond rather than be silently declined.

The project is available to respond promptly to any reviewer question through the same submission channel.

— end of document —
