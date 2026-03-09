use crate::config::MinerConfig;
use crate::crypto::DynamicMiningCommitment;
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
        function getMiningChallenge(uint8 poolId) external view returns (bytes32[] outputs, uint256 difficulty)
        function nonces(address miner) external view returns (uint256)
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

    pub async fn submit_solution(&self, submission: &LiveSubmission) -> Result<TransactionReceipt> {
        let contract = MiningContract::new(self.contract_address, Arc::clone(&self.client));
        let commitment_signature = self.sign_commitment(&submission.commitment).await?;

        let commit_tx = contract.submit_mining_commitment(
            submission.commitment.commit_hash.into(),
            submission.commitment.pool_id,
            U256::from(submission.commitment.nonce),
            U256::from(submission.commitment.deadline),
            commitment_signature,
        );
        let pending_commit = commit_tx.send().await.context("Failed to send commitment")?;
        let commit_receipt = pending_commit
            .await?
            .ok_or_else(|| anyhow!("No commitment receipt"))?;

        let min_blocks = contract.min_commitment_age().call().await?.as_u64();
        let commit_block = commit_receipt
            .block_number
            .ok_or_else(|| anyhow!("Missing commitment block number"))?
            .as_u64();
        loop {
            let current_block = self.provider.get_block_number().await?.as_u64();
            if current_block >= commit_block.saturating_add(min_blocks) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }

        let reveal_tx = contract.reveal_mining_commitment(
            submission.previous_output.into(),
            Bytes::from(submission.temporal_seed.to_vec()),
            submission.nonce,
            Bytes::from(submission.reveal_signature.clone()),
            submission.secret_value.into(),
            submission.commitment.pool_id,
        );
        let pending_reveal = reveal_tx.send().await.context("Failed to send reveal")?;
        pending_reveal
            .await?
            .ok_or_else(|| anyhow!("No reveal receipt"))
    }

    pub fn extract_reward_from_receipt(receipt: &TransactionReceipt) -> Option<f64> {
        let event_signature_hash = H256::from(keccak256("BeaconBlockMined(address,bytes32,uint256,uint64,uint64,uint8)"));
        for log in &receipt.logs {
            if log.topics.first() == Some(&event_signature_hash) && log.data.0.len() >= 32 {
                let reward_u256 = U256::from_big_endian(&log.data.0[0..32]);
                return ethers::utils::format_units(reward_u256, 18).ok().and_then(|value| value.parse().ok());
            }
        }
        None
    }
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

