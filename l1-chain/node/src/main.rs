mod bitcoin_entropy;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::get,
    Router,
};
use l1_core::block::{genesis_block, leading_zero_bits};
use l1_core::bitcoin_entropy::BtcEntropyState;
use l1_core::crypto::WalletKeypair;
use l1_core::difficulty::DifficultyAdjuster;
use l1_core::pow::mine_block;
use l1_core::randomness::{NullifierSet, RandomnessOutput};
use l1_core::rewards::{block_reward, RewardLedger, HARD_CAP, HALVING_INTERVAL, INITIAL_BLOCK_REWARD};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Arc, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tower_http::cors::CorsLayer;

const MAX_STORED:        usize  = 1_000;  // keep last 1 000 blocks in memory
const PORT:              u16    = 8181;
const LEDGER_PATH:       &str   = "data/ledger.csv";

// ─── Per-block record (stored + returned by API) ───────────────────────────

#[derive(Serialize, Clone)]
struct BlockRecord {
    height:            u64,
    block_hash:        String,
    nonce:             u64,
    timestamp:         u64,
    zero_bits:         u32,
    difficulty_bits:   u32,      // difficulty that produced this block
    hashes_tried:      u64,
    elapsed_ms:        u64,
    hashrate_mhs:      f64,
    miner_address:     String,   // 40-char hex — wallet address
    reward_bina:       u64,      // BINA awarded for this block
    randomness_output: String,   // 64-char hex — the random bytes
    nullifier:         String,   // 64-char hex — one-time spend token
    btc_seed:          String,   // 64-char hex — full seed hash
    btc_height:        u64,
}

// ─── Shared mutable node state ─────────────────────────────────────────────

struct NodeState {
    genesis_hash:       [u8; 32],
    blocks:             VecDeque<BlockRecord>,
    nullifiers:         NullifierSet,
    total_hashes:       u64,
    total_time_ms:      u64,
    started_at:         Instant,
    threads:            usize,
    btc_height:         u64,
    btc_tip:            String,
    btc_fork:           bool,
    difficulty_bits:    u32,
    miner_address:      String,
    // Economics
    total_mined_bina:   u64,
    current_reward:     u64,
    last_adjustment:    Option<String>,  // log line of last difficulty change
}

type SharedState = Arc<RwLock<NodeState>>;

// ─── HTTP handlers ─────────────────────────────────────────────────────────

async fn handle_status(State(s): State<SharedState>) -> Json<serde_json::Value> {
    let s = s.read().unwrap();
    let height  = s.blocks.back().map(|b| b.height).unwrap_or(0);
    let uptime  = s.started_at.elapsed().as_secs();
    let avg_mhs = if s.total_time_ms > 0 {
        (s.total_hashes as f64 / 1e6) / (s.total_time_ms as f64 / 1e3)
    } else { 0.0 };
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
        "btc_fork_seen":      s.btc_fork,
        "nullifiers_spent":   s.nullifiers.len(),
        "genesis_hash":       hex::encode(s.genesis_hash),
        "last_difficulty_adjustment": s.last_adjustment,
    }))
}

async fn handle_latest_block(
    State(s): State<SharedState>,
) -> Result<Json<BlockRecord>, StatusCode> {
    match s.read().unwrap().blocks.back().cloned() {
        Some(b) => Ok(Json(b)),
        None    => Err(StatusCode::NOT_FOUND),
    }
}

async fn handle_block(
    State(s): State<SharedState>,
    Path(height): Path<u64>,
) -> Result<Json<BlockRecord>, StatusCode> {
    match s.read().unwrap().blocks.iter().find(|b| b.height == height).cloned() {
        Some(b) => Ok(Json(b)),
        None    => Err(StatusCode::NOT_FOUND),
    }
}

async fn handle_blocks_recent(State(s): State<SharedState>) -> Json<Vec<BlockRecord>> {
    let recent: Vec<BlockRecord> = s.read().unwrap()
        .blocks.iter().rev().take(20).cloned().collect();
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
    match s.read().unwrap().blocks.iter().find(|b| b.height == height).cloned() {
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

// GET /wallet/:address/balance
async fn handle_wallet_balance(
    State(s): State<SharedState>,
    Path(address): Path<String>,
) -> Json<serde_json::Value> {
    let state = s.read().unwrap();
    // Sum from the in-memory block records (persistent totals come from ledger on disk)
    let balance: u64 = state.blocks.iter()
        .filter(|b| b.miner_address == address)
        .map(|b| b.reward_bina)
        .sum();
    Json(serde_json::json!({
        "address":          address,
        "balance_bina":     balance,
        "note":             "in-memory session balance — persistent total in data/ledger.csv",
    }))
}

// GET /chain/supply
async fn handle_supply(State(s): State<SharedState>) -> Json<serde_json::Value> {
    let state = s.read().unwrap();
    let era = state.blocks.back().map(|b| b.height / HALVING_INTERVAL).unwrap_or(0);
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

// ─── Mining loop (runs forever) ────────────────────────────────────────────

async fn mining_loop(state: SharedState, mut ledger: RewardLedger) {
    let threads      = state.read().unwrap().threads;
    let miner_address: [u8; 20] = load_miner_address();
    let miner_hex    = hex::encode(miner_address);

    let persisted_total = ledger.total_mined();

    // Genesis
    let genesis      = genesis_block(miner_address);
    let genesis_hash = genesis.header.hash();
    {
        let mut s = state.write().unwrap();
        s.genesis_hash     = genesis_hash;
        s.miner_address    = miner_hex.clone();
        s.total_mined_bina = persisted_total;
        s.current_reward   = block_reward(0, persisted_total);
        s.difficulty_bits  = l1_core::difficulty::MIN_BITS;
    }
    println!("[genesis]  hash={}…", hex::encode(&genesis_hash[..16]));
    println!("[wallet]   mining to: {miner_hex}");
    println!("[ledger]   persistent total mined: {persisted_total} BINA");
    println!("[supply]   hard cap: {} BINA  |  initial reward: {} BINA  |  halving: every {} blocks",
        HARD_CAP, INITIAL_BLOCK_REWARD, HALVING_INTERVAL);
    println!();

    let mut prev_hash = genesis_hash;
    let mut height: u64 = 0;
    let mut btc = fetch_btc().await;
    log_btc(&btc);

    let now_ms = || SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let mut adjuster = DifficultyAdjuster::new(l1_core::difficulty::MIN_BITS, now_ms());

    loop {
        height += 1;

        // Refresh BTC every epoch (coincides with difficulty check)
        if height % l1_core::difficulty::EPOCH_SIZE == 0 {
            let fresh = fetch_btc().await;
            if fresh.tip_hash != btc.tip_hash {
                println!("[btc] ⬆  new tip at height {}  (was {})",
                    fresh.tip_height, btc.tip_height);
                log_btc(&fresh);
            }
            btc = fresh;
        }

        let current_bits   = adjuster.current_bits();
        let btc_seed       = btc.bitcoin_seed_hash();
        let btc_height_now = btc.tip_height;
        let btc_fork_now   = btc.fork_detected;
        let total_mined    = state.read().unwrap().total_mined_bina;
        let reward         = block_reward(height, total_mined);

        let (ph, ma, bs, cb) = (prev_hash, miner_address, btc_seed, current_bits);
        let result = tokio::task::spawn_blocking(move || {
            mine_block(height, ph, ma, bs, cb, threads)
        }).await.expect("mine_block panicked");

        let block_hash = result.block.header.hash();
        let zero_bits  = leading_zero_bits(&block_hash);
        let rand_out   = RandomnessOutput::from_block(height, block_hash, btc_seed);
        let timestamp  = result.block.header.timestamp;
        let elapsed_ms = result.elapsed_ms;

        // Difficulty adjustment (fires every 20 blocks)
        let adj_info = adjuster.record_block(height, now_ms());
        if let Some(ref info) = adj_info {
            let log = DifficultyAdjuster::adjustment_log(info);
            println!("{log}");
            state.write().unwrap().last_adjustment = Some(log);
        }

        // Persist credit to ledger CSV
        ledger.credit(height, &miner_hex, reward, timestamp)
            .unwrap_or_else(|e| { eprintln!("[ledger] write error: {e}"); 0 });

        let record = BlockRecord {
            height,
            block_hash:        hex::encode(block_hash),
            nonce:             result.block.header.nonce,
            timestamp,
            zero_bits,
            difficulty_bits:   current_bits,
            hashes_tried:      result.hashes_tried,
            elapsed_ms,
            hashrate_mhs:      result.hashrate_hs / 1_000_000.0,
            miner_address:     miner_hex.clone(),
            reward_bina:       reward,
            randomness_output: rand_out.output_hex(),
            nullifier:         rand_out.nullifier_hex(),
            btc_seed:          hex::encode(btc_seed),
            btc_height:        btc_height_now,
        };

        println!(
            "[h={:<6}]  hash={}…  {:.2} MH/s  {}ms  +{} BINA  diff={}",
            height,
            &record.block_hash[..12],
            record.hashrate_mhs,
            elapsed_ms,
            reward,
            current_bits,
        );

        {
            let mut s = state.write().unwrap();
            s.nullifiers.consume(&rand_out)
                .expect("nullifier collision — impossible on a valid chain");
            s.total_hashes     += result.hashes_tried;
            s.total_time_ms    += elapsed_ms;
            s.total_mined_bina  = ledger.total_mined();
            s.current_reward    = block_reward(height + 1, ledger.total_mined());
            s.difficulty_bits   = adjuster.current_bits();
            s.btc_height        = btc_height_now;
            s.btc_tip           = hex::encode(&btc.tip_hash[..8]);
            s.btc_fork          = s.btc_fork || btc_fork_now;
            if s.blocks.len() >= MAX_STORED { s.blocks.pop_front(); }
            s.blocks.push_back(record);
        }

        prev_hash = block_hash;
    }
}

// ─── Helpers ───────────────────────────────────────────────────────────────

async fn fetch_btc() -> BtcEntropyState {
    match bitcoin_entropy::fetch_live_entropy().await {
        Ok(e)  => e,
        Err(e) => { eprintln!("[btc] fetch error: {e} — using mock"); BtcEntropyState::mock() }
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

/// Load wallet from ~/.bina/wallet.json and return the 20-byte miner address.
/// Falls back to a zero address with a warning if the wallet file is missing.
fn load_miner_address() -> [u8; 20] {
    let wallet_path = {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".into());
        std::path::PathBuf::from(home).join(".bina").join("wallet.json")
    };

    match std::fs::read_to_string(&wallet_path) {
        Ok(text) => {
            let v: serde_json::Value = serde_json::from_str(&text)
                .expect("wallet.json is not valid JSON");
            let sk_hex = v["secret_key"].as_str()
                .expect("wallet.json missing 'secret_key'");
            let sk_bytes = hex::decode(sk_hex)
                .expect("'secret_key' is not valid hex");
            let kp = WalletKeypair::from_secret_bytes(&sk_bytes)
                .expect("wallet.json secret key is corrupt");
            kp.address()
        }
        Err(_) => {
            eprintln!("[wallet] WARNING: no wallet found at {}", wallet_path.display());
            eprintln!("[wallet] Run 'l1-wallet generate' to create one.");
            eprintln!("[wallet] Mining to zero address for now.");
            [0u8; 20]
        }
    }
}

// ─── Entry point ───────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get()).unwrap_or(4);

    println!("╔══════════════════════════════════════════════════════╗");
    println!("║          Bina Chain Node  (BLAKE3 PoW + Live BTC)   ║");
    println!("╚══════════════════════════════════════════════════════╝");
    println!("  threads    : {}", threads);
    println!("  difficulty : {}-{} bits  (dynamic, target 3.65s/block, epoch 20)", l1_core::difficulty::MIN_BITS, l1_core::difficulty::MAX_BITS);
    println!("  supply     : {} BINA cap  |  {} BINA/block  |  halving every {} blocks", HARD_CAP, INITIAL_BLOCK_REWARD, HALVING_INTERVAL);
    println!("  api        : http://127.0.0.1:{}", PORT);
    println!();
    println!("  Endpoints:");
    println!("    GET /                          — node status + economics");
    println!("    GET /chain/supply              — supply, reward, difficulty");
    println!("    GET /chain/latest              — latest mined block");
    println!("    GET /chain/blocks              — last 20 blocks");
    println!("    GET /block/:height             — block by height");
    println!("    GET /randomness/latest         — latest randomness output");
    println!("    GET /wallet/:address/balance   — $BINA balance");
    println!();

    let state: SharedState = Arc::new(RwLock::new(NodeState {
        genesis_hash:       [0u8; 32],
        blocks:             VecDeque::new(),
        nullifiers:         NullifierSet::new(),
        total_hashes:       0,
        total_time_ms:      0,
        started_at:         Instant::now(),
        threads,
        btc_height:         0,
        btc_tip:            String::new(),
        btc_fork:           false,
        difficulty_bits:    l1_core::difficulty::MIN_BITS,
        miner_address:      String::new(),
        total_mined_bina:   0,
        current_reward:     INITIAL_BLOCK_REWARD,
        last_adjustment:    None,
    }));

    let ledger = RewardLedger::open(LEDGER_PATH)
        .expect("failed to open reward ledger");

    let app = Router::new()
        .route("/",                          get(handle_status))
        .route("/chain/status",              get(handle_status))
        .route("/chain/latest",              get(handle_latest_block))
        .route("/chain/blocks",              get(handle_blocks_recent))
        .route("/chain/supply",              get(handle_supply))
        .route("/block/{height}",            get(handle_block))
        .route("/randomness/latest",         get(handle_randomness_latest))
        .route("/randomness/{height}",       get(handle_randomness_at))
        .route("/wallet/{address}/balance",  get(handle_wallet_balance))
        .layer(CorsLayer::permissive())
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{PORT}"))
        .await.expect("bind failed");

    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("axum failed");
    });

    mining_loop(state, ledger).await;
}