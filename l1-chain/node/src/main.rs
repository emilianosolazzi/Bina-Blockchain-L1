mod bitcoin_entropy;
mod envelope;
mod gossip;
mod peers;

use axum::{
    extract::{ConnectInfo, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Extension, Json, Router,
};
use envelope::{BinaMessage, BlockClaimEnvelope, PeerHelloEnvelope};
use gossip::Gossip;
use l1_core::bitcoin_entropy::BtcEntropyState;
use l1_core::block::{genesis_block, leading_zero_bits};
use l1_core::claims::{select_winning_claim, SignedBlockClaim};
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
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tower_http::cors::CorsLayer;
use zeroize::{Zeroize, Zeroizing};

const MAX_STORED: usize = 1_000; // keep last 1 000 blocks in memory
const PORT: u16 = 8181;
const NETWORK_ID: &str = "bina-l1";
const DEFAULT_P2P_TTL: u8 = 8;
const MAX_PEERS: usize = 128;
const DEFAULT_SEED_PEERS: &[&str] = &["144.126.157.197:8181"];
const LEDGER_PATH: &str = "data/ledger.csv";
const CHAIN_STATE_PATH: &str = "data/chain-state.json";
const SUBMISSION_GRACE_MS: u64 = 1_500;
const MAX_FUTURE_BLOCK_SECS: u64 = 30;

// ─── Per-block record (stored + returned by API) ───────────────────────────

#[derive(Serialize, Clone)]
struct BlockRecord {
    height: u64,
    block_hash: String,
    nonce: u64,
    timestamp: u64,
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
}

// ─── Shared mutable node state ─────────────────────────────────────────────

struct NodeState {
    genesis_hash: [u8; 32],
    tip_hash: [u8; 32],
    chain_height: u64,
    blocks: VecDeque<BlockRecord>,
    pending_claims: HashMap<u64, HashMap<String, SignedBlockClaim>>,
    nullifiers: NullifierSet,
    total_hashes: u64,
    total_time_ms: u64,
    started_at: Instant,
    threads: usize,
    btc_height: u64,
    btc_tip: String,
    btc_seed_hash: [u8; 32],
    btc_seed_changed_at: u64,
    btc_fork: bool,
    difficulty_bits: u32,
    miner_address: String,
    // Economics
    total_mined_bina: u64,
    current_reward: u64,
    last_adjustment: Option<String>, // log line of last difficulty change
}

type SharedState = Arc<RwLock<NodeState>>;
type SharedLedger = Arc<Mutex<RewardLedger>>;

#[derive(Clone)]
struct AcceptedClaim {
    height: u64,
    miner_hex: String,
    block_hash: String,
    election_score: String,
    work_bits: u32,
}

struct ClaimReject {
    status: StatusCode,
    message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChainStateFile {
    version: u32,
    network: String,
    genesis_hash: String,
    tip_hash: String,
    height: u64,
    updated_at: u64,
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

#[derive(Debug, Clone)]
struct LoadedChainState {
    genesis_hash: [u8; 32],
    tip_hash: [u8; 32],
    height: u64,
    created: bool,
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

async fn handle_status(State(s): State<SharedState>) -> Json<serde_json::Value> {
    let s = s.read().unwrap();
    let height = s.chain_height;
    let uptime = s.started_at.elapsed().as_secs();
    let (block_time_avg_ms, block_time_stddev_ms) = rolling_block_time_stats(&s.blocks, 20);
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
        "btc_height":         s.btc_height,
        "btc_tip":            s.btc_tip,
        "btc_seed_age_secs":  btc_seed_age_secs,
        "btc_seed_changed_at": s.btc_seed_changed_at,
        "btc_fork_seen":      s.btc_fork,
        "block_time_avg_ms":  block_time_avg_ms,
        "block_time_stddev_ms": block_time_stddev_ms,
        "nullifiers_spent":   s.nullifiers.len(),
        "genesis_hash":       hex::encode(s.genesis_hash),
        "tip_hash":           hex::encode(s.tip_hash),
        "last_difficulty_adjustment": s.last_adjustment,
    }))
}

async fn handle_latest_block(
    State(s): State<SharedState>,
) -> Result<Json<BlockRecord>, StatusCode> {
    match s.read().unwrap().blocks.back().cloned() {
        Some(b) => Ok(Json(b)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn handle_block(
    State(s): State<SharedState>,
    Path(height): Path<u64>,
) -> Result<Json<BlockRecord>, StatusCode> {
    match s
        .read()
        .unwrap()
        .blocks
        .iter()
        .find(|b| b.height == height)
        .cloned()
    {
        Some(b) => Ok(Json(b)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn handle_blocks_recent(State(s): State<SharedState>) -> Json<Vec<BlockRecord>> {
    let recent: Vec<BlockRecord> = s
        .read()
        .unwrap()
        .blocks
        .iter()
        .rev()
        .take(20)
        .cloned()
        .collect();
    Json(recent)
}

async fn handle_randomness_latest(
    State(s): State<SharedState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match s.read().unwrap().blocks.back().cloned() {
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
    State(s): State<SharedState>,
    Path(height): Path<u64>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match s
        .read()
        .unwrap()
        .blocks
        .iter()
        .find(|b| b.height == height)
        .cloned()
    {
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
    Json(claim): Json<SignedBlockClaim>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let accepted = accept_signed_claim(&s, claim.clone()).map_err(claim_error_response)?;
    let envelope = BlockClaimEnvelope::from_claim(gossip.network().to_string(), DEFAULT_P2P_TTL, claim);
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
    let (height, miner_address) = {
        let state = s.read().unwrap();
        (state.chain_height, state.miner_address.clone())
    };
    let timestamp = unix_secs();

    let mut ledger = ledger
        .lock()
        .map_err(|_| json_error(StatusCode::INTERNAL_SERVER_ERROR, "ledger lock poisoned"))?;
    ledger
        .apply_transaction(height, &signed, &miner_address, timestamp)
        .map_err(|e| json_error(StatusCode::BAD_REQUEST, format!("transaction rejected: {e}")))?;

    Ok(Json(serde_json::json!({
        "status": "accepted",
        "tx_id": signed.tx_id_hex(),
        "from": signed.from_hex(),
        "to": signed.to_hex(),
        "amount": signed.tx.amount,
        "fee": signed.tx.fee,
        "nonce": signed.tx.nonce,
        "signature_mode": signature_mode,
        "from_balance_bina": ledger.balance(&signed.from_hex()),
        "to_balance_bina": ledger.balance(&signed.to_hex()),
        "height": height,
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
    let (height, miner_address) = {
        let state = s.read().unwrap();
        (state.chain_height, state.miner_address.clone())
    };
    let timestamp = unix_secs();

    let mut ledger = ledger
        .lock()
        .map_err(|_| json_error(StatusCode::INTERNAL_SERVER_ERROR, "ledger lock poisoned"))?;
    let nonce = payload.nonce.unwrap_or_else(|| ledger.nonce(&from_hex));
    let tx = Transaction::new(from, to, payload.amount, nonce, fee);
    let signed = SignedTransaction::sign(tx, &miner_keypair)
        .map_err(|e| json_error(StatusCode::BAD_REQUEST, format!("transaction signing failed: {e}")))?;
    ledger
        .apply_transaction(height, &signed, &miner_address, timestamp)
        .map_err(|e| json_error(StatusCode::BAD_REQUEST, format!("transaction rejected: {e}")))?;

    Ok(Json(serde_json::json!({
        "status": "accepted",
        "tx_id": signed.tx_id_hex(),
        "from": signed.from_hex(),
        "to": signed.to_hex(),
        "amount": signed.tx.amount,
        "fee": signed.tx.fee,
        "nonce": signed.tx.nonce,
        "tx_digest": signed.tx.digest_hex(),
        "public_key": signed.public_key_hex(),
        "signature": signed.signature_hex(),
        "signature_mode": "hybrid",
        "from_balance_bina": ledger.balance(&signed.from_hex()),
        "to_balance_bina": ledger.balance(&signed.to_hex()),
        "height": height,
    })))
}

async fn handle_p2p_message(
    State(s): State<SharedState>,
    Extension(gossip): Extension<Arc<Gossip>>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    Json(message): Json<BinaMessage>,
) -> impl IntoResponse {
    let Some(message) = gossip.handle_incoming(message, peer_addr).await else {
        return StatusCode::OK;
    };

    if let BinaMessage::BlockClaim(envelope) = message {
        match accept_signed_claim(&s, envelope.claim.clone()) {
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
        .unwrap_or_else(|_| format!("127.0.0.1:{PORT}"));
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

fn accept_signed_claim(state: &SharedState, claim: SignedBlockClaim) -> Result<AcceptedClaim, ClaimReject> {
    claim.verify().map_err(|e| {
        ClaimReject::new(
            StatusCode::BAD_REQUEST,
            format!("invalid signed claim: {e}"),
        )
    })?;

    let height = claim.header.height;
    let miner_hex = claim.miner_address_hex();
    let block_hash = claim.block_hash_hex();
    let election_score = claim.election_score_hex();
    let work_bits = claim.work_bits();

    let mut state = state.write().unwrap();
    if state.genesis_hash == [0u8; 32] {
        return Err(ClaimReject::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "genesis not initialized",
        ));
    }

    let next_height = state.chain_height + 1;
    if height != next_height {
        return Err(ClaimReject::new(
            StatusCode::CONFLICT,
            format!("claim height {height} does not match next height {next_height}"),
        ));
    }
    if claim.header.prev_hash != state.tip_hash {
        return Err(ClaimReject::new(
            StatusCode::CONFLICT,
            "claim prev_hash does not match chain tip",
        ));
    }
    if claim.header.difficulty_bits != state.difficulty_bits {
        return Err(ClaimReject::new(
            StatusCode::CONFLICT,
            "claim difficulty does not match current node difficulty",
        ));
    }
    if claim.header.bitcoin_seed_hash != state.btc_seed_hash {
        return Err(ClaimReject::new(
            StatusCode::CONFLICT,
            "claim Bitcoin seed does not match current node seed",
        ));
    }
    if claim.header.timestamp > unix_secs().saturating_add(MAX_FUTURE_BLOCK_SECS) {
        return Err(ClaimReject::new(
            StatusCode::BAD_REQUEST,
            "claim timestamp is too far in the future",
        ));
    }

    let claims = state.pending_claims.entry(height).or_default();
    if claims.contains_key(&miner_hex) {
        return Err(ClaimReject::new(
            StatusCode::CONFLICT,
            "miner already submitted a claim for this height; first valid claim is kept",
        ));
    }
    claims.insert(miner_hex.clone(), claim);

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
        "source":           LEDGER_PATH,
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

fn rolling_block_time_stats(blocks: &VecDeque<BlockRecord>, window: usize) -> (u64, u64) {
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

fn chain_state_file(genesis_hash: [u8; 32], tip_hash: [u8; 32], height: u64) -> ChainStateFile {
    ChainStateFile {
        version: 1,
        network: NETWORK_ID.to_string(),
        genesis_hash: hex::encode(genesis_hash),
        tip_hash: hex::encode(tip_hash),
        height,
        updated_at: unix_secs(),
    }
}

fn save_chain_state(path: &str, genesis_hash: [u8; 32], tip_hash: [u8; 32], height: u64) -> std::io::Result<()> {
    let path = std::path::Path::new(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = chain_state_file(genesis_hash, tip_hash, height);
    let text = serde_json::to_string_pretty(&file).expect("chain state serialization cannot fail");
    std::fs::write(path, text)
}

fn load_or_create_chain_state(path: &str, genesis_hash: [u8; 32]) -> LoadedChainState {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let file: ChainStateFile = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{path} is not valid chain state JSON: {e}"));
            if file.version != 1 {
                panic!("unsupported chain state version {} in {path}", file.version);
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
            }
        }
        Err(_) => {
            save_chain_state(path, genesis_hash, genesis_hash, 0)
                .unwrap_or_else(|e| panic!("failed to create {path}: {e}"));
            LoadedChainState {
                genesis_hash,
                tip_hash: genesis_hash,
                height: 0,
                created: true,
            }
        }
    }
}

// ─── Mining loop (runs forever) ────────────────────────────────────────────

async fn mining_loop(state: SharedState, ledger: SharedLedger, gossip: Arc<Gossip>) {
    let threads = state.read().unwrap().threads;
    let miner_keypair = load_miner_keypair();
    let miner_address: [u8; 20] = miner_keypair.address();
    let miner_hex = hex::encode(miner_address);

    // Genesis
    let genesis = genesis_block();
    let genesis_hash = genesis.header.hash();
    let chain_state = load_or_create_chain_state(CHAIN_STATE_PATH, genesis_hash);
    let persisted_total = {
        let scoped_ledger = RewardLedger::open_scoped(LEDGER_PATH, chain_state.height)
            .unwrap_or_else(|e| panic!("failed to open scoped reward ledger: {e}"));
        let total = scoped_ledger.total_mined();
        *ledger.lock().unwrap() = scoped_ledger;
        total
    };
    {
        let mut s = state.write().unwrap();
        s.genesis_hash = chain_state.genesis_hash;
        s.tip_hash = chain_state.tip_hash;
        s.chain_height = chain_state.height;
        s.miner_address = miner_hex.clone();
        s.total_mined_bina = persisted_total;
        s.current_reward = block_reward(chain_state.height + 1, persisted_total);
        s.difficulty_bits = l1_core::difficulty::MIN_BITS;
    }
    println!("[genesis]  hash={}…", hex::encode(&genesis_hash[..16]));
    println!(
        "[chain]    {} height={} tip={}… state={}",
        if chain_state.created { "initialized" } else { "resumed" },
        chain_state.height,
        hex::encode(&chain_state.tip_hash[..16]),
        CHAIN_STATE_PATH,
    );
    println!("[wallet]   mining to: {miner_hex}");
    println!("[ledger]   persistent total mined: {persisted_total} BINA");
    println!(
        "[supply]   hard cap: {} BINA  |  initial reward: {} BINA  |  halving: every {} blocks",
        HARD_CAP, INITIAL_BLOCK_REWARD, HALVING_INTERVAL
    );
    println!();

    let mut prev_hash = chain_state.tip_hash;
    let mut height: u64 = chain_state.height;
    let mut btc = fetch_btc(None).await;
    let mut btc_seed_changed_at = unix_secs();
    log_btc(&btc);

    let now_ms = || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    };

    let mut adjuster = DifficultyAdjuster::new(l1_core::difficulty::MIN_BITS, now_ms());

    loop {
        height += 1;

        let previous_btc_seed = btc.bitcoin_seed_hash();
        let fresh = fetch_btc(Some(&btc)).await;
        let fresh_seed = fresh.bitcoin_seed_hash();
        if fresh_seed != previous_btc_seed {
            btc_seed_changed_at = unix_secs();
            println!(
                "[btc] new seed at height {}  (was {})",
                fresh.tip_height, btc.tip_height
            );
            log_btc(&fresh);
        }
        btc = fresh;

        let current_bits = adjuster.current_bits();
        let btc_seed = btc.bitcoin_seed_hash();
        let btc_height_now = btc.tip_height;
        let btc_fork_now = btc.fork_detected;
        {
            let mut s = state.write().unwrap();
            s.difficulty_bits = current_bits;
            s.btc_height = btc_height_now;
            s.btc_tip = hex::encode(&btc.tip_hash[..8]);
            s.btc_seed_hash = btc_seed;
            s.btc_seed_changed_at = btc_seed_changed_at;
            s.btc_fork = s.btc_fork || btc_fork_now;
        }

        let total_mined = state.read().unwrap().total_mined_bina;
        let reward = block_reward(height, total_mined);

        let (ph, ma, bs, cb) = (prev_hash, miner_address, btc_seed, current_bits);
        let result =
            tokio::task::spawn_blocking(move || mine_block(height, ph, ma, bs, cb, threads))
                .await
                .expect("mine_block panicked");

        let local_claim = SignedBlockClaim::sign(result.block.header.clone(), &miner_keypair);
        local_claim
            .verify()
            .expect("locally mined signed claim must verify");
        let local_block_hash = local_claim.block_hash();

        {
            let mut s = state.write().unwrap();
            let claims = s.pending_claims.entry(height).or_default();
            claims
                .entry(miner_hex.clone())
                .or_insert_with(|| local_claim.clone());
        }

        gossip
            .broadcast_claim(BlockClaimEnvelope::from_claim(
                gossip.network().to_string(),
                DEFAULT_P2P_TTL,
                local_claim,
            ))
            .await;

        tokio::time::sleep(Duration::from_millis(SUBMISSION_GRACE_MS)).await;

        let winning_claim = {
            let mut s = state.write().unwrap();
            let candidates: Vec<SignedBlockClaim> = s
                .pending_claims
                .remove(&height)
                .unwrap_or_default()
                .into_values()
                .collect();
            s.pending_claims
                .retain(|candidate_height, _| *candidate_height > height);
            select_winning_claim(candidates)
                .expect("local signed claim missing from candidate pool")
        };

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

        // Difficulty adjustment (fires every 20 blocks)
        let adj_info = adjuster.record_block(height, now_ms());
        if let Some(ref info) = adj_info {
            let log = DifficultyAdjuster::adjustment_log(info);
            println!("{log}");
            state.write().unwrap().last_adjustment = Some(log);
        }

        // Persist credit to ledger CSV
        let mut ledger = ledger.lock().unwrap();
        ledger
            .credit(height, &winner_miner_hex, reward, timestamp)
            .unwrap_or_else(|e| {
                eprintln!("[ledger] write error: {e}");
                0
            });

        if let Err(e) = save_chain_state(CHAIN_STATE_PATH, genesis_hash, block_hash, height) {
            eprintln!("[chain] state write error: {e}");
        }

        let record = BlockRecord {
            height,
            block_hash: hex::encode(block_hash),
            nonce: winning_claim.header.nonce,
            timestamp,
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
            btc_height: btc_height_now,
        };

        println!(
            "[h={:<6}]  hash={}…  {:.2} MH/s  {}ms  +{} BINA  diff={}  winner={}…  source={}",
            height,
            &record.block_hash[..12],
            record.hashrate_mhs,
            elapsed_ms,
            reward,
            current_bits,
            &record.miner_address[..12],
            record.source,
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
            s.btc_height = btc_height_now;
            s.btc_tip = hex::encode(&btc.tip_hash[..8]);
            s.btc_seed_hash = btc_seed;
            s.btc_seed_changed_at = btc_seed_changed_at;
            s.btc_fork = s.btc_fork || btc_fork_now;
            if s.blocks.len() >= MAX_STORED {
                s.blocks.pop_front();
            }
            s.blocks.push_back(record);
        }

        prev_hash = block_hash;
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
    println!("  api        : http://127.0.0.1:{}", PORT);
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
        blocks: VecDeque::new(),
        pending_claims: HashMap::new(),
        nullifiers: NullifierSet::new(),
        total_hashes: 0,
        total_time_ms: 0,
        started_at: Instant::now(),
        threads,
        btc_height: 0,
        btc_tip: String::new(),
        btc_seed_hash: [0u8; 32],
        btc_seed_changed_at: unix_secs(),
        btc_fork: false,
        difficulty_bits: l1_core::difficulty::MIN_BITS,
        miner_address: String::new(),
        total_mined_bina: 0,
        current_reward: INITIAL_BLOCK_REWARD,
        last_adjustment: None,
    }));

    let ledger = Arc::new(Mutex::new(
        RewardLedger::open(LEDGER_PATH).expect("failed to open reward ledger")
    ));
    let peers = Arc::new(PeerList::new(MAX_PEERS));
    for seed in parse_seed_peers() {
        peers.add(seed);
    }
    let gossip = Arc::new(Gossip::new(peers, NETWORK_ID));
    let p2p_listen_addr = std::env::var("BINA_P2P_LISTEN_ADDR")
        .unwrap_or_else(|_| format!("127.0.0.1:{PORT}"));

    println!("  p2p network: {NETWORK_ID}");
    println!("  p2p listen : {p2p_listen_addr}");
    println!("  p2p seeds  : {} ({})", gossip.peers().count(), DEFAULT_SEED_PEERS.join(", "));

    let app = Router::new()
        .route("/", get(handle_status))
        .route("/chain/status", get(handle_status))
        .route("/chain/latest", get(handle_latest_block))
        .route("/chain/blocks", get(handle_blocks_recent))
        .route("/chain/submit", post(handle_submit_claim))
        .route("/chain/supply", get(handle_supply))
        .route("/tx/submit", post(handle_submit_transaction))
        .route("/wallet/send", post(handle_wallet_send))
        .route("/p2p/message", post(handle_p2p_message))
        .route("/p2p/hello", post(handle_peer_hello))
        .route("/p2p/connect", post(handle_peer_connect))
        .route("/p2p/peers", get(handle_get_peers))
        .route("/block/{height}", get(handle_block))
        .route("/randomness/latest", get(handle_randomness_latest))
        .route("/randomness/{height}", get(handle_randomness_at))
        .route("/wallet/{address}/balance", get(handle_wallet_balance))
        .layer(CorsLayer::permissive())
        .layer(Extension(ledger.clone()))
        .layer(Extension(gossip.clone()))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{PORT}"))
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

    mining_loop(state, ledger, gossip).await;
}
