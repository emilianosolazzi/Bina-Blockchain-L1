use sha2::{Digest, Sha256, Sha512};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PqcMode {
    ClassicalCompatible,
    Enhanced,
}

impl PqcMode {
    pub fn parse(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "classical" | "compatible" | "classical-compatible" => Self::ClassicalCompatible,
            _ => Self::Enhanced,
        }
    }
}

pub fn apply_pqc_enhancement(data: &[u8], mode: PqcMode) -> [u8; 32] {
    match mode {
        PqcMode::ClassicalCompatible => Sha256::digest(data).into(),
        PqcMode::Enhanced => {
            let mut current = Sha512::digest(data).to_vec();
            for _ in 0..3 {
                current = Sha256::digest(&current).to_vec();
            }

            let mut out = [0u8; 32];
            out.copy_from_slice(&current[..32]);
            out
        }
    }
}
