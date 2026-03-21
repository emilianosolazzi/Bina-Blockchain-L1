pub mod bitcoin_dead_utxo_anchor;
pub mod chain;
pub mod config;
pub mod cpu;
pub mod crypto;
pub mod logging;
pub mod memory;
pub mod paths;
pub mod pending;
pub mod runtime;
pub mod seed;
pub mod storage_verification;
pub mod telemetry;
pub mod tg_output_filter;
pub mod tg_relay_transport;
pub mod utxo_fetcher;

pub use bitcoin_dead_utxo_anchor::{DeadUTXOAnchor, DeadUTXOAnchorDB, DeadUTXOType};
pub use chain::{wallet_address_from_config, LiveChallenge, LiveMiningClient, LiveSubmission};
pub use config::{load_or_create_config, default_config_json, MinerConfig};
pub use cpu::{detect_cpu_safely, get_cpu_temperature, has_cpu_feature, mask_cpu_identity, CpuFeature, CpuIdentity, MaskingConfig};
pub use crypto::{
    build_commitment_payload, contract_hash_message, create_entropy_hash, has_leading_zero_bits,
    miner_address_from_signing_key, CommitmentPayload, DynamicMiningCommitment, MiningMaterial,
};
pub use logging::setup_logging;
pub use memory::{SecureBuffer, SecureBufferError};
pub use paths::{app_paths, ensure_app_layout, AppPaths};
pub use runtime::{spawn_miner, MinerHandle};
pub use seed::{decode_temporal_seed_timestamp, encode_temporal_seed, generate_temporal_seed};
pub use storage_verification::{
    AttestationStatus, ChallengeType, EntropyStorageVerifier, ProviderReputation,
    SettlementGateDecision, StorageAttestation, StorageChallenge, StorageProof, StorageProtocol,
    VerificationResult, VerificationStats,
};
pub use telemetry::{MinerState, MiningPhase, PhaseTracker, TelemetrySnapshot};
pub use tg_output_filter::{FilterConfig, FilterError, OutputRecord, Ready, TgOutputFilter, Uninitialized};
pub use tg_relay_transport::{
    relay_connect, relay_connect_pinned, ReliableRelayChannel, RelayChannel, SecureTransport,
    TransportConfig, TransportError, TransportStats, TransportStatsSnapshot,
};
pub use utxo_fetcher::{
    AnchorSelectionPreview, UTXOAnchorPreference, UTXOFetcher, UTXOInfo, UTXOQuery,
    UTXOSearchQuery,
};
