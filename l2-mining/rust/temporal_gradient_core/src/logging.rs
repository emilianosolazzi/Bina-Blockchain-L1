use anyhow::{anyhow, Result};
use tracing_subscriber::EnvFilter;

pub fn setup_logging(level: &str) -> Result<()> {
    let filter = EnvFilter::try_new(level.to_ascii_lowercase()).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_line_number(true)
        .try_init()
        .map_err(|err| anyhow!("Failed to initialize tracing subscriber: {err}"))
}
