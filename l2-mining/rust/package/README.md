# Temporal Gradient Miner Bootstrap Package

This package is the first step toward a user-friendly Windows distribution for the L2 miner.

## What it does today

- builds a small bootstrap binary: `tg-miner-installer.exe`
- builds the main miner runtime: `temporal-gradient-miner.exe`
- creates per-user config/data/log directories
- writes a default miner config template
- runs a basic install health check
- installs both binaries into the expected per-user bin directory
- launches the miner as a child process
- produces a portable zip bundle for distribution

## Commands

- `cargo run -- install`
- `cargo run -- init`
- `cargo run -- paths`
- `cargo run -- doctor`
- `cargo run -- launch --foreground`
- `cargo run -- write-config --output .\miner-config.json`

## Install locally

1. Build and install:
   - `powershell -ExecutionPolicy Bypass -File .\install.ps1`
2. Check the generated paths:
   - `tg-miner-installer.exe paths`
3. Launch the miner in the foreground:
   - `tg-miner-installer.exe launch --foreground`

For a fast local smoke test:

- `cargo run --bin temporal-gradient-miner -- --quiet --exit-after-solutions 1`

## Create a portable bundle

- `powershell -ExecutionPolicy Bypass -File .\build-portable.ps1`

## Next packaging steps

- bundle signed updates and a proper release manifest
- add MSI/Inno Setup packaging
- optionally add auto-start and service mode for managed mining fleets