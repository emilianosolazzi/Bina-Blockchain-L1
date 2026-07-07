# SPSF-AI ↔ TGB Randomness Integration Plan

> **Status:** Planning / validated seam · rollout selected · UTXO anchor API validated · **Last updated:** 2026-06-25
> **Owner:** TGBT mining stack (single-operator) · **Consumer:** SPSF-AI (external Python repo)

This document describes how the **TGBT mining stack** (producer of verified randomness)
integrates with **SPSF-AI** (a decentralized federated-learning network that consumes that
randomness for tamper-resistant validator/sample selection). It records what was validated
on-chain on 2026-06-25, the blocker that was found, and the selected rollout path.

---

## 1. Roles

| System | Role | Location |
|--------|------|----------|
| **TGBT mining stack** | **Producer** — mines verified `bytes32` randomness outputs on Arbitrum One | this workspace |
| **SPSF-AI** | **Consumer** — uses TGB outputs to seed which validation samples each model-update is tested against | `C:\Users\comar\Downloads\SPFS-AI-main(1)\SPFS-AI-main` (separate Python repo) |

SPSF-AI's design rule: *everyone can propose model improvements, but only validated improvements
enter the shared model* — and validation-sample selection must be auditable and un-gameable.
That is why it sources randomness from on-chain TGB outputs instead of a local PRNG.

---

## 2. Data flow

```
TGBT MINING STACK (producer)              SPSF-AI (consumer)
  MiningModule commit-reveal      ──▶  TGBRandomnessIndexer.py
  → CoreOutputRecorded event           scans Arbitrum logs for CoreOutputRecorded (+ Transfer mint)
  → TGBT minted in reveal tx           → SQLite inventory (AVAILABLE → RESERVED → CONSUMED)
                                            │
                                            ▼
                                       SPSFRandomnessProvider.py
                                       consumes 1 verified output/round →
                                       seed = H(output ‖ purpose ‖ package ‖ model) →
                                       selects validation dataset rows
```

### Relevant on-chain event (verified in source + on-chain)

```solidity
// TemporalGradientCore.sol
event CoreOutputRecorded(
    bytes32 indexed newOutput,
    address indexed miner,
    uint8   indexed poolId,
    uint256 reward,
    uint64  nonce        // ← NOT an epoch; this is the mining nonce
);
```

---

## 3. What was validated (2026-06-25, live chain)

✅ **Ingestion works.** `CoreOutputRecorded` is flowing from Core `0xF6556DDC…`, signed by the hot
wallet `0x5cB4…cEfe`, with 11-bit-difficulty output hashes and 12.5 TGBT (bonus) rewards.
**3 outputs in the last ~2M blocks (~5.8 days)** → ~0.5 outputs/day under the current **manual**
submission mode. The SPSF indexer's contract address, event topic, and parser all match the real
contract.

✅ **Reward-mint proof works.** The TGBT mint `Transfer(0x0 → miner)` happens in the same reveal tx,
so `reward_mint_seen = 1` is satisfiable.

✅ **Both referenced modules are live + registered** (`isModule(Core) = true`):

| Module | Address | Bytecode |
|--------|---------|----------|
| RandomnessModule | `0x583863CFC5EFc0106886BA485e1b67F0966584f9` | 10,106 bytes |
| RateLimitModule  | `0x61dEEEf2B2956db3AD291c639939669cD5399c1B` | 5,274 bytes |

✅ **Dead-UTXO anchor API works, but SPSF is not using it yet.** The running randomness API can
create canonical Bitcoin dead-UTXO anchors and build verifier / certificate payloads from them:

| Endpoint | Role |
|----------|------|
| `GET /api/utxo/scan?seed=...&storageReference=...` | Selects and verifies a dead Bitcoin output, then creates a canonical `dead_utxo_anchor_v1` record. |
| `GET /api/utxo/anchor/latest` | Returns the latest stored anchor (`anchorId`, `utxoId`, `dataHash`, `metadataDigest`, Bitcoin block data). |
| `POST /api/utxo/receipt-upload` | Uploads receipt JSON to IPFS via configured Pinata / web3.storage credentials and returns the real `ipfs://CID`. |
| `POST /api/utxo/certificate-payload` | Returns API receipt context by default; only adds `registerAnchor` / `mintCertificate` calldata when an attestor and wallet owner are supplied. |

The default enterprise mode is trustless API proof:

```text
Artifact hash
→ Bitcoin dead-UTXO anchor
→ self-contained receipt JSON
→ optional IPFS CID for the receipt
```

No attestor is required for that mode. The receipt is sold as an independently verifiable
Bitcoin-backed proof, not as "attested by X". If an enterprise customer wants organizational
accountability on top, add an issuer / attestor wallet and a recipient wallet, then build the
optional `UTXOAnchorVerifier.registerAnchor` and `UTXOCertificateRegistry.mintCertificate` payloads.

This means the current SPSF plan uses only the **randomness** part of TGBT. It does **not yet** use
the Bitcoin anchor layer at full strength.

### What “full power” would mean for SPSF

For SPSF-AI, the dead-UTXO layer should not replace randomness selection. It should bind the
validation artifact to Bitcoin-backed provenance:

1. SPSF consumes a TGBT output to pick validation samples.
2. SPSF hashes the validation artifact: model update hash, dataset slice hash, validator identity,
   round ID, selected sample IDs, validation score, and timestamp.
3. TGBT randomness API creates a dead-UTXO anchor with `storageReference` such as
   `spsf://round/<roundId>/model/<modelHash>`.
4. SPSF stores the returned `anchorId`, `metadataDigest`, Bitcoin `utxoId`, and `documentHash`
   beside the validation result.
5. Optional robust path: register the anchor on-chain with `UTXOAnchorVerifier`, then mint a
   `UTXOCertificateRegistry` certificate for the validation record. This blockchain mode requires
   a wallet owner plus an authorized issuer / attestor signature; API receipts do not.

That gives SPSF two independent assurances:

| Layer | What it proves |
|-------|----------------|
| TGBT randomness output | Sample selection was ungameable. |
| Bitcoin dead-UTXO anchor | The validation record existed with a specific content hash and external Bitcoin-backed provenance. |

### Stronger option — future-beacon anchor requests

A stronger anti-manipulation version is possible, but it is a separate API mode from today's
immediate `GET /api/utxo/scan` flow.

Conceptual flow:

```text
POST /anchor
→ API records request: documentHash, requester, requestBlock, requestId
→ API replies: Waiting for next beacon...

Later, after a new TGBT output is mined on Arbitrum:
Epoch/leaf/output = 0x00052ab...
→ seed = H(documentHash || requestId || futureOutput)
→ API selects the dead UTXO from that seed
→ API creates the Bitcoin anchor + receipt
```

That gives a stronger story than customer-chosen or same-block anchoring: the customer cannot know
which UTXO will be selected when they submit the request, because the deciding randomness does not
exist yet.

For high assurance, the request itself must be committed before the future beacon output is known.
There are three levels:

| Level | Request commitment | Manipulation resistance |
|-------|--------------------|-------------------------|
| Basic | API stores pending request off-chain and returns a signed `requestId` | Good UX, but users trust the API not to reorder/drop requests. |
| Strong | Request hash is written on Arbitrum before the next `CoreOutputRecorded` output | Customer and API cannot choose after seeing the randomness. |
| Strongest | Request hash + future output consumption are recorded on-chain, then the UTXO anchor is registered/minted | Auditable request ordering, randomness authenticity, consumed-once semantics, and Bitcoin provenance. |

What already exists on Arbitrum helps, but does not fully replace this new request layer:

- `CoreOutputRecorded` already provides the future unpredictable output.
- `TGBTConsumableRandomness` + `RandomnessConsumptionRegistry` can later make the future output
   authentic and consumed once across nodes.
- `UTXOAnchorVerifier` + `UTXOCertificateRegistry` can later register and mint the resulting
   Bitcoin-backed receipt.
- A small `AnchorRequestRegistry` or equivalent request-commit event is still needed if the product
   needs public proof that `POST /anchor` happened before the deciding output existed.

---

## 4. The blocker — `require_epoch_finalized` is unsatisfiable for commit-reveal outputs

SPSF only consumes outputs whose epoch is `EpochFinalized`. That gate **cannot ever pass** for
commit-reveal outputs, for three compounding reasons:

1. **`CoreOutputRecorded` carries no epoch.** Its 5th field is the mining **nonce**
   (e.g. `2947`, `2215`, `3529`), not an epoch ID.
2. **`EpochFinalized(uint256,uint256)` is emitted only by `BatchMiningModule`**, whose epoch IDs are
   sequential (`0 … ~94`). The indexer fakes the link with `epoch_id = nonce`, then matches via
   `WHERE nonce = epochId`. Nonces (thousands) never equal batch epoch IDs (0–94) → **never matches**.
3. **There were 0 `EpochFinalized` events in the last ~5.8 days** anyway.

**Net effect today:** with `require_epoch_finalized = True`, SPSF sees **zero** consumable outputs —
even though outputs are being mined and rewarded.

### Key insight

Commit-reveal `CoreOutputRecorded` outputs are **already final at inclusion**: on-chain uniqueness is
enforced (`usedOutputs`), each output chains from the previous, and the TGBT reward mints in the same
reveal tx. **The 96h / 28,800-block challenge window applies only to batch epochs, not commit-reveal.**

Therefore the binding constraint is **gate semantics, not throughput**. The correct authenticity gate
for the commit-reveal stream is:

> `require_reward_mint_seen = True` + N block confirmations — **not** `require_epoch_finalized`.

---

## 5. How the attached contracts help

Two contracts in `l2-mining/contracts/` (written, **not yet deployed**) address a deeper layer than
the Python gate — they make `epoch_finalized` unnecessary and replace it with on-chain guarantees.

| Contract | Purpose |
|----------|---------|
| `RandomnessConsumptionRegistry.sol` | On-chain, tamper-proof "consumed once" ledger: `markAsConsumed()`, `isConsumed()`, authorized consumers, owner batch-backfill. |
| `TGBTConsumableRandomness.sol` | Pay-to-consume gateway (ModuleBase). Optional `validateOutputExists` checks the output against Core's on-chain history; pulls a TGBT fee split between burn + treasury. |

### Mapping to the blocker

| Problem | Contract replacement |
|---------|----------------------|
| `require_epoch_finalized` can't bind to commit-reveal outputs | `TGBTConsumableRandomness.validateOutputExists` → `_outputExistsInHistory(output)` checks Core's **on-chain ring buffer**. Immediate authenticity, **no epoch needed**. |
| SPSF dedups consumption only in **local SQLite** (useless across multiple validator nodes) | `RandomnessConsumptionRegistry.isConsumed()/markAsConsumed()` — **on-chain, multi-node, tamper-proof** dedup. |

So the gate becomes: **on-chain `validateOutputExists` (authenticity) + on-chain `isConsumed`
(global dedup)** — fully replacing the broken `epoch_finalized` linkage.

### Caveats to weigh

1. **32-output window.** `_outputExistsInHistory` only sees Core's **last 32** outputs
   (`outputHistory[32]` ring buffer). At ~0.5 outputs/day that's ~2 months of coverage — fine now —
   but anything older than the last 32 mined outputs **cannot be on-chain-validated** by the gateway.
   Faster output rate shrinks that window in wall-clock time. (`batchMarkConsumed` can still backfill
   *dedup* records for old outputs, but not *authenticity*.)
2. **Consumption becomes a paid on-chain tx.** `consumeRandomness()` costs a TGBT fee + gas + an
   Arbitrum confirmation. SPSF currently consumes off-chain (free, instant). Moving on-chain adds
   cost + latency to every validator sampling round — but it burns TGBT (aligns with the
   RandomnessShop tokenomics). Decide who pays (validators) and whether every round is gated this way.
3. **The indexer is still required.** The registry tracks *consumed/authentic*; it does not enumerate
   outputs or expose their `bytes32` value. SPSF still needs `TGBRandomnessIndexer` to **discover**
   outputs and read the value to derive seeds. The contracts and indexer are **complementary**.

---

## 6. Selected rollout

**Decision:** do **Path 1 now**, keep the **indexer as discovery**, deploy **Path 2 later** when SPSF
is multi-node, and use **Path 3 as the premium / provenance layer**.

That order keeps SPSF unblocked without paying per-round on-chain costs immediately, while still
preserving the stronger decentralized and Bitcoin-anchored paths for when product demand justifies
them.

### Path 1 — Lightweight (off-chain indexer fix)

Fix the SPSF indexer/provider gate only. No deployment, no per-use cost.

- Set `require_reward_mint_seen = True`.
- Add N Arbitrum confirmations before an output becomes `AVAILABLE`.
- Set `require_epoch_finalized = False` for commit-reveal outputs.
- Treat commit-reveal `CoreOutputRecorded` as consumable on `reward_mint_seen = True` + N confirmations.
- Drop `require_epoch_finalized` for the commit-reveal stream; reserve it only for true batch-epoch
  outputs (which carry real sequential epoch IDs + the 96h challenge window).
- Keep the local SQLite `AVAILABLE → RESERVED → CONSUMED` dedup.

**Best when:** SPSF stays single-operator / trusts its own off-chain dedup. Fastest unblock.

### Path 2 — Robust / decentralized (deploy the contracts)

Use on-chain authenticity + dedup later, once SPSF needs multi-node global consumed-once protection.

1. Deploy `RandomnessConsumptionRegistry`.
2. Deploy `TGBTConsumableRandomness`; `initialize(core, tgbt, registry, treasury, burn, fee, burnBps)`.
3. Register `TGBTConsumableRandomness` as a Core module (so `_outputHistory()` + `whenSystemActive`
   work).
4. `registry.authorizeConsumer(consumable)`.
5. Tune fee / burn / treasury; `setOutputValidation(true)` once Core has ≥32 outputs of history.
6. Point `SPSFRandomnessProvider` at `consumeRandomness(output, poolId)` instead of (or in addition
   to) local SQLite marking.

**Best when:** SPSF becomes multi-node and needs tamper-proof "consumed once" + TGBT utility.

### Recommended combination

Use the **indexer for discovery** (read which outputs exist + their values) and the **on-chain
registry for the authoritative authentic + consumed-once check**. The indexer is not replaced by
the contracts; it remains the discovery layer that finds output values and feeds SPSF seeds.

Start with Path 1 to unblock now; adopt Path 2 when the validator network goes multi-node.

### Miner UX lane — clean knowledge contribution

Keep three operator concepts separate in the product:

| Lane | What the miner sees | What happens downstream |
|------|---------------------|-------------------------|
| Mining / participation | Hashrate, solutions, TGBT rewards, stale blocks, Bitcoin provenance | Produces randomness and on-chain mining rewards. |
| Clean knowledge contribution | **Help improve answers** card with a prompt and one completion box | Sends prompt-completion examples to SPSF for review. |
| Model operations | Hidden from miner UX | Promotion, quorum validation, anchoring, and later model rounds. |

The dashboard contribution card should mirror `SPSFMiner.py --add-examples`:

```text
Help improve answers
Complete this clearly: "Validators protect"
[operator writes a completion]
Submit improvement
```

Response copy:

- Accepted: `Accepted. This may be used in the next model round.`
- Rejected: `Rejected: completion is too short.`

Do not describe this card as training, weights, or federated learning. The miner is contributing
clean examples, not directly modifying a shared model. The browser posts to the dashboard proxy:

```http
POST /api/spsf/contribute/example
{ "prompt": "Validators protect", "completion": "..." }
```

The dashboard server forwards to the SPSF endpoint:

```http
POST <SPSF_SERVER_URL>/contribute/example
```

Reward semantics should stay delayed and auditable:

| Event | Miner-facing meaning | Reward effect |
|-------|----------------------|---------------|
| Accepted | Example passed basic intake checks | Reputation point only. |
| Promoted | Example passed SPSF quality review | Contribution credit. |
| Included in validated anchored round | Example appears in a quorum-validated, Bitcoin/TGBT-anchored round | Reward eligible. |

There is no instant TGBT payout for submitting examples. That avoids spam incentives and keeps the
reward tied to validated usefulness rather than raw submission volume.

### Path 3 — Full SPSF provenance (add dead-UTXO anchoring)

Use the existing UTXO API as the premium provenance layer. Do **not** anchor every validation round
at first.

Rollout order:

1. **Accepted model updates only** — first production target.
2. **Daily checkpoint anchors** — add after accepted-update anchoring proves useful.
3. **Every validation round** — only if SPSF later needs full per-round audit trails.

- Keep Path 1 or Path 2 for randomness authenticity / consumption.
- Add `GET /api/utxo/scan` with an SPSF-specific seed and `storageReference` for accepted model
  updates first, such as `spsf://accepted-update/<roundId>/<modelHash>`.
- Build receipt JSON from the artifact hash + anchor facts, without embedding its own IPFS URI:

   ```json
   {
      "name": "Bitcoin Provenance Certificate",
      "artifactHash": "0x...",
      "anchorId": "...",
      "utxoId": "txid:vout",
      "bitcoinBlockHeight": 920000,
      "bitcoinBlockHash": "...",
      "blockHeader": "...",
      "scriptPubKey": "...",
      "deadOutputReason": "OP_RETURN output is provably unspendable",
      "metadataDigest": "...",
      "canonicalMetadataJson": "{...}",
      "verificationFormula": "anchor_id = sha256(utxo_id || data_hash_hex || merkle_root_hex || storage_reference || created_at_le)",
      "links": {
         "explorer": "https://mempool.space/tx/...",
         "txApi": "https://mempool.space/api/tx/...",
         "outspendApi": "https://mempool.space/api/tx/.../outspend/0"
      },
      "issuedAt": "...",
      "schema": "tgbt-bitcoin-provenance-receipt-v1"
   }
   ```

- Upload that receipt JSON with `POST /api/utxo/receipt-upload`, receive the real `ipfs://CID`,
   then pass that URI as `metadataURI` to `POST /api/utxo/certificate-payload` if blockchain calldata
   is needed.
- For API mode, do not require a recipient wallet or attestor. Optional `owner` can be a wallet,
   organization name, customer ID, email, or DID. Optional `attestor` means organizational signature,
   not proof validity.
- For blockchain mode, call `POST /api/utxo/certificate-payload` with the hash of the SPSF validation
   artifact, uploaded receipt URI, a 0x recipient wallet, and an authorized attestor.
- Store `anchorId`, `utxoId`, `dataHash`, `metadataDigest`, `documentHash`, and Bitcoin block
   metadata in SPSF's local result database.
- Later, if product / compliance value justifies it, submit `verifierRegistration` and
   `certificateMint` on-chain so SPSF validation records become transferable / independently
   verifiable certificates.

#### Path 3B — Future-beacon anchor mode

After the immediate accepted-update anchoring flow is working, add a queued anchor mode for stronger
anti-manipulation guarantees:

1. SPSF or an enterprise customer submits `documentHash` to a pending anchor endpoint.
2. The service records `requestId = H(documentHash || requester || requestBlock || nonce)` and,
   for high assurance, emits or submits that request hash on Arbitrum before the next beacon output.
3. The service waits for the next eligible `CoreOutputRecorded` output, preferably one that has
   `reward_mint_seen = True` plus N confirmations.
4. The service derives the UTXO selection seed from `H(documentHash || requestId || futureOutput)`.
5. The service creates the dead-UTXO anchor and returns a receipt containing the request proof,
   future output, selected UTXO, anchor ID, document hash, and verification instructions.

This is the best product story for customers worried about anchor manipulation: neither the customer
nor the API can know or choose the UTXO at request time, as long as the request commitment predates
the future TGBT output.

**Best when:** SPSF wants audit-grade model-update provenance, not just fair validator/sample
selection.

---

## 7. Open decisions

- [x] Rollout order: Path 1 now, Path 2 later, Path 3 for premium / provenance.
- [x] First anchoring scope: accepted model updates only.
- [ ] Choose N Arbitrum confirmations for Path 1 availability.
- [ ] If Path 2 later: who pays the per-consume TGBT fee + gas (validators)? What fee / burn split?
- [ ] Is the 32-output authenticity window acceptable for Path 2, or is deep-backlog consumption required?
- [ ] Does SPSF need a steadier TGBT output supply (revisit manual vs. auto submission cadence) or is bursty fine?
- [ ] RPC for the indexer: keep public `arb1.arbitrum.io` (independent) or use the NativeBTC RPC?
- [ ] When should SPSF add daily checkpoint anchors after accepted-update anchoring is proven?
- [ ] Should accepted-update anchors stay off-chain/API-only first, or register/mint certificates
  on-chain from day one?
- [ ] For future-beacon anchor mode: is an off-chain signed pending receipt enough, or should the
   request hash be committed on Arbitrum before waiting for the next TGBT output?

---

## 8. Reference — addresses & events

| Item | Value |
|------|-------|
| Core (TemporalGradientCore) | `0xF6556DDC7CdD3635A05428BD85BCf33A09F752e6` |
| MiningModule | `0xb2b3d9bC63993b725Aea36aC90601c22292F3171` |
| BatchMiningModule | `0xAf07E37D104E9be17639FE7a51B36972D4738651` |
| RandomnessModule | `0x583863CFC5EFc0106886BA485e1b67F0966584f9` |
| RateLimitModule | `0x61dEEEf2B2956db3AD291c639939669cD5399c1B` |
| TokenomicsModule | `0x7B871bdeDdED0064C34e22902181A9a983C9E2ab` |
| TGBT Token | `0x31228eE520e895DA19f728DE5459b1b317d9b8D8` |
| `CoreOutputRecorded` topic0 | `0x67d3ce0ebd64fd365d2b43b2091893b6db60c67e003e248b9ce37b17dbc8c458` |
| `EpochFinalized(uint256,uint256)` topic0 | `0x6debf9c0b8bd7ecda40db89a2641f61251d80a576b5c5e5f06de7f1c2a65850a` |

**Consumer files (external repo):** `TGBRandomnessIndexer.py`, `SPSFRandomnessProvider.py`,
`SPSFMinerValidatorNode.py`, `test_randomness_integration.py`.
