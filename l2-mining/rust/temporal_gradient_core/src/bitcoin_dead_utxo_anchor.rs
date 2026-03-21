use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeadUTXOType {
    Spent {
        txid: String,
        vout: u32,
        spent_in_block: u64,
        spent_at_height: u64,
    },
    OpReturn {
        txid: String,
        vout: u32,
        block_height: u64,
        data: Vec<u8>,
    },
    Dust {
        txid: String,
        vout: u32,
        satoshis: u64,
        block_height: u64,
        fee_rate_threshold: u64,
    },
    BurnAddress {
        txid: String,
        vout: u32,
        address: String,
        satoshis: u64,
        block_height: u64,
    },
}

impl DeadUTXOType {
    pub fn id(&self) -> String {
        match self {
            DeadUTXOType::Spent { txid, vout, .. }
            | DeadUTXOType::OpReturn { txid, vout, .. }
            | DeadUTXOType::Dust { txid, vout, .. }
            | DeadUTXOType::BurnAddress { txid, vout, .. } => format!("{txid}:{vout}"),
        }
    }

    pub fn category(&self) -> &'static str {
        match self {
            DeadUTXOType::Spent { .. } => "spent",
            DeadUTXOType::OpReturn { .. } => "op_return",
            DeadUTXOType::Dust { .. } => "dust",
            DeadUTXOType::BurnAddress { .. } => "burn",
        }
    }

    pub fn display_label(&self) -> &'static str {
        match self {
            DeadUTXOType::Spent { .. } => "Spent output",
            DeadUTXOType::OpReturn { .. } => "OP_RETURN output",
            DeadUTXOType::Dust { .. } => "Dust output",
            DeadUTXOType::BurnAddress { .. } => "Burn-address output",
        }
    }

    pub fn summary(&self) -> String {
        match self {
            DeadUTXOType::Spent {
                txid,
                vout,
                spent_at_height,
                ..
            } => format!(
                "{} {}:{} spent at Bitcoin height {}",
                self.display_label(),
                txid,
                vout,
                spent_at_height
            ),
            DeadUTXOType::OpReturn {
                txid,
                vout,
                block_height,
                ..
            } => format!(
                "{} {}:{} confirmed at Bitcoin height {}",
                self.display_label(),
                txid,
                vout,
                block_height
            ),
            DeadUTXOType::Dust {
                txid,
                vout,
                satoshis,
                block_height,
                ..
            } => format!(
                "{} {}:{} with {} sats at Bitcoin height {}",
                self.display_label(),
                txid,
                vout,
                satoshis,
                block_height
            ),
            DeadUTXOType::BurnAddress {
                txid,
                vout,
                address,
                satoshis,
                block_height,
            } => format!(
                "{} {}:{} to {} with {} sats at Bitcoin height {}",
                self.display_label(),
                txid,
                vout,
                address,
                satoshis,
                block_height
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadUTXOAnchor {
    pub anchor_id: String,
    pub utxo_id: String,
    pub data_hash: String,
    pub merkle_root: String,
    pub storage_reference: String,
    pub metadata: HashMap<String, String>,
    pub created_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeadUTXOAnchorDB {
    pub dead_utxos: HashMap<String, DeadUTXOType>,
    pub anchors: HashMap<String, DeadUTXOAnchor>,
}

impl DeadUTXOAnchorDB {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_dead_utxo(&mut self, utxo: DeadUTXOType) -> Result<(), String> {
        self.dead_utxos.insert(utxo.id(), utxo);
        Ok(())
    }

    pub fn create_anchor(
        &mut self,
        utxo_id: &str,
        data_hash: &str,
        merkle_root: &str,
        storage_reference: &str,
        metadata: HashMap<String, String>,
    ) -> Result<String, String> {
        if !self.dead_utxos.contains_key(utxo_id) {
            return Err(format!("Unknown dead UTXO: {utxo_id}"));
        }

        let created_at = now_secs();
        let anchor_id = compute_anchor_id(utxo_id, data_hash, merkle_root, storage_reference, created_at);

        let anchor = DeadUTXOAnchor {
            anchor_id: anchor_id.clone(),
            utxo_id: utxo_id.to_string(),
            data_hash: data_hash.to_string(),
            merkle_root: merkle_root.to_string(),
            storage_reference: storage_reference.to_string(),
            metadata,
            created_at,
        };

        self.anchors.insert(anchor_id.clone(), anchor);
        Ok(anchor_id)
    }

    pub fn verify_anchor(&self, anchor_id: &str) -> Result<bool, String> {
        let Some(anchor) = self.anchors.get(anchor_id) else {
            return Ok(false);
        };

        if !self.dead_utxos.contains_key(&anchor.utxo_id) {
            return Ok(false);
        }

        let recomputed = compute_anchor_id(
            &anchor.utxo_id,
            &anchor.data_hash,
            &anchor.merkle_root,
            &anchor.storage_reference,
            anchor.created_at,
        );
        Ok(recomputed == anchor.anchor_id)
    }

    pub fn find_anchor_by_cid(&self, cid: &str) -> Option<(String, DeadUTXOAnchor)> {
        self.anchors.iter().find_map(|(id, anchor)| {
            let reference = anchor.storage_reference.trim();
            let matched = reference == cid
                || reference.ends_with(cid)
                || reference.strip_prefix("ipfs://") == Some(cid);
            matched.then(|| (id.clone(), anchor.clone()))
        })
    }

    pub fn load_from_csv(&mut self, path: &str) -> Result<(), String> {
        let raw = fs::read_to_string(path).map_err(|e| format!("Failed to read {path}: {e}"))?;
        let mut lines = raw.lines();
        let header_line = lines.next().ok_or_else(|| "CSV file is empty".to_string())?;
        let headers: Vec<String> = header_line.split(',').map(|h| h.trim().to_ascii_lowercase()).collect();

        for (row_idx, line) in lines.enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let values: Vec<&str> = line.split(',').map(|v| v.trim()).collect();
            let row = CsvRow::new(&headers, &values);
            let utxo = parse_utxo_row(&row).map_err(|e| format!("CSV row {}: {e}", row_idx + 2))?;
            self.add_dead_utxo(utxo)?;
        }

        Ok(())
    }

    pub fn get_stats(&self) -> HashMap<String, u64> {
        let mut stats = HashMap::new();
        stats.insert("dead_utxos".to_string(), self.dead_utxos.len() as u64);
        stats.insert("anchors".to_string(), self.anchors.len() as u64);

        let mut spent = 0u64;
        let mut op_return = 0u64;
        let mut dust = 0u64;
        let mut burn = 0u64;
        for utxo in self.dead_utxos.values() {
            match utxo {
                DeadUTXOType::Spent { .. } => spent += 1,
                DeadUTXOType::OpReturn { .. } => op_return += 1,
                DeadUTXOType::Dust { .. } => dust += 1,
                DeadUTXOType::BurnAddress { .. } => burn += 1,
            }
        }

        stats.insert("spent_utxos".to_string(), spent);
        stats.insert("op_return_utxos".to_string(), op_return);
        stats.insert("dust_utxos".to_string(), dust);
        stats.insert("burn_utxos".to_string(), burn);
        stats
    }
}

struct CsvRow<'a> {
    data: HashMap<&'a str, &'a str>,
}

impl<'a> CsvRow<'a> {
    fn new(headers: &'a [String], values: &'a [&'a str]) -> Self {
        let mut data = HashMap::new();
        for (idx, header) in headers.iter().enumerate() {
            data.insert(header.as_str(), values.get(idx).copied().unwrap_or(""));
        }
        Self { data }
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.data.get(key).copied().filter(|v| !v.is_empty())
    }
}

fn parse_utxo_row(row: &CsvRow<'_>) -> Result<DeadUTXOType, String> {
    let utxo_type = row
        .get("type")
        .or_else(|| row.get("utxo_type"))
        .ok_or_else(|| "missing type".to_string())?;
    let txid = row.get("txid").ok_or_else(|| "missing txid".to_string())?.to_string();
    let vout = parse_u32_field(row, "vout")?;

    match utxo_type {
        "spent" => Ok(DeadUTXOType::Spent {
            txid,
            vout,
            spent_in_block: parse_u64_field_alias(row, &["spent_in_block", "block_height"])? ,
            spent_at_height: parse_u64_field_alias(row, &["spent_at_height", "block_height"])? ,
        }),
        "op_return" => Ok(DeadUTXOType::OpReturn {
            txid,
            vout,
            block_height: parse_u64_field(row, "block_height")?,
            data: parse_bytes_field(row, "data")?,
        }),
        "dust" => Ok(DeadUTXOType::Dust {
            txid,
            vout,
            satoshis: parse_u64_field_alias(row, &["satoshis", "value"])? ,
            block_height: parse_u64_field(row, "block_height")?,
            fee_rate_threshold: parse_u64_field_alias(row, &["fee_rate_threshold", "fee_rate"]).unwrap_or(0),
        }),
        "burn" | "burn_address" => Ok(DeadUTXOType::BurnAddress {
            txid,
            vout,
            address: row.get("address").unwrap_or_default().to_string(),
            satoshis: parse_u64_field_alias(row, &["satoshis", "value"])? ,
            block_height: parse_u64_field(row, "block_height")?,
        }),
        other => Err(format!("unsupported type {other}")),
    }
}

fn parse_u32_field(row: &CsvRow<'_>, key: &str) -> Result<u32, String> {
    row.get(key)
        .ok_or_else(|| format!("missing {key}"))?
        .parse::<u32>()
        .map_err(|e| format!("invalid {key}: {e}"))
}

fn parse_u64_field(row: &CsvRow<'_>, key: &str) -> Result<u64, String> {
    row.get(key)
        .ok_or_else(|| format!("missing {key}"))?
        .parse::<u64>()
        .map_err(|e| format!("invalid {key}: {e}"))
}

fn parse_u64_field_alias(row: &CsvRow<'_>, keys: &[&str]) -> Result<u64, String> {
    for key in keys {
        if let Some(value) = row.get(key) {
            return value
                .parse::<u64>()
                .map_err(|e| format!("invalid {key}: {e}"));
        }
    }
    Err(format!("missing one of {}", keys.join(", ")))
}

fn parse_bytes_field(row: &CsvRow<'_>, key: &str) -> Result<Vec<u8>, String> {
    let value = row.get(key).ok_or_else(|| format!("missing {key}"))?;
    let normalized = value.strip_prefix("0x").unwrap_or(value);
    hex::decode(normalized).or_else(|_| Ok(value.as_bytes().to_vec()))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn compute_anchor_id(
    utxo_id: &str,
    data_hash: &str,
    merkle_root: &str,
    storage_reference: &str,
    created_at: u64,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(utxo_id.as_bytes());
    hasher.update(data_hash.as_bytes());
    hasher.update(merkle_root.as_bytes());
    hasher.update(storage_reference.as_bytes());
    hasher.update(created_at.to_le_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_roundtrip_verifies() {
        let mut db = DeadUTXOAnchorDB::new();
        db.add_dead_utxo(DeadUTXOType::Dust {
            txid: "abc".to_string(),
            vout: 1,
            satoshis: 546,
            block_height: 100,
            fee_rate_threshold: 10,
        })
        .unwrap();

        let anchor_id = db
            .create_anchor(
                "abc:1",
                "hash",
                "root",
                "ipfs://cid",
                HashMap::new(),
            )
            .unwrap();

        assert!(db.verify_anchor(&anchor_id).unwrap());
        assert!(db.find_anchor_by_cid("cid").is_some());
    }
}