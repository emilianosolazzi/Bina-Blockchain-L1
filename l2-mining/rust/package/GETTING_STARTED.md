# Getting Started

## 1. Build the package

From [l2-mining/rust/package](l2-mining/rust/package):

- `cargo build --release --bins`

## 2. Install locally

- `powershell -ExecutionPolicy Bypass -File .\install.ps1`

Or directly from Cargo:

- `cargo run -- install`

This installs:

- `tg-miner-installer.exe`
- `temporal-gradient-miner.exe`

## 3. Initialize config

- `tg-miner-installer.exe init`

Default config path:

- `%APPDATA%\entropy\TemporalGradientMiner\config\miner-config.json`

## 4. Review config

Important fields:

- `threads`
- `batch_size`
- `difficulty_zero_bits`
- `rpc_url`
- `contract_address`
- `telemetry_file`
- `pqc_mode`

## 5. Launch the miner

Foreground:

- `tg-miner-installer.exe launch --foreground`

Direct binary:

- `temporal-gradient-miner.exe --config <path-to-config>`

Direct Cargo run:

- `cargo run --bin temporal-gradient-miner -- --config <path-to-config>`
- `cargo run --bin temporal-gradient-miner -- --quiet --exit-after-solutions 1`

## 6. Watch telemetry

The miner writes JSONL telemetry snapshots to the configured telemetry file.

Typical fields include:

- `hashes`
- `hashrate_hs`
- `solutions`
- `accepted_submissions`
- `last_solution_hash_hex`
- `temperature_c`

## Current scope

This runtime is the stabilized executable foundation for the miner package:

- config loading
- graceful shutdown
- telemetry streaming
- commitment/reveal helper logic in the core crate
- live challenge polling and live commit/reveal submission
- receipt reward parsing
- PQC-enhanced solution hashing hooks
