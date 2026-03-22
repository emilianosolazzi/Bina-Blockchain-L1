use crate::config::MinerConfig;
use crate::crypto::DynamicMiningCommitment;
use crate::memory::SecureBuffer;
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
use reqwest::header;
use rand::{rngs::OsRng, RngCore};
use serde_json::Value;
use std::{fs, path::Path, sync::Arc, time::Duration};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Retry an async operation with exponential backoff.  Useful for transient
/// RPC errors (429, timeouts, etc.) that resolve after a short wait.
async fn retry_with_backoff<F, Fut, T>(label: &str, max_retries: usize, mut f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut delay = Duration::from_secs(2);
    for attempt in 0..=max_retries {
        match f().await {
            Ok(v) => return Ok(v),
            Err(err) if attempt < max_retries => {
                let msg = format!("{err:#}");
                if msg.contains("429")
                    || msg.contains("Too Many Requests")
                    || msg.contains("rate limit")
                    || msg.contains("timeout")
                    || msg.contains("timed out")
                {
                    tracing::warn!(
                        "{label}: transient RPC error (attempt {}/{}), retrying in {}s — {msg}",
                        attempt + 1,
                        max_retries,
                        delay.as_secs(),
                    );
                    tokio::time::sleep(delay).await;
                    delay = delay.saturating_mul(2).min(Duration::from_secs(30));
                } else {
                    return Err(err);
                }
            }
            Err(err) => return Err(err),
        }
    }
    unreachable!()
}

pub const DEFAULT_CONTRACT_PLACEHOLDER: &str = "0xYourContractAddress";

abigen!(
    MiningContract,
    r#"[
        function submitMiningCommitment(bytes32 commitHash, uint8 poolId, uint256 nonce, uint256 deadline, bytes signature) external returns (bool)
        function revealMiningCommitment(bytes32 previousOutput, bytes temporalSeed, uint64 nonce, bytes signature, bytes32 secretValue, uint8 poolId) external
        function minCommitmentAge() external view returns (uint256)
        function maxCommitmentAge() external view returns (uint256)
        function getMiningChallenge(uint8 poolId) external view returns (bytes32[] outputs, uint256 difficulty)
        function getPoolInfo(uint8 poolId) external view returns (uint256 difficulty, uint256 emission, uint256 mined, bool active)
        function nonces(address miner) external view returns (uint256)
        function minBlockInterval() external view returns (uint8)
        function lastMinerBlock(address miner) external view returns (uint64)
    ]"#
);

pub type MiningClient = SignerMiddleware<Provider<Http>, LocalWallet>;

/// On-chain commitment state for the miner address.
#[derive(Debug, Clone)]
pub struct OnChainCommitment {
    pub commit_block: u64,
    pub revealed: bool,
    pub pool_id: u8,
    pub expires_at: u64,
    pub expired: bool,
}

#[derive(Debug, Clone)]
pub struct LiveChallenge {
    pub previous_output: [u8; 32],
    pub difficulty: U256,
    pub block_timestamp: u64,
    pub prevrandao: [u8; 32],
}

#[derive(Debug, Clone, Zeroize, ZeroizeOnDrop)]
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
    pub block_time_millis: u64,
    pub gas_price_multiplier: f64,
}

impl LiveMiningClient {
    pub async fn connect(config: &MinerConfig) -> Result<Option<Self>> {
        if !config.has_live_target() {
            return Ok(None);
        }

        let provider = if let Some(ref api_key) = config.rpc_api_key {
            let mut headers = header::HeaderMap::new();
            headers.insert(
                "x-api-key",
                header::HeaderValue::from_str(api_key)
                    .context("invalid rpc_api_key value")?,
            );
            let client = reqwest::Client::builder()
                .default_headers(headers)
                .build()
                .context("failed to build HTTP client with API key")?;
            let http = Http::new_with_client(
                url::Url::parse(config.rpc_url.as_str())
                    .context("invalid rpc_url")?,
                client,
            );
            Provider::new(http).interval(Duration::from_millis(750))
        } else {
            Provider::<Http>::try_from(config.rpc_url.as_str())?
                .interval(Duration::from_millis(750))
        };
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
            block_time_millis: config.block_time_millis.max(1_000),
            gas_price_multiplier: config.gas_price_multiplier,
        }))
    }

    pub fn miner_address(&self) -> Address {
        self.client.address()
    }

    async fn current_contract_block_number(&self) -> Result<u64> {
        let latest: Value = retry_with_backoff("current_contract_block_number", 5, || {
            let provider = Arc::clone(&self.provider);
            async move {
                provider
                    .request("eth_getBlockByNumber", ("latest", false))
                    .await
                    .context("Failed to fetch latest block payload")
            }
        })
        .await?;

        if let Some(l1_block_hex) = latest.get("l1BlockNumber").and_then(Value::as_str) {
            return u64::from_str_radix(l1_block_hex.trim_start_matches("0x"), 16)
                .with_context(|| format!("Invalid l1BlockNumber value {l1_block_hex}"));
        }

        Ok(self.provider.get_block_number().await?.as_u64())
    }

    pub fn signer_clone(&self) -> LocalWallet {
        self.client.signer().clone()
    }

    pub async fn current_challenge(&self) -> Result<LiveChallenge> {
        let contract_addr = self.contract_address;
        let provider = Arc::clone(&self.provider);
        let pool_id = self.pool_id;
        retry_with_backoff("current_challenge", 5, || {
            let provider = Arc::clone(&provider);
            async move {
                let contract = MiningContract::new(contract_addr, provider.clone());
                let (outputs, difficulty) = contract.get_mining_challenge(pool_id).call().await?;
                let previous_output = *outputs
                    .first()
                    .ok_or_else(|| anyhow!("No outputs returned from getMiningChallenge"))?;
                let block = provider
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
        }).await
    }

    pub async fn next_commit_nonce(&self) -> Result<u64> {
        let contract_addr = self.contract_address;
        let provider = Arc::clone(&self.provider);
        let miner = self.miner_address();
        retry_with_backoff("next_commit_nonce", 5, || {
            let provider = Arc::clone(&provider);
            async move {
                let contract = MiningContract::new(contract_addr, provider);
                let nonce = contract.nonces(miner).call().await?;
                Ok(nonce.as_u64())
            }
        }).await
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
        let _ = self.wait_for_commitment_clearance(phase).await?;

        // ── Verify pool has remaining emission before wasting gas ──
        let (remaining_emission, _, pool_active) = self.check_pool_emission().await?;
        if !pool_active {
            return Err(anyhow!("Pool {} is not active — aborting commit", self.pool_id));
        }
        if remaining_emission <= 0.0 {
            return Err(anyhow!(
                "Pool {} has ZERO remaining emission — aborting commit to avoid wasting gas",
                self.pool_id
            ));
        }
        tracing::info!("Pool {} has {remaining_emission:.2} TGBT remaining — proceeding with commit", self.pool_id);

        // ── Wait for mining cooldown (minBlockInterval) ──────────────
        self.wait_for_mining_cooldown(&contract, phase).await?;

        // ── Save pending commitment to disk BEFORE sending the tx ──
        let mut pending = crate::pending::PendingCommitment {
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

        // ── Final re-check for active commitment right before sending ──
        // Guards against race between clearance check and commit TX broadcast.
        if let Some(c) = self.get_onchain_commitment().await? {
            if !c.expired {
                tracing::warn!(
                    "Active commitment appeared after clearance check (pool {}, block {}) — aborting to avoid revert",
                    c.pool_id, c.commit_block
                );
                crate::pending::clear(key_path)?;
                return Err(anyhow!(
                    "ActiveCommitmentExists race detected — previous commitment still on-chain, will retry next cycle"
                ));
            }
        }

        let commitment_signature = self.sign_commitment(&submission.commitment).await?;

        let commit_tx = contract.submit_mining_commitment(
            submission.commitment.commit_hash.into(),
            submission.commitment.pool_id,
            U256::from(submission.commitment.nonce),
            U256::from(submission.commitment.deadline),
            commitment_signature,
        );
        let commit_gas = match commit_tx.estimate_gas().await {
            Ok(gas) => gas,
            Err(err) => {
                // Gas estimation failure means the TX would revert — clean up pending
                tracing::warn!("Commitment gas estimation failed (likely contract revert): {err:#}");
                crate::pending::clear(key_path)?;
                return Err(anyhow!("Failed to estimate commitment gas: {err:#}"));
            }
        };
        let commit_tx = commit_tx
            .legacy()
            .gas(apply_gas_buffer(commit_gas))
            .gas_price(self.legacy_gas_price().await?);
        let pending_commit = match commit_tx.send().await {
            Ok(pending_tx) => pending_tx,
            Err(err) => {
                // TX failed to send — clean up pending since nothing reached the chain
                tracing::warn!("Commitment TX failed to send: {err:#}");
                crate::pending::clear(key_path)?;
                return Err(anyhow!("Failed to send commitment: {err:#}"));
            }
        };
        let _commit_receipt = pending_commit
            .await?
            .ok_or_else(|| anyhow!("No commitment receipt"))?;

        // Update pending file with the actual commit block
        let commit_block = self.current_contract_block_number().await?;
        pending.commit_block = commit_block;
        crate::pending::save(key_path, &pending)?;

        let min_blocks = contract.min_commitment_age().call().await?.as_u64();
        let target_block = commit_block.saturating_add(min_blocks);
        phase.set(MiningPhase::CommitmentLocked, Some(min_blocks));

        loop {
            let current_block = self.current_contract_block_number().await?;
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
        let current_block = self.current_contract_block_number().await?;
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
                let now = self.current_contract_block_number().await?;
                if now >= reveal_at {
                    break;
                }
                let remaining = reveal_at.saturating_sub(now);
                phase.set(MiningPhase::CommitmentLocked, Some(remaining));
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }

        phase.set(MiningPhase::Revealing, None);
        let secure_reveal_signature = pending.secure_reveal_signature()?;
        let secure_secret_value = pending.secure_secret_value()?;
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
            reveal_signature: secure_reveal_signature
                .as_slice()
                .ok_or_else(|| anyhow!("Secure pending reveal signature unavailable"))?
                .to_vec(),
            secret_value: secure_secret_value
                .to_array::<32>()
                .map_err(|err| anyhow!("Failed to read pending secret value from secure memory: {err}"))?,
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
        let secure_reveal_signature = SecureBuffer::from_slice(&submission.reveal_signature)
            .map_err(|err| anyhow!("Failed to protect reveal signature in memory: {err}"))?;
        let secure_secret_value = SecureBuffer::from_slice(&submission.secret_value)
            .map_err(|err| anyhow!("Failed to protect reveal secret in memory: {err}"))?;
        let reveal_tx = contract.reveal_mining_commitment(
            submission.previous_output.into(),
            Bytes::from(submission.temporal_seed.to_vec()),
            submission.nonce,
            Bytes::from(
                secure_reveal_signature
                    .as_slice()
                    .ok_or_else(|| anyhow!("Secure reveal signature unavailable"))?
                    .to_vec(),
            ),
            secure_secret_value
                .to_array::<32>()
                .map_err(|err| anyhow!("Failed to read reveal secret from secure memory: {err}"))?
                .into(),
            submission.commitment.pool_id,
        );
        let reveal_gas = reveal_tx
            .estimate_gas()
            .await
            .context("Failed to estimate reveal gas")?;
        let reveal_tx = reveal_tx
            .legacy()
            .gas(apply_gas_buffer(reveal_gas))
            .gas_price(self.legacy_gas_price().await?);
        let pending_reveal = reveal_tx.send().await.context("Failed to send reveal")?;
        pending_reveal
            .await?
            .ok_or_else(|| anyhow!("No reveal receipt"))
    }

    /// Read the full on-chain commitment for this miner, if any active one exists.
    /// Returns `Ok(None)` when no commitment, already revealed, or zero hash.
    /// Returns the commitment info including pool_id and whether it's expired.
    pub async fn get_onchain_commitment(&self) -> Result<Option<OnChainCommitment>> {
        let selector = keccak256("minerCommitments(address)");
        let mut full_calldata = selector[..4].to_vec();
        full_calldata.extend_from_slice(&encode(&[Token::Address(self.miner_address())]));

        let tx = ethers::types::TransactionRequest::new()
            .to(self.contract_address)
            .data(Bytes::from(full_calldata));
        let result = self.provider.call(&tx.into(), None).await?;

        // ABI-flattened layout for MiningLib.Commitment:
        //   word 0 (0..32):    commitHash (bytes32)
        //   word 1 (32..64):   timestamp  (uint64 → uint256) — commit block
        //   word 2 (64..96):   flags.revealed (bool)
        //   word 3 (96..128):  flags.validated (bool)
        //   word 4 (128..160): flags.revoked (bool)
        //   word 5 (160..192): flags.emergency (bool)
        //   word 6 (192..224): revealedValue (bytes32)
        //   word 7 (224..256): poolId (uint8 → uint256)
        if result.len() < 256 {
            return Ok(None);
        }
        let commit_hash = &result[0..32];
        if commit_hash.iter().all(|&b| b == 0) {
            return Ok(None); // no commitment
        }

        let commit_block = U256::from_big_endian(&result[32..64]).as_u64();
        let revealed = U256::from_big_endian(&result[64..96]) != U256::zero();

        if revealed {
            return Ok(None); // already revealed
        }

        let pool_id = U256::from_big_endian(&result[224..256]).as_u64() as u8;

        let contract = MiningContract::new(self.contract_address, Arc::clone(&self.client));
        let max_age = contract.max_commitment_age().call().await?.as_u64();
        let expires_at = commit_block.saturating_add(max_age);
        let current_block = self.current_contract_block_number().await?;
        let expired = current_block > expires_at;

        Ok(Some(OnChainCommitment {
            commit_block,
            revealed,
            pool_id,
            expires_at,
            expired,
        }))
    }

    /// Returns `true` if there is an unrevealed, unexpired commitment on chain.
    pub async fn has_pending_commitment(&self) -> Result<bool> {
        match self.get_onchain_commitment().await? {
            Some(c) => Ok(!c.expired),
            None => Ok(false),
        }
    }

    /// Check if the configured pool has remaining TGBT emission.
    /// Returns `(remaining, total_mined, active)`.
    pub async fn check_pool_emission(&self) -> Result<(f64, f64, bool)> {
        let contract = MiningContract::new(self.contract_address, Arc::clone(&self.client));
        let (_, emission, mined, active) = contract.get_pool_info(self.pool_id).call().await?;
        let remaining = ethers::utils::format_units(emission, 18)
            .unwrap_or_default()
            .parse::<f64>()
            .unwrap_or(0.0);
        let total_mined = ethers::utils::format_units(mined, 18)
            .unwrap_or_default()
            .parse::<f64>()
            .unwrap_or(0.0);
        Ok((remaining, total_mined, active))
    }

    /// Checks if there is an existing unrevealed commitment for the miner.
    /// If one exists, waits until it expires (block > timestamp + maxCommitmentAge).
    /// Returns the on-chain commitment info if one was found (even if expired).
    pub async fn wait_for_commitment_clearance_public(&self, phase: &PhaseTracker) -> Result<Option<OnChainCommitment>> {
        self.wait_for_commitment_clearance(phase).await
    }

    /// Internal: checks and waits for commitment clearance.
    /// Returns `Ok(Some(info))` with the commitment that was cleared/found,
    /// or `Ok(None)` if no active commitment exists.
    async fn wait_for_commitment_clearance(
        &self,
        phase: &PhaseTracker,
    ) -> Result<Option<OnChainCommitment>> {
        let commitment = match self.get_onchain_commitment().await? {
            Some(c) => c,
            None => return Ok(None), // no commitment at all
        };

        // Already expired — return immediately, no waiting
        if commitment.expired {
            tracing::info!(
                "On-chain commitment (pool {}, block {}) already expired — proceeding immediately",
                commitment.pool_id, commitment.commit_block
            );
            return Ok(Some(commitment));
        }

        let pool_label = if commitment.pool_id != self.pool_id {
            format!(" [WRONG POOL: on-chain={}, configured={}]", commitment.pool_id, self.pool_id)
        } else {
            String::new()
        };

        let current_block = self.current_contract_block_number().await?;
        let total_remaining = commitment.expires_at.saturating_sub(current_block);
        let eta_secs = self.eta_seconds_for_blocks(total_remaining);

        tracing::warn!(
            "Active unrevealed commitment found (pool {}, block {}){pool_label} — \
             must wait {total_remaining} blocks (~{eta_secs}s) for expiry at block {}",
            commitment.pool_id, commitment.commit_block, commitment.expires_at
        );

        // Use a longer sleep interval since L1 blocks are ~12s on Arbitrum
        let sleep_duration = Duration::from_secs(12).max(self.block_sleep_duration());
        let mut last_log_block = 0u64;

        loop {
            let current_block = self.current_contract_block_number().await?;
            if current_block > commitment.expires_at {
                tracing::info!("Stale commitment expired at block {current_block} — proceeding");
                return Ok(Some(commitment));
            }
            let remaining = commitment.expires_at.saturating_sub(current_block);
            phase.set(MiningPhase::WaitingForClearance, Some(remaining));

            // Only log every ~10 blocks to avoid log spam
            if last_log_block == 0 || current_block.saturating_sub(last_log_block) >= 10 {
                tracing::info!(
                    "Waiting for commitment expiry: {remaining} blocks remaining (~{}s)",
                    self.eta_seconds_for_blocks(remaining)
                );
                last_log_block = current_block;
            }
            tokio::time::sleep(sleep_duration).await;
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
            let current_block = self.current_contract_block_number().await?;
            if current_block >= ready_at {
                return Ok(());
            }
            let remaining = ready_at.saturating_sub(current_block);
            phase.set(MiningPhase::WaitingForClearance, Some(remaining));
            tracing::info!(
                "Mining cooldown: {remaining} blocks until minBlockInterval clears (~{}s)",
                self.eta_seconds_for_blocks(remaining)
            );
            tokio::time::sleep(self.block_sleep_duration()).await;
        }
    }

    fn eta_seconds_for_blocks(&self, blocks: u64) -> u64 {
        let millis = blocks.saturating_mul(self.block_time_millis);
        millis.saturating_add(999).checked_div(1_000).unwrap_or(0)
    }

    async fn legacy_gas_price(&self) -> Result<U256> {
        let network_gas_price = self
            .provider
            .get_gas_price()
            .await
            .context("Failed to fetch gas price")?;
        Ok(apply_gas_price_multiplier(
            network_gas_price,
            self.gas_price_multiplier,
        ))
    }

    fn block_sleep_duration(&self) -> Duration {
        Duration::from_millis(self.block_time_millis.max(1_000))
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

fn apply_gas_price_multiplier(gas_price: U256, multiplier: f64) -> U256 {
    let bounded = if multiplier.is_finite() && multiplier > 0.0 {
        multiplier
    } else {
        1.0
    };

    let scaled = (bounded * 1000.0).round() as u64;
    gas_price
        .saturating_mul(U256::from(scaled))
        .checked_div(U256::from(1000u64))
        .unwrap_or(gas_price)
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

    let mut key_bytes = if key_path.exists() {
        let raw = fs::read_to_string(key_path)
            .with_context(|| format!("Failed to read {}", key_path.display()))?;
        hex::decode(raw.trim()).context("Private key must be hex-encoded")?
    } else {
        let mut generated = [0u8; 32];
        OsRng.fill_bytes(&mut generated);
        fs::write(key_path, hex::encode(generated))
            .with_context(|| format!("Failed to write {}", key_path.display()))?;
        let bytes = generated.to_vec();
        generated.zeroize();
        bytes
    };

    if key_bytes.len() != 32 {
        key_bytes.zeroize();
        return Err(anyhow!("Expected a 32-byte private key"));
    }

    let secure_key = SecureBuffer::from_slice(&key_bytes)
        .map_err(|err| anyhow!("Failed to protect private key in memory: {err}"))?;
    key_bytes.zeroize();
    let wallet = LocalWallet::from_bytes(
        secure_key
            .as_slice()
            .ok_or_else(|| anyhow!("Secure private key buffer unavailable"))?,
    )
    .context("Invalid secp256k1 private key")?;
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

