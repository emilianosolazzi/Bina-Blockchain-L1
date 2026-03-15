pub mod chain;
pub mod config;
pub mod crypto;
pub mod logging;
pub mod paths;
pub mod pending;
pub mod pqc;
pub mod runtime;
pub mod seed;
pub mod telemetry;

pub use chain::{wallet_address_from_config, LiveChallenge, LiveMiningClient, LiveSubmission};
pub use config::{load_or_create_config, default_config_json, MinerConfig};
pub use crypto::{
    build_commitment_payload, contract_hash_message, create_entropy_hash, has_leading_zero_bits,
    miner_address_from_signing_key, CommitmentPayload, DynamicMiningCommitment, MiningMaterial,
};
pub use logging::setup_logging;
pub use paths::{app_paths, ensure_app_layout, AppPaths};
pub use pqc::{apply_pqc_enhancement, PqcMode};
pub use runtime::{spawn_miner, MinerHandle};
pub use seed::{decode_temporal_seed_timestamp, encode_temporal_seed, generate_temporal_seed};
pub use telemetry::{MinerState, MiningPhase, PhaseTracker, TelemetrySnapshot};
