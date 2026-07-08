//! Durable block store (SQLite).
//!
//! Everything the mining loop previously kept only in an in-memory
//! `VecDeque<BlockRecord>` — headers, the miner's signature, the exact
//! transaction list a block executed, and its cumulative chain work — now
//! lives here instead, so it survives a restart. That's what lets
//! `/chain/headers` serve real history to a syncing/reconciling peer even
//! after this node has been restarted, and lets a restarted node rebuild its
//! own ledger by replaying its own history rather than trusting whatever
//! `chain-state.json` last said.
//!
//! This is deliberately a single-canonical-chain store: there is no notion
//! of orphaned/side-chain blocks kept around. A reorg (see
//! `reconcile_fork` in `main.rs`) simply deletes every row above the fork
//! point before replaying the heavier branch forward — once a height is
//! rolled back it is gone, exactly as if it had never been mined, which is
//! the correct semantics for "this was never the canonical chain."

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::Mutex;

use crate::BlockRecord;
use l1_core::transaction::SignedTransaction;

pub struct BlockStore {
    conn: Mutex<Connection>,
}

impl BlockStore {
    pub fn open(path: &str) -> Result<Self> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent).context("creating block store directory")?;
        }
        let conn = Connection::open(path).context("opening block store database")?;
        conn.pragma_update(None, "journal_mode", "WAL").context("setting WAL mode")?;
        conn.pragma_update(None, "synchronous", "NORMAL").context("setting synchronous mode")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS blocks (
                height              INTEGER PRIMARY KEY,
                block_hash          TEXT NOT NULL UNIQUE,
                prev_hash           TEXT NOT NULL,
                nonce               INTEGER NOT NULL,
                timestamp           INTEGER NOT NULL,
                zero_bits           INTEGER NOT NULL,
                difficulty_bits     INTEGER NOT NULL,
                hashes_tried        INTEGER NOT NULL,
                elapsed_ms          INTEGER NOT NULL,
                hashrate_mhs        REAL NOT NULL,
                miner_address       TEXT NOT NULL,
                miner_public_key    TEXT NOT NULL,
                miner_signature     TEXT NOT NULL,
                claim_digest        TEXT NOT NULL,
                election_score      TEXT NOT NULL,
                source              TEXT NOT NULL,
                reward_bina         INTEGER NOT NULL,
                randomness_output   TEXT NOT NULL,
                nullifier           TEXT NOT NULL,
                btc_seed            TEXT NOT NULL,
                btc_height          INTEGER NOT NULL,
                merkle_root         TEXT NOT NULL,
                state_root          TEXT NOT NULL,
                chain_work_hex      TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS block_transactions (
                height      INTEGER NOT NULL,
                tx_index    INTEGER NOT NULL,
                tx_json     TEXT NOT NULL,
                PRIMARY KEY (height, tx_index)
            );
            CREATE TABLE IF NOT EXISTS mempool (
                tx_id       TEXT PRIMARY KEY,
                tx_json     TEXT NOT NULL
            );",
        )
        .context("creating block store schema")?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Insert or overwrite the block at `record.height` (overwrite only
    /// happens if a caller deliberately re-inserts after a rollback).
    pub fn insert_block(&self, record: &BlockRecord) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO blocks (
                height, block_hash, prev_hash, nonce, timestamp, zero_bits, difficulty_bits,
                hashes_tried, elapsed_ms, hashrate_mhs, miner_address, miner_public_key,
                miner_signature, claim_digest, election_score, source, reward_bina,
                randomness_output, nullifier, btc_seed, btc_height, merkle_root, state_root,
                chain_work_hex
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24)",
            params![
                record.height as i64,
                record.block_hash,
                record.prev_hash,
                record.nonce as i64,
                record.timestamp as i64,
                record.zero_bits,
                record.difficulty_bits,
                record.hashes_tried as i64,
                record.elapsed_ms as i64,
                record.hashrate_mhs,
                record.miner_address,
                record.miner_public_key,
                record.miner_signature,
                record.claim_digest,
                record.election_score,
                record.source,
                record.reward_bina as i64,
                record.randomness_output,
                record.nullifier,
                record.btc_seed,
                record.btc_height as i64,
                record.merkle_root,
                record.state_root,
                record.chain_work_hex,
            ],
        )
        .context("inserting block row")?;

        tx.execute("DELETE FROM block_transactions WHERE height = ?1", params![record.height as i64])
            .context("clearing old block_transactions row")?;
        for (i, txn) in record.transactions.iter().enumerate() {
            let json = serde_json::to_string(txn).context("serializing transaction for storage")?;
            tx.execute(
                "INSERT INTO block_transactions (height, tx_index, tx_json) VALUES (?1, ?2, ?3)",
                params![record.height as i64, i as i64, json],
            )
            .context("inserting block_transactions row")?;
        }
        tx.commit().context("committing block insert")?;
        Ok(())
    }

    pub fn get(&self, height: u64) -> Result<Option<BlockRecord>> {
        let conn = self.conn.lock().unwrap();
        let record = conn
            .query_row("SELECT * FROM blocks WHERE height = ?1", params![height as i64], row_to_record)
            .optional()
            .context("querying block by height")?;
        let Some(mut record) = record else { return Ok(None) };
        record.transactions = self.get_transactions_locked(&conn, height)?;
        Ok(Some(record))
    }

    pub fn get_by_hash(&self, block_hash: &str) -> Result<Option<BlockRecord>> {
        let conn = self.conn.lock().unwrap();
        let record = conn
            .query_row("SELECT * FROM blocks WHERE block_hash = ?1", params![block_hash], row_to_record)
            .optional()
            .context("querying block by hash")?;
        let Some(mut record) = record else { return Ok(None) };
        record.transactions = self.get_transactions_locked(&conn, record.height)?;
        Ok(Some(record))
    }

    /// Inclusive range `[from, to]`, capped at `limit` rows, ordered by height.
    pub fn get_range(&self, from: u64, to: u64, limit: usize) -> Result<Vec<BlockRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT * FROM blocks WHERE height >= ?1 AND height <= ?2 ORDER BY height ASC LIMIT ?3")
            .context("preparing range query")?;
        let rows = stmt
            .query_map(params![from as i64, to as i64, limit as i64], row_to_record)
            .context("querying block range")?;
        let mut out = Vec::new();
        for row in rows {
            let mut record = row.context("reading block row")?;
            record.transactions = self.get_transactions_locked(&conn, record.height)?;
            out.push(record);
        }
        Ok(out)
    }

    fn get_transactions_locked(&self, conn: &Connection, height: u64) -> Result<Vec<SignedTransaction>> {
        let mut stmt = conn
            .prepare("SELECT tx_json FROM block_transactions WHERE height = ?1 ORDER BY tx_index ASC")
            .context("preparing transactions query")?;
        let rows = stmt
            .query_map(params![height as i64], |row| row.get::<_, String>(0))
            .context("querying block transactions")?;
        let mut out = Vec::new();
        for row in rows {
            let json = row.context("reading transaction row")?;
            out.push(serde_json::from_str(&json).context("deserializing stored transaction")?);
        }
        Ok(out)
    }

    pub fn tip_height(&self) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let height: Option<i64> = conn
            .query_row("SELECT MAX(height) FROM blocks", [], |row| row.get(0))
            .context("querying tip height")?;
        Ok(height.unwrap_or(0) as u64)
    }

    pub fn block_hash_at(&self, height: u64) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT block_hash FROM blocks WHERE height = ?1", params![height as i64], |row| row.get(0))
            .optional()
            .context("querying block hash at height")
    }

    /// Delete every block (and its transactions) above `height` — used by
    /// reorg rollback. The caller is responsible for rebuilding any other
    /// state (ledger, difficulty adjuster, pinned checkpoint) to match.
    pub fn rollback_above(&self, height: u64) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM block_transactions WHERE height > ?1", params![height as i64])
            .context("rolling back block_transactions")?;
        tx.execute("DELETE FROM blocks WHERE height > ?1", params![height as i64])
            .context("rolling back blocks")?;
        tx.commit().context("committing rollback")?;
        Ok(())
    }

    /// The most recent block that pinned a *real* Bitcoin checkpoint —
    /// mock/failed observations record `btc_height = 0` and are skipped.
    /// Returns `(btc_seed_hex, btc_tip_height)`.
    pub fn latest_checkpoint(&self) -> Result<Option<(String, u64)>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT btc_seed, btc_height FROM blocks WHERE btc_height > 0 ORDER BY height DESC LIMIT 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64)),
        )
        .optional()
        .context("querying latest btc checkpoint")
    }

    /// Every nullifier on the canonical chain (hex), for rebuilding the
    /// in-memory spent set at startup.
    pub fn all_nullifiers(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT nullifier FROM blocks ORDER BY height ASC")
            .context("preparing nullifiers query")?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .context("querying nullifiers")?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.context("reading nullifier row")?);
        }
        Ok(out)
    }

    pub fn mempool_insert(&self, tx: &SignedTransaction) -> Result<()> {
        let json = serde_json::to_string(tx).context("serializing pending transaction")?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO mempool (tx_id, tx_json) VALUES (?1, ?2)",
            params![tx.tx_id_hex(), json],
        )
        .context("inserting mempool row")?;
        Ok(())
    }

    pub fn mempool_remove(&self, tx_id: &[u8; 32]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM mempool WHERE tx_id = ?1", params![hex::encode(tx_id)])
            .context("deleting mempool row")?;
        Ok(())
    }

    /// Load every persisted pending transaction. Rows that no longer
    /// deserialize (e.g. after a format change) are deleted rather than
    /// crashing startup — the caller re-validates the survivors anyway.
    pub fn mempool_load(&self) -> Result<Vec<SignedTransaction>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT tx_id, tx_json FROM mempool")
            .context("preparing mempool query")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .context("querying mempool")?;
        let mut out = Vec::new();
        let mut corrupt = Vec::new();
        for row in rows {
            let (tx_id, json) = row.context("reading mempool row")?;
            match serde_json::from_str(&json) {
                Ok(tx) => out.push(tx),
                Err(_) => corrupt.push(tx_id),
            }
        }
        drop(stmt);
        for tx_id in corrupt {
            conn.execute("DELETE FROM mempool WHERE tx_id = ?1", params![tx_id])
                .context("deleting corrupt mempool row")?;
        }
        Ok(out)
    }
}

fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<BlockRecord> {
    let timestamp: u64 = row.get::<_, i64>("timestamp")? as u64;
    Ok(BlockRecord {
        height: row.get::<_, i64>("height")? as u64,
        block_hash: row.get("block_hash")?,
        prev_hash: row.get("prev_hash")?,
        nonce: row.get::<_, i64>("nonce")? as u64,
        timestamp,
        mined_timestamp_secs: timestamp / 1000,
        zero_bits: row.get("zero_bits")?,
        difficulty_bits: row.get("difficulty_bits")?,
        hashes_tried: row.get::<_, i64>("hashes_tried")? as u64,
        elapsed_ms: row.get::<_, i64>("elapsed_ms")? as u64,
        hashrate_mhs: row.get("hashrate_mhs")?,
        miner_address: row.get("miner_address")?,
        miner_public_key: row.get("miner_public_key")?,
        miner_signature: row.get("miner_signature")?,
        claim_digest: row.get("claim_digest")?,
        election_score: row.get("election_score")?,
        source: row.get("source")?,
        reward_bina: row.get::<_, i64>("reward_bina")? as u64,
        randomness_output: row.get("randomness_output")?,
        nullifier: row.get("nullifier")?,
        btc_seed: row.get("btc_seed")?,
        btc_height: row.get::<_, i64>("btc_height")? as u64,
        merkle_root: row.get("merkle_root")?,
        state_root: row.get("state_root")?,
        chain_work_hex: row.get("chain_work_hex")?,
        transactions: Vec::new(), // filled in by the caller
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use l1_core::crypto::WalletKeypair;
    use l1_core::transaction::{SignedTransaction, Transaction};

    fn dummy_record(height: u64, block_hash: &str, prev_hash: &str, txs: Vec<SignedTransaction>) -> BlockRecord {
        BlockRecord {
            height,
            block_hash: block_hash.to_string(),
            prev_hash: prev_hash.to_string(),
            nonce: height,
            timestamp: 1_000_000 + height,
            mined_timestamp_secs: (1_000_000 + height) / 1000,
            zero_bits: 25,
            difficulty_bits: 25,
            hashes_tried: 1000,
            elapsed_ms: 10,
            hashrate_mhs: 1.0,
            miner_address: "3054ac8bc5c9b358e270e17183851201d0bc6b69".to_string(),
            miner_public_key: "aa".to_string(),
            miner_signature: "bb".to_string(),
            claim_digest: "cc".to_string(),
            election_score: "dd".to_string(),
            source: "local".to_string(),
            reward_bina: 50,
            randomness_output: "ee".to_string(),
            nullifier: "ff".to_string(),
            btc_seed: "00".repeat(32),
            btc_height: 900_000,
            merkle_root: hex::encode(l1_core::transaction::merkle_root(&txs)),
            state_root: "11".repeat(32),
            chain_work_hex: format!("{:x}", (height as u128) * (1u128 << 25)),
            transactions: txs,
        }
    }

    fn dummy_tx() -> SignedTransaction {
        let sender = WalletKeypair::generate();
        let recipient = WalletKeypair::generate();
        let tx = Transaction::new(sender.address(), recipient.address(), 5, 0, 1);
        SignedTransaction::sign(tx, &sender).unwrap()
    }

    fn temp_store() -> BlockStore {
        let path = std::env::temp_dir().join(format!("bina_store_test_{}.sqlite3", uuid_ish()));
        BlockStore::open(path.to_str().unwrap()).unwrap()
    }

    fn uuid_ish() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64 ^ n
    }

    #[test]
    fn insert_and_get_round_trips() {
        let store = temp_store();
        let record = dummy_record(1, "aa11", "0000", vec![dummy_tx()]);
        store.insert_block(&record).unwrap();

        let fetched = store.get(1).unwrap().unwrap();
        assert_eq!(fetched.block_hash, "aa11");
        assert_eq!(fetched.transactions.len(), 1);
        assert_eq!(fetched.transactions[0].tx_id_hex(), record.transactions[0].tx_id_hex());
    }

    #[test]
    fn get_missing_height_returns_none() {
        let store = temp_store();
        assert!(store.get(42).unwrap().is_none());
    }

    #[test]
    fn get_by_hash_finds_the_right_block() {
        let store = temp_store();
        store.insert_block(&dummy_record(1, "hash1", "0000", vec![])).unwrap();
        store.insert_block(&dummy_record(2, "hash2", "hash1", vec![])).unwrap();

        let found = store.get_by_hash("hash2").unwrap().unwrap();
        assert_eq!(found.height, 2);
        assert!(store.get_by_hash("nonexistent").unwrap().is_none());
    }

    #[test]
    fn get_range_is_ordered_and_bounded() {
        let store = temp_store();
        for h in 1..=10u64 {
            store.insert_block(&dummy_record(h, &format!("h{h}"), &format!("h{}", h.saturating_sub(1)), vec![])).unwrap();
        }
        let page = store.get_range(3, 7, 100).unwrap();
        assert_eq!(page.iter().map(|b| b.height).collect::<Vec<_>>(), vec![3, 4, 5, 6, 7]);

        let capped = store.get_range(1, 10, 3).unwrap();
        assert_eq!(capped.len(), 3);
        assert_eq!(capped[0].height, 1);
    }

    #[test]
    fn tip_height_tracks_highest_inserted() {
        let store = temp_store();
        assert_eq!(store.tip_height().unwrap(), 0);
        store.insert_block(&dummy_record(1, "h1", "0000", vec![])).unwrap();
        store.insert_block(&dummy_record(5, "h5", "h4", vec![])).unwrap();
        assert_eq!(store.tip_height().unwrap(), 5);
    }

    #[test]
    fn block_hash_at_returns_the_right_hash() {
        let store = temp_store();
        store.insert_block(&dummy_record(3, "h3", "h2", vec![])).unwrap();
        assert_eq!(store.block_hash_at(3).unwrap().as_deref(), Some("h3"));
        assert_eq!(store.block_hash_at(4).unwrap(), None);
    }

    #[test]
    fn rollback_above_deletes_blocks_and_their_transactions() {
        let store = temp_store();
        for h in 1..=5u64 {
            store.insert_block(&dummy_record(h, &format!("h{h}"), &format!("h{}", h - 1), vec![dummy_tx()])).unwrap();
        }
        store.rollback_above(3).unwrap();

        assert_eq!(store.tip_height().unwrap(), 3);
        assert!(store.get(4).unwrap().is_none());
        assert!(store.get(5).unwrap().is_none());
        // Surviving blocks and their transactions must be untouched.
        let survivor = store.get(3).unwrap().unwrap();
        assert_eq!(survivor.transactions.len(), 1);
    }

    #[test]
    fn reinserting_a_height_overwrites_its_transactions() {
        let store = temp_store();
        store.insert_block(&dummy_record(1, "h1a", "0000", vec![dummy_tx(), dummy_tx()])).unwrap();
        assert_eq!(store.get(1).unwrap().unwrap().transactions.len(), 2);

        store.insert_block(&dummy_record(1, "h1b", "0000", vec![dummy_tx()])).unwrap();
        let after = store.get(1).unwrap().unwrap();
        assert_eq!(after.block_hash, "h1b");
        assert_eq!(after.transactions.len(), 1, "old row's transactions must not linger after overwrite");
    }

    #[test]
    fn latest_checkpoint_skips_mock_rows_and_returns_the_newest_real_one() {
        let store = temp_store();
        assert!(store.latest_checkpoint().unwrap().is_none(), "empty store has no checkpoint");

        let mut real = dummy_record(1, "h1", "0000", vec![]);
        real.btc_seed = "aa".repeat(32);
        real.btc_height = 900_100;
        store.insert_block(&real).unwrap();

        // A later block whose checkpoint was mock/failed (btc_height = 0)
        // must not shadow the older real checkpoint.
        let mut mock = dummy_record(2, "h2", "h1", vec![]);
        mock.btc_seed = "de".repeat(32);
        mock.btc_height = 0;
        store.insert_block(&mock).unwrap();

        let (seed, height) = store.latest_checkpoint().unwrap().unwrap();
        assert_eq!(seed, "aa".repeat(32));
        assert_eq!(height, 900_100);

        let mut newer = dummy_record(3, "h3", "h2", vec![]);
        newer.btc_seed = "bb".repeat(32);
        newer.btc_height = 900_200;
        store.insert_block(&newer).unwrap();
        let (seed, height) = store.latest_checkpoint().unwrap().unwrap();
        assert_eq!(seed, "bb".repeat(32));
        assert_eq!(height, 900_200);
    }

    #[test]
    fn all_nullifiers_returns_every_stored_block_nullifier() {
        let store = temp_store();
        for h in 1..=3u64 {
            let mut record = dummy_record(h, &format!("h{h}"), &format!("h{}", h - 1), vec![]);
            record.nullifier = format!("{:02x}", h).repeat(32);
            store.insert_block(&record).unwrap();
        }
        let nullifiers = store.all_nullifiers().unwrap();
        assert_eq!(nullifiers.len(), 3);
        assert!(nullifiers.contains(&"02".repeat(32)));
    }

    #[test]
    fn mempool_rows_survive_a_reopen_and_are_removable() {
        let path = std::env::temp_dir().join(format!("bina_store_test_{}.sqlite3", uuid_ish()));
        let tx1 = dummy_tx();
        let tx2 = dummy_tx();
        {
            let store = BlockStore::open(path.to_str().unwrap()).unwrap();
            store.mempool_insert(&tx1).unwrap();
            store.mempool_insert(&tx2).unwrap();
            // Re-inserting the same tx must not duplicate it.
            store.mempool_insert(&tx1).unwrap();
        }
        let store = BlockStore::open(path.to_str().unwrap()).unwrap();
        let loaded = store.mempool_load().unwrap();
        assert_eq!(loaded.len(), 2, "pending transactions must survive a restart");

        store.mempool_remove(&tx1.tx_id()).unwrap();
        let after = store.mempool_load().unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].tx_id_hex(), tx2.tx_id_hex());
    }
}
