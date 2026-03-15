use crate::config::MinerConfig;
use crate::crypto::DynamicMiningCommitment;
use crate::telemetry::{MiningPhase, PhaseTracker};
use anyhow::{anyhow, Context, Result};
use ethers::{
    abi::{encode, Token},
    contract::abigen,
    middleware::SignerMiddleware,
    providers::{Http, Middleware, Provider},
    signers::{LocalWallet, Signer},
    types::{Address, Bytes, H256, TransactionReceipt, U256},
    utils::keccak256,
};
use rand::{rngs::OsRng, RngCore};
use std::{fs, path::Path, sync::Arc, time::Duration};
use zeroize::Zeroize;

pub const DEFAULT_CONTRACT_PLACEHOLDER: &str = "0xYourContractAddress";

abigen!(
    MiningContract,
    r#"[
        function submitMiningCommitment(bytes32 commitHash, uint8 poolId, uint256 nonce, uint256 deadline, bytes signature) external returns (bool)
        function revealMiningCommitment(bytes32 previousOutput, bytes temporalSeed, uint64 nonce, bytes signature, bytes32 secretValue, uint8 poolId) external
        function minCommitmentAge() external view returns (uint256)
        function maxCommitmentAge() external view returns (uint256)
        function getMiningChallenge(uint8 poolId) external view returns (bytes32[] outputs, uint256 difficulty)
        function nonces(address miner) external view returns (uint256)
        function minBlockInterval() external view returns (uint8)
        function lastMinerBlock(address miner) external view returns (uint64)
    ]"#
);

pub type MiningClient = SignerMiddleware<Provider<Http>, LocalWallet>;

#[derive(Debug, Clone)]
pub struct LiveChallenge {
    pub previous_output: [u8; 32],
    pub difficulty: U256,
    pub block_timestamp: u64,
    pub prevrandao: [u8; 32],
}

#[derive(Debug, Clone)]
pub struct LiveSubmission {
    pub commitment: DynamicMiningCommitment,
    pub previous_output: [u8; 32],
    pub temporal_seed: [u8; 8],
    pub nonce: u64,
    pub reveal_signature: Vec<u8>,
    pub secret_value: [u8; 32],
}

#[derive(Clone)]
pub struct LiveMiningClient {
    pub provider: Arc<Provider<Http>>,
    pub client: Arc<MiningClient>,
    pub contract_address: Address,
    pub pool_id: u8,
}

impl LiveMiningClient {
    pub async fn connect(config: &MinerConfig) -> Result<Option<Self>> {
        if !config.has_live_target() {
            return Ok(None);
        }

        let provider = Provider::<Http>::try_from(config.rpc_url.as_str())?
            .interval(Duration::from_millis(750));
        let provider = Arc::new(provider);
        let chain_id = provider.get_chainid().await?.as_u64();
        let wallet = load_or_create_wallet(config)?.with_chain_id(chain_id);
        let client = Arc::new(SignerMiddleware::new(provider.as_ref().clone(), wallet));
        let contract_address = config
            .contract_address
            .parse::<Address>()
            .with_context(|| format!("Invalid contract address {}", config.contract_address))?;

        Ok(Some(Self {
            provider,
            client,
            contract_address,
            pool_id: config.pool_id,
        }))
    }

    pub fn miner_address(&self) -> Address {
        self.client.address()
    }

    pub fn signer_clone(&self) -> LocalWallet {
        self.client.signer().clone()
    }

    pub async fn current_challenge(&self) -> Result<LiveChallenge> {
        let contract = MiningContract::new(self.contract_address, Arc::clone(&self.provider));
        let (outputs, difficulty) = contract.get_mining_challenge(self.pool_id).call().await?;
        let previous_output = *outputs
            .first()
            .ok_or_else(|| anyhow!("No outputs returned from getMiningChallenge"))?;
        let block = self
            .provider
            .get_block(ethers::types::BlockNumber::Latest)
            .await?
            .ok_or_else(|| anyhow!("Latest block unavailable"))?;

        Ok(LiveChallenge {
            previous_output,
            difficulty,
            block_timestamp: block.timestamp.as_u64(),
            prevrandao: block.mix_hash.unwrap_or_default().0,
        })
    }

    pub async fn next_commit_nonce(&self) -> Result<u64> {
        let contract = MiningContract::new(self.contract_address, Arc::clone(&self.provider));
        let nonce = contract.nonces(self.miner_address()).call().await?;
        Ok(nonce.as_u64())
    }

    pub fn sign_entropy_hash(&self, entropy_hash: [u8; 32]) -> Result<Vec<u8>> {
        Ok(self.client.signer().sign_hash(H256::from(entropy_hash))?.to_vec())
    }

    pub async fn sign_commitment(&self, commitment: &DynamicMiningCommitment) -> Result<Bytes> {
        let digest = mining_commitment_digest(
            self.miner_address(),
            commitment,
            self.client.signer().chain_id(),
            self.contract_address,
        );
        Ok(self.client.signer().sign_hash(H256::from(digest))?.to_vec().into())
    }

    pub async fn submit_solution(
        &self,
        submission: &LiveSubmission,
        phase: &PhaseTracker,
        key_path: &str,
    ) -> Result<TransactionReceipt> {
        let contract = MiningContract::new(self.contract_address, Arc::clone(&self.client));

        // Check for and wait out any existing active commitment
        self.wait_for_commitment_clearance(&contract, phase).await?;

        // ── Wait for mining cooldown (minBlockInterval) ──────────────
        self.wait_for_mining_cooldown(&contract, phase).await?;

        // ── Save pending commitment to disk BEFORE sending the tx ──
        let pending = crate::pending::PendingCommitment {
            commit_block: 0, // updated after tx confirms
            previous_output: submission.previous_output,
            temporal_seed: submission.temporal_seed,
            nonce: submission.nonce,
            reveal_signature: submission.reveal_signature.clone(),
            secret_value: submission.secret_value,
            commit_hash: submission.commitment.commit_hash,
            pool_id: submission.commitment.pool_id,
        };
        crate::pending::save(key_path, &pending)?;

        phase.set(MiningPhase::Committing, None);
        let commitment_signature = self.sign_commitment(&submission.commitment).await?;

        let commit_tx = contract.submit_mining_commitment(
            submission.commitment.commit_hash.into(),
            submission.commitment.pool_id,
            U256::from(submission.commitment.nonce),
            U256::from(submission.commitment.deadline),
            commitment_signature,
        );
        let commit_gas = commit_tx
            .estimate_gas()
            .await
            .context("Failed to estimate commitment gas")?;
        let commit_tx = commit_tx.gas(apply_gas_buffer(commit_gas));
        let pending_commit = commit_tx.send().await.context("Failed to send commitment")?;
        let commit_receipt = pending_commit
            .await?
            .ok_or_else(|| anyhow!("No commitment receipt"))?;

        // Update pending file with the actual commit block
        let commit_block = commit_receipt
            .block_number
            .ok_or_else(|| anyhow!("Missing commitment block number"))?
            .as_u64();
        let updated = crate::pending::PendingCommitment {
            commit_block,
            ..pending
        };
        crate::pending::save(key_path, &updated)?;

        let min_blocks = contract.min_commitment_age().call().await?.as_u64();
        let target_block = commit_block.saturating_add(min_blocks);
        phase.set(MiningPhase::CommitmentLocked, Some(min_blocks));

        loop {
            let current_block = self.provider.get_block_number().await?.as_u64();
            if current_block >= target_block {
                break;
            }
            let remaining = target_block.saturating_sub(current_block);
            phase.set(MiningPhase::CommitmentLocked, Some(remaining));
            tokio::time::sleep(Duration::from_secs(2)).await;
        }

        phase.set(MiningPhase::Revealing, None);
        let receipt = self.send_reveal(&contract, submission).await?;

        // Reveal succeeded — clear the pending file
        crate::pending::clear(key_path)?;
        Ok(receipt)
    }

    /// Attempt to reveal a commitment that was saved to disk from a previous
    /// run. Returns `Ok(Some(receipt))` if the reveal succeeds, `Ok(None)` if
    /// the commitment has expired or been revealed already, and `Err` on
    /// unexpected failures.
    pub async fn reveal_pending(
        &self,
        pending: &crate::pending::PendingCommitment,
        phase: &PhaseTracker,
        key_path: &str,
    ) -> Result<Option<TransactionReceipt>> {
        let contract = MiningContract::new(self.contract_address, Arc::clone(&self.client));
        let max_age = contract.max_commitment_age().call().await?.as_u64();
        let min_age = contract.min_commitment_age().call().await?.as_u64();
        let current_block = self.provider.get_block_number().await?.as_u64();
        let expires_at = pending.commit_block.saturating_add(max_age);
        let reveal_at = pending.commit_block.saturating_add(min_age);

        if current_block > expires_at {
            tracing::warn!(
                "Saved commitment from block {} has expired (current {current_block} > {expires_at}), discarding",
                pending.commit_block
            );
            crate::pending::clear(key_path)?;
            return Ok(None);
        }

        tracing::info!(
            "Resuming reveal for saved commitment from block {} (reveal window {reveal_at}..{expires_at}, current {current_block})",
            pending.commit_block
        );

        // Wait for reveal window
        if current_block < reveal_at {
            let blocks_to_wait = reveal_at.saturating_sub(current_block);
            phase.set(MiningPhase::CommitmentLocked, Some(blocks_to_wait));
            loop {
                let now = self.provider.get_block_number().await?.as_u64();
                if now >= reveal_at {
                    break;
                }
                let remaining = reveal_at.saturating_sub(now);
                phase.set(MiningPhase::CommitmentLocked, Some(remaining));
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }

        phase.set(MiningPhase::Revealing, None);
        let submission = LiveSubmission {
            commitment: crate::crypto::DynamicMiningCommitment {
                commit_hash: pending.commit_hash,
                pool_id: pending.pool_id,
                nonce: 0,    // not used for reveal
                deadline: 0, // not used for reveal
            },
            previous_output: pending.previous_output,
            temporal_seed: pending.temporal_seed,
            nonce: pending.nonce,
            reveal_signature: pending.reveal_signature.clone(),
            secret_value: pending.secret_value,
        };

        let receipt = self.send_reveal(&contract, &submission).await?;

        crate::pending::clear(key_path)?;
        tracing::info!("Successfully revealed saved commitment!");
        Ok(Some(receipt))
    }

    /// Internal helper that sends only the reveal tx.
    async fn send_reveal(
        &self,
        contract: &MiningContract<MiningClient>,
        submission: &LiveSubmission,
    ) -> Result<TransactionReceipt> {
        let reveal_tx = contract.reveal_mining_commitment(
            submission.previous_output.into(),
            Bytes::from(submission.temporal_seed.to_vec()),
            submission.nonce,
            Bytes::from(submission.reveal_signature.clone()),
            submission.secret_value.into(),
            submission.commitment.pool_id,
        );
        let reveal_gas = reveal_tx
            .estimate_gas()
            .await
            .context("Failed to estimate reveal gas")?;
        let reveal_tx = reveal_tx.gas(apply_gas_buffer(reveal_gas));
        let pending_reveal = reveal_tx.send().await.context("Failed to send reveal")?;
        pending_reveal
            .await?
            .ok_or_else(|| anyhow!("No reveal receipt"))
    }

    /// Checks if there is an existing unrevealed commitment for the miner.
    /// If one exists, waits until it expires (block > timestamp + maxCommitmentAge).
    async fn wait_for_commitment_clearance(
        &self,
        contract: &MiningContract<MiningClient>,
        phase: &PhaseTracker,
    ) -> Result<()> {
        // Call minerCommitments(address) and decode raw ABI output
        let selector = keccak256("minerCommitments(address)");
        let mut full_calldata = selector[..4].to_vec();
        full_calldata.extend_from_slice(&encode(&[Token::Address(self.miner_address())]));

        let tx = ethers::types::TransactionRequest::new()
            .to(self.contract_address)
            .data(Bytes::from(full_calldata));
        let result = self.provider.call(&tx.into(), None).await?;

        if result.len() < 128 {
            // Not enough data — no commitment exists
            return Ok(());
        }

        // ABI layout: slot 0 = commitHash (bytes32), slot 1 = timestamp (uint64 packed),
        // slot 2 = flags struct: revealed is the first bool at offset 64..96
        let commit_hash = &result[0..32];
        if commit_hash.iter().all(|&b| b == 0) {
            return Ok(());
        }

        // timestamp is uint64, ABI-encoded as uint256 in slot 1
        let commit_block = U256::from_big_endian(&result[32..64]).as_u64();

        // flags struct starts at slot 2: first element is `revealed` (bool as uint256)
        let revealed = U256::from_big_endian(&result[64..96]) != U256::zero();

        if revealed {
            return Ok(());
        }

        let max_age = contract.max_commitment_age().call().await?.as_u64();
        let expires_at = commit_block.saturating_add(max_age);

        tracing::info!(
            "Active unrevealed commitment found (block {commit_block}), waiting for expiry at block {expires_at}"
        );

        loop {
            let current_block = self.provider.get_block_number().await?.as_u64();
            if current_block > expires_at {
                tracing::info!("Stale commitment expired at block {current_block}, proceeding");
                return Ok(());
            }
            let remaining = expires_at.saturating_sub(current_block);
            phase.set(MiningPhase::WaitingForClearance, Some(remaining));
            tracing::info!("Waiting for commitment expiry: {remaining} blocks remaining (~{}s)", remaining * 12);
            tokio::time::sleep(Duration::from_secs(12)).await;
        }
    }

    /// Waits until the contract's `minBlockInterval` cooldown has elapsed since
    /// the miner's last successfully mined block. Without this, rapid re-commits
    /// after a reveal are rejected by the contract with `MiningTooFrequently`.
    async fn wait_for_mining_cooldown(
        &self,
        contract: &MiningContract<MiningClient>,
        phase: &PhaseTracker,
    ) -> Result<()> {
        let interval = contract.min_block_interval().call().await.unwrap_or(0) as u64;
        if interval == 0 {
            return Ok(());
        }
        let last_block = contract.last_miner_block(self.miner_address()).call().await.unwrap_or(0);
        if last_block == 0 {
            return Ok(());
        }
        let ready_at = last_block.saturating_add(interval);
        loop {
            let current_block = self.provider.get_block_number().await?.as_u64();
            if current_block >= ready_at {
                return Ok(());
            }
            let remaining = ready_at.saturating_sub(current_block);
            phase.set(MiningPhase::WaitingForClearance, Some(remaining));
            tracing::info!(
                "Mining cooldown: {remaining} blocks until minBlockInterval clears (~{}s)",
                remaining * 12
            );
            tokio::time::sleep(Duration::from_secs(12)).await;
        }
    }

    pub fn extract_reward_from_receipt(receipt: &TransactionReceipt) -> Option<f64> {
        let beacon_mined_sig = H256::from(keccak256("BeaconBlockMined(address,bytes32,uint256,uint64,uint64,uint8)"));
        let core_output_recorded_sig = H256::from(keccak256("CoreOutputRecorded(bytes32,address,uint8,uint256,uint64)"));
        for log in &receipt.logs {
            if log.topics.first() == Some(&beacon_mined_sig) && log.data.0.len() >= 64 {
                let reward_u256 = U256::from_big_endian(&log.data.0[32..64]);
                return ethers::utils::format_units(reward_u256, 18).ok().and_then(|value| value.parse().ok());
            }

            if log.topics.first() == Some(&core_output_recorded_sig) && log.data.0.len() >= 32 {
                let reward_u256 = U256::from_big_endian(&log.data.0[0..32]);
                return ethers::utils::format_units(reward_u256, 18).ok().and_then(|value| value.parse().ok());
            }
        }
        None
    }

    pub fn extract_output_hash_from_receipt(receipt: &TransactionReceipt) -> Option<String> {
        let core_output_recorded_sig = H256::from(keccak256("CoreOutputRecorded(bytes32,address,uint8,uint256,uint64)"));
        let commitment_revealed_sig = H256::from(keccak256("CommitmentRevealed(address,bytes32,uint8)"));

        for log in &receipt.logs {
            if log.topics.first() == Some(&core_output_recorded_sig) {
                if let Some(topic) = log.topics.get(1) {
                    return Some(format_hash(topic));
                }
            }

            if log.topics.first() == Some(&commitment_revealed_sig) && log.data.0.len() >= 32 {
                let mut out = [0u8; 32];
                out.copy_from_slice(&log.data.0[0..32]);
                return Some(format_bytes32(&out));
            }
        }

        None
    }
}

fn format_hash(hash: &H256) -> String {
    format_bytes32(hash.as_fixed_bytes())
}

fn format_bytes32(bytes: &[u8; 32]) -> String {
    let mut text = String::from("0x");
    text.push_str(&hex::encode(bytes));
    text
}

fn apply_gas_buffer(estimated: U256) -> U256 {
    estimated
        .saturating_mul(U256::from(130u64))
        .checked_div(U256::from(100u64))
        .unwrap_or(estimated)
}

/// Returns the hex-formatted wallet address derived from the miner key file.
pub fn wallet_address_from_config(config: &MinerConfig) -> Result<String> {
    let wallet = load_or_create_wallet(config)?;
    Ok(format!("{:?}", wallet.address()))
}

fn load_or_create_wallet(config: &MinerConfig) -> Result<LocalWallet> {
    let key_path = Path::new(&config.private_key_path);
    if let Some(parent) = key_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    let key_bytes = if key_path.exists() {
        let raw = fs::read_to_string(key_path)
            .with_context(|| format!("Failed to read {}", key_path.display()))?;
        hex::decode(raw.trim()).context("Private key must be hex-encoded")?
    } else {
        let mut generated = [0u8; 32];
        OsRng.fill_bytes(&mut generated);
        fs::write(key_path, hex::encode(generated))
            .with_context(|| format!("Failed to write {}", key_path.display()))?;
        generated.to_vec()
    };

    if key_bytes.len() != 32 {
        return Err(anyhow!("Expected a 32-byte private key"));
    }

    let mut temp = [0u8; 32];
    temp.copy_from_slice(&key_bytes);
    let wallet = LocalWallet::from_bytes(&temp).context("Invalid secp256k1 private key")?;
    temp.zeroize();
    Ok(wallet)
}

fn mining_commitment_digest(
    miner: Address,
    commitment: &DynamicMiningCommitment,
    chain_id: u64,
    verifying_contract: Address,
) -> [u8; 32] {
    let domain_typehash = keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)");
    let name_hash = keccak256("TemporalGradientBeacon");
    let version_hash = keccak256("1");
    let typehash = keccak256("MiningCommitment(address miner,bytes32 commitHash,uint256 poolId,uint256 nonce,uint256 deadline)");

    let domain_separator = keccak256(encode(&[
        Token::FixedBytes(domain_typehash.to_vec()),
        Token::FixedBytes(name_hash.to_vec()),
        Token::FixedBytes(version_hash.to_vec()),
        Token::Uint(U256::from(chain_id)),
        Token::Address(verifying_contract),
    ]));

    let struct_hash = keccak256(encode(&[
        Token::FixedBytes(typehash.to_vec()),
        Token::Address(miner),
        Token::FixedBytes(commitment.commit_hash.to_vec()),
        Token::Uint(U256::from(commitment.pool_id)),
        Token::Uint(U256::from(commitment.nonce)),
        Token::Uint(U256::from(commitment.deadline)),
    ]));

    keccak256([b"\x19\x01".as_slice(), domain_separator.as_slice(), struct_hash.as_slice()].concat())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethers::types::{Log, TransactionReceipt};

    #[test]
    fn extract_reward_from_receipt_parses_event_data() {
        let reward = U256::from_dec_str("12500000000000000000").unwrap();
        let solution_hash = [0x11u8; 32];
        let mut reward_bytes = [0u8; 32];
        reward.to_big_endian(&mut reward_bytes);
        let mut event_data = Vec::with_capacity(64);
        event_data.extend_from_slice(&solution_hash);
        event_data.extend_from_slice(&reward_bytes);

        let receipt = TransactionReceipt {
            logs: vec![Log {
                topics: vec![H256::from(keccak256(
                    "BeaconBlockMined(address,bytes32,uint256,uint64,uint64,uint8)"
                ))],
                data: Bytes::from(event_data),
                ..Default::default()
            }],
            ..Default::default()
        };

        assert_eq!(LiveMiningClient::extract_reward_from_receipt(&receipt), Some(12.5));
    }

    #[test]
    fn extract_reward_from_core_output_recorded() {
        let reward = U256::from_dec_str("528125000000000000").unwrap();
        let nonce = U256::from(40832u64);
        let mut reward_bytes = [0u8; 32];
        let mut nonce_bytes = [0u8; 32];
        reward.to_big_endian(&mut reward_bytes);
        nonce.to_big_endian(&mut nonce_bytes);

        let mut event_data = Vec::with_capacity(64);
        event_data.extend_from_slice(&reward_bytes);
        event_data.extend_from_slice(&nonce_bytes);

        let receipt = TransactionReceipt {
            logs: vec![Log {
                topics: vec![H256::from(keccak256(
                    "CoreOutputRecorded(bytes32,address,uint8,uint256,uint64)"
                ))],
                data: Bytes::from(event_data),
                ..Default::default()
            }],
            ..Default::default()
        };

        assert_eq!(LiveMiningClient::extract_reward_from_receipt(&receipt), Some(0.528125));
    }

    #[test]
    fn extract_output_hash_from_core_output_recorded() {
        let output = H256::from([0x59u8; 32]);
        let receipt = TransactionReceipt {
            logs: vec![Log {
                topics: vec![
                    H256::from(keccak256("CoreOutputRecorded(bytes32,address,uint8,uint256,uint64)")),
                    output,
                ],
                ..Default::default()
            }],
            ..Default::default()
        };

        assert_eq!(
            LiveMiningClient::extract_output_hash_from_receipt(&receipt),
            Some(format!("0x{}", hex::encode([0x59u8; 32])))
        );
    }

    #[test]
    fn mining_commitment_digest_is_stable() {
        let miner: Address = "0x000000000000000000000000000000000000bEEF".parse().unwrap();
        let contract: Address = "0x000000000000000000000000000000000000c0DE".parse().unwrap();
        let commitment = DynamicMiningCommitment {
            commit_hash: [0x44; 32],
            pool_id: 3,
            nonce: 7,
            deadline: 42,
        };

        let digest_a = mining_commitment_digest(miner, &commitment, 42161, contract);
        let digest_b = mining_commitment_digest(miner, &commitment, 42161, contract);

        assert_eq!(digest_a, digest_b);
        assert_ne!(digest_a, [0u8; 32]);
    }
}

