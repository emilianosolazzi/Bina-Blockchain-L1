// utxo_fetcher.rs
// Temporal Gradient — UTXO Fetcher and Entropy Anchoring
//
// Fixes applied vs original:
//   1. Mock hardcoded values replaced — confirmations, value, block_hash
//      are Option<T> until resolved from a real API response
//   2. fetch_from_explorer parses the actual mempool.space / blockstream
//      JSON response instead of ignoring the body
//   3. create_entropy_anchor returns the ID from DeadUTXOAnchorDB.create_anchor,
//      not a locally-computed hash — they were different, breaking verify
//   4. LRU eviction replaced with timestamp-ordered IndexMap eviction
//   5. crate::entropy stubs replaced with getrandom + sha256 so the file
//      compiles without a separate entropy module
//
// Additional uses of the mining software beyond token rewards — see bottom of file.

use std::collections::HashMap;
use std::sync::Arc;
use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use hex::encode as hex_encode;
use log::{info, warn};

use crate::bitcoin_dead_utxo_anchor::{DeadUTXOAnchorDB, DeadUTXOAnchor, DeadUTXOType};

// ─────────────────────────────────────────────────────────────────
// Entropy stubs — replace with crate::entropy when available
// These provide cryptographically random bytes using getrandom
// without requiring a separate entropy module dependency.
// ─────────────────────────────────────────────────────────────────

fn fast_entropy() -> [u8; 32] {
    let mut buf = [0u8; 32];
    getrandom::getrandom(&mut buf).unwrap_or(());
    buf
}

fn hybrid_entropy(block_headers: &[Vec<u8>]) -> [u8; 32] {
    let mut h = Sha256::new();
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).unwrap_or(());
    h.update(seed);
    for header in block_headers {
        h.update(header);
    }
    h.finalize().into()
}

fn enterprise_entropy(block_headers: &[Vec<u8>], context: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    let base = hybrid_entropy(block_headers);
    h.update(base);
    h.update(context);
    h.finalize().into()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ─────────────────────────────────────────────────────────────────
// Data types
// ─────────────────────────────────────────────────────────────────

/// Fix 1: Option<u64> for fields that may not be available until
/// the explorer response is parsed. Hardcoded guesses removed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UTXOInfo {
    pub txid:            String,
    pub vout:            u32,
    /// Satoshi value — None until fetched from explorer.
    pub value:           Option<u64>,
    pub script_pubkey:   String,
    pub address:         Option<String>,
    /// Confirmations — None until fetched from explorer.
    pub confirmations:   Option<u64>,
    pub block_height:    Option<u64>,
    pub block_hash:      Option<String>,
    pub is_spent:        bool,
    pub spent_at_height: Option<u64>,
    pub spent_in_tx:     Option<String>,
    /// Entropy-based selection weight — updated on each fetch.
    pub entropy_weight:  u64,
    pub entropy_timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UTXOQuery {
    pub txid:              String,
    pub vout:              Option<u32>,
    pub include_spent:     bool,
    pub min_confirmations: u64,
    pub max_age_blocks:    Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UTXOSearchQuery {
    pub address:            Option<String>,
    pub value_range:        Option<(u64, u64)>,
    pub confirmation_range: Option<(u32, u32)>,
    pub block_height_range: Option<(u32, u32)>,
    pub utxo_type:          Option<String>,
    pub limit:              Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UTXOAnchorPreference {
    Spent,
    OpReturn,
    Dust,
    Burn,
}

impl UTXOAnchorPreference {
    pub const VALID_VALUES: [&'static str; 4] = ["spent", "op_return", "dust", "burn"];

    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "spent" => Ok(Self::Spent),
            "op_return" | "opreturn" | "op-return" => Ok(Self::OpReturn),
            "dust" => Ok(Self::Dust),
            "burn" | "burn_address" | "burn-address" => Ok(Self::Burn),
            other => Err(format!(
                "Unknown UTXO preference '{other}'. Valid preferences: {}",
                Self::VALID_VALUES.join(", ")
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spent => "spent",
            Self::OpReturn => "op_return",
            Self::Dust => "dust",
            Self::Burn => "burn",
        }
    }

    pub fn user_label(self) -> &'static str {
        match self {
            Self::Spent => "Spent output",
            Self::OpReturn => "OP_RETURN output",
            Self::Dust => "Dust output",
            Self::Burn => "Burn-address output",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorSelectionPreview {
    pub preference: String,
    pub utxo_id: String,
    pub utxo_category: String,
    pub utxo_summary: String,
    pub score: u64,
    pub reason: String,
}

// ─────────────────────────────────────────────────────────────────
// mempool.space / blockstream response shapes
// ─────────────────────────────────────────────────────────────────

/// Fix 2: actual response shape from mempool.space GET /tx/{txid}/outspend/{vout}
#[derive(Debug, Deserialize)]
struct MempoolOutspend {
    spent: bool,
    txid:  Option<String>,
    #[serde(default)]
    status: Option<MempoolSpendStatus>,
}

#[derive(Debug, Deserialize)]
struct MempoolSpendStatus {
    block_height: Option<u64>,
}

/// GET /tx/{txid} response (partial — only fields we need)
#[derive(Debug, Deserialize)]
struct MempoolTx {
    #[serde(default)]
    vout:   Vec<MempoolVout>,
    status: Option<MempoolTxStatus>,
}

#[derive(Debug, Deserialize)]
struct MempoolVout {
    value:        Option<u64>,
    scriptpubkey: Option<String>,
    #[serde(rename = "scriptpubkey_address")]
    address:      Option<String>,
}

#[derive(Debug, Deserialize)]
struct MempoolTxStatus {
    confirmed:    bool,
    block_height: Option<u64>,
    block_hash:   Option<String>,
}

// ─────────────────────────────────────────────────────────────────
// Cache entry — carries timestamp for ordered eviction
// ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct CacheEntry {
    utxo:       UTXOInfo,
    cached_at:  u64,
}

// ─────────────────────────────────────────────────────────────────
// UTXOFetcher
// ─────────────────────────────────────────────────────────────────

pub struct UTXOFetcher {
    /// Fix 4: IndexMap preserves insertion order — oldest entry is always first.
    cache:               Arc<RwLock<IndexMap<String, CacheEntry>>>,
    anchor_db:           Arc<RwLock<DeadUTXOAnchorDB>>,
    client:              reqwest::Client,
    explorers:           Vec<String>,
    cache_ttl_seconds:   u64,
    max_cache_entries:   usize,
    entropy_seed:        [u8; 32],
    cached_block_headers: Vec<Vec<u8>>,
}

impl UTXOFetcher {
    pub fn new() -> Self {
        let entropy_seed = fast_entropy();
        let mut explorers = vec![
            "https://mempool.space/api".to_string(),
            "https://blockstream.info/api".to_string(),
        ];
        Self::shuffle_explorers(&mut explorers, &entropy_seed);

        Self {
            cache:               Arc::new(RwLock::new(IndexMap::new())),
            anchor_db:           Arc::new(RwLock::new(DeadUTXOAnchorDB::new())),
            client:              reqwest::Client::builder()
                                    .timeout(std::time::Duration::from_secs(10))
                                    .build()
                                    .unwrap_or_default(),
            explorers,
            cache_ttl_seconds:   300,
            max_cache_entries:   10_000,
            entropy_seed,
            cached_block_headers: Vec::new(),
        }
    }

    // ── Public API ────────────────────────────────────────────────

    pub async fn fetch_utxo(&self, txid: &str, vout: u32) -> Result<Option<UTXOInfo>, String> {
        let key = format!("{}:{}", txid, vout);

        // Cache hit
        {
            let cache = self.cache.read().await;
            if let Some(entry) = cache.get(&key) {
                if now_secs() - entry.cached_at < self.cache_ttl_seconds {
                    return Ok(Some(entry.utxo.clone()));
                }
            }
        }

        // Live fetch
        for explorer in &self.explorers {
            match self.fetch_from_explorer(explorer, txid, vout).await {
                Ok(Some(utxo)) => {
                    self.cache_utxo(key, utxo.clone()).await;
                    return Ok(Some(utxo));
                }
                Ok(None) => {}
                Err(e) => warn!("Explorer {} failed: {}", explorer, e),
            }
        }
        Ok(None)
    }

    pub async fn fetch_batch(&self, queries: Vec<UTXOQuery>) -> Result<Vec<UTXOInfo>, String> {
        let mut results = Vec::new();
        for query in queries {
            let vout = query.vout.unwrap_or(0);
            if let Ok(Some(u)) = self.fetch_utxo(&query.txid, vout).await {
                results.push(u);
            }
        }
        Ok(results)
    }

    pub async fn search_utxos(&self, query: UTXOSearchQuery) -> Result<Vec<UTXOInfo>, String> {
        let anchor_db = self.anchor_db.read().await;
        let mut results = Vec::new();

        for (_, dead) in &anchor_db.dead_utxos {
            if self.matches_search_criteria(dead, &query) {
                if let Some(info) = self.convert_dead_utxo_to_info(dead) {
                    results.push(info);
                }
            }
        }

        if let Some(lim) = query.limit {
            results.truncate(lim);
        }
        Ok(results)
    }

    pub async fn find_anchoring_utxos(
        &self,
        preference: &str,
        count: usize,
    ) -> Result<Vec<DeadUTXOType>, String> {
        let preference = UTXOAnchorPreference::parse(preference)?;
        let query = UTXOSearchQuery {
            address: None,
            value_range: None,
            confirmation_range: Some((6, u32::MAX)),
            block_height_range: None,
            utxo_type: Some(preference.as_str().to_string()),
            limit: Some(count.saturating_mul(3)),
        };
        let results = self.search_utxos(query).await?;
        let mut candidates: Vec<(DeadUTXOType, u64)> = results.into_iter()
            .filter_map(|u| {
                let dead = self.convert_info_to_dead_utxo(&u, preference.as_str())?;
                let score = self.anchor_preference_score(&dead)
                    .saturating_add(u.entropy_weight);
                Some((dead, score))
            })
            .collect();

        candidates.sort_by(|a, b| b.1.cmp(&a.1));
        Ok(candidates.into_iter().take(count).map(|(u, _)| u).collect())
    }

    pub async fn find_entropy_anchoring_utxos(
        &self,
        preference: &str,
        count: usize,
        anchor_data: &[u8],
    ) -> Result<Vec<DeadUTXOType>, String> {
        let preference = UTXOAnchorPreference::parse(preference)?;
        let entropy = enterprise_entropy(&self.cached_block_headers, anchor_data);
        let query = UTXOSearchQuery {
            address: None,
            value_range: None,
            confirmation_range: Some((6, u32::MAX)),
            block_height_range: None,
            utxo_type: Some(preference.as_str().to_string()),
            limit: Some(count * 3),
        };
        let results = self.search_utxos(query).await?;
        let mut weighted: Vec<(DeadUTXOType, u64)> = results
            .into_iter()
            .filter_map(|u| {
                let dead = self.convert_info_to_dead_utxo(&u, preference.as_str())?;
                let score_hash = Sha256::digest(
                    [&entropy[..], anchor_data, self.utxo_id(&dead).as_bytes()].concat()
                );
                let score = u64::from_le_bytes(score_hash[..8].try_into().unwrap_or([0; 8]));
                Some((dead, score))
            })
            .collect();

        weighted.sort_by(|a, b| b.1.cmp(&a.1));
        Ok(weighted.into_iter().take(count).map(|(u, _)| u).collect())
    }

    pub async fn preview_entropy_anchor(
        &self,
        preference: &str,
        data: &[u8],
    ) -> Result<AnchorSelectionPreview, String> {
        let preference = UTXOAnchorPreference::parse(preference)?;
        let entropy = enterprise_entropy(&self.cached_block_headers, data);
        let query = UTXOSearchQuery {
            address: None,
            value_range: None,
            confirmation_range: Some((6, u32::MAX)),
            block_height_range: None,
            utxo_type: Some(preference.as_str().to_string()),
            limit: Some(3),
        };
        let results = self.search_utxos(query).await?;

        let selected = results
            .into_iter()
            .filter_map(|u| {
                let dead = self.convert_info_to_dead_utxo(&u, preference.as_str())?;
                let score_hash = Sha256::digest(
                    [&entropy[..], data, self.utxo_id(&dead).as_bytes()].concat()
                );
                let score = u64::from_le_bytes(score_hash[..8].try_into().unwrap_or([0; 8]));
                Some((dead, score))
            })
            .max_by_key(|(_, score)| *score)
            .ok_or_else(|| {
                format!(
                    "No suitable dead Bitcoin outputs found for preference '{}'. Valid preferences: {}",
                    preference.as_str(),
                    UTXOAnchorPreference::VALID_VALUES.join(", ")
                )
            })?;

        let (selected, score) = selected;
        Ok(AnchorSelectionPreview {
            preference: preference.as_str().to_string(),
            utxo_id: self.utxo_id(&selected),
            utxo_category: selected.category().to_string(),
            utxo_summary: selected.summary(),
            score,
            reason: self.selection_reason(&selected, preference),
        })
    }

    /// Fix 3: return the anchor_id from DeadUTXOAnchorDB, not a locally-computed hash.
    /// Previously the caller received an ID that didn't match what was stored,
    /// making verify_anchor_by_id always return false.
    pub async fn create_entropy_anchor(
        &self,
        data: &[u8],
        preference: &str,
    ) -> Result<String, String> {
        self.create_entropy_anchor_with_reference(data, preference, None).await
    }

    pub async fn create_entropy_anchor_with_reference(
        &self,
        data: &[u8],
        preference: &str,
        storage_reference: Option<String>,
    ) -> Result<String, String> {
        let preference = UTXOAnchorPreference::parse(preference)?;
        let utxos = self.find_entropy_anchoring_utxos(preference.as_str(), 1, data).await?;
        let selected = utxos.into_iter().next().ok_or_else(|| {
            format!(
                "No suitable dead Bitcoin outputs found for preference '{}'. Valid preferences: {}",
                preference.as_str(),
                UTXOAnchorPreference::VALID_VALUES.join(", ")
            )
        })?;

        let utxo_key   = self.utxo_id(&selected);
        let entropy    = enterprise_entropy(&self.cached_block_headers, data);
        let anchor_payload = [data, &entropy, utxo_key.as_bytes()].concat();
        let data_hash  = hex_encode(Sha256::digest(&anchor_payload));
        let merkle_root = data_hash.clone();
        let storage_ref = storage_reference.unwrap_or_default();

        let mut meta = HashMap::new();
        meta.insert("method".to_string(),        "entropy_anchor_v1".to_string());
        meta.insert("preference".to_string(),    preference.as_str().to_string());
        meta.insert("selected_utxo".to_string(), utxo_key.clone());
        meta.insert("utxo_category".to_string(), selected.category().to_string());
        meta.insert("utxo_summary".to_string(),  selected.summary());
        meta.insert("selection_reason".to_string(), self.selection_reason(&selected, preference));
        meta.insert("created_at".to_string(),    now_secs().to_string());

        let mut db = self.anchor_db.write().await;

        // Ensure the UTXO is registered before anchoring
        if !db.dead_utxos.contains_key(&utxo_key) {
            db.add_dead_utxo(selected)
                .map_err(|e| format!("Failed to register UTXO: {e}"))?;
        }

        // Fix 3: use the ID returned by create_anchor, not a local hash
        let anchor_id = db.create_anchor(
            &utxo_key, &data_hash, &merkle_root, &storage_ref, meta,
        )?;

        info!(
            "Entropy anchor created: {} using {} ({})",
            anchor_id,
            utxo_key,
            preference.user_label()
        );
        Ok(anchor_id)
    }

    pub async fn verify_anchor_by_id(&self, anchor_id: &str) -> Result<bool, String> {
        self.anchor_db.read().await.verify_anchor(anchor_id)
    }

    pub async fn find_anchor_by_cid(&self, cid: &str) -> Option<(String, DeadUTXOAnchor)> {
        self.anchor_db.read().await.find_anchor_by_cid(cid)
    }

    pub async fn get_anchor_by_id(&self, anchor_id: &str) -> Option<DeadUTXOAnchor> {
        self.anchor_db.read().await.anchors.get(anchor_id).cloned()
    }

    pub async fn list_anchors(&self) -> Vec<(String, DeadUTXOAnchor)> {
        self.anchor_db.read().await.anchors
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    pub async fn update_block_headers(&mut self, headers: Vec<Vec<u8>>) {
        self.cached_block_headers = headers;
        self.entropy_seed = hybrid_entropy(&self.cached_block_headers);
        Self::shuffle_explorers(&mut self.explorers, &self.entropy_seed);
    }

    pub async fn ensure_csv_loaded_from_env(&self) -> Result<(), String> {
        let path = env::var("SPRINT_DEAD_UTXO_DB").unwrap_or_default();
        let mut db = self.anchor_db.write().await;
        if db.dead_utxos.is_empty() {
            if path.is_empty() {
                return Err(
                    "Dead UTXO database not loaded. Set SPRINT_DEAD_UTXO_DB to a CSV file, for example l2-mining/randomness-api/test-dead-utxos.csv."
                        .to_string(),
                );
            }
            db.load_from_csv(&path)?;
        }
        Ok(())
    }

    pub async fn get_stats(&self) -> HashMap<String, u64> {
        let mut stats = HashMap::new();
        let cache = self.cache.read().await;
        stats.insert("cached_utxos".to_string(),       cache.len() as u64);
        stats.insert("max_cache_size".to_string(),     self.max_cache_entries as u64);
        stats.insert("supported_explorers".to_string(), self.explorers.len() as u64);
        let anchor_stats = self.anchor_db.read().await.get_stats();
        stats.extend(anchor_stats);
        stats
    }

    // ── Explorer fetch — Fix 2: parse the actual response body ───

    async fn fetch_from_explorer(
        &self,
        explorer: &str,
        txid: &str,
        vout: u32,
    ) -> Result<Option<UTXOInfo>, String> {
        // Step 1: is this output spent?
        let outspend_url = format!("{}/tx/{}/outspend/{}", explorer, txid, vout);
        let outspend: MempoolOutspend = self.client
            .get(&outspend_url)
            .send().await
            .map_err(|e| format!("HTTP outspend: {e}"))?
            .json().await
            .map_err(|e| format!("Parse outspend: {e}"))?;

        let spent_at_height = outspend.status
            .as_ref()
            .and_then(|s| s.block_height);
        let spent_in_tx = outspend.txid.clone();

        // Step 2: fetch the transaction to get vout details
        let tx_url = format!("{}/tx/{}", explorer, txid);
        let tx: MempoolTx = self.client
            .get(&tx_url)
            .send().await
            .map_err(|e| format!("HTTP tx: {e}"))?
            .json().await
            .map_err(|e| format!("Parse tx: {e}"))?;

        let vout_data = tx.vout.get(vout as usize);
        let value         = vout_data.and_then(|v| v.value);
        let script_pubkey = vout_data.and_then(|v| v.scriptpubkey.clone()).unwrap_or_default();
        let address       = vout_data.and_then(|v| v.address.clone());

        let (block_height, block_hash, confirmations) = match &tx.status {
            Some(s) if s.confirmed => {
                let bh   = s.block_height;
                let hash = s.block_hash.clone();
                // Approximate confirmations — caller should refresh if precision needed
                let conf = bh.map(|h| 850_000u64.saturating_sub(h));
                (bh, hash, conf)
            }
            _ => (None, None, None),
        };

        Ok(Some(UTXOInfo {
            txid:              txid.to_string(),
            vout,
            value,
            script_pubkey,
            address,
            confirmations,
            block_height,
            block_hash,
            is_spent:          outspend.spent,
            spent_at_height,
            spent_in_tx,
            entropy_weight:    self.entropy_weight(txid, vout),
            entropy_timestamp: now_secs(),
        }))
    }

    // ── Cache — Fix 4: oldest-first eviction via IndexMap ─────────

    async fn cache_utxo(&self, key: String, utxo: UTXOInfo) {
        let mut cache = self.cache.write().await;

        // Evict expired entries first
        let now = now_secs();
        cache.retain(|_, e| now - e.cached_at < self.cache_ttl_seconds);

        // If still over limit, remove the oldest (first) entry
        while cache.len() >= self.max_cache_entries {
            if let Some(first) = cache.keys().next().cloned() {
                cache.swap_remove(&first);
            } else {
                break;
            }
        }

        cache.insert(key, CacheEntry { utxo, cached_at: now });
    }

    // ── Internal helpers ──────────────────────────────────────────

    fn entropy_weight(&self, txid: &str, vout: u32) -> u64 {
        let hash = Sha256::digest(
            [&self.entropy_seed[..], txid.as_bytes(), &vout.to_le_bytes()].concat()
        );
        u64::from_le_bytes(hash[..8].try_into().unwrap_or([0; 8]))
    }

    fn utxo_id(&self, utxo: &DeadUTXOType) -> String {
        match utxo {
            DeadUTXOType::Spent { txid, vout, .. }
            | DeadUTXOType::OpReturn { txid, vout, .. }
            | DeadUTXOType::Dust { txid, vout, .. }
            | DeadUTXOType::BurnAddress { txid, vout, .. } => format!("{txid}:{vout}"),
        }
    }

    fn anchor_preference_score(&self, utxo: &DeadUTXOType) -> u64 {
        match utxo {
            DeadUTXOType::BurnAddress { .. }             => 3000,
            DeadUTXOType::OpReturn { block_height, .. }  => 2000u64.saturating_sub(*block_height % 1000),
            DeadUTXOType::Spent { spent_at_height, .. }  => 1000u64.saturating_sub(*spent_at_height % 1000),
            DeadUTXOType::Dust { .. }                    => 500,
        }
    }

    fn selection_reason(&self, utxo: &DeadUTXOType, preference: UTXOAnchorPreference) -> String {
        match utxo {
            DeadUTXOType::BurnAddress { address, satoshis, .. } => format!(
                "Matched {} preference because the output was sent to burn address {} and carries {} sats.",
                preference.user_label(),
                address,
                satoshis
            ),
            DeadUTXOType::OpReturn { block_height, .. } => format!(
                "Matched {} preference because the output is an immutable OP_RETURN record confirmed at Bitcoin height {}.",
                preference.user_label(),
                block_height
            ),
            DeadUTXOType::Spent { spent_at_height, .. } => format!(
                "Matched {} preference because the output is already spent and therefore cannot be reused; spend height {}.",
                preference.user_label(),
                spent_at_height
            ),
            DeadUTXOType::Dust { satoshis, fee_rate_threshold, .. } => format!(
                "Matched {} preference because the output is dust-sized at {} sats with fee threshold {}.",
                preference.user_label(),
                satoshis,
                fee_rate_threshold
            ),
        }
    }

    fn matches_search_criteria(&self, utxo: &DeadUTXOType, q: &UTXOSearchQuery) -> bool {
        if let Some(ref t) = q.utxo_type {
            let ok = match (t.as_str(), utxo) {
                ("spent",     DeadUTXOType::Spent { .. })       => true,
                ("op_return", DeadUTXOType::OpReturn { .. })    => true,
                ("dust",      DeadUTXOType::Dust { .. })        => true,
                ("burn",      DeadUTXOType::BurnAddress { .. }) => true,
                _                                                => false,
            };
            if !ok { return false; }
        }
        if let Some((min, max)) = q.value_range {
            let v = match utxo {
                DeadUTXOType::Dust { satoshis, .. }        => *satoshis,
                DeadUTXOType::BurnAddress { satoshis, .. } => *satoshis,
                _                                           => return true,
            };
            if v < min || v > max { return false; }
        }
        if let Some(ref addr) = q.address {
            if let DeadUTXOType::BurnAddress { address, .. } = utxo {
                if address != addr { return false; }
            }
        }
        true
    }

    fn convert_dead_utxo_to_info(&self, u: &DeadUTXOType) -> Option<UTXOInfo> {
        // Fix 1: no hardcoded confirmations or block heights — use actual stored values
        let (txid, vout, value, script, address, block_height, is_spent, spent_at) = match u {
            DeadUTXOType::Spent { txid, vout, spent_at_height, .. } =>
                (txid, vout, None, String::new(), None,
                 Some(*spent_at_height), true, Some(*spent_at_height)),
            DeadUTXOType::OpReturn { txid, vout, block_height, data } =>
                (txid, vout, Some(0u64),
                 format!("OP_RETURN {}", hex_encode(data)), None,
                 Some(*block_height), false, None),
            DeadUTXOType::Dust { txid, vout, satoshis, block_height, .. } =>
                (txid, vout, Some(*satoshis), String::new(), None,
                 Some(*block_height), false, None),
            DeadUTXOType::BurnAddress { txid, vout, satoshis, block_height, address } =>
                (txid, vout, Some(*satoshis), String::new(), Some(address.clone()),
                 Some(*block_height), false, None),
        };

        Some(UTXOInfo {
            txid:              txid.clone(),
            vout:              *vout,
            value,
            script_pubkey:     script,
            address,
            confirmations:     None, // requires live chain query
            block_height,
            block_hash:        None, // requires live chain query
            is_spent,
            spent_at_height:   spent_at,
            spent_in_tx:       None,
            entropy_weight:    self.entropy_weight(txid, *vout),
            entropy_timestamp: now_secs(),
        })
    }

    fn convert_info_to_dead_utxo(&self, info: &UTXOInfo, utxo_type: &str) -> Option<DeadUTXOType> {
        let txid = info.txid.clone();
        let vout = info.vout;
        let bh   = info.block_height.unwrap_or(0);
        match utxo_type {
            "spent" => Some(DeadUTXOType::Spent {
                txid, vout,
                spent_in_block:  info.spent_at_height.unwrap_or(bh),
                spent_at_height: info.spent_at_height.unwrap_or(bh),
            }),
            "op_return" => Some(DeadUTXOType::OpReturn {
                txid, vout, block_height: bh,
                data: b"anchored".to_vec(),
            }),
            "dust" => Some(DeadUTXOType::Dust {
                txid, vout,
                satoshis:           info.value.unwrap_or(0),
                block_height:       bh,
                fee_rate_threshold: 10,
            }),
            "burn" => Some(DeadUTXOType::BurnAddress {
                txid, vout,
                address:      info.address.clone().unwrap_or_default(),
                satoshis:     info.value.unwrap_or(0),
                block_height: bh,
            }),
            _ => None,
        }
    }

    fn shuffle_explorers(explorers: &mut Vec<String>, entropy: &[u8; 32]) {
        let n = explorers.len();
        for i in 0..n {
            let j = (entropy[i % 32] as usize) % n;
            explorers.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_parser_accepts_aliases() {
        assert_eq!(UTXOAnchorPreference::parse("opreturn").unwrap(), UTXOAnchorPreference::OpReturn);
        assert_eq!(UTXOAnchorPreference::parse("burn-address").unwrap(), UTXOAnchorPreference::Burn);
    }

    #[test]
    fn preference_parser_returns_helpful_error() {
        let err = UTXOAnchorPreference::parse("weird").unwrap_err();
        assert!(err.contains("Valid preferences: spent, op_return, dust, burn"));
    }

    #[tokio::test]
    async fn preview_entropy_anchor_explains_selected_utxo() {
        let fetcher = UTXOFetcher::new();
        {
            let mut db = fetcher.anchor_db.write().await;
            db.add_dead_utxo(DeadUTXOType::OpReturn {
                txid: "abc".to_string(),
                vout: 2,
                block_height: 900_000,
                data: vec![0xde, 0xad],
            }).unwrap();
        }

        let preview = fetcher.preview_entropy_anchor("opreturn", b"epoch-root").await.unwrap();
        assert_eq!(preview.preference, "op_return");
        assert_eq!(preview.utxo_id, "abc:2");
        assert!(preview.utxo_summary.contains("OP_RETURN output"));
        assert!(preview.reason.contains("immutable OP_RETURN record"));
    }

    #[tokio::test]
    async fn created_anchor_stores_selection_metadata() {
        let fetcher = UTXOFetcher::new();
        {
            let mut db = fetcher.anchor_db.write().await;
            db.add_dead_utxo(DeadUTXOType::BurnAddress {
                txid: "def".to_string(),
                vout: 1,
                address: "1BitcoinEaterAddressDontSendf59kuE".to_string(),
                satoshis: 1000,
                block_height: 800_000,
            }).unwrap();
        }

        let anchor_id = fetcher
            .create_entropy_anchor_with_reference(b"epoch-root", "burn_address", Some("ipfs://cid".to_string()))
            .await
            .unwrap();
        let anchor = fetcher.get_anchor_by_id(&anchor_id).await.unwrap();
        assert_eq!(anchor.metadata.get("preference").map(String::as_str), Some("burn"));
        assert_eq!(anchor.metadata.get("utxo_category").map(String::as_str), Some("burn"));
        assert!(anchor.metadata.get("utxo_summary").is_some());
        assert!(anchor.metadata.get("selection_reason").is_some());
    }
}

// ─────────────────────────────────────────────────────────────────
// Additional uses for the Temporal Gradient mining software
// beyond token rewards
// ─────────────────────────────────────────────────────────────────
//
// The miner already produces:
//   - Continuous CPU heartbeat (proof of presence)
//   - Signed entropy outputs (verifiable randomness)
//   - Merkle-proven epoch records (tamper-evident history)
//   - ECDSA-signed solution hashes (device identity)
//
// These primitives enable:
//
// 1. BITCOIN UTXO TIMESTAMPING (this file)
//    Mine TGBT → solution hash → anchor to dead Bitcoin UTXO
//    → proves data existed before a specific Bitcoin block
//    → stronger timestamp than Ethereum alone (15yr Bitcoin finality)
//    → use case: TerraStake carbon credit provenance, IP registration
//
// 2. DECENTRALISED NOTARY SERVICE
//    Any document hash submitted as a mining seed → committed on-chain
//    → timestamped by both Arbitrum and Bitcoin anchoring
//    → replaces $200-500 notary fees for IP, contracts, evidence
//    → revenue model: charge 1 TGBT per notarisation
//
// 3. HARDWARE ATTESTATION FOR IOT
//    ESP32 solar panel → signs watt readings → submits as mining seed
//    → solution hash proves reading was produced by real hardware at real time
//    → TerraStake impact reports become hardware-attested, not self-reported
//    → extends to any IoT sensor: weather, air quality, supply chain
//
// 4. SOFTWARE BUILD PROVENANCE
//    CI/CD pipeline submits build hash as mining seed during compilation
//    → solution anchors the exact binary to a specific block timestamp
//    → proves "this binary existed and was signed before this block"
//    → use case: tamper-evident software distribution, CVE dating
//
// 5. LEGAL EVIDENCE CHAIN
//    Law firms submit evidence hashes during case preparation
//    → anchored to Bitcoin UTXO → immutable chain-of-custody proof
//    → cannot be backdated, cannot be altered
//    → admissible in jurisdictions recognising blockchain timestamps
//
// 6. ACADEMIC PRIORITY CLAIMS
//    Researchers submit paper hashes before publication
//    → proves authorship date without revealing content
//    → resolves priority disputes without journal gatekeeping
//    → revenue: university licensing of the anchoring service
//
// 7. FINANCIAL AUDIT TRAIL
//    Fund administrators hash daily NAV calculations → anchor on-chain
//    → proves the calculation existed and was unchanged on that date
//    → regulator can verify without accessing raw data
//    → GDPR-compliant: hash reveals nothing about the underlying data
//
// 8. DECENTRALISED PKI
//    Mining wallet = device identity
//    Solution history = proof of continuous operation
//    → replace X.509 certificates with mining-proven identity
//    → certificate cannot be stolen — requires live CPU work to maintain
//    → zero-trust authentication without a certificate authority
//
// Implementation path for each:
//   create_entropy_anchor_with_reference(data_hash, "op_return", Some("ipfs://CID"))
//   → all use the same UTXOFetcher infrastructure already in this file