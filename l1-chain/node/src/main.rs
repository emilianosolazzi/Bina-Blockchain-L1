mod bitcoin_entropy;
mod envelope;
mod gossip;
mod peers;
mod store;

use axum::{
    extract::{ConnectInfo, Path, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use envelope::{BinaMessage, BlockClaimEnvelope, PeerHelloEnvelope};
use gossip::Gossip;
use store::BlockStore;
use l1_core::bitcoin_entropy::{
    governing_checkpoint_height, is_checkpoint_height, BtcCheckpointProof, BtcEntropyState, BTC_CHECKPOINT_INTERVAL,
};
use l1_core::block::{genesis_block, leading_zero_bits, timestamp_is_valid, L1BlockHeader};
use l1_core::claims::{claim_is_better, SignedBlockClaim};
use l1_core::crypto::WalletKeypair;
use l1_core::difficulty::{DifficultyAdjuster, TARGET_BLOCK_MS};
use l1_core::pow::mine_block;
use l1_core::randomness::{NullifierSet, RandomnessOutput};
use l1_core::rewards::{
    block_reward, RewardLedger, HALVING_INTERVAL, HARD_CAP, INITIAL_BLOCK_REWARD,
};
use l1_core::secure_memory::SecureBuffer;
use l1_core::transaction::{parse_address_hex, SignedTransaction, Transaction, ED25519_PUBLIC_KEY_BYTES};
use peers::PeerList;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use zeroize::{Zeroize, Zeroizing};

const DEFAULT_PORT: u16 = 8181;
const NETWORK_ID: &str = "bina-l1";
const DEFAULT_P2P_TTL: u8 = 8;
const MAX_PEERS: usize = 128;
const DEFAULT_SEED_PEERS: &[&str] = &["144.126.157.197:8181"];

/// HTTP API port. Overridable so multiple nodes can run on one machine
/// (local dev/test networks, or a host running more than one instance).
fn http_port() -> u16 {
    std::env::var("BINA_HTTP_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

/// Data directory for the reward ledger and chain-state files. Overridable
/// for the same reason as `http_port` — distinct local nodes must not share
/// a data directory.
fn data_dir() -> String {
    std::env::var("BINA_DATA_DIR").unwrap_or_else(|_| "data".to_string())
}

fn ledger_path() -> String {
    format!("{}/ledger.csv", data_dir())
}

fn chain_state_path() -> String {
    format!("{}/chain-state.json", data_dir())
}
const CHAIN_STATE_VERSION: u32 = 2;
const SUBMISSION_GRACE_MS: u64 = 1_500;
/// How far a claim's embedded timestamp may sit ahead of a validator's own clock.
const MAX_FUTURE_MS: u64 = 30_000;
/// Bitcoin-block tolerance for checkpoint plausibility checks — see
/// `BtcCheckpointProof::plausible`. Wide enough to absorb ordinary
/// provider/propagation lag between independent validators, narrow enough
/// that a miner cannot pin an arbitrary/fabricated Bitcoin state.
const BTC_HEIGHT_TOLERANCE: u64 = 2;
/// How often the background task refreshes the node's own live observation
/// of Bitcoin chain state (used for checkpoint plausibility + telemetry).
const BTC_OBSERVE_INTERVAL: Duration = Duration::from_secs(5);
/// Maximum number of blocks returned by a single `/chain/headers` sync page.
const MAX_SYNC_PAGE: usize = 500;
/// Maximum accepted JSON request body size for mutating endpoints.
const MAX_BODY_BYTES: usize = 64 * 1024;
/// Sliding-window request budget per source IP for mutating endpoints.
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(10);
const RATE_LIMIT_MAX_REQUESTS: u32 = 200;
/// Bound on how many unconfirmed transactions a node will hold at once.
const MAX_MEMPOOL_SIZE: usize = 50_000;
/// Path (under the node's data directory) for the durable block store.
fn block_store_path() -> String {
    format!("{}/blocks.sqlite3", data_dir())
}

// ─── Per-block record (stored + returned by API) ───────────────────────────

#[derive(Serialize, Deserialize, Clone)]
struct BlockRecord {
    height: u64,
    block_hash: String,
    prev_hash: String,
    nonce: u64,
    /// Consensus timestamp in Unix **milliseconds** (see
    /// `L1BlockHeader.timestamp`'s doc comment for why this chain uses ms
    /// rather than the more common Unix-seconds).
    timestamp: u64,
    /// Same instant as `timestamp`, floor-divided to Unix **seconds**. Added
    /// specifically for EVM/Solidity relaying: `block.timestamp` and every
    /// timestamp check in `Solidity/BinaOracle.sol` (`minedTimestamp`,
    /// `MAX_TIMESTAMP_DRIFT`, `MAX_TIMESTAMP_AGE`) are Unix-seconds. Relaying
    /// the raw `timestamp` field into `BinaOutput.minedTimestamp` would be
    /// ~1000x too large and make `submitOutput` revert on every block —
    /// use this field for that purpose instead.
    #[serde(default)]
    mined_timestamp_secs: u64,
    zero_bits: u32,
    difficulty_bits: u32, // difficulty that produced this block
    hashes_tried: u64,
    elapsed_ms: u64,
    hashrate_mhs: f64,
    miner_address: String,     // 40-char hex — wallet address
    miner_public_key: String,  // hex WalletPublicKey bytes
    miner_signature: String,   // hex HybridSignature over claim digest
    claim_digest: String,      // signed 32-byte claim digest
    election_score: String,    // deterministic candidate tie-break score
    source: String,            // "local" or "submitted"
    reward_bina: u64,          // BINA awarded for this block
    randomness_output: String, // 64-char hex — the random bytes
    nullifier: String,         // 64-char hex — one-time spend token
    btc_seed: String,          // 64-char hex — full seed hash
    btc_height: u64,
    merkle_root: String,       // 64-char hex — root over `transactions`
    state_root: String,        // 64-char hex — ledger commitment after this block
    chain_work_hex: String,    // cumulative chain work as of this block (hex u128)
    #[serde(default)]
    transactions: Vec<SignedTransaction>, // the exact, ordered set this block executed
}

/// A claim not yet finalized for its height, plus the Bitcoin-checkpoint
/// proof it carried (only present when the claim targets a checkpoint
/// height — see `l1_core::bitcoin_entropy`).
#[derive(Clone)]
struct PendingClaim {
    claim: SignedBlockClaim,
    btc_checkpoint: Option<BtcCheckpointProof>,
    /// The exact, ordered transaction list this claim's header commits to
    /// via `merkle_root`/`state_root`.
    transactions: Vec<SignedTransaction>,
}

/// Deterministic winner-selection over a height's candidate claims, using
/// the same objective-work-then-election-score rule as
/// `l1_core::claims::select_winning_claim`, but carrying the checkpoint
/// proof of whichever candidate wins so it can be pinned on finalization.
fn select_winning_pending<I>(claims: I) -> Option<PendingClaim>
where
    I: IntoIterator<Item = PendingClaim>,
{
    claims.into_iter().reduce(|winner, candidate| {
        if claim_is_better(&candidate.claim, &winner.claim) {
            candidate
        } else {
            winner
        }
    })
}

// ─── Shared mutable node state ─────────────────────────────────────────────

struct NodeState {
    genesis_hash: [u8; 32],
    tip_hash: [u8; 32],
    chain_height: u64,
    pending_claims: HashMap<u64, HashMap<String, PendingClaim>>,
    /// Signed, structurally-valid transactions waiting to be included in a
    /// block. Never mutates ledger balances directly — only block execution
    /// (mining a block or accepting/replaying one) does that.
    mempool: HashMap<[u8; 32], SignedTransaction>,
    nullifiers: NullifierSet,
    total_hashes: u64,
    total_time_ms: u64,
    started_at: Instant,
    threads: usize,
    /// Freshest live observation of Bitcoin chain state (telemetry +
    /// checkpoint-plausibility input). Refreshed by a background task and
    /// opportunistically whenever this node mines a checkpoint block.
    last_observed_btc: BtcEntropyState,
    /// Consensus-pinned Bitcoin seed for the current checkpoint epoch. Every
    /// non-checkpoint block must reuse this value exactly; it only changes
    /// when a checkpoint-height block is finalized.
    btc_seed_hash: [u8; 32],
    btc_seed_changed_at: u64,
    /// Bitcoin tip height committed by the last accepted checkpoint.
    btc_checkpoint_tip_height: u64,
    difficulty_bits: u32,
    /// Consensus timestamp (Unix ms) of the current chain tip — the
    /// monotonicity floor the next block's timestamp must exceed.
    last_block_timestamp_ms: u64,
    /// Cumulative proof-of-work across the whole chain (sum of 2^work_bits
    /// per block), used for heaviest-chain fork choice during sync.
    chain_work: u128,
    miner_address: String,
    // Economics
    total_mined_bina: u64,
    current_reward: u64,
    last_adjustment: Option<String>, // log line of last difficulty change
}

type SharedState = Arc<RwLock<NodeState>>;
type SharedLedger = Arc<Mutex<RewardLedger>>;

#[derive(Debug, Clone)]
struct AcceptedClaim {
    height: u64,
    miner_hex: String,
    block_hash: String,
    election_score: String,
    work_bits: u32,
}

#[derive(Debug)]
struct ClaimReject {
    status: StatusCode,
    message: String,
}

/// Consensus-critical fields persisted alongside the chain tip so a
/// restarted node resumes deterministic difficulty/checkpoint state exactly
/// where it left off, instead of re-seeding from wall clock or defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConsensusResumeState {
    difficulty_bits: u32,
    epoch_start_ms: u64,
    epoch_start_height: u64,
    btc_seed_hash: String,
    btc_checkpoint_tip_height: u64,
    last_block_timestamp_ms: u64,
    /// u128 chain-work total, hex-encoded (JSON numbers cannot hold u128 safely).
    chain_work_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChainStateFile {
    version: u32,
    network: String,
    genesis_hash: String,
    tip_hash: String,
    height: u64,
    updated_at: u64,
    #[serde(flatten)]
    resume: ConsensusResumeState,
}

#[derive(Debug, Deserialize)]
struct SignedTransactionPayload {
    from: String,
    to: String,
    amount: u64,
    nonce: u64,
    fee: u64,
    public_key: String,
    signature: String,
}

impl SignedTransactionPayload {
    fn into_signed_transaction(self) -> anyhow::Result<SignedTransaction> {
        let from = parse_address_hex(&self.from)?;
        let to = parse_address_hex(&self.to)?;
        let public_key = hex::decode(&self.public_key)
            .map_err(|e| anyhow::anyhow!("public_key is not valid hex: {e}"))?;
        let signature = hex::decode(&self.signature)
            .map_err(|e| anyhow::anyhow!("signature is not valid hex: {e}"))?;
        Ok(SignedTransaction {
            tx: Transaction::new(from, to, self.amount, self.nonce, self.fee),
            public_key,
            signature,
        })
    }
}

#[derive(Debug, Deserialize)]
struct LocalSendRequest {
    to: String,
    amount: u64,
    fee: Option<u64>,
    nonce: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct PeerConnectRequest {
    address: String,
}

/// Wire format for `POST /chain/submit` and the gossip `BlockClaim` message:
/// a signed claim plus its Bitcoin-checkpoint proof, when the claim targets
/// a checkpoint height (see `l1_core::bitcoin_entropy::is_checkpoint_height`).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClaimSubmission {
    claim: SignedBlockClaim,
    #[serde(default)]
    btc_checkpoint: Option<BtcCheckpointProof>,
    #[serde(default)]
    transactions: Vec<SignedTransaction>,
}

#[derive(Debug, Clone)]
struct LoadedChainState {
    genesis_hash: [u8; 32],
    tip_hash: [u8; 32],
    height: u64,
    created: bool,
    resume: ConsensusResumeState,
}

impl ClaimReject {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

// ─── HTTP handlers ─────────────────────────────────────────────────────────

async fn handle_status(
    State(s): State<SharedState>,
    Extension(store): Extension<Arc<BlockStore>>,
) -> Json<serde_json::Value> {
    let s = s.read().unwrap();
    let height = s.chain_height;
    let uptime = s.started_at.elapsed().as_secs();
    let recent = store.get_range(height.saturating_sub(19), height, 20).unwrap_or_default();
    let (block_time_avg_ms, block_time_stddev_ms) = rolling_block_time_stats(&recent, 20);
    let btc_seed_age_secs = unix_secs().saturating_sub(s.btc_seed_changed_at);
    let avg_mhs = if s.total_time_ms > 0 {
        (s.total_hashes as f64 / 1e6) / (s.total_time_ms as f64 / 1e3)
    } else {
        0.0
    };
    Json(serde_json::json!({
        "status":             "running",
        "height":             height,
        "difficulty_bits":    s.difficulty_bits,
        "current_reward_bina": s.current_reward,
        "total_mined_bina":   s.total_mined_bina,
        "supply_remaining":   HARD_CAP.saturating_sub(s.total_mined_bina),
        "hard_cap":           HARD_CAP,
        "halving_interval":   HALVING_INTERVAL,
        "initial_reward":     INITIAL_BLOCK_REWARD,
        "total_hashes":       s.total_hashes,
        "avg_hashrate_mhs":   format!("{:.2}", avg_mhs),
        "uptime_secs":        uptime,
        "threads":            s.threads,
        "miner_address":      s.miner_address,
        // "btc_height"/"btc_tip" are kept as aliases of the live-observed
        // Bitcoin state for dashboard compatibility; the pinned consensus
        // value miners must actually match is "btc_checkpoint_tip_height".
        "btc_height":         s.last_observed_btc.tip_height,
        "btc_tip":            hex::encode(&s.last_observed_btc.tip_hash[..8]),
        "btc_observed_height": s.last_observed_btc.tip_height,
        "btc_observed_tip":   hex::encode(&s.last_observed_btc.tip_hash[..8]),
        "btc_checkpoint_tip_height": s.btc_checkpoint_tip_height,
        "btc_tip_divergence_height": s.last_observed_btc.fork_detected.then_some(s.last_observed_btc.tip_height),
        "btc_seed_age_secs":  btc_seed_age_secs,
        "btc_seed_changed_at": s.btc_seed_changed_at,
        "btc_fork_seen":      s.last_observed_btc.fork_detected,
        "btc_checkpoint_interval": BTC_CHECKPOINT_INTERVAL,
        "block_time_avg_ms":  block_time_avg_ms,
        "block_time_stddev_ms": block_time_stddev_ms,
        "nullifiers_spent":   s.nullifiers.len(),
        "genesis_hash":       hex::encode(s.genesis_hash),
        "tip_hash":           hex::encode(s.tip_hash),
        "chain_work":         format!("{:x}", s.chain_work),
        "last_difficulty_adjustment": s.last_adjustment,
        "mempool_size":       s.mempool.len(),
        "reorg_depth_limit":  MAX_AUTO_REORG_DEPTH,
        "difficulty_epoch_size": l1_core::difficulty::EPOCH_SIZE,
        "block_store":        "sqlite-persistent",
    }))
}

async fn handle_latest_block(
    State(s): State<SharedState>,
    Extension(store): Extension<Arc<BlockStore>>,
) -> Result<Json<BlockRecord>, StatusCode> {
    let height = s.read().unwrap().chain_height;
    match store.get(height).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? {
        Some(b) => Ok(Json(b)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn handle_block(
    Extension(store): Extension<Arc<BlockStore>>,
    Path(height): Path<u64>,
) -> Result<Json<BlockRecord>, StatusCode> {
    match store.get(height).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? {
        Some(b) => Ok(Json(b)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn handle_blocks_recent(
    State(s): State<SharedState>,
    Extension(store): Extension<Arc<BlockStore>>,
) -> Json<Vec<BlockRecord>> {
    let height = s.read().unwrap().chain_height;
    let from = height.saturating_sub(19);
    let mut recent = store.get_range(from, height, 20).unwrap_or_default();
    recent.reverse();
    Json(recent)
}

/// Bounded-range block header sync — lets a lagging or newly-joined peer
/// catch up. Backed by the durable block store, so it serves this node's
/// entire history, not just what has been mined since its last restart.
async fn handle_chain_headers(
    Extension(store): Extension<Arc<BlockStore>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, u64>>,
) -> Json<Vec<BlockRecord>> {
    let from = params.get("from").copied().unwrap_or(0);
    let to = params
        .get("to")
        .copied()
        .unwrap_or(u64::MAX)
        .min(from.saturating_add(MAX_SYNC_PAGE as u64));
    Json(store.get_range(from, to, MAX_SYNC_PAGE).unwrap_or_default())
}

async fn handle_randomness_latest(
    State(s): State<SharedState>,
    Extension(store): Extension<Arc<BlockStore>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let height = s.read().unwrap().chain_height;
    match store.get(height).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? {
        Some(b) => Ok(Json(serde_json::json!({
            "height":           b.height,
            "output":           b.randomness_output,
            "nullifier":        b.nullifier,
            "block_hash":       b.block_hash,
            "btc_seed":         b.btc_seed,
            "btc_height":       b.btc_height,
            "verified":         true,
            "non_double_spend": true,
        }))),
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn handle_randomness_at(
    Extension(store): Extension<Arc<BlockStore>>,
    Path(height): Path<u64>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match store.get(height).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? {
        Some(b) => Ok(Json(serde_json::json!({
            "height":           b.height,
            "output":           b.randomness_output,
            "nullifier":        b.nullifier,
            "block_hash":       b.block_hash,
            "btc_seed":         b.btc_seed,
            "btc_height":       b.btc_height,
            "verified":         true,
            "non_double_spend": true,
        }))),
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn handle_submit_claim(
    State(s): State<SharedState>,
    Extension(gossip): Extension<Arc<Gossip>>,
    Extension(ledger): Extension<SharedLedger>,
    Json(submission): Json<ClaimSubmission>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let accepted = accept_signed_claim(
        &s,
        &ledger,
        submission.claim.clone(),
        submission.btc_checkpoint.clone(),
        submission.transactions.clone(),
    )
    .map_err(claim_error_response)?;
    let envelope = BlockClaimEnvelope::from_claim(
        gossip.network().to_string(),
        DEFAULT_P2P_TTL,
        submission.claim,
        submission.btc_checkpoint,
        submission.transactions,
    );
    gossip.broadcast_claim(envelope).await;

    Ok(Json(serde_json::json!({
        "status": "accepted",
        "height": accepted.height,
        "miner_address": accepted.miner_hex,
        "block_hash": accepted.block_hash,
        "work_bits": accepted.work_bits,
        "election_score": accepted.election_score,
        "candidate_window_ms": SUBMISSION_GRACE_MS,
        "broadcast": true,
    })))
}

/// Structural + best-effort validation against the ledger's current
/// (last-confirmed) state, then queue into the mempool. This is NOT the
/// authoritative check — a transaction only really takes effect once a
/// miner includes it in a block and every node re-validates it against the
/// exact parent state that block executes against (see
/// `l1_core::rewards::simulate_block_execution`). This gate exists purely
/// to reject obviously-bad submissions before they occupy mempool space.
///
/// KNOWN ALPHA LIMITATION: the mempool is entirely local — there is no
/// gossip propagating a submitted transaction to other nodes, no
/// deduplication/TTL/fee-ordering policy beyond FIFO-ish insertion order,
/// and no eviction beyond the blunt `MAX_MEMPOOL_SIZE` cap. A transaction
/// only ever gets mined if it reaches an active miner directly (e.g. via
/// that miner's own `/tx/submit`). The next real feature here is mempool
/// gossip (propagate + dedupe by tx_id + a TTL so stale entries don't pin
/// memory forever), then fee-based selection when building a candidate
/// block instead of the current "whatever's in the map" order.
fn enqueue_transaction(state: &SharedState, ledger: &SharedLedger, signed: SignedTransaction) -> Result<(), String> {
    signed.verify().map_err(|e| format!("transaction rejected: {e}"))?;

    let ledger = ledger.lock().map_err(|_| "ledger lock poisoned".to_string())?;
    let from = signed.from_hex();
    let expected_nonce = ledger.nonce(&from);
    if signed.tx.nonce != expected_nonce {
        return Err(format!("bad transaction nonce: expected {expected_nonce}, got {}", signed.tx.nonce));
    }
    let debit_total = signed
        .tx
        .amount
        .checked_add(signed.tx.fee)
        .ok_or_else(|| "transaction amount + fee overflow".to_string())?;
    let balance = ledger.balance(&from);
    if balance < debit_total {
        return Err(format!("insufficient balance: have {balance}, need {debit_total}"));
    }
    drop(ledger);

    let mut s = state.write().unwrap();
    if s.mempool.len() >= MAX_MEMPOOL_SIZE && !s.mempool.contains_key(&signed.tx_id()) {
        return Err("mempool is full, try again shortly".to_string());
    }
    s.mempool.insert(signed.tx_id(), signed);
    Ok(())
}

async fn handle_submit_transaction(
    State(s): State<SharedState>,
    Extension(ledger): Extension<SharedLedger>,
    Json(payload): Json<SignedTransactionPayload>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let signed = payload
        .into_signed_transaction()
        .map_err(|e| json_error(StatusCode::BAD_REQUEST, format!("invalid transaction payload: {e}")))?;
    let signature_mode = if signed.public_key.len() == ED25519_PUBLIC_KEY_BYTES {
        "ed25519-only"
    } else {
        "hybrid"
    };
    let tx_id = signed.tx_id_hex();
    let (from, to, amount, fee, nonce) =
        (signed.from_hex(), signed.to_hex(), signed.tx.amount, signed.tx.fee, signed.tx.nonce);
    enqueue_transaction(&s, &ledger, signed)
        .map_err(|e| json_error(StatusCode::BAD_REQUEST, e))?;

    Ok(Json(serde_json::json!({
        "status": "queued",
        "tx_id": tx_id,
        "from": from,
        "to": to,
        "amount": amount,
        "fee": fee,
        "nonce": nonce,
        "signature_mode": signature_mode,
        "note": "transaction is pending in the mempool; it takes effect once a miner includes it in a confirmed block",
    })))
}

async fn handle_wallet_send(
    State(s): State<SharedState>,
    Extension(ledger): Extension<SharedLedger>,
    Json(payload): Json<LocalSendRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let to = parse_address_hex(&payload.to)
        .map_err(|e| json_error(StatusCode::BAD_REQUEST, format!("bad recipient address: {e}")))?;
    let fee = payload.fee.unwrap_or(0);
    let miner_keypair = load_miner_keypair();
    let from = miner_keypair.address();
    let from_hex = hex::encode(from);
    let nonce = {
        let ledger = ledger
            .lock()
            .map_err(|_| json_error(StatusCode::INTERNAL_SERVER_ERROR, "ledger lock poisoned"))?;
        payload.nonce.unwrap_or_else(|| ledger.nonce(&from_hex))
    };
    let tx = Transaction::new(from, to, payload.amount, nonce, fee);
    let signed = SignedTransaction::sign(tx, &miner_keypair)
        .map_err(|e| json_error(StatusCode::BAD_REQUEST, format!("transaction signing failed: {e}")))?;

    let tx_id = signed.tx_id_hex();
    let tx_digest = signed.tx.digest_hex();
    let public_key = signed.public_key_hex();
    let signature = signed.signature_hex();
    enqueue_transaction(&s, &ledger, signed)
        .map_err(|e| json_error(StatusCode::BAD_REQUEST, e))?;

    Ok(Json(serde_json::json!({
        "status": "queued",
        "tx_id": tx_id,
        "from": from_hex,
        "to": payload.to,
        "amount": payload.amount,
        "fee": fee,
        "nonce": nonce,
        "tx_digest": tx_digest,
        "public_key": public_key,
        "signature": signature,
        "signature_mode": "hybrid",
        "note": "transaction is pending in the mempool; it takes effect once a miner includes it in a confirmed block",
    })))
}

async fn handle_p2p_message(
    State(s): State<SharedState>,
    Extension(gossip): Extension<Arc<Gossip>>,
    Extension(ledger): Extension<SharedLedger>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    Json(message): Json<BinaMessage>,
) -> impl IntoResponse {
    let Some(message) = gossip.handle_incoming(message, peer_addr).await else {
        return StatusCode::OK;
    };

    if let BinaMessage::BlockClaim(envelope) = message {
        match accept_signed_claim(
            &s,
            &ledger,
            envelope.claim.clone(),
            envelope.btc_checkpoint.clone(),
            envelope.transactions.clone(),
        ) {
            Ok(accepted) => {
                println!(
                    "[p2p] accepted claim from {} height={} hash={}…",
                    peer_addr,
                    accepted.height,
                    &accepted.block_hash[..12]
                );
                if envelope.ttl > 0 {
                    gossip
                        .relay_message(BinaMessage::BlockClaim(envelope), peer_addr)
                        .await;
                }
            }
            Err(e) => eprintln!("[p2p] rejected claim from {}: {}", peer_addr, e.message),
        }
    }

    StatusCode::OK
}

async fn handle_get_peers(Extension(gossip): Extension<Arc<Gossip>>) -> Json<Vec<String>> {
    Json(
        gossip
            .peers()
            .all()
            .into_iter()
            .map(|addr| addr.to_string())
            .collect(),
    )
}

async fn handle_peer_hello(
    Extension(gossip): Extension<Arc<Gossip>>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    Json(hello): Json<PeerHelloEnvelope>,
) -> StatusCode {
    if hello.network != gossip.network() {
        return StatusCode::BAD_REQUEST;
    }
    match hello.listen_addr.parse() {
        Ok(addr) => {
            gossip.peers().add(addr);
            println!("[p2p] hello from {} listen={}", peer_addr, addr);
            StatusCode::OK
        }
        Err(_) => StatusCode::BAD_REQUEST,
    }
}

async fn handle_peer_connect(
    State(s): State<SharedState>,
    Extension(gossip): Extension<Arc<Gossip>>,
    Json(payload): Json<PeerConnectRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let addr: SocketAddr = payload
        .address
        .trim()
        .parse()
        .map_err(|e| json_error(StatusCode::BAD_REQUEST, format!("invalid peer address: {e}")))?;
    let (best_height, best_hash) = {
        let state = s.read().unwrap();
        (state.chain_height, hex::encode(state.tip_hash))
    };
    let listen_addr = std::env::var("BINA_P2P_LISTEN_ADDR")
        .unwrap_or_else(|_| format!("127.0.0.1:{}", http_port()));
    let hello = PeerHelloEnvelope {
        network: gossip.network().to_string(),
        version: 1,
        best_height,
        best_hash,
        listen_addr,
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(1_200))
        .build()
        .map_err(|e| json_error(StatusCode::INTERNAL_SERVER_ERROR, format!("HTTP client error: {e}")))?;
    let hello_url = format!("http://{addr}/p2p/hello");
    let response = client
        .post(&hello_url)
        .json(&hello)
        .send()
        .await
        .map_err(|e| json_error(StatusCode::BAD_GATEWAY, format!("peer hello failed: {e}")))?;
    if !response.status().is_success() {
        return Err(json_error(
            StatusCode::BAD_GATEWAY,
            format!("peer rejected hello with HTTP {}", response.status()),
        ));
    }

    gossip.peers().add(addr);
    let peers_url = format!("http://{addr}/p2p/peers");
    if let Ok(response) = client.get(&peers_url).send().await {
        if let Ok(peers) = response.json::<Vec<String>>().await {
            for peer in peers.into_iter().filter_map(|peer| peer.parse().ok()) {
                gossip.peers().add(peer);
            }
        }
    }

    Ok(Json(serde_json::json!({
        "status": "connected",
        "peer": addr.to_string(),
        "known_peers": gossip.peers().count(),
    })))
}

fn accept_signed_claim(
    state: &SharedState,
    ledger: &SharedLedger,
    claim: SignedBlockClaim,
    btc_checkpoint: Option<BtcCheckpointProof>,
    transactions: Vec<SignedTransaction>,
) -> Result<AcceptedClaim, ClaimReject> {
    claim.verify().map_err(|e| {
        ClaimReject::new(
            StatusCode::BAD_REQUEST,
            format!("invalid signed claim: {e}"),
        )
    })?;

    if transactions.len() > l1_core::transaction::MAX_TXS_PER_BLOCK {
        return Err(ClaimReject::new(
            StatusCode::BAD_REQUEST,
            format!("claim carries {} transactions, over the {} limit", transactions.len(), l1_core::transaction::MAX_TXS_PER_BLOCK),
        ));
    }
    if l1_core::transaction::merkle_root(&transactions) != claim.header.merkle_root {
        return Err(ClaimReject::new(
            StatusCode::BAD_REQUEST,
            "claim transaction list does not match its header merkle_root",
        ));
    }

    let height = claim.header.height;
    let miner_hex = claim.miner_address_hex();
    let block_hash = claim.block_hash_hex();
    let election_score = claim.election_score_hex();
    let work_bits = claim.work_bits();

    // Phase 1: structural checks against shared node state.
    let reward = {
        let state = state.read().unwrap();
        if state.genesis_hash == [0u8; 32] {
            return Err(ClaimReject::new(StatusCode::SERVICE_UNAVAILABLE, "genesis not initialized"));
        }
        let next_height = state.chain_height + 1;
        if height != next_height {
            return Err(ClaimReject::new(
                StatusCode::CONFLICT,
                format!("claim height {height} does not match next height {next_height}"),
            ));
        }
        if claim.header.prev_hash != state.tip_hash {
            return Err(ClaimReject::new(StatusCode::CONFLICT, "claim prev_hash does not match chain tip"));
        }
        if claim.header.difficulty_bits != state.difficulty_bits {
            return Err(ClaimReject::new(
                StatusCode::CONFLICT,
                "claim difficulty does not match the deterministic difficulty for this height",
            ));
        }
        if !timestamp_is_valid(claim.header.timestamp, state.last_block_timestamp_ms, unix_ms(), MAX_FUTURE_MS) {
            return Err(ClaimReject::new(
                StatusCode::BAD_REQUEST,
                "claim timestamp must be after the previous block and not too far in the future",
            ));
        }

        if is_checkpoint_height(height) {
            let proof = btc_checkpoint.as_ref().ok_or_else(|| {
                ClaimReject::new(StatusCode::BAD_REQUEST, "height requires a Bitcoin checkpoint proof")
            })?;
            if proof.seed_hash() != claim.header.bitcoin_seed_hash {
                return Err(ClaimReject::new(
                    StatusCode::BAD_REQUEST,
                    "checkpoint proof does not match the claim's bitcoin_seed_hash",
                ));
            }
            let observed_tip_height = state.last_observed_btc.tip_height;
            if !proof.plausible(state.btc_checkpoint_tip_height, observed_tip_height, BTC_HEIGHT_TOLERANCE) {
                return Err(ClaimReject::new(
                    StatusCode::CONFLICT,
                    format!(
                        "checkpoint proof not plausible: claimed btc height {}, last checkpoint {}, locally observed {}",
                        proof.tip_height, state.btc_checkpoint_tip_height, observed_tip_height
                    ),
                ));
            }
        } else if claim.header.bitcoin_seed_hash != state.btc_seed_hash {
            return Err(ClaimReject::new(
                StatusCode::CONFLICT,
                "claim Bitcoin seed does not match the seed pinned at the last checkpoint",
            ));
        }

        block_reward(height, state.total_mined_bina)
    };

    // Phase 2: re-execute the claimed transactions against our own copy of
    // the parent state (guaranteed identical to the miner's, since
    // prev_hash matched above) and confirm the miner's claimed state_root
    // is exactly what that execution produces. This is what makes the
    // transaction list trustworthy without trusting the miner at all.
    {
        let ledger = ledger.lock().map_err(|_| {
            ClaimReject::new(StatusCode::INTERNAL_SERVER_ERROR, "ledger lock poisoned")
        })?;
        let (applied, computed_state_root) =
            l1_core::rewards::simulate_block_execution(&ledger, &transactions, &miner_hex, reward);
        if applied.len() != transactions.len() {
            return Err(ClaimReject::new(
                StatusCode::BAD_REQUEST,
                "claim includes a transaction that does not validate against the parent state",
            ));
        }
        if computed_state_root != claim.header.state_root {
            return Err(ClaimReject::new(
                StatusCode::BAD_REQUEST,
                "claim state_root does not match re-executing its transactions against the parent state",
            ));
        }
    }

    // Phase 3: admit as a candidate for this height.
    let mut state = state.write().unwrap();
    let claims = state.pending_claims.entry(height).or_default();
    if claims.contains_key(&miner_hex) {
        return Err(ClaimReject::new(
            StatusCode::CONFLICT,
            "miner already submitted a claim for this height; first valid claim is kept",
        ));
    }
    claims.insert(miner_hex.clone(), PendingClaim { claim, btc_checkpoint, transactions });

    Ok(AcceptedClaim {
        height,
        miner_hex,
        block_hash,
        election_score,
        work_bits,
    })
}

// GET /wallet/:address/balance
async fn handle_wallet_balance(
    Extension(ledger): Extension<SharedLedger>,
    Path(address): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let ledger = ledger
        .lock()
        .map_err(|_| json_error(StatusCode::INTERNAL_SERVER_ERROR, "ledger lock poisoned"))?;
    let balance = ledger.balance(&address);
    let nonce = ledger.nonce(&address);
    Ok(Json(serde_json::json!({
        "address":          address,
        "balance_bina":     balance,
        "next_nonce":       nonce,
        "source":           ledger_path(),
        "note":             "persistent ledger balance",
    })))
}

// GET /chain/supply
async fn handle_supply(State(s): State<SharedState>) -> Json<serde_json::Value> {
    let state = s.read().unwrap();
    let era = state
        .chain_height
        / HALVING_INTERVAL;
    Json(serde_json::json!({
        "total_mined_bina":     state.total_mined_bina,
        "supply_remaining":     HARD_CAP.saturating_sub(state.total_mined_bina),
        "hard_cap":             HARD_CAP,
        "current_reward_bina":  state.current_reward,
        "current_era":          era,
        "halving_interval":     HALVING_INTERVAL,
        "initial_reward":       INITIAL_BLOCK_REWARD,
        "next_halving_at":      (era + 1) * HALVING_INTERVAL,
        "difficulty_bits":      state.difficulty_bits,
        "last_adjustment":      state.last_adjustment,
    }))
}

fn json_error(
    status: StatusCode,
    message: impl Into<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(serde_json::json!({ "error": message.into() })))
}

fn claim_error_response(error: ClaimReject) -> (StatusCode, Json<serde_json::Value>) {
    json_error(error.status, error.message)
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Validator's own wall clock in Unix ms — used only as the upper bound in
/// `timestamp_is_valid`, never as a consensus value itself.
fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn rolling_block_time_stats(blocks: &[BlockRecord], window: usize) -> (u64, u64) {
    let values: Vec<f64> = blocks
        .iter()
        .rev()
        .take(window)
        .filter_map(|block| (block.elapsed_ms > 0).then_some(block.elapsed_ms as f64))
        .collect();
    if values.is_empty() {
        return (0, 0);
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance = values
        .iter()
        .map(|value| {
            let delta = value - mean;
            delta * delta
        })
        .sum::<f64>()
        / values.len() as f64;
    (mean.round() as u64, variance.sqrt().round() as u64)
}

fn hex_to_32(label: &str, value: &str) -> [u8; 32] {
    let bytes = hex::decode(value).unwrap_or_else(|e| panic!("{label} is not valid hex: {e}"));
    if bytes.len() != 32 {
        panic!("{label} must be 32 bytes / 64 hex chars, got {} bytes", bytes.len());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    out
}

fn chain_state_file(
    genesis_hash: [u8; 32],
    tip_hash: [u8; 32],
    height: u64,
    resume: ConsensusResumeState,
) -> ChainStateFile {
    ChainStateFile {
        version: CHAIN_STATE_VERSION,
        network: NETWORK_ID.to_string(),
        genesis_hash: hex::encode(genesis_hash),
        tip_hash: hex::encode(tip_hash),
        height,
        updated_at: unix_secs(),
        resume,
    }
}

fn save_chain_state(
    path: &str,
    genesis_hash: [u8; 32],
    tip_hash: [u8; 32],
    height: u64,
    resume: ConsensusResumeState,
) -> std::io::Result<()> {
    let path = std::path::Path::new(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = chain_state_file(genesis_hash, tip_hash, height, resume);
    let text = serde_json::to_string_pretty(&file).expect("chain state serialization cannot fail");
    std::fs::write(path, text)
}

fn genesis_resume_state(genesis_timestamp_ms: u64) -> ConsensusResumeState {
    ConsensusResumeState {
        difficulty_bits: l1_core::difficulty::MIN_BITS,
        epoch_start_ms: genesis_timestamp_ms,
        epoch_start_height: 0,
        btc_seed_hash: hex::encode([0u8; 32]),
        btc_checkpoint_tip_height: 0,
        last_block_timestamp_ms: genesis_timestamp_ms,
        chain_work_hex: "0".to_string(),
    }
}

fn load_or_create_chain_state(path: &str, genesis_hash: [u8; 32], genesis_timestamp_ms: u64) -> LoadedChainState {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let file: ChainStateFile = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{path} is not valid chain state JSON: {e}"));
            if file.version != CHAIN_STATE_VERSION {
                panic!(
                    "unsupported chain state version {} in {path} (expected {CHAIN_STATE_VERSION}). \
                     Move {path} aside to resync from genesis under the current consensus rules.",
                    file.version
                );
            }
            if file.network != NETWORK_ID {
                panic!("chain state network mismatch: expected {NETWORK_ID}, found {}", file.network);
            }
            let stored_genesis = hex_to_32("chain_state.genesis_hash", &file.genesis_hash);
            if stored_genesis != genesis_hash {
                panic!(
                    "chain state genesis mismatch: expected {}, found {}. Move {path} aside before joining a different chain.",
                    hex::encode(genesis_hash),
                    file.genesis_hash
                );
            }
            LoadedChainState {
                genesis_hash: stored_genesis,
                tip_hash: hex_to_32("chain_state.tip_hash", &file.tip_hash),
                height: file.height,
                created: false,
                resume: file.resume,
            }
        }
        Err(_) => {
            let resume = genesis_resume_state(genesis_timestamp_ms);
            save_chain_state(path, genesis_hash, genesis_hash, 0, resume.clone())
                .unwrap_or_else(|e| panic!("failed to create {path}: {e}"));
            LoadedChainState {
                genesis_hash,
                tip_hash: genesis_hash,
                height: 0,
                created: true,
                resume,
            }
        }
    }
}

// ─── Chain sync (one-shot catch-up before mining starts) ──────────────────

/// A peer's self-reported chain status, used to pick a sync source.
#[derive(Debug, Clone)]
struct PeerStatus {
    addr: SocketAddr,
    height: u64,
    genesis_hash: [u8; 32],
    chain_work: u128,
}

async fn fetch_peer_status(client: &reqwest::Client, addr: SocketAddr) -> Option<PeerStatus> {
    let url = format!("http://{addr}/chain/status");
    let json: serde_json::Value = client.get(&url).send().await.ok()?.json().await.ok()?;
    let genesis_hash = l1_core::bitcoin_entropy::hex_to_32(json.get("genesis_hash")?.as_str()?).ok()?;
    let chain_work = u128::from_str_radix(json.get("chain_work")?.as_str()?, 16).ok()?;
    let height = json.get("height")?.as_u64()?;
    Some(PeerStatus { addr, height, genesis_hash, chain_work })
}

/// Reconstruct the header a `BlockRecord` claims to represent, given the
/// previous block's hash. `version` and `merkle_root` are not carried by
/// `BlockRecord` because they are presently consensus constants (no
/// transactions are bound into blocks yet) — this must be revisited if that
/// changes.
fn header_from_record(record: &BlockRecord, prev_hash: [u8; 32]) -> anyhow::Result<L1BlockHeader> {
    let miner_address = parse_address_hex(&record.miner_address)?;
    let bitcoin_seed_hash = l1_core::bitcoin_entropy::hex_to_32(&record.btc_seed)?;
    let merkle_root = l1_core::bitcoin_entropy::hex_to_32(&record.merkle_root)?;
    let state_root = l1_core::bitcoin_entropy::hex_to_32(&record.state_root)?;
    Ok(L1BlockHeader {
        version: 1,
        height: record.height,
        prev_hash,
        merkle_root,
        state_root,
        timestamp: record.timestamp,
        nonce: record.nonce,
        miner_address,
        difficulty_bits: record.difficulty_bits,
        bitcoin_seed_hash,
    })
}

/// Fetch and apply a peer's block history strictly forward from `*height`,
/// up to `target_height`. Every record is independently validated (chain
/// linkage, timestamp, deterministic difficulty, checkpoint continuity,
/// signature + PoW) before being applied — this never trusts the peer's
/// framing, only what each record cryptographically proves.
///
/// Historical checkpoint blocks are validated for signature + PoW +
/// difficulty + timestamp + chain-linkage, but NOT re-checked for Bitcoin
/// plausibility against this node's live observation the way a fresh live
/// claim is (see `BtcCheckpointProof::plausible`) — for buried history,
/// trust comes from the accumulated PoW built on top, the same way any
/// other historical chain data is trusted, not from re-fetching live oracle
/// state for blocks that are long past.
///
/// Each record's bound transactions are re-executed against this node's own
/// ledger state before being trusted — a peer cannot hand over a state_root
/// without also handing over a transaction list that actually produces it.
#[allow(clippy::too_many_arguments)]
async fn sync_forward_from_peer(
    client: &reqwest::Client,
    peer_addr: SocketAddr,
    target_height: u64,
    state: &SharedState,
    ledger: &SharedLedger,
    store: &BlockStore,
    height: &mut u64,
    tip_hash: &mut [u8; 32],
    last_block_timestamp_ms: &mut u64,
    btc_seed_hash: &mut [u8; 32],
    btc_checkpoint_tip_height: &mut u64,
    chain_work: &mut u128,
    adjuster: &mut DifficultyAdjuster,
) -> u64 {
    let mut synced = 0u64;
    loop {
        if *height >= target_height {
            break;
        }
        let from = *height + 1;
        let to = from.saturating_add(MAX_SYNC_PAGE as u64 - 1);
        let url = format!("http://{peer_addr}/chain/headers?from={from}&to={to}");
        let page: Vec<BlockRecord> = match client.get(&url).send().await {
            Ok(resp) => match resp.json().await {
                Ok(records) => records,
                Err(e) => {
                    eprintln!("[sync] peer {peer_addr} sent invalid headers page: {e}");
                    break;
                }
            },
            Err(e) => {
                eprintln!("[sync] peer {peer_addr} unreachable during sync: {e}");
                break;
            }
        };
        if page.is_empty() {
            eprintln!("[sync] peer {peer_addr} has no more headers to offer at height {from} — stopping short of its reported tip");
            break;
        }

        let mut stop = false;
        for record in &page {
            if record.height != *height + 1 {
                eprintln!("[sync] peer {peer_addr} sent out-of-sequence height {} (expected {})", record.height, *height + 1);
                stop = true;
                break;
            }
            if record.prev_hash != hex::encode(*tip_hash) {
                eprintln!("[sync] peer {peer_addr} record at height {} does not chain from our tip", record.height);
                stop = true;
                break;
            }
            let header = match header_from_record(record, *tip_hash) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("[sync] peer {peer_addr} record at height {} malformed: {e}", record.height);
                    stop = true;
                    break;
                }
            };
            if !timestamp_is_valid(header.timestamp, *last_block_timestamp_ms, unix_ms(), MAX_FUTURE_MS) {
                eprintln!("[sync] peer {peer_addr} record at height {} has an invalid timestamp", record.height);
                stop = true;
                break;
            }
            if header.difficulty_bits != adjuster.current_bits() {
                eprintln!(
                    "[sync] peer {peer_addr} record at height {} difficulty {} does not match expected {}",
                    record.height, header.difficulty_bits, adjuster.current_bits()
                );
                stop = true;
                break;
            }
            if !is_checkpoint_height(record.height) {
                let expected_seed = hex::encode(*btc_seed_hash);
                if record.btc_seed != expected_seed {
                    eprintln!("[sync] peer {peer_addr} record at height {} has a Bitcoin seed that does not match the pinned checkpoint", record.height);
                    stop = true;
                    break;
                }
            }

            let public_key = match hex::decode(&record.miner_public_key) {
                Ok(b) => b,
                Err(_) => { stop = true; break; }
            };
            let signature = match hex::decode(&record.miner_signature) {
                Ok(b) => b,
                Err(_) => { stop = true; break; }
            };
            let claim = SignedBlockClaim { header: header.clone(), public_key, signature };
            if let Err(e) = claim.verify() {
                eprintln!("[sync] peer {peer_addr} record at height {} failed verification: {e}", record.height);
                stop = true;
                break;
            }
            if claim.block_hash_hex() != record.block_hash {
                eprintln!("[sync] peer {peer_addr} record at height {} hash mismatch", record.height);
                stop = true;
                break;
            }
            if l1_core::transaction::merkle_root(&record.transactions) != header.merkle_root {
                eprintln!("[sync] peer {peer_addr} record at height {} transaction list does not match its merkle_root", record.height);
                stop = true;
                break;
            }
            if record.transactions.len() > l1_core::transaction::MAX_TXS_PER_BLOCK {
                eprintln!("[sync] peer {peer_addr} record at height {} carries too many transactions", record.height);
                stop = true;
                break;
            }

            // Re-execute the claimed transactions against our own copy of
            // the parent state before trusting the claimed state_root.
            let (applied, computed_state_root) = {
                let ledger = ledger.lock().unwrap();
                l1_core::rewards::simulate_block_execution(&ledger, &record.transactions, &record.miner_address, record.reward_bina)
            };
            if applied.len() != record.transactions.len() || computed_state_root != header.state_root {
                eprintln!("[sync] peer {peer_addr} record at height {} state_root does not match re-executing its transactions", record.height);
                stop = true;
                break;
            }

            // Apply.
            let block_hash = claim.block_hash();
            {
                let mut ledger = ledger.lock().unwrap();
                for tx in &record.transactions {
                    if let Err(e) = ledger.apply_transaction(record.height, tx, &record.miner_address, record.timestamp / 1000) {
                        eprintln!("[sync] BUG: pre-validated transaction {} failed to apply: {e}", tx.tx_id_hex());
                    }
                }
                let _ = ledger.credit(record.height, &record.miner_address, record.reward_bina, record.timestamp / 1000);
            }
            if let Err(e) = store.insert_block(record) {
                eprintln!("[sync] failed to persist synced block {}: {e}", record.height);
            }
            adjuster.record_block(record.height, header.timestamp);
            if is_checkpoint_height(record.height) {
                *btc_seed_hash = header.bitcoin_seed_hash;
                *btc_checkpoint_tip_height = record.btc_height;
            }
            // Work is counted from the *required* difficulty, not the
            // winning hash's actual leading-zero count — otherwise a single
            // lucky block would count for far more than its real expected
            // cost, letting a chain claim disproportionate cumulative work.
            *chain_work = chain_work.saturating_add(1u128 << header.difficulty_bits.min(127));
            *last_block_timestamp_ms = header.timestamp;
            *tip_hash = block_hash;
            *height = record.height;
            synced += 1;

            {
                let mut s = state.write().unwrap();
                s.chain_height = *height;
                s.tip_hash = *tip_hash;
                s.difficulty_bits = adjuster.current_bits();
                s.btc_seed_hash = *btc_seed_hash;
                s.btc_checkpoint_tip_height = *btc_checkpoint_tip_height;
                s.last_block_timestamp_ms = *last_block_timestamp_ms;
                s.chain_work = *chain_work;
                s.total_mined_bina = ledger.lock().unwrap().total_mined();
                // Confirmed via sync — drop from our own mempool if present.
                for tx in &applied {
                    s.mempool.remove(&tx.tx_id());
                }
                // Drop any now-stale pending candidates at or below the new tip.
                s.pending_claims.retain(|candidate_height, _| *candidate_height > *height);
            }
        }
        if stop {
            break;
        }
    }
    synced
}

/// Query every known peer's `/chain/status`, keeping only those on our
/// genesis whose reported cumulative work exceeds `local_work`.
async fn best_peer_by_work(client: &reqwest::Client, gossip: &Gossip, local_genesis: [u8; 32], local_work: u128) -> Option<PeerStatus> {
    let mut statuses = Vec::new();
    for peer in gossip.peers().all() {
        if let Some(status) = fetch_peer_status(client, peer).await {
            statuses.push(status);
        }
    }
    select_best_peer(statuses, local_genesis, local_work)
}

/// Pure fork-choice rule: the heaviest reported chain on our own genesis
/// that is strictly heavier than what we already have. Split out from
/// `best_peer_by_work` so the rule itself — not the network call — is
/// directly unit-testable.
fn select_best_peer(statuses: Vec<PeerStatus>, local_genesis: [u8; 32], local_work: u128) -> Option<PeerStatus> {
    statuses
        .into_iter()
        .filter(|p| p.genesis_hash == local_genesis && p.chain_work > local_work)
        .max_by_key(|p| p.chain_work)
}

/// One-shot startup catch-up: if a known peer on the same genesis reports
/// strictly more cumulative chain work than this node has, fetch and
/// replay its block history (bounded by what the peer still retains) before
/// mining begins, so a restarted or newly-joined node doesn't immediately
/// start extending a chain that the rest of the network has already
/// surpassed. See `sync_forward_from_peer` for validation details.
#[allow(clippy::too_many_arguments)]
async fn catch_up_from_peers(
    state: &SharedState,
    ledger: &SharedLedger,
    store: &BlockStore,
    gossip: &Gossip,
    height: &mut u64,
    tip_hash: &mut [u8; 32],
    last_block_timestamp_ms: &mut u64,
    btc_seed_hash: &mut [u8; 32],
    btc_checkpoint_tip_height: &mut u64,
    chain_work: &mut u128,
    adjuster: &mut DifficultyAdjuster,
) {
    let client = match reqwest::Client::builder().timeout(Duration::from_secs(5)).build() {
        Ok(c) => c,
        Err(_) => return,
    };
    let local_genesis = state.read().unwrap().genesis_hash;
    let Some(best) = best_peer_by_work(&client, gossip, local_genesis, *chain_work).await else {
        return;
    };

    println!(
        "[sync] peer {} reports heavier chain (work {:x} > {:x}, height {}) — catching up",
        best.addr, best.chain_work, *chain_work, best.height
    );

    let synced = sync_forward_from_peer(
        &client, best.addr, best.height, state, ledger, store,
        height, tip_hash, last_block_timestamp_ms, btc_seed_hash, btc_checkpoint_tip_height, chain_work, adjuster,
    ).await;

    println!("[sync] applied {synced} block(s) from {}, now at height {}", best.addr, *height);
}

/// Maximum depth (in blocks) a live reorg will roll back automatically.
/// Bounded to less than one difficulty epoch so the rollback never needs to
/// cross an already-applied epoch boundary — see `reconcile_fork` for why
/// that keeps the difficulty-adjuster rebuild trivially correct without a
/// full from-genesis replay. Deeper divergences are logged, not silently
/// resolved automatically: an operator should look at those.
///
/// KNOWN ALPHA LIMITATION: this bound is a deliberate scope cut, not a
/// hard protocol ceiling. A public network needs one of two things this
/// codebase does not have yet: (a) a difficulty-adjuster rebuild that can
/// safely replay across arbitrary epoch boundaries (removing the need for
/// this bound at all), or (b) explicit finality/checkpoint rules (e.g. "N
/// blocks buried = irreversible") so deep reorgs are rejected by protocol
/// rather than merely un-handled by this node's reconciler. Until one of
/// those lands, a deliberate or accidental fork deeper than
/// `MAX_AUTO_REORG_DEPTH` requires operator intervention to resolve.
const MAX_AUTO_REORG_DEPTH: u64 = l1_core::difficulty::EPOCH_SIZE - 1;

/// Re-admit every transaction bound into a rolled-back block so a reorg
/// never silently drops a transaction the network had accepted — it simply
/// becomes eligible for inclusion again, exactly like a fresh submission.
fn requeue_rolled_back_transactions(mempool: &mut HashMap<[u8; 32], SignedTransaction>, rolled_back: &[BlockRecord]) {
    for block in rolled_back {
        for tx in &block.transactions {
            mempool.insert(tx.tx_id(), tx.clone());
        }
    }
}

/// Continuously-running reorg reconciliation: if a peer's chain has more
/// cumulative work than ours *and* the two chains have actually diverged
/// (not just a peer being further ahead on the same history — that's plain
/// catch-up, handled the same way here), find the common ancestor within
/// the locally-retained block window, roll back to it, and adopt the
/// heavier branch. This is what lets two nodes that each mined a
/// competing tail (e.g. a gossip claim arrived a moment too late) converge
/// back onto one chain instead of silently forking forever.
#[allow(clippy::too_many_arguments)]
async fn reconcile_fork(
    state: &SharedState,
    ledger: &SharedLedger,
    ledger_path: &str,
    store: &BlockStore,
    gossip: &Gossip,
    height: &mut u64,
    tip_hash: &mut [u8; 32],
    last_block_timestamp_ms: &mut u64,
    btc_seed_hash: &mut [u8; 32],
    btc_checkpoint_tip_height: &mut u64,
    chain_work: &mut u128,
    adjuster: &mut DifficultyAdjuster,
) {
    let client = match reqwest::Client::builder().timeout(Duration::from_secs(5)).build() {
        Ok(c) => c,
        Err(_) => return,
    };
    let local_genesis = state.read().unwrap().genesis_hash;
    let Some(best) = best_peer_by_work(&client, gossip, local_genesis, *chain_work).await else {
        return;
    };
    if best.height < *height {
        // More work over fewer-or-equal blocks than us would mean we
        // disagree on the difficulty rules entirely — outside what this
        // reconciler can safely fix. Leave it for an operator to notice.
        eprintln!("[reorg] peer {} reports more work at a lower height ({} < {}) — not attempting to reconcile", best.addr, best.height, *height);
        return;
    }

    // Find the common ancestor by probing the peer's record at each height
    // going backward from our tip, compared against our own durable store,
    // bounded to MAX_AUTO_REORG_DEPTH. Both sides now hold full history (see
    // `store.rs`), so this is no longer limited by either node's uptime —
    // the depth bound below is purely for difficulty-adjuster safety.
    let probe_floor = height.saturating_sub(MAX_AUTO_REORG_DEPTH);
    let local_hash_at = |h: u64| -> Option<[u8; 32]> {
        store.block_hash_at(h).ok().flatten().and_then(|s| l1_core::bitcoin_entropy::hex_to_32(&s).ok())
    };

    let mut fork_point = *height;
    while fork_point > probe_floor {
        let peer_hash = match client
            .get(format!("http://{}/chain/headers?from={fork_point}&to={fork_point}", best.addr))
            .send()
            .await
        {
            Ok(resp) => match resp.json::<Vec<BlockRecord>>().await {
                Ok(records) => records.into_iter().find(|r| r.height == fork_point).and_then(|r| l1_core::bitcoin_entropy::hex_to_32(&r.block_hash).ok()),
                Err(_) => None,
            },
            Err(_) => None,
        };
        let Some(peer_hash) = peer_hash else { return };
        match local_hash_at(fork_point) {
            Some(local_hash) if local_hash == peer_hash => break, // common ancestor found
            Some(_) => {} // definite mismatch at this height — keep searching further back
            None => {
                eprintln!("[reorg] no local record at height {fork_point} — cannot determine the fork point, refusing to auto-reorg");
                return;
            }
        }
        if fork_point == 0 {
            return;
        }
        fork_point -= 1;
    }

    if fork_point == *height {
        // No divergence — peer is just further ahead on our own history.
        let synced = sync_forward_from_peer(
            &client, best.addr, best.height, state, ledger, store,
            height, tip_hash, last_block_timestamp_ms, btc_seed_hash, btc_checkpoint_tip_height, chain_work, adjuster,
        ).await;
        if synced > 0 {
            println!("[sync] applied {synced} block(s) from {}, now at height {}", best.addr, *height);
        }
        return;
    }
    if fork_point <= probe_floor {
        eprintln!(
            "[reorg] peer {} diverges more than {MAX_AUTO_REORG_DEPTH} blocks back from our tip {} — refusing to auto-reorg that deep",
            best.addr, *height
        );
        return;
    }
    if fork_point < adjuster.epoch_start_height() {
        eprintln!("[reorg] fork point {fork_point} predates our current difficulty epoch (started {}) — refusing to auto-reorg across an epoch boundary", adjuster.epoch_start_height());
        return;
    }

    // Roll back: pull what we're discarding from the store (to subtract
    // its work) and what fork_point itself looked like (to recover
    // tip/timestamp/checkpoint state), then delete everything above it.
    let rolled_back = store.get_range(fork_point + 1, *height, (*height - fork_point) as usize + 1).unwrap_or_default();
    let work_removed: u128 = rolled_back.iter().fold(0u128, |acc, b| acc.saturating_add(1u128 << b.difficulty_bits.min(127)));

    eprintln!(
        "[reorg] rolling back {} block(s) (height {} -> {}) to adopt peer {}'s heavier chain",
        rolled_back.len(), *height, fork_point, best.addr
    );

    let (new_tip, new_ts) = if fork_point == 0 {
        (state.read().unwrap().genesis_hash, adjuster.epoch_start_ms())
    } else {
        match store.get(fork_point) {
            Ok(Some(b)) => match l1_core::bitcoin_entropy::hex_to_32(&b.block_hash) {
                Ok(hash) => (hash, b.timestamp),
                Err(_) => return,
            },
            _ => return, // not actually retained despite the probe above — bail out safely
        }
    };

    let governing = governing_checkpoint_height(fork_point);
    let (new_seed, new_seed_tip_height) = if governing == 0 {
        (*btc_seed_hash, *btc_checkpoint_tip_height)
    } else {
        match store.get(governing) {
            Ok(Some(b)) => match l1_core::bitcoin_entropy::hex_to_32(&b.btc_seed) {
                Ok(seed) => (seed, b.btc_height),
                Err(_) => return,
            },
            _ => return,
        }
    };

    let new_ledger = match RewardLedger::open_scoped(ledger_path, fork_point) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[reorg] failed to reopen ledger scoped to height {fork_point}: {e}");
            return;
        }
    };
    if let Err(e) = store.rollback_above(fork_point) {
        eprintln!("[reorg] failed to roll back block store to height {fork_point}: {e}");
        return;
    }

    {
        let mut l = ledger.lock().unwrap();
        *l = new_ledger;
    }
    *chain_work = chain_work.saturating_sub(work_removed);
    *height = fork_point;
    *tip_hash = new_tip;
    *last_block_timestamp_ms = new_ts;
    *btc_seed_hash = new_seed;
    *btc_checkpoint_tip_height = new_seed_tip_height;

    {
        let mut s = state.write().unwrap();
        s.pending_claims.retain(|candidate_height, _| *candidate_height > fork_point);
        s.chain_height = *height;
        s.tip_hash = *tip_hash;
        s.last_block_timestamp_ms = *last_block_timestamp_ms;
        s.btc_seed_hash = *btc_seed_hash;
        s.btc_checkpoint_tip_height = *btc_checkpoint_tip_height;
        s.chain_work = *chain_work;
        s.total_mined_bina = ledger.lock().unwrap().total_mined();
        // Rolled-back transactions are no longer confirmed — make them
        // eligible for inclusion again rather than losing them outright.
        requeue_rolled_back_transactions(&mut s.mempool, &rolled_back);
    }

    let synced = sync_forward_from_peer(
        &client, best.addr, best.height, state, ledger, store,
        height, tip_hash, last_block_timestamp_ms, btc_seed_hash, btc_checkpoint_tip_height, chain_work, adjuster,
    ).await;
    println!("[reorg] adopted {} block(s) from {} after rollback, now at height {}", synced, best.addr, *height);
}

// ─── Mining loop (runs forever) ────────────────────────────────────────────

async fn mining_loop(state: SharedState, ledger: SharedLedger, gossip: Arc<Gossip>, store: Arc<BlockStore>) {
    let threads = state.read().unwrap().threads;
    let miner_keypair = load_miner_keypair();
    let miner_address: [u8; 20] = miner_keypair.address();
    let miner_hex = hex::encode(miner_address);

    // Genesis
    let genesis = genesis_block();
    let genesis_hash = genesis.header.hash();
    let genesis_timestamp_ms = genesis.header.timestamp;
    let chain_state_path = chain_state_path();
    let ledger_path = ledger_path();
    let chain_state = load_or_create_chain_state(&chain_state_path, genesis_hash, genesis_timestamp_ms);
    let resume = chain_state.resume.clone();
    let persisted_total = {
        let scoped_ledger = RewardLedger::open_scoped(&ledger_path, chain_state.height)
            .unwrap_or_else(|e| panic!("failed to open scoped reward ledger: {e}"));
        let total = scoped_ledger.total_mined();
        *ledger.lock().unwrap() = scoped_ledger;
        total
    };

    let mut btc_seed_hash = hex_to_32("resume.btc_seed_hash", &resume.btc_seed_hash);
    let mut chain_work: u128 = u128::from_str_radix(&resume.chain_work_hex, 16).unwrap_or(0);
    let initial_observed_btc = fetch_btc(None).await;

    {
        let mut s = state.write().unwrap();
        s.genesis_hash = chain_state.genesis_hash;
        s.tip_hash = chain_state.tip_hash;
        s.chain_height = chain_state.height;
        s.miner_address = miner_hex.clone();
        s.total_mined_bina = persisted_total;
        s.current_reward = block_reward(chain_state.height + 1, persisted_total);
        s.difficulty_bits = resume.difficulty_bits;
        s.btc_seed_hash = btc_seed_hash;
        s.btc_checkpoint_tip_height = resume.btc_checkpoint_tip_height;
        s.last_block_timestamp_ms = resume.last_block_timestamp_ms;
        s.chain_work = chain_work;
        s.last_observed_btc = initial_observed_btc.clone();
    }
    println!("[genesis]  hash={}…", hex::encode(&genesis_hash[..16]));
    println!(
        "[chain]    {} height={} tip={}… state={}",
        if chain_state.created { "initialized" } else { "resumed" },
        chain_state.height,
        hex::encode(&chain_state.tip_hash[..16]),
        chain_state_path,
    );
    println!("[wallet]   mining to: {miner_hex}");
    println!("[ledger]   persistent total mined: {persisted_total} BINA");
    println!(
        "[supply]   hard cap: {} BINA  |  initial reward: {} BINA  |  halving: every {} blocks",
        HARD_CAP, INITIAL_BLOCK_REWARD, HALVING_INTERVAL
    );
    println!(
        "[btc]      checkpoint every {} blocks  |  tolerance ±{} btc blocks",
        BTC_CHECKPOINT_INTERVAL, BTC_HEIGHT_TOLERANCE
    );
    println!();

    // Background: keep `state.last_observed_btc` fresh regardless of mining
    // cadence, so incoming peer checkpoint claims always have a recent
    // observation to be plausibility-checked against.
    tokio::spawn(btc_observer_task(state.clone()));

    let mut prev_hash = chain_state.tip_hash;
    let mut height: u64 = chain_state.height;
    let mut last_block_timestamp_ms = resume.last_block_timestamp_ms;
    let mut btc_checkpoint_tip_height = resume.btc_checkpoint_tip_height;

    let mut adjuster = DifficultyAdjuster::restore(
        resume.difficulty_bits,
        resume.epoch_start_ms,
        resume.epoch_start_height,
    );

    // One-shot catch-up: if a peer already has a heavier chain than we do
    // (e.g. we're a fresh node, or we were offline while the network kept
    // mining), adopt it before we start extending our own tip.
    catch_up_from_peers(
        &state,
        &ledger,
        &store,
        &gossip,
        &mut height,
        &mut prev_hash,
        &mut last_block_timestamp_ms,
        &mut btc_seed_hash,
        &mut btc_checkpoint_tip_height,
        &mut chain_work,
        &mut adjuster,
    )
    .await;

    let mut last_reorg_check = Instant::now();
    const REORG_CHECK_INTERVAL: Duration = Duration::from_secs(10);

    loop {
        // Throttled: check whether a peer has a heavier chain than ours and,
        // if the chains have diverged, reconcile onto it. See
        // `reconcile_fork` for why this only auto-resolves shallow forks.
        if last_reorg_check.elapsed() >= REORG_CHECK_INTERVAL {
            last_reorg_check = Instant::now();
            reconcile_fork(
                &state,
                &ledger,
                &ledger_path,
                &store,
                &gossip,
                &mut height,
                &mut prev_hash,
                &mut last_block_timestamp_ms,
                &mut btc_seed_hash,
                &mut btc_checkpoint_tip_height,
                &mut chain_work,
                &mut adjuster,
            )
            .await;
        }

        height += 1;
        let is_ckpt = is_checkpoint_height(height);

        // Bitcoin seed: only a live fetch at checkpoint heights. Every other
        // block reuses the seed pinned by the last accepted checkpoint —
        // chain data, not a race against independently-polled live APIs.
        let (btc_seed, checkpoint_proof) = if is_ckpt {
            let fresh = fetch_btc(Some(&state.read().unwrap().last_observed_btc)).await;
            {
                let mut s = state.write().unwrap();
                s.last_observed_btc = fresh.clone();
            }
            log_btc(&fresh);
            (fresh.bitcoin_seed_hash(), Some(BtcCheckpointProof::from_state(&fresh)))
        } else {
            (btc_seed_hash, None)
        };

        let current_bits = adjuster.current_bits();
        {
            let mut s = state.write().unwrap();
            s.difficulty_bits = current_bits;
        }

        let total_mined = state.read().unwrap().total_mined_bina;
        let reward = block_reward(height, total_mined);
        let candidate_timestamp = unix_ms().max(last_block_timestamp_ms.saturating_add(1));

        // Build the candidate transaction list from our mempool, then
        // deterministically evaluate exactly what it would do to the
        // ledger. This never mutates the real ledger — only a winning
        // block does that, once selection below settles who actually won.
        let candidate_txs: Vec<SignedTransaction> = {
            let s = state.read().unwrap();
            s.mempool.values().take(l1_core::transaction::MAX_TXS_PER_BLOCK).cloned().collect()
        };
        let (applied_txs, candidate_state_root) = {
            let ledger = ledger.lock().unwrap();
            l1_core::rewards::simulate_block_execution(&ledger, &candidate_txs, &miner_hex, reward)
        };
        let candidate_merkle_root = l1_core::transaction::merkle_root(&applied_txs);

        let (ph, mr, sr, ma, bs, cb, ts) =
            (prev_hash, candidate_merkle_root, candidate_state_root, miner_address, btc_seed, current_bits, candidate_timestamp);
        let result =
            tokio::task::spawn_blocking(move || mine_block(height, ph, mr, sr, ma, bs, cb, ts, threads))
                .await
                .expect("mine_block panicked");

        let local_claim = SignedBlockClaim::sign(result.block.header.clone(), &miner_keypair);
        local_claim
            .verify()
            .expect("locally mined signed claim must verify");
        let local_block_hash = local_claim.block_hash();
        let local_pending = PendingClaim {
            claim: local_claim.clone(),
            btc_checkpoint: checkpoint_proof.clone(),
            transactions: applied_txs.clone(),
        };

        {
            let mut s = state.write().unwrap();
            let claims = s.pending_claims.entry(height).or_default();
            claims
                .entry(miner_hex.clone())
                .or_insert_with(|| local_pending);
        }

        gossip
            .broadcast_claim(BlockClaimEnvelope::from_claim(
                gossip.network().to_string(),
                DEFAULT_P2P_TTL,
                local_claim,
                checkpoint_proof,
                applied_txs,
            ))
            .await;

        tokio::time::sleep(Duration::from_millis(SUBMISSION_GRACE_MS)).await;

        let winning = {
            let mut s = state.write().unwrap();
            let candidates: Vec<PendingClaim> = s
                .pending_claims
                .remove(&height)
                .unwrap_or_default()
                .into_values()
                .collect();
            s.pending_claims
                .retain(|candidate_height, _| *candidate_height > height);
            select_winning_pending(candidates)
                .expect("local signed claim missing from candidate pool")
        };
        let winning_claim = winning.claim;
        let winning_txs = winning.transactions;

        let block_hash = winning_claim.block_hash();
        let zero_bits = leading_zero_bits(&block_hash);
        let rand_out = RandomnessOutput::from_block(
            height,
            block_hash,
            winning_claim.header.bitcoin_seed_hash,
        );
        let timestamp = winning_claim.header.timestamp;
        let local_won = block_hash == local_block_hash;
        let source = if local_won { "local" } else { "submitted" };
        let elapsed_ms = if local_won { result.elapsed_ms } else { 0 };
        let hashes_tried = if local_won { result.hashes_tried } else { 0 };
        let hashrate_mhs = if local_won {
            result.hashrate_hs / 1_000_000.0
        } else {
            0.0
        };
        let winner_miner_hex = winning_claim.miner_address_hex();

        // Pin the checkpoint seed from whichever claim won the election —
        // every node reaches the same value this way, regardless of who won.
        let mut btc_checkpoint_tip_height_now = state.read().unwrap().btc_checkpoint_tip_height;
        if is_ckpt {
            if let Some(proof) = &winning.btc_checkpoint {
                btc_seed_hash = winning_claim.header.bitcoin_seed_hash;
                btc_checkpoint_tip_height_now = proof.tip_height;
            }
        }

        // Difficulty adjustment (fires every 20 blocks), fed the winning
        // claim's own consensus timestamp — never wall clock.
        let adj_info = adjuster.record_block(height, timestamp);
        if let Some(ref info) = adj_info {
            let log = DifficultyAdjuster::adjustment_log(info);
            println!("{log}");
            state.write().unwrap().last_adjustment = Some(log);
        }

        // Work is counted from the *required* difficulty for this height,
        // not the winning hash's actual leading-zero count — otherwise a
        // single lucky block would count for far more than its real
        // expected cost, letting a chain claim disproportionate work.
        chain_work = chain_work.saturating_add(1u128 << winning_claim.header.difficulty_bits.min(127));
        last_block_timestamp_ms = timestamp;

        // Execute the winning block's transactions for real, in order, then
        // credit the reward. Every transaction here already passed
        // `simulate_block_execution` — either in this process when it built
        // this exact candidate, or in `accept_signed_claim` when it
        // admitted a peer's claim — against this same parent state, so
        // real application is expected to succeed identically. A failure
        // here would mean a state-root collision or a logic bug, not an
        // adversarial input; it's logged rather than trusted silently.
        let mut ledger = ledger.lock().unwrap();
        for tx in &winning_txs {
            if let Err(e) = ledger.apply_transaction(height, tx, &winner_miner_hex, timestamp / 1000) {
                eprintln!("[ledger] BUG: winning block's pre-validated transaction {} failed to apply: {e}", tx.tx_id_hex());
            }
        }
        ledger
            .credit(height, &winner_miner_hex, reward, timestamp / 1000)
            .unwrap_or_else(|e| {
                eprintln!("[ledger] write error: {e}");
                0
            });

        // Confirmed — drop these from the mempool regardless of whether
        // they were our own candidate's or came from the winning peer.
        {
            let mut s = state.write().unwrap();
            for tx in &winning_txs {
                s.mempool.remove(&tx.tx_id());
            }
        }

        let record = BlockRecord {
            height,
            block_hash: hex::encode(block_hash),
            prev_hash: hex::encode(winning_claim.header.prev_hash),
            nonce: winning_claim.header.nonce,
            timestamp,
            mined_timestamp_secs: timestamp / 1000,
            zero_bits,
            difficulty_bits: winning_claim.header.difficulty_bits,
            hashes_tried,
            elapsed_ms,
            hashrate_mhs,
            miner_address: winner_miner_hex.clone(),
            miner_public_key: hex::encode(&winning_claim.public_key),
            miner_signature: hex::encode(&winning_claim.signature),
            claim_digest: winning_claim.claim_digest_hex(),
            election_score: winning_claim.election_score_hex(),
            source: source.to_string(),
            reward_bina: reward,
            randomness_output: rand_out.output_hex(),
            nullifier: rand_out.nullifier_hex(),
            btc_seed: hex::encode(winning_claim.header.bitcoin_seed_hash),
            btc_height: btc_checkpoint_tip_height_now,
            merkle_root: hex::encode(winning_claim.header.merkle_root),
            state_root: hex::encode(winning_claim.header.state_root),
            chain_work_hex: format!("{:x}", chain_work),
            transactions: winning_txs,
        };

        // Durable block data lands before the lightweight resume pointer is
        // updated to reference it — if this process dies in between, a
        // restart's chain-state.json can never point at a height the store
        // doesn't actually have.
        if let Err(e) = store.insert_block(&record) {
            eprintln!("[store] failed to persist block {height}: {e}");
        }

        let resume_state = ConsensusResumeState {
            difficulty_bits: adjuster.current_bits(),
            epoch_start_ms: adjuster.epoch_start_ms(),
            epoch_start_height: adjuster.epoch_start_height(),
            btc_seed_hash: hex::encode(btc_seed_hash),
            btc_checkpoint_tip_height: btc_checkpoint_tip_height_now,
            last_block_timestamp_ms,
            chain_work_hex: format!("{:x}", chain_work),
        };
        if let Err(e) = save_chain_state(&chain_state_path, genesis_hash, block_hash, height, resume_state) {
            eprintln!("[chain] state write error: {e}");
        }

        println!(
            "[h={:<6}]  hash={}…  {:.2} MH/s  {}ms  +{} BINA  diff={}  winner={}…  source={}{}",
            height,
            &record.block_hash[..12],
            record.hashrate_mhs,
            elapsed_ms,
            reward,
            current_bits,
            &record.miner_address[..12],
            record.source,
            if is_ckpt { "  [btc checkpoint pinned]" } else { "" },
        );

        {
            let mut s = state.write().unwrap();
            s.nullifiers
                .consume(&rand_out)
                .expect("nullifier collision — impossible on a valid chain");
            s.total_hashes += result.hashes_tried;
            s.total_time_ms += result.elapsed_ms;
            s.chain_height = height;
            s.total_mined_bina = ledger.total_mined();
            s.current_reward = block_reward(height + 1, ledger.total_mined());
            s.difficulty_bits = adjuster.current_bits();
            s.tip_hash = block_hash;
            s.btc_seed_hash = btc_seed_hash;
            s.btc_checkpoint_tip_height = btc_checkpoint_tip_height_now;
            if is_ckpt {
                s.btc_seed_changed_at = unix_secs();
            }
            s.last_block_timestamp_ms = last_block_timestamp_ms;
            s.chain_work = chain_work;
        }

        prev_hash = block_hash;
    }
}

/// Refreshes `state.last_observed_btc` on a fixed cadence, independent of
/// mining/checkpoint timing, so plausibility checks on incoming peer
/// checkpoint claims always have a recent observation to compare against.
async fn btc_observer_task(state: SharedState) {
    loop {
        tokio::time::sleep(BTC_OBSERVE_INTERVAL).await;
        let previous = state.read().unwrap().last_observed_btc.clone();
        let fresh = fetch_btc(Some(&previous)).await;
        state.write().unwrap().last_observed_btc = fresh;
    }
}

// ─── Helpers ───────────────────────────────────────────────────────────────

async fn fetch_btc(previous: Option<&BtcEntropyState>) -> BtcEntropyState {
    match bitcoin_entropy::fetch_live_entropy().await {
        Ok(e) => e,
        Err(e) => {
            if let Some(previous) = previous {
                eprintln!("[btc] fetch error: {e} — keeping previous Bitcoin tip");
                previous.clone()
            } else {
                eprintln!("[btc] fetch error: {e} — using mock");
                BtcEntropyState::mock()
            }
        }
    }
}

fn log_btc(btc: &BtcEntropyState) {
    let seed = btc.bitcoin_seed_hash();
    println!(
        "[btc]      height={}  tip={}…  seed={}…{}",
        btc.tip_height,
        hex::encode(&btc.tip_hash[..8]),
        hex::encode(&seed[..8]),
        if btc.fork_detected { "  ⚡ fork" } else { "" },
    );
}

/// Load wallet from ~/.bina/wallet.json. Mining requires the secret key because
/// every accepted block claim is signed before rewards are credited.
fn load_miner_keypair() -> WalletKeypair {
    let wallet_path = {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".into());
        std::path::PathBuf::from(home)
            .join(".bina")
            .join("wallet.json")
    };

    #[derive(Deserialize)]
    struct WalletFile {
        secret_key: String,
    }

    match std::fs::read_to_string(&wallet_path) {
        Ok(text) => {
            let mut text = Zeroizing::new(text);
            let mut wallet: WalletFile =
                serde_json::from_str(&text).expect("wallet.json is not valid JSON");
            let secret = SecureBuffer::from_hex(&wallet.secret_key)
                .expect("wallet.json 'secret_key' is not valid hex");
            wallet.secret_key.zeroize();
            text.zeroize();
            WalletKeypair::from_secret_bytes(secret.as_slice().expect("secure secret buffer is invalid"))
                .expect("wallet.json secret key is corrupt")
        }
        Err(_) => {
            panic!(
                "wallet file not found at {}. Run `cargo run -p l1-node --bin l1-wallet -- generate` before starting the node.",
                wallet_path.display()
            );
        }
    }
}

fn parse_seed_peers() -> Vec<SocketAddr> {
    let configured = std::env::var("BINA_SEEDS").unwrap_or_default();
    DEFAULT_SEED_PEERS
        .iter()
        .copied()
        .chain(configured.split(','))
        .filter_map(|raw| {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return None;
            }
            match trimmed.parse() {
                Ok(addr) => Some(addr),
                Err(e) => {
                    eprintln!("[p2p] ignoring invalid seed '{trimmed}': {e}");
                    None
                }
            }
        })
        .fold(Vec::new(), |mut peers, addr| {
            if !peers.contains(&addr) {
                peers.push(addr);
            }
            peers
        })
}

// ─── Rate limiting ──────────────────────────────────────────────────────────

/// Simple fixed-window per-IP request budget for mutating endpoints
/// (`/chain/submit`, `/tx/submit`, `/wallet/send`, `/p2p/*`). Claim
/// verification is itself cheap to reject garbage early (PoW is checked
/// before the expensive signature check — see `SignedBlockClaim::verify`),
/// but nothing previously stopped a source from simply flooding requests;
/// this bounds that regardless of payload validity.
#[derive(Clone)]
struct RateLimiter {
    buckets: Arc<Mutex<HashMap<IpAddr, (Instant, u32)>>>,
}

impl RateLimiter {
    fn new() -> Self {
        Self { buckets: Arc::new(Mutex::new(HashMap::new())) }
    }

    fn allow(&self, ip: IpAddr) -> bool {
        let mut buckets = self.buckets.lock().unwrap();
        let now = Instant::now();
        if buckets.len() > 50_000 {
            buckets.retain(|_, (started, _)| now.duration_since(*started) < RATE_LIMIT_WINDOW);
        }
        let entry = buckets.entry(ip).or_insert((now, 0));
        if now.duration_since(entry.0) >= RATE_LIMIT_WINDOW {
            *entry = (now, 1);
            return true;
        }
        entry.1 += 1;
        entry.1 <= RATE_LIMIT_MAX_REQUESTS
    }
}

async fn rate_limit_middleware(
    Extension(limiter): Extension<RateLimiter>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    if limiter.allow(addr.ip()) {
        next.run(req).await
    } else {
        (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded — slow down").into_response()
    }
}

// ─── Adversarial / failure-path tests ──────────────────────────────────────
//
// These exercise the actual consensus-acceptance functions in-process
// (no HTTP, no real PoW — difficulty_bits=0 so `meets_difficulty` is
// trivially satisfied) against inputs a malicious or buggy peer might send,
// confirming each is rejected for the right reason rather than merely
// "not crashing". Complements the live multi-node validation, which proves
// the happy path converges; these prove the failure paths hold the line.
#[cfg(test)]
mod adversarial_tests {
    use super::*;
    use l1_core::crypto::WalletKeypair;

    fn test_env(tag: &str) -> (SharedState, SharedLedger, WalletKeypair, [u8; 32]) {
        let genesis_hash = [7u8; 32];
        let miner = WalletKeypair::generate();
        let state: SharedState = Arc::new(RwLock::new(NodeState {
            genesis_hash,
            tip_hash: genesis_hash,
            chain_height: 1, // next claim targets height 2 (non-checkpoint) unless a test overrides this
            pending_claims: HashMap::new(),
            mempool: HashMap::new(),
            nullifiers: NullifierSet::new(),
            total_hashes: 0,
            total_time_ms: 0,
            started_at: Instant::now(),
            threads: 1,
            last_observed_btc: BtcEntropyState::mock(),
            btc_seed_hash: [9u8; 32],
            btc_seed_changed_at: 0,
            btc_checkpoint_tip_height: 900_000,
            difficulty_bits: 0,
            last_block_timestamp_ms: 1_000_000,
            chain_work: 0,
            miner_address: miner.address_hex(),
            total_mined_bina: 0,
            current_reward: 50,
            last_adjustment: None,
        }));
        let csv_path = std::env::temp_dir().join(format!("bina_adversarial_test_{tag}.csv"));
        let _ = std::fs::remove_file(&csv_path);
        let ledger: SharedLedger = Arc::new(Mutex::new(RewardLedger::open(&csv_path).expect("open test ledger")));
        (state, ledger, miner, genesis_hash)
    }

    #[allow(clippy::too_many_arguments)]
    fn claim_at(
        miner: &WalletKeypair,
        height: u64,
        prev_hash: [u8; 32],
        merkle_root: [u8; 32],
        state_root: [u8; 32],
        bitcoin_seed_hash: [u8; 32],
        timestamp: u64,
    ) -> SignedBlockClaim {
        let header = L1BlockHeader {
            version: 1,
            height,
            prev_hash,
            merkle_root,
            state_root,
            timestamp,
            nonce: 0, // difficulty_bits=0 in these tests, so any nonce satisfies PoW
            miner_address: miner.address(),
            difficulty_bits: 0,
            bitcoin_seed_hash,
        };
        SignedBlockClaim::sign(header, miner)
    }

    fn signed_transfer(from: &WalletKeypair, to: &WalletKeypair, amount: u64, nonce: u64, fee: u64) -> SignedTransaction {
        let tx = Transaction::new(from.address(), to.address(), amount, nonce, fee);
        SignedTransaction::sign(tx, from).unwrap()
    }

    fn dummy_block_record(height: u64, txs: Vec<SignedTransaction>) -> BlockRecord {
        BlockRecord {
            height,
            block_hash: format!("h{height}"),
            prev_hash: format!("h{}", height.saturating_sub(1)),
            nonce: 0,
            timestamp: 1_000_000 + height,
            mined_timestamp_secs: (1_000_000 + height) / 1000,
            zero_bits: 0,
            difficulty_bits: 0,
            hashes_tried: 0,
            elapsed_ms: 0,
            hashrate_mhs: 0.0,
            miner_address: "aa".to_string(),
            miner_public_key: "bb".to_string(),
            miner_signature: "cc".to_string(),
            claim_digest: "dd".to_string(),
            election_score: "ee".to_string(),
            source: "local".to_string(),
            reward_bina: 50,
            randomness_output: "ff".to_string(),
            nullifier: "00".to_string(),
            btc_seed: "11".repeat(32),
            btc_height: 900_000,
            merkle_root: hex::encode(l1_core::transaction::merkle_root(&txs)),
            state_root: "22".repeat(32),
            chain_work_hex: "0".to_string(),
            transactions: txs,
        }
    }

    // ── merkle_root / state_root integrity ──────────────────────────────

    #[test]
    fn claim_with_wrong_merkle_root_is_rejected() {
        let (state, ledger, miner, genesis) = test_env("bad_merkle");
        let recipient = WalletKeypair::generate();
        let tx = signed_transfer(&miner, &recipient, 5, 0, 1);

        let wrong_merkle = [0xAAu8; 32]; // does not match merkle_root(&[tx])
        let claim = claim_at(&miner, 2, genesis, wrong_merkle, l1_core::rewards::empty_state_root(), [9u8; 32], 1_000_001);

        let err = accept_signed_claim(&state, &ledger, claim, None, vec![tx]).unwrap_err();
        assert!(err.message.contains("merkle_root"), "unexpected message: {}", err.message);
    }

    #[test]
    fn claim_with_wrong_state_root_is_rejected() {
        let (state, ledger, miner, genesis) = test_env("bad_state_root");
        // Empty transaction list: merkle_root([]) == [0;32], so that check
        // passes cleanly and isolates the state_root check.
        let wrong_state_root = [0x55u8; 32];
        let claim = claim_at(&miner, 2, genesis, [0u8; 32], wrong_state_root, [9u8; 32], 1_000_001);

        let err = accept_signed_claim(&state, &ledger, claim, None, vec![]).unwrap_err();
        assert!(err.message.contains("state_root"), "unexpected message: {}", err.message);
    }

    #[test]
    fn claim_with_correct_roots_is_accepted() {
        // Sanity check that the two rejection tests above are actually
        // exercising the right thing and not just always failing.
        let (state, ledger, miner, genesis) = test_env("good_roots");
        let reward = {
            let s = state.read().unwrap();
            block_reward(2, s.total_mined_bina)
        };
        let (_, state_root) = {
            let l = ledger.lock().unwrap();
            l1_core::rewards::simulate_block_execution(&l, &[], &miner.address_hex(), reward)
        };
        let claim = claim_at(&miner, 2, genesis, [0u8; 32], state_root, [9u8; 32], 1_000_001);
        accept_signed_claim(&state, &ledger, claim, None, vec![]).expect("well-formed claim must be accepted");
    }

    // ── transaction execution integrity ─────────────────────────────────

    #[test]
    fn claim_including_a_double_spend_is_rejected() {
        let (state, ledger, miner, genesis) = test_env("double_spend");
        let r1 = WalletKeypair::generate();
        let r2 = WalletKeypair::generate();
        ledger.lock().unwrap().credit(1, &miner.address_hex(), 100, 1_000_000).unwrap();

        // Two transactions, both claiming nonce 0, spending more than the
        // balance can cover twice over — a double-spend within one block.
        let tx1 = signed_transfer(&miner, &r1, 80, 0, 0);
        let tx2 = signed_transfer(&miner, &r2, 80, 0, 0);
        let txs = vec![tx1, tx2];
        let merkle = l1_core::transaction::merkle_root(&txs);
        // Whatever the (incorrect) claimed state_root is doesn't matter —
        // the transaction-validity check must fire first.
        let claim = claim_at(&miner, 2, genesis, merkle, [0u8; 32], [9u8; 32], 1_000_001);

        let err = accept_signed_claim(&state, &ledger, claim, None, txs).unwrap_err();
        assert!(err.message.contains("does not validate"), "unexpected message: {}", err.message);
    }

    #[test]
    fn mempool_rejects_a_submission_with_the_wrong_nonce() {
        let (state, ledger, miner, _genesis) = test_env("wrong_nonce");
        let recipient = WalletKeypair::generate();
        ledger.lock().unwrap().credit(1, &miner.address_hex(), 100, 1_000_000).unwrap();

        // Sender's ledger nonce is 0; submit a transaction claiming nonce 5.
        let tx = signed_transfer(&miner, &recipient, 5, 5, 1);
        let err = enqueue_transaction(&state, &ledger, tx).unwrap_err();
        assert!(err.contains("nonce"), "unexpected message: {err}");
    }

    #[test]
    fn claim_with_a_transaction_that_overdraws_is_rejected() {
        let (state, ledger, miner, genesis) = test_env("overdraw");
        let recipient = WalletKeypair::generate();
        // Miner has 0 balance — any nonzero transfer must fail execution.
        let tx = signed_transfer(&miner, &recipient, 5, 0, 1);
        let txs = vec![tx];
        let merkle = l1_core::transaction::merkle_root(&txs);
        let claim = claim_at(&miner, 2, genesis, merkle, [0u8; 32], [9u8; 32], 1_000_001);

        let err = accept_signed_claim(&state, &ledger, claim, None, txs).unwrap_err();
        assert!(err.message.contains("does not validate"), "unexpected message: {}", err.message);
    }

    // ── chain-linkage / fork choice ──────────────────────────────────────

    #[test]
    fn select_best_peer_ignores_lower_or_equal_work_and_foreign_genesis() {
        let genesis = [1u8; 32];
        let addr: SocketAddr = "127.0.0.1:9001".parse().unwrap();
        let lower = PeerStatus { addr, height: 3, genesis_hash: genesis, chain_work: 50 };
        let equal = PeerStatus { addr, height: 5, genesis_hash: genesis, chain_work: 100 };
        let foreign_genesis = PeerStatus { addr, height: 20, genesis_hash: [2u8; 32], chain_work: 999 };
        let heavier = PeerStatus { addr, height: 6, genesis_hash: genesis, chain_work: 150 };

        assert!(select_best_peer(vec![lower.clone()], genesis, 100).is_none(), "lower work must not be selected");
        assert!(select_best_peer(vec![equal.clone()], genesis, 100).is_none(), "equal (not strictly heavier) work must not be selected");
        assert!(select_best_peer(vec![foreign_genesis.clone()], genesis, 100).is_none(), "a different genesis must never be selected regardless of work");

        let picked = select_best_peer(vec![lower, equal, foreign_genesis, heavier.clone()], genesis, 100).unwrap();
        assert_eq!(picked.chain_work, heavier.chain_work);
    }

    #[test]
    fn claim_with_wrong_prev_hash_is_rejected() {
        let (state, ledger, miner, genesis) = test_env("wrong_prev_hash");
        let not_the_tip = [0x44u8; 32];
        assert_ne!(not_the_tip, genesis);
        let claim = claim_at(&miner, 2, not_the_tip, [0u8; 32], l1_core::rewards::empty_state_root(), [9u8; 32], 1_000_001);
        let err = accept_signed_claim(&state, &ledger, claim, None, vec![]).unwrap_err();
        assert!(err.message.contains("prev_hash"), "unexpected message: {}", err.message);
    }

    #[test]
    fn claim_for_wrong_height_is_rejected() {
        let (state, ledger, miner, genesis) = test_env("wrong_height");
        // Chain is at height 1, so the only valid next claim is height 2.
        let claim = claim_at(&miner, 9, genesis, [0u8; 32], l1_core::rewards::empty_state_root(), [9u8; 32], 1_000_001);
        let err = accept_signed_claim(&state, &ledger, claim, None, vec![]).unwrap_err();
        assert!(err.message.contains("height"), "unexpected message: {}", err.message);
    }

    // ── reorg / rollback ─────────────────────────────────────────────────

    #[test]
    fn rollback_requeues_transactions_into_mempool() {
        let miner = WalletKeypair::generate();
        let recipient = WalletKeypair::generate();
        let tx1 = signed_transfer(&miner, &recipient, 5, 0, 1);
        let tx2 = signed_transfer(&miner, &recipient, 5, 1, 1);
        let rolled_back = vec![
            dummy_block_record(4, vec![tx1.clone()]),
            dummy_block_record(5, vec![tx2.clone()]),
        ];

        let mut mempool: HashMap<[u8; 32], SignedTransaction> = HashMap::new();
        requeue_rolled_back_transactions(&mut mempool, &rolled_back);

        assert_eq!(mempool.len(), 2);
        assert!(mempool.contains_key(&tx1.tx_id()), "tx from the first rolled-back block must be requeued");
        assert!(mempool.contains_key(&tx2.tx_id()), "tx from the second rolled-back block must be requeued");
    }

    // ── Bitcoin checkpoint integrity ─────────────────────────────────────

    #[test]
    fn checkpoint_height_without_a_proof_is_rejected() {
        let (state, ledger, miner, genesis) = test_env("ckpt_missing");
        { state.write().unwrap().chain_height = 0; } // next height = 1, always a checkpoint height
        let claim = claim_at(&miner, 1, genesis, [0u8; 32], l1_core::rewards::empty_state_root(), [9u8; 32], 1_000_001);
        let err = accept_signed_claim(&state, &ledger, claim, None, vec![]).unwrap_err();
        assert!(err.message.contains("checkpoint proof"), "unexpected message: {}", err.message);
    }

    #[test]
    fn checkpoint_proof_with_mismatched_seed_hash_is_rejected() {
        let (state, ledger, miner, genesis) = test_env("ckpt_seed_mismatch");
        { state.write().unwrap().chain_height = 0; }
        let proof = BtcCheckpointProof {
            tip_hash: [1u8; 32],
            tip_height: 900_000,
            utxo_entropy: [2u8; 32],
            stale_xor_pool: [3u8; 32],
        };
        // Header claims a different seed than what the proof actually hashes to.
        let claimed_seed = [0u8; 32];
        assert_ne!(proof.seed_hash(), claimed_seed);
        let claim = claim_at(&miner, 1, genesis, [0u8; 32], l1_core::rewards::empty_state_root(), claimed_seed, 1_000_001);
        let err = accept_signed_claim(&state, &ledger, claim, Some(proof), vec![]).unwrap_err();
        assert!(err.message.contains("checkpoint proof"), "unexpected message: {}", err.message);
    }

    #[test]
    fn checkpoint_proof_with_implausible_btc_height_is_rejected() {
        let (state, ledger, miner, genesis) = test_env("ckpt_implausible");
        {
            let mut s = state.write().unwrap();
            s.chain_height = 0;
            s.btc_checkpoint_tip_height = 0;
            s.last_observed_btc = BtcEntropyState::mock(); // tip_height = 0
        }
        // Claims a wildly different Bitcoin tip height than what this
        // validator itself observes — must not be trusted even though the
        // proof's own hash is internally consistent.
        let proof = BtcCheckpointProof {
            tip_hash: [1u8; 32],
            tip_height: 900_000,
            utxo_entropy: [2u8; 32],
            stale_xor_pool: [3u8; 32],
        };
        let claim = claim_at(&miner, 1, genesis, [0u8; 32], l1_core::rewards::empty_state_root(), proof.seed_hash(), 1_000_001);
        let err = accept_signed_claim(&state, &ledger, claim, Some(proof), vec![]).unwrap_err();
        assert!(err.message.contains("not plausible"), "unexpected message: {}", err.message);
    }

    // ── timestamp integrity ──────────────────────────────────────────────

    #[test]
    fn claim_with_non_monotonic_timestamp_is_rejected() {
        let (state, ledger, miner, genesis) = test_env("ts_non_monotonic");
        // state's last_block_timestamp_ms is 1_000_000; a claim at or before
        // that must be rejected regardless of everything else being valid.
        let claim = claim_at(&miner, 2, genesis, [0u8; 32], l1_core::rewards::empty_state_root(), [9u8; 32], 1_000_000);
        let err = accept_signed_claim(&state, &ledger, claim, None, vec![]).unwrap_err();
        assert!(err.message.contains("timestamp"), "unexpected message: {}", err.message);
    }

    #[test]
    fn claim_with_far_future_timestamp_is_rejected() {
        let (state, ledger, miner, genesis) = test_env("ts_future");
        let absurd_future = unix_ms() + MAX_FUTURE_MS + 3_600_000; // an hour past the allowed window
        let claim = claim_at(&miner, 2, genesis, [0u8; 32], l1_core::rewards::empty_state_root(), [9u8; 32], absurd_future);
        let err = accept_signed_claim(&state, &ledger, claim, None, vec![]).unwrap_err();
        assert!(err.message.contains("timestamp"), "unexpected message: {}", err.message);
    }
}

// ─── Integration tests (spawn real processes) ──────────────────────────────
//
// Ignored by default: these spawn real `l1-node` processes, do real (if
// minimal) PoW mining, and use real ports and disk. Run explicitly with:
//   cargo test -p l1-node -- --ignored
#[cfg(test)]
mod integration_tests {
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    /// Kills the wrapped process on drop, so a failed assertion (which
    /// unwinds the stack) still cleans up instead of leaking a background
    /// mining node.
    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// `CARGO_BIN_EXE_<name>` is only set for integration tests (`tests/`
    /// dir); this is a unit test inside the binary's own `main.rs`, so we
    /// locate the sibling `l1-node` executable relative to the test
    /// binary's own path instead (`target/debug/deps/l1_node-*.exe` ->
    /// `target/debug/l1-node.exe`).
    fn node_exe_path() -> std::path::PathBuf {
        let test_exe = std::env::current_exe().expect("current_exe");
        let target_debug = test_exe
            .parent() // .../target/debug/deps
            .and_then(|p| p.parent()) // .../target/debug
            .expect("test binary must live under target/<profile>/deps");
        target_debug.join(format!("l1-node{}", std::env::consts::EXE_SUFFIX))
    }

    fn spawn_node(port: u16, data_dir: &std::path::Path, seeds: &str) -> ChildGuard {
        let exe = node_exe_path();
        assert!(exe.exists(), "l1-node binary not found at {exe:?} — run `cargo build -p l1-node` first");
        let child = Command::new(exe)
            .env("BINA_HTTP_PORT", port.to_string())
            .env("BINA_DATA_DIR", data_dir.to_str().unwrap())
            .env("BINA_P2P_LISTEN_ADDR", format!("127.0.0.1:{port}"))
            .env("BINA_SEEDS", seeds)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn l1-node binary");
        ChildGuard(child)
    }

    async fn wait_for_height(client: &reqwest::Client, port: u16, min_height: u64, timeout: Duration) -> Option<u64> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(resp) = client.get(format!("http://127.0.0.1:{port}/chain/status")).send().await {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    if let Some(h) = json.get("height").and_then(|v| v.as_u64()) {
                        if h >= min_height {
                            return Some(h);
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        None
    }

    /// Full lifecycle: a node mines a few blocks, gets restarted (proving
    /// the persistent store survives and keeps serving pre-restart
    /// history), then a completely fresh second node with nothing but a
    /// seed address syncs the entire chain from genesis on its own. This is
    /// the exact scenario that exposed the original in-memory-only block
    /// storage gap during manual testing.
    #[tokio::test]
    #[ignore]
    async fn fresh_node_resyncs_from_genesis_against_a_running_peer() {
        let base = std::env::temp_dir().join(format!("bina_integration_{}", std::process::id()));
        let dir_a = base.join("a");
        let dir_b = base.join("b");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();

        // Deliberately non-default ports so this never collides with a
        // manually-run node on the developer's machine.
        let port_a: u16 = 18281;
        let port_b: u16 = 18282;
        let client = reqwest::Client::builder().timeout(Duration::from_secs(3)).build().unwrap();

        // Node A mines from genesis.
        let mut node_a = spawn_node(port_a, &dir_a, "");
        let height = wait_for_height(&client, port_a, 2, Duration::from_secs(90))
            .await
            .expect("node A did not reach height 2 in time");
        assert!(height >= 2);

        // Restart it — the durable store must let it resume and keep
        // serving its pre-restart history, not just its resume pointer.
        drop(node_a);
        node_a = spawn_node(port_a, &dir_a, "");
        let resumed_height = wait_for_height(&client, port_a, height, Duration::from_secs(30))
            .await
            .expect("node A did not resume after restart");
        assert!(resumed_height >= height, "restarted node must not have lost its height");

        let early_block = client
            .get(format!("http://127.0.0.1:{port_a}/block/1"))
            .send()
            .await
            .expect("request block 1 from A")
            .json::<serde_json::Value>()
            .await
            .expect("parse block 1");
        assert_eq!(early_block["height"].as_u64(), Some(1), "restarted node must still serve pre-restart block 1");

        // A completely fresh node, seeded only with A's address, must sync
        // the entire chain from genesis on its own.
        let node_b = spawn_node(port_b, &dir_b, &format!("127.0.0.1:{port_a}"));
        let synced_height = wait_for_height(&client, port_b, resumed_height, Duration::from_secs(90))
            .await
            .expect("node B did not sync up to node A's height in time");
        assert!(synced_height >= resumed_height);

        let status_a: serde_json::Value = client
            .get(format!("http://127.0.0.1:{port_a}/chain/status"))
            .send().await.unwrap().json().await.unwrap();
        let status_b: serde_json::Value = client
            .get(format!("http://127.0.0.1:{port_b}/chain/status"))
            .send().await.unwrap().json().await.unwrap();

        assert_eq!(status_a["genesis_hash"], status_b["genesis_hash"], "must be the same chain");
        if status_a["height"] == status_b["height"] {
            assert_eq!(status_a["tip_hash"], status_b["tip_hash"], "agreeing on height must mean agreeing on the tip");
        }

        drop(node_a);
        drop(node_b);
        let _ = std::fs::remove_dir_all(&base);
    }
}

// ─── Entry point ───────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    println!("╔══════════════════════════════════════════════════════╗");
    println!("║          Bina Chain Node  (BLAKE3 PoW + Live BTC)   ║");
    println!("╚══════════════════════════════════════════════════════╝");
    println!("  threads    : {}", threads);
    println!(
        "  difficulty : {}-{} bits  (dynamic, target {}ms/block, epoch 20)",
        l1_core::difficulty::MIN_BITS,
        l1_core::difficulty::MAX_BITS,
        TARGET_BLOCK_MS
    );
    println!(
        "  supply     : {} BINA cap  |  {} BINA/block  |  halving every {} blocks",
        HARD_CAP, INITIAL_BLOCK_REWARD, HALVING_INTERVAL
    );
    println!("  api        : http://127.0.0.1:{}", http_port());
    println!();
    println!("  Endpoints:");
    println!("    GET /                          — node status + economics");
    println!("    GET /chain/supply              — supply, reward, difficulty");
    println!("    GET /chain/latest              — latest mined block");
    println!("    GET /chain/blocks              — last 20 blocks");
    println!("    POST /chain/submit             — submit a signed mined block claim");
    println!("    POST /tx/submit                — submit a signed BINA transfer");
    println!("    POST /wallet/send              — sign and submit from local node wallet");
    println!("    POST /p2p/message              — receive signed gossip messages");
    println!("    POST /p2p/hello                — peer introduction");
    println!("    POST /p2p/connect              — connect to a BINA HTTP peer");
    println!("    GET /p2p/peers                 — known peer list");
    println!("    GET /block/:height             — block by height");
    println!("    GET /randomness/latest         — latest randomness output");
    println!("    GET /wallet/:address/balance   — $BINA balance");
    println!();

    let state: SharedState = Arc::new(RwLock::new(NodeState {
        genesis_hash: [0u8; 32],
        tip_hash: [0u8; 32],
        chain_height: 0,
        pending_claims: HashMap::new(),
        mempool: HashMap::new(),
        nullifiers: NullifierSet::new(),
        total_hashes: 0,
        total_time_ms: 0,
        started_at: Instant::now(),
        threads,
        last_observed_btc: BtcEntropyState::mock(),
        btc_seed_hash: [0u8; 32],
        btc_seed_changed_at: unix_secs(),
        btc_checkpoint_tip_height: 0,
        difficulty_bits: l1_core::difficulty::MIN_BITS,
        last_block_timestamp_ms: 0,
        chain_work: 0,
        miner_address: String::new(),
        total_mined_bina: 0,
        current_reward: INITIAL_BLOCK_REWARD,
        last_adjustment: None,
    }));

    let ledger = Arc::new(Mutex::new(
        RewardLedger::open(ledger_path()).expect("failed to open reward ledger")
    ));
    let store = Arc::new(BlockStore::open(&block_store_path()).expect("failed to open block store"));
    let peers = Arc::new(PeerList::new(MAX_PEERS));
    for seed in parse_seed_peers() {
        peers.add(seed);
    }
    let gossip = Arc::new(Gossip::new(peers, NETWORK_ID));
    let p2p_listen_addr = std::env::var("BINA_P2P_LISTEN_ADDR")
        .unwrap_or_else(|_| format!("127.0.0.1:{}", http_port()));

    println!("  p2p network: {NETWORK_ID}");
    println!("  p2p listen : {p2p_listen_addr}");
    println!("  p2p seeds  : {} ({})", gossip.peers().count(), DEFAULT_SEED_PEERS.join(", "));

    let rate_limiter = RateLimiter::new();

    // Read-only endpoints: no rate limit, no body to bound.
    let open_routes = Router::<SharedState>::new()
        .route("/", get(handle_status))
        .route("/chain/status", get(handle_status))
        .route("/chain/latest", get(handle_latest_block))
        .route("/chain/blocks", get(handle_blocks_recent))
        .route("/chain/headers", get(handle_chain_headers))
        .route("/chain/supply", get(handle_supply))
        .route("/p2p/peers", get(handle_get_peers))
        .route("/block/{height}", get(handle_block))
        .route("/randomness/latest", get(handle_randomness_latest))
        .route("/randomness/{height}", get(handle_randomness_at))
        .route("/wallet/{address}/balance", get(handle_wallet_balance))
        .layer(CorsLayer::permissive());

    // Mutating endpoints: bounded body size + per-IP rate limit. Claim/tx
    // verification already rejects unworked/invalid payloads cheaply, but
    // nothing previously stopped raw request or body-size flooding.
    let mutating_routes = Router::<SharedState>::new()
        .route("/chain/submit", post(handle_submit_claim))
        .route("/tx/submit", post(handle_submit_transaction))
        .route("/wallet/send", post(handle_wallet_send))
        .route("/p2p/message", post(handle_p2p_message))
        .route("/p2p/hello", post(handle_peer_hello))
        .route("/p2p/connect", post(handle_peer_connect))
        .layer(middleware::from_fn(rate_limit_middleware))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(Extension(rate_limiter))
        .layer(CorsLayer::permissive());

    let app = open_routes
        .merge(mutating_routes)
        .layer(Extension(ledger.clone()))
        .layer(Extension(gossip.clone()))
        .layer(Extension(store.clone()))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", http_port()))
        .await
        .expect("bind failed");

    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("axum failed");
    });

    let bootstrap_gossip = gossip.clone();
    let bootstrap_listen_addr = p2p_listen_addr.clone();
    tokio::spawn(async move {
        bootstrap_gossip
            .bootstrap(&bootstrap_listen_addr, 0, "")
            .await;
    });

    mining_loop(state, ledger, gossip, store).await;
}
