use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use temporal_gradient_core::{app_paths, default_config_json, ensure_app_layout, load_or_create_config, AppPaths};

#[derive(Debug, Parser)]
#[command(name = "tg-miner-installer")]
#[command(about = "Bootstrap installer and config helper for the Temporal Gradient miner")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Install the bootstrap and miner binaries into the per-user bin directory.
    Install,
    /// Initialize the per-user config and data folders.
    Init,
    /// Print the important install paths.
    Paths,
    /// Run a simple install health check.
    Doctor,
    /// Launch the miner executable as a child process.
    Launch {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        foreground: bool,
    },
    /// Write a fresh config template to stdout or a file.
    WriteConfig {
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Install => install_binaries(),
        Commands::Init => init_installation(),
        Commands::Paths => print_paths(),
        Commands::Doctor => run_doctor(),
        Commands::Launch { config, foreground } => launch_miner(config.as_deref(), foreground),
        Commands::WriteConfig { output } => write_config(output.as_deref()),
    }
}

fn write_config(path: Option<&Path>) -> Result<()> {
    let json = default_config_json()?;

    if let Some(path) = path {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        fs::write(path, json).with_context(|| format!("Failed to write {}", path.display()))?;
        println!("Wrote config template to {}", path.display());
    } else {
        println!("{}", json);
    }

    Ok(())
}

fn init_installation() -> Result<()> {
    let paths = ensure_app_layout()?;
    let (config, config_path) = load_or_create_config(Some(&paths.config_file))?;

    println!("Initialized Temporal Gradient miner folders:");
    println!("  Config: {}", paths.config_dir.display());
    println!("  Data:   {}", paths.data_dir.display());
    println!("  Logs:   {}", paths.log_dir.display());
    println!("  Keys:   {}", paths.key_dir.display());
    println!("  Config file: {}", config_path.display());
    println!("  Threads: {}", config.threads);
    Ok(())
}

fn install_binaries() -> Result<()> {
    let paths = ensure_app_layout()?;
    let current = std::env::current_exe().context("Failed to determine current executable")?;
    let current_name = current
        .file_name()
        .and_then(|name| name.to_str())
        .context("Failed to determine installer executable name")?;

    let sibling_dir = current
        .parent()
        .context("Failed to determine installer executable directory")?;
    let miner_name = format!("temporal-gradient-miner{}", std::env::consts::EXE_SUFFIX);
    let sibling_miner = sibling_dir.join(&miner_name);
    if !sibling_miner.exists() {
        anyhow::bail!(
            "Expected miner binary next to installer at {}",
            sibling_miner.display()
        );
    }

    let installed_installer = paths.bin_dir.join(current_name);
    let installed_miner = paths.bin_dir.join(&miner_name);

    fs::copy(&current, &installed_installer)
        .with_context(|| format!("Failed to copy installer to {}", installed_installer.display()))?;
    fs::copy(&sibling_miner, &installed_miner)
        .with_context(|| format!("Failed to copy miner to {}", installed_miner.display()))?;

    println!("Installed bootstrap binary: {}", installed_installer.display());
    println!("Installed miner binary:     {}", installed_miner.display());

    let (_, config_path) = load_or_create_config(Some(&paths.config_file))?;
    println!("Config file ready at:       {}", config_path.display());
    Ok(())
}

fn print_paths() -> Result<()> {
    let paths = app_paths()?;
    println!("Config directory: {}", paths.config_dir.display());
    println!("Config file:      {}", paths.config_file.display());
    println!("Data directory:   {}", paths.data_dir.display());
    println!("Key directory:    {}", paths.key_dir.display());
    println!("Logs directory:   {}", paths.log_dir.display());
    println!("Binary directory: {}", paths.bin_dir.display());
    Ok(())
}

fn run_doctor() -> Result<()> {
    let paths = ensure_app_layout()?;
    let config_path = paths.config_file.clone();
    let miner_path = installed_miner_path(&paths);
    let resolved = resolve_miner_path(&paths).ok();

    println!("Temporal Gradient miner health check");
    println!("- Config dir exists: {}", paths.config_dir.exists());
    println!("- Data dir exists:   {}", paths.data_dir.exists());
    println!("- Logs dir exists:   {}", paths.log_dir.exists());
    println!("- Config file exists:{}", config_path.exists());
    println!("- Miner binary:      {}", miner_path.display());
    println!("- Miner present:     {}", miner_path.exists());
    if let Some(resolved) = resolved {
        println!("- Resolved launch:   {}", resolved.display());
    }

    if config_path.exists() {
        let (parsed, _) = load_or_create_config(Some(&config_path))?;
        println!("- Contract address:  {}", parsed.contract_address);
        println!("- RPC URL:           {}", parsed.rpc_url);
        println!("- Threads:           {}", parsed.threads);
        println!("- Log level:         {}", parsed.log_level);
    } else {
        println!("- Hint: run `tg-miner-installer init` first");
    }

    Ok(())
}

fn launch_miner(config: Option<&Path>, foreground: bool) -> Result<()> {
    let paths = ensure_app_layout()?;
    let (_, config_path) = load_or_create_config(config.or(Some(paths.config_file.as_path())))?;
    let miner_path = resolve_miner_path(&paths)?;

    let mut command = Command::new(&miner_path);
    command.arg("--config").arg(&config_path);

    if foreground {
        let status = command.status().with_context(|| format!("Failed to launch {}", miner_path.display()))?;
        if !status.success() {
            anyhow::bail!("Miner exited with status {status}");
        }
    } else {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("Failed to launch {}", miner_path.display()))?;
        println!("Launched miner in background using {}", config_path.display());
    }

    Ok(())
}

fn resolve_miner_path(paths: &AppPaths) -> Result<PathBuf> {
    let installed = installed_miner_path(paths);
    if installed.exists() {
        return Ok(installed);
    }

    let current = std::env::current_exe().context("Failed to determine current executable")?;
    let sibling = current
        .parent()
        .map(|parent| parent.join(format!("temporal-gradient-miner{}", std::env::consts::EXE_SUFFIX)))
        .context("Failed to determine miner sibling path")?;

    if sibling.exists() {
        return Ok(sibling);
    }

    anyhow::bail!("temporal-gradient-miner binary not found in installed or sibling paths")
}

fn installed_miner_path(paths: &AppPaths) -> PathBuf {
    paths.bin_dir.join(format!("temporal-gradient-miner{}", std::env::consts::EXE_SUFFIX))
}