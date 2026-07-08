//! Bina Chain emission schedule + reward ledger
//!
//! Supply parameters
//! ─────────────────────────────────────────────────────────────────────────
//!   Hard cap             : 2,000,000,000 BINA  (2 billion, absolute ceiling)
//!   Initial block reward : 50 BINA
//!   Halving interval     : 1,576,800,000 blocks (≈ 2 years at 40 ms/block)
//!   Halving schedule     :
//!     Era 0  blocks      0 – 1,576,800,000  reward 50 BINA
//!     (emission stops immediately when total_mined == HARD_CAP)
//!
//! Ledger persistence
//! ─────────────────────────────────────────────────────────────────────────
//!   Append-only CSV: data/ledger.csv
//!   Columns: height, miner_address, reward_bina, total_mined_bina, timestamp_unix

use crate::transaction::SignedTransaction;
use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

// ─── Emission constants ──────────────────────────────────────────────────────

/// Hard cap on total $BINA supply (2 billion).
pub const HARD_CAP:             u64 = 2_000_000_000;
/// Block reward in era 0.
pub const INITIAL_BLOCK_REWARD: u64 = 50;
/// Number of blocks between halvings at a 40 ms target block time.
pub const HALVING_INTERVAL:     u64 = 1_576_800_000;

/// Compute the block reward for a given height, capped by remaining supply.
///
/// Returns 0 once `total_mined >= HARD_CAP`.
pub fn block_reward(height: u64, total_mined: u64) -> u64 {
    if total_mined >= HARD_CAP { return 0; }
    let era    = height / HALVING_INTERVAL;
    // After 64 halvings the reward rounds to 0 (u64 shift would overflow otherwise)
    let reward = if era >= 64 { 0 } else { INITIAL_BLOCK_REWARD >> era };
    reward.min(HARD_CAP - total_mined)   // never exceed the hard cap
}

/// Estimate the era for a given height.
pub fn era(height: u64) -> u64 { height / HALVING_INTERVAL }

/// Domain tag for the ledger state-root commitment.
pub const STATE_ROOT_TAG: &[u8] = b"BINA-STATE-v1";

/// Pure commitment over a canonical (address, balance, nonce) view of the
/// ledger. Entries are sorted by address before hashing so the result is
/// independent of iteration order — any two nodes with the same logical
/// state produce the same root.
pub fn compute_state_root<'a, I>(entries: I) -> [u8; 32]
where
    I: IntoIterator<Item = (&'a str, u64, u64)>,
{
    let mut sorted: Vec<(&str, u64, u64)> = entries.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut h = blake3::Hasher::new();
    h.update(STATE_ROOT_TAG);
    for (addr, balance, nonce) in sorted {
        h.update(addr.as_bytes());
        h.update(&balance.to_le_bytes());
        h.update(&nonce.to_le_bytes());
    }
    *h.finalize().as_bytes()
}

/// The state root of a ledger with no accounts at all — genesis's value.
pub fn empty_state_root() -> [u8; 32] {
    compute_state_root(std::iter::empty())
}

/// BINA remaining before the hard cap is hit.
pub fn supply_remaining(total_mined: u64) -> u64 {
    HARD_CAP.saturating_sub(total_mined)
}

// ─── Reward ledger ───────────────────────────────────────────────────────────

/// Append-only balance ledger.  Loaded from CSV on startup; each new credit
/// is immediately flushed to disk so balances survive a node restart.
pub struct RewardLedger {
    balances:    HashMap<String, u64>,   // address → cumulative balance (BINA)
    nonces:      HashMap<String, u64>,   // address → next transaction nonce
    total_mined: u64,
    csv_path:    PathBuf,
}

impl RewardLedger {
    // ── Construction ────────────────────────────────────────────────────────

    /// Create or load from `csv_path`.  Missing file → empty ledger.
    pub fn open(csv_path: impl AsRef<Path>) -> Result<Self> {
        Self::open_inner(csv_path, None)
    }

    /// Create or load the ledger scoped to the active chain height.
    ///
    /// Historical development runs can leave duplicate reward rows for the same
    /// block heights in the append-only CSV. For a resumed chain, only the latest
    /// reward row for each height up to `active_height` belongs to the active tip.
    pub fn open_scoped(csv_path: impl AsRef<Path>, active_height: u64) -> Result<Self> {
        Self::open_inner(csv_path, Some(active_height))
    }

    fn open_inner(csv_path: impl AsRef<Path>, active_height: Option<u64>) -> Result<Self> {
        let csv_path = csv_path.as_ref().to_path_buf();
        let mut balances: HashMap<String, u64> = HashMap::new();
        let mut nonces: HashMap<String, u64> = HashMap::new();
        let mut total_mined: u64 = 0;
        let scoped = active_height.is_some();

        enum ReplayRow {
            Reward { index: usize, height: u64, addr: String, reward: u64 },
            Tx { from: String, to: String, amount: u64, fee: u64, nonce: u64, miner: String },
        }

        let mut replay_rows: Vec<ReplayRow> = Vec::new();
        let mut latest_reward_index: HashMap<u64, usize> = HashMap::new();

        if csv_path.exists() {
            let file   = File::open(&csv_path)
                .context("opening ledger CSV")?;
            let reader = BufReader::new(file);
            for (lineno, line) in reader.lines().enumerate() {
                let line = line?;
                // skip header or blank lines
                if lineno == 0 || line.trim().is_empty() { continue; }
                let cols: Vec<&str> = line.split(',').collect();
                if cols.first().copied() == Some("tx") {
                    // tx,height,tx_id,from,to,amount,fee,nonce,miner_address,total_mined_bina,timestamp_unix
                    if cols.len() < 11 { continue; }
                    let height = cols[1].trim().parse::<u64>().unwrap_or(0);
                    if active_height.is_some_and(|max_height| height > max_height) { continue; }
                    let from = cols[3].trim().to_string();
                    let to = cols[4].trim().to_string();
                    let amount = cols[5].trim().parse::<u64>().unwrap_or(0);
                    let fee = cols[6].trim().parse::<u64>().unwrap_or(0);
                    let nonce = cols[7].trim().parse::<u64>().unwrap_or(0);
                    let miner = cols[8].trim().to_string();
                    if scoped {
                        replay_rows.push(ReplayRow::Tx { from, to, amount, fee, nonce, miner });
                    } else {
                        debit(&mut balances, &from, amount.saturating_add(fee));
                        credit_balance(&mut balances, &to, amount);
                        if fee > 0 && !miner.is_empty() {
                            credit_balance(&mut balances, &miner, fee);
                        }
                        let next_nonce = nonce.saturating_add(1);
                        let entry = nonces.entry(from).or_insert(0);
                        if next_nonce > *entry { *entry = next_nonce; }
                        total_mined = cols[9].trim().parse::<u64>().unwrap_or(total_mined);
                    }
                } else {
                    if cols.len() < 4 { continue; }
                    // Legacy reward row: height, miner_address, reward_bina, total_mined_bina [, timestamp]
                    let height = cols[0].trim().parse::<u64>().unwrap_or(0);
                    if active_height.is_some_and(|max_height| height > max_height) { continue; }
                    let addr = cols[1].trim().to_string();
                    let reward = cols[2].trim().parse::<u64>().unwrap_or(0);
                    let total = cols[3].trim().parse::<u64>().unwrap_or(0);
                    if scoped {
                        let index = replay_rows.len();
                        latest_reward_index.insert(height, index);
                        replay_rows.push(ReplayRow::Reward { index, height, addr, reward });
                    } else {
                        credit_balance(&mut balances, &addr, reward);
                        total_mined = total;
                    }
                }
            }

            if scoped {
                for row in replay_rows {
                    match row {
                        ReplayRow::Reward { index, height, addr, reward } => {
                            if latest_reward_index.get(&height).copied() == Some(index) {
                                credit_balance(&mut balances, &addr, reward);
                                total_mined = total_mined.saturating_add(reward);
                            }
                        }
                        ReplayRow::Tx { from, to, amount, fee, nonce, miner } => {
                            debit(&mut balances, &from, amount.saturating_add(fee));
                            credit_balance(&mut balances, &to, amount);
                            if fee > 0 && !miner.is_empty() {
                                credit_balance(&mut balances, &miner, fee);
                            }
                            let next_nonce = nonce.saturating_add(1);
                            let entry = nonces.entry(from).or_insert(0);
                            if next_nonce > *entry { *entry = next_nonce; }
                        }
                    }
                }
            }
        } else {
            // Ensure parent directory exists
            if let Some(parent) = csv_path.parent() {
                std::fs::create_dir_all(parent)
                    .context("creating data directory for ledger")?;
            }
            // Write CSV header
            let mut f = File::create(&csv_path)
                .context("creating ledger CSV")?;
            writeln!(f, "height,miner_address,reward_bina,total_mined_bina,timestamp_unix")?;
        }

        Ok(Self { balances, nonces, total_mined, csv_path })
    }

    // ── Balance queries ──────────────────────────────────────────────────────

    pub fn balance(&self, address: &str) -> u64 {
        self.balances.get(address).copied().unwrap_or(0)
    }

    pub fn nonce(&self, address: &str) -> u64 {
        self.nonces.get(address).copied().unwrap_or(0)
    }

    pub fn total_mined(&self) -> u64 { self.total_mined }

    pub fn hard_cap() -> u64 { HARD_CAP }

    pub fn supply_remaining(&self) -> u64 { supply_remaining(self.total_mined) }

    /// All addresses with a non-zero balance (for rich-list / API).
    pub fn all_balances(&self) -> Vec<(String, u64)> {
        let mut v: Vec<_> = self.balances.iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v
    }

    // ── State commitment ────────────────────────────────────────────────────

    /// Deterministic commitment to the full ledger state (every address with
    /// a nonzero balance and/or a recorded nonce). Any two nodes that agree
    /// on chain history up to and including a given block must compute the
    /// same value here — this is what `L1BlockHeader.state_root` commits to.
    pub fn state_root(&self) -> [u8; 32] {
        self.state_root_with_overlay(&HashMap::new())
    }

    /// Same commitment, but with `overlay` entries (address → (balance,
    /// nonce)) taking precedence over the ledger's own stored values. Used
    /// to compute the state root a block WOULD produce before actually
    /// applying it (see `simulate_block_execution`).
    pub fn state_root_with_overlay(&self, overlay: &HashMap<String, (u64, u64)>) -> [u8; 32] {
        let mut addrs: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        addrs.extend(self.balances.keys().map(String::as_str));
        addrs.extend(self.nonces.keys().map(String::as_str));
        addrs.extend(overlay.keys().map(String::as_str));

        let entries = addrs.into_iter().map(|addr| {
            let (balance, nonce) = overlay
                .get(addr)
                .copied()
                .unwrap_or_else(|| (self.balance(addr), self.nonce(addr)));
            (addr, balance, nonce)
        });
        compute_state_root(entries)
    }

    // ── Crediting ────────────────────────────────────────────────────────────

    /// Credit `reward` BINA to `miner_address` for `height`.
    /// Appends a row to the CSV immediately.
    /// Returns the new balance for that address.
    pub fn credit(
        &mut self,
        height:        u64,
        miner_address: &str,
        reward:        u64,
        timestamp:     u64,
    ) -> Result<u64> {
        if reward == 0 { return Ok(self.balance(miner_address)); }

        self.total_mined = self.total_mined.saturating_add(reward);
        let balance = self.balances.entry(miner_address.to_string())
            .or_insert(0);
        *balance += reward;
        let new_balance = *balance;

        // Append row to CSV
        let mut f = OpenOptions::new()
            .append(true)
            .open(&self.csv_path)
            .context("opening ledger CSV for append")?;
        writeln!(
            f,
            "{height},{miner_address},{reward},{},{timestamp}",
            self.total_mined
        )?;

        Ok(new_balance)
    }

    /// Apply a verified user transfer and credit the fee to `miner_address`.
    ///
    /// The transaction nonce must match the sender's next nonce exactly.
    /// `total_mined` is unchanged because transfers do not create new BINA.
    pub fn apply_transaction(
        &mut self,
        height: u64,
        signed: &SignedTransaction,
        miner_address: &str,
        timestamp: u64,
    ) -> Result<()> {
        signed.verify()?;

        let from = signed.from_hex();
        let to = signed.to_hex();
        let expected_nonce = self.nonce(&from);
        if signed.tx.nonce != expected_nonce {
            bail!("bad transaction nonce: expected {expected_nonce}, got {}", signed.tx.nonce);
        }

        let debit_total = signed.tx.amount.checked_add(signed.tx.fee)
            .ok_or_else(|| anyhow!("transaction amount + fee overflow"))?;
        let balance = self.balance(&from);
        if balance < debit_total {
            bail!("insufficient balance: have {balance}, need {debit_total}");
        }

        debit_checked(&mut self.balances, &from, debit_total)?;
        credit_balance(&mut self.balances, &to, signed.tx.amount);
        if signed.tx.fee > 0 && !miner_address.is_empty() {
            credit_balance(&mut self.balances, miner_address, signed.tx.fee);
        }
        self.nonces.insert(from.clone(), signed.tx.nonce.saturating_add(1));

        let mut f = OpenOptions::new()
            .append(true)
            .open(&self.csv_path)
            .context("opening ledger CSV for transaction append")?;
        writeln!(
            f,
            "tx,{height},{},{from},{to},{},{},{},{},{},{timestamp}",
            signed.tx_id_hex(),
            signed.tx.amount,
            signed.tx.fee,
            signed.tx.nonce,
            miner_address,
            self.total_mined
        )?;

        Ok(())
    }
}

/// Deterministically evaluate what a block containing `txs` (in order) plus
/// a `reward` credit to `miner_address` would do to `ledger`'s state,
/// WITHOUT mutating it. Invalid transactions (bad signature, wrong nonce,
/// insufficient balance against the state as of their position in the
/// list) are silently dropped rather than aborting the block — the same
/// rule every node applies, so any two nodes evaluating the same candidate
/// list against the same parent state reach the same `applied` set and the
/// same state root.
///
/// Returns the transactions that actually applied (in order) and the
/// resulting state root. Callers apply the returned list for real via
/// `apply_transaction`/`credit` only once a block is actually finalized —
/// simulating first lets a miner (or a validator checking a peer's claim)
/// learn the state root before doing any durable/expensive work.
pub fn simulate_block_execution(
    ledger: &RewardLedger,
    txs: &[SignedTransaction],
    miner_address: &str,
    reward: u64,
) -> (Vec<SignedTransaction>, [u8; 32]) {
    let mut overlay: HashMap<String, (u64, u64)> = HashMap::new();
    let mut applied = Vec::new();

    let read = |overlay: &HashMap<String, (u64, u64)>, addr: &str| -> (u64, u64) {
        overlay
            .get(addr)
            .copied()
            .unwrap_or_else(|| (ledger.balance(addr), ledger.nonce(addr)))
    };

    for tx in txs {
        if tx.verify().is_err() {
            continue;
        }
        let from = tx.from_hex();
        let to = tx.to_hex();

        let (from_balance, from_nonce) = read(&overlay, &from);
        if tx.tx.nonce != from_nonce {
            continue;
        }
        let debit_total = match tx.tx.amount.checked_add(tx.tx.fee) {
            Some(d) => d,
            None => continue,
        };
        if from_balance < debit_total {
            continue;
        }

        overlay.insert(from.clone(), (from_balance - debit_total, from_nonce + 1));
        let (to_balance, to_nonce) = read(&overlay, &to);
        overlay.insert(to.clone(), (to_balance.saturating_add(tx.tx.amount), to_nonce));
        if tx.tx.fee > 0 && !miner_address.is_empty() {
            let (miner_balance, miner_nonce) = read(&overlay, miner_address);
            overlay.insert(miner_address.to_string(), (miner_balance.saturating_add(tx.tx.fee), miner_nonce));
        }

        applied.push(tx.clone());
    }

    if reward > 0 {
        let (miner_balance, miner_nonce) = read(&overlay, miner_address);
        overlay.insert(miner_address.to_string(), (miner_balance.saturating_add(reward), miner_nonce));
    }

    let state_root = ledger.state_root_with_overlay(&overlay);
    (applied, state_root)
}

fn credit_balance(balances: &mut HashMap<String, u64>, address: &str, amount: u64) {
    let balance = balances.entry(address.to_string()).or_insert(0);
    *balance = balance.saturating_add(amount);
}

fn debit(balances: &mut HashMap<String, u64>, address: &str, amount: u64) {
    let balance = balances.entry(address.to_string()).or_insert(0);
    *balance = balance.saturating_sub(amount);
}

fn debit_checked(balances: &mut HashMap<String, u64>, address: &str, amount: u64) -> Result<()> {
    let balance = balances.entry(address.to_string()).or_insert(0);
    if *balance < amount {
        bail!("insufficient balance: have {}, need {amount}", *balance);
    }
    *balance -= amount;
    Ok(())
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reward_era_0() {
        assert_eq!(block_reward(0, 0), 50);
        assert_eq!(block_reward(HALVING_INTERVAL - 1, 0), 50);
    }

    #[test]
    fn reward_era_1() {
        assert_eq!(block_reward(HALVING_INTERVAL, 0), 25);
    }

    #[test]
    fn reward_era_2() {
        assert_eq!(block_reward(HALVING_INTERVAL * 2, 0), 12);
    }

    #[test]
    fn hard_cap_enforced() {
        let almost = HARD_CAP - 10;
        assert_eq!(block_reward(0, almost), 10);
        assert_eq!(block_reward(0, HARD_CAP), 0);
    }

    #[test]
    fn ledger_roundtrip() {
        let tmp = std::env::temp_dir().join("bina_test_ledger.csv");
        let _ = std::fs::remove_file(&tmp);

        let mut ledger = RewardLedger::open(&tmp).unwrap();
        ledger.credit(1, "aabbcc", 50, 1_000_000).unwrap();
        ledger.credit(2, "aabbcc", 50, 1_000_004).unwrap();
        ledger.credit(3, "ddeeff", 50, 1_000_008).unwrap();

        assert_eq!(ledger.balance("aabbcc"), 100);
        assert_eq!(ledger.balance("ddeeff"), 50);
        assert_eq!(ledger.total_mined(), 150);

        // Reload
        let ledger2 = RewardLedger::open(&tmp).unwrap();
        assert_eq!(ledger2.balance("aabbcc"), 100);
        assert_eq!(ledger2.total_mined(), 150);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn ledger_applies_signed_transaction_with_nonce_and_fee() {
        let tmp = std::env::temp_dir().join("bina_test_tx_ledger.csv");
        let _ = std::fs::remove_file(&tmp);

        let sender = crate::crypto::WalletKeypair::generate();
        let recipient = crate::crypto::WalletKeypair::generate();
        let miner = crate::crypto::WalletKeypair::generate();
        let sender_hex = sender.address_hex();
        let recipient_hex = recipient.address_hex();
        let miner_hex = miner.address_hex();

        let mut ledger = RewardLedger::open(&tmp).unwrap();
        ledger.credit(1, &sender_hex, 100, 1_000_000).unwrap();
        let tx = crate::transaction::Transaction::new(sender.address(), recipient.address(), 25, 0, 2);
        let signed = crate::transaction::SignedTransaction::sign(tx, &sender).unwrap();
        ledger.apply_transaction(2, &signed, &miner_hex, 1_000_004).unwrap();

        assert_eq!(ledger.balance(&sender_hex), 73);
        assert_eq!(ledger.balance(&recipient_hex), 25);
        assert_eq!(ledger.balance(&miner_hex), 2);
        assert_eq!(ledger.nonce(&sender_hex), 1);
        assert_eq!(ledger.total_mined(), 100);

        let reloaded = RewardLedger::open(&tmp).unwrap();
        assert_eq!(reloaded.balance(&sender_hex), 73);
        assert_eq!(reloaded.balance(&recipient_hex), 25);
        assert_eq!(reloaded.balance(&miner_hex), 2);
        assert_eq!(reloaded.nonce(&sender_hex), 1);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn ledger_applies_ed25519_only_transaction_to_native_wallet() {
        let tmp = std::env::temp_dir().join("bina_test_ed25519_tx_ledger.csv");
        let _ = std::fs::remove_file(&tmp);

        let sender = ed25519_dalek::SigningKey::from_bytes(&[11u8; 32]);
        let recipient = crate::crypto::WalletKeypair::generate();
        let sender_address = crate::transaction::ed25519_only_address(
            sender.verifying_key().as_bytes(),
        ).unwrap();
        let sender_hex = hex::encode(sender_address);
        let recipient_hex = recipient.address_hex();

        let mut ledger = RewardLedger::open(&tmp).unwrap();
        ledger.credit(1, &sender_hex, 100, 1_000_000).unwrap();
        let tx = crate::transaction::Transaction::new(sender_address, recipient.address(), 40, 0, 1);
        let signed = crate::transaction::SignedTransaction::sign_ed25519_only(tx, &sender).unwrap();
        ledger.apply_transaction(2, &signed, "", 1_000_004).unwrap();

        assert_eq!(ledger.balance(&sender_hex), 59);
        assert_eq!(ledger.balance(&recipient_hex), 40);
        assert_eq!(ledger.nonce(&sender_hex), 1);

        let reloaded = RewardLedger::open(&tmp).unwrap();
        assert_eq!(reloaded.balance(&sender_hex), 59);
        assert_eq!(reloaded.balance(&recipient_hex), 40);
        assert_eq!(reloaded.nonce(&sender_hex), 1);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn ledger_rejects_replayed_nonce() {
        let tmp = std::env::temp_dir().join("bina_test_tx_replay_ledger.csv");
        let _ = std::fs::remove_file(&tmp);

        let sender = crate::crypto::WalletKeypair::generate();
        let recipient = crate::crypto::WalletKeypair::generate();
        let sender_hex = sender.address_hex();
        let mut ledger = RewardLedger::open(&tmp).unwrap();
        ledger.credit(1, &sender_hex, 100, 1_000_000).unwrap();

        let tx = crate::transaction::Transaction::new(sender.address(), recipient.address(), 25, 1, 2);
        let signed = crate::transaction::SignedTransaction::sign(tx, &sender).unwrap();
        assert!(ledger.apply_transaction(2, &signed, "", 1_000_004).is_err());

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn empty_state_root_matches_compute_with_no_entries() {
        assert_eq!(empty_state_root(), compute_state_root(std::iter::empty()));
    }

    #[test]
    fn state_root_is_order_independent() {
        let a = compute_state_root(vec![("bbbb", 1, 0), ("aaaa", 2, 0)]);
        let b = compute_state_root(vec![("aaaa", 2, 0), ("bbbb", 1, 0)]);
        assert_eq!(a, b, "state root must not depend on iteration order");
    }

    #[test]
    fn state_root_changes_with_any_balance_or_nonce_change() {
        let base = compute_state_root(vec![("aaaa", 10, 0)]);
        let diff_balance = compute_state_root(vec![("aaaa", 11, 0)]);
        let diff_nonce = compute_state_root(vec![("aaaa", 10, 1)]);
        assert_ne!(base, diff_balance);
        assert_ne!(base, diff_nonce);
    }

    #[test]
    fn simulate_applies_valid_tx_and_matches_real_apply() {
        let tmp = std::env::temp_dir().join("bina_test_simulate_ledger.csv");
        let _ = std::fs::remove_file(&tmp);

        let sender = crate::crypto::WalletKeypair::generate();
        let recipient = crate::crypto::WalletKeypair::generate();
        let miner = crate::crypto::WalletKeypair::generate();
        let mut ledger = RewardLedger::open(&tmp).unwrap();
        ledger.credit(1, &sender.address_hex(), 100, 1_000_000).unwrap();

        let tx = crate::transaction::Transaction::new(sender.address(), recipient.address(), 25, 0, 2);
        let signed = crate::transaction::SignedTransaction::sign(tx, &sender).unwrap();

        let (applied, sim_root) = simulate_block_execution(&ledger, &[signed.clone()], &miner.address_hex(), 50);
        assert_eq!(applied.len(), 1, "valid tx must be included");

        // Apply for real and confirm the resulting ledger's own state root matches the simulated one.
        ledger.apply_transaction(2, &signed, &miner.address_hex(), 1_000_004).unwrap();
        ledger.credit(2, &miner.address_hex(), 50, 1_000_004).unwrap();
        assert_eq!(ledger.state_root(), sim_root, "real execution must match the simulated state root");

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn simulate_drops_transactions_that_would_overdraw() {
        let tmp = std::env::temp_dir().join("bina_test_simulate_overdraw_ledger.csv");
        let _ = std::fs::remove_file(&tmp);

        let sender = crate::crypto::WalletKeypair::generate();
        let recipient = crate::crypto::WalletKeypair::generate();
        let ledger = RewardLedger::open(&tmp).unwrap(); // sender has 0 balance

        let tx = crate::transaction::Transaction::new(sender.address(), recipient.address(), 25, 0, 2);
        let signed = crate::transaction::SignedTransaction::sign(tx, &sender).unwrap();

        let (applied, root) = simulate_block_execution(&ledger, &[signed], "", 0);
        assert!(applied.is_empty(), "overdrawing tx must be dropped, not applied");
        assert_eq!(root, ledger.state_root(), "dropped-only block must leave state root unchanged");

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn simulate_drops_second_conflicting_nonce_in_same_batch() {
        let tmp = std::env::temp_dir().join("bina_test_simulate_double_spend_ledger.csv");
        let _ = std::fs::remove_file(&tmp);

        let sender = crate::crypto::WalletKeypair::generate();
        let r1 = crate::crypto::WalletKeypair::generate();
        let r2 = crate::crypto::WalletKeypair::generate();
        let mut ledger = RewardLedger::open(&tmp).unwrap();
        ledger.credit(1, &sender.address_hex(), 100, 1_000_000).unwrap();

        // Two transactions both claiming nonce 0 from the same sender, spending
        // more than the balance can cover twice over.
        let tx1 = crate::transaction::Transaction::new(sender.address(), r1.address(), 80, 0, 0);
        let signed1 = crate::transaction::SignedTransaction::sign(tx1, &sender).unwrap();
        let tx2 = crate::transaction::Transaction::new(sender.address(), r2.address(), 80, 0, 0);
        let signed2 = crate::transaction::SignedTransaction::sign(tx2, &sender).unwrap();

        let (applied, _root) = simulate_block_execution(&ledger, &[signed1, signed2], "", 0);
        assert_eq!(applied.len(), 1, "only the first valid nonce-0 tx may apply; the conflicting one must be dropped");

        let _ = std::fs::remove_file(&tmp);
    }
}
