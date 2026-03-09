use anyhow::{anyhow, Result};
use std::time::{SystemTime, UNIX_EPOCH};

pub const TEMPORAL_SEED_LENGTH: usize = 8;
pub const TEMPORAL_SEED_BOM: u8 = 0x00;
const MAX_TEMPORAL_SEED_TIMESTAMP: u64 = (1u64 << 56) - 1;

pub fn encode_temporal_seed(timestamp_secs: u64) -> Result<[u8; TEMPORAL_SEED_LENGTH]> {
    if timestamp_secs > MAX_TEMPORAL_SEED_TIMESTAMP {
        return Err(anyhow!("timestamp exceeds 56-bit temporal seed capacity"));
    }

    let mut seed = [0u8; TEMPORAL_SEED_LENGTH];
    seed[0] = TEMPORAL_SEED_BOM;
    seed[1..].copy_from_slice(&timestamp_secs.to_be_bytes()[1..]);
    Ok(seed)
}

pub fn decode_temporal_seed_timestamp(temporal_seed: &[u8]) -> Result<u64> {
    if temporal_seed.len() != TEMPORAL_SEED_LENGTH {
        return Err(anyhow!("invalid temporal seed length"));
    }
    if temporal_seed[0] != TEMPORAL_SEED_BOM {
        return Err(anyhow!("invalid temporal seed BOM marker"));
    }

    let mut timestamp_bytes = [0u8; 8];
    timestamp_bytes[1..].copy_from_slice(&temporal_seed[1..]);
    Ok(u64::from_be_bytes(timestamp_bytes))
}

pub fn generate_temporal_seed() -> Result<[u8; TEMPORAL_SEED_LENGTH]> {
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    encode_temporal_seed(now_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_timestamp() {
        let ts = 1_746_385_920u64;
        let seed = encode_temporal_seed(ts).unwrap();
        assert_eq!(decode_temporal_seed_timestamp(&seed).unwrap(), ts);
    }
}
