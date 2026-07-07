//! Bina Chain emission schedule + reward ledger
//!
//! Supply parameters
//! ─────────────────────────────────────────────────────────────────────────
//!   Hard cap             : 1,000,000,000 BINA  (1 billion, absolute ceiling)
//!   Initial block reward : 50 BINA
//!   Halving interval     : 17,280,000 blocks   (≈ 2 years at 3.65 s/block)
//!   Halving schedule     :
//!     Era 0  blocks      0 – 17,280,000  reward 50   BINA   total ≈  864 M
//!     Era 1  blocks 17.28 M – 34.56 M   reward 25   BINA   total ≈  864 + 432 M
//!                                         ↑ hard cap 1 B hit ≈ 136 M into era 1
//!     (emission stops when total_mined == HARD_CAP)
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

/// Hard cap on total $BINA supply (1 billion).
pub const HARD_CAP:             u64 = 1_000_000_000;
/// Block reward in era 0.
pub const INITIAL_BLOCK_REWARD: u64 = 50;
/// Number of blocks between halvings (≈ 2 years at 3.65 s/block).
pub const HALVING_INTERVAL:     u64 = 17_280_000;

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
        let csv_path = csv_path.as_ref().to_path_buf();
        let mut balances: HashMap<String, u64> = HashMap::new();
        let mut nonces: HashMap<String, u64> = HashMap::new();
        let mut total_mined: u64 = 0;

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
                    let from = cols[3].trim().to_string();
                    let to = cols[4].trim().to_string();
                    let amount = cols[5].trim().parse::<u64>().unwrap_or(0);
                    let fee = cols[6].trim().parse::<u64>().unwrap_or(0);
                    let nonce = cols[7].trim().parse::<u64>().unwrap_or(0);
                    let miner = cols[8].trim().to_string();
                    debit(&mut balances, &from, amount.saturating_add(fee));
                    credit_balance(&mut balances, &to, amount);
                    if fee > 0 && !miner.is_empty() {
                        credit_balance(&mut balances, &miner, fee);
                    }
                    let next_nonce = nonce.saturating_add(1);
                    let entry = nonces.entry(from).or_insert(0);
                    if next_nonce > *entry { *entry = next_nonce; }
                    total_mined = cols[9].trim().parse::<u64>().unwrap_or(total_mined);
                } else {
                    if cols.len() < 4 { continue; }
                    // Legacy reward row: height, miner_address, reward_bina, total_mined_bina [, timestamp]
                    let addr = cols[1].trim().to_string();
                    let reward = cols[2].trim().parse::<u64>().unwrap_or(0);
                    let total = cols[3].trim().parse::<u64>().unwrap_or(0);
                    credit_balance(&mut balances, &addr, reward);
                    total_mined = total;
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
}
