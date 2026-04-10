mod keygen;
mod paths;
mod server;
mod heartbeat;
mod cpu_info;
mod entropy_scorer;

use anyhow::{Context, Result};
use clap::Parser;
use paths::SelfMinerPaths;
use server::AppState;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use temporal_gradient_core::{
    load_or_create_config, spawn_miner,
    MinerConfig, TelemetrySnapshot,
};
use tokio::signal;
use tokio::sync::RwLock;

/// Default RPC URL (Arbitrum One public endpoint).
const DEFAULT_RPC_URL: &str = "https://arb1.arbitrum.io/rpc";
/// Production core contract on Arbitrum One.
const DEFAULT_CONTRACT: &str = "0xF6556DDC7CdD3635A05428BD85BCf33A09F752e6";
/// Default pool ID (community pool).
const DEFAULT_POOL_ID: u8 = 1;
/// Known developer API key that must NEVER ship in user configs.
const DEV_API_KEY_PREFIX: &str = "fp_2d93df5e";

#[derive(Debug, Parser)]
#[command(name = "tg-self-miner")]
#[command(about = "Temporal Gradient — Self-contained PoW miner with embedded dashboard")]
struct Cli {
    /// Path to miner-config.json. Auto-created on first run if absent.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Dashboard HTTP port (default: 5000).
    #[arg(long, default_value_t = 5000)]
    port: u16,

    /// Don't open a browser on startup.
    #[arg(long, default_value_t = false)]
    no_browser: bool,

    /// Stop after N solutions (for testing).
    #[arg(long)]
    exit_after_solutions: Option<u64>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let version = env!("CARGO_PKG_VERSION");

    // ── Console setup (UTF-8 + window title) ────────────────────────
    #[cfg(windows)]
    {
        init_windows_console();
        set_console_title(&format!("Temporal Gradient Miner v{version}"));
    }

    // ── Header ──────────────────────────────────────────────────────
    eprintln!();
    eprintln!("  \x1b[36m╔═══════════════════════════════════════════════════════╗\x1b[0m");
    eprintln!("  \x1b[36m║\x1b[0m  Temporal Gradient \x1b[1m— PoW Self Miner\x1b[0m         \x1b[90mv{:<7}\x1b[0m\x1b[36m║\x1b[0m", version);
    eprintln!("  \x1b[36m╚═══════════════════════════════════════════════════════╝\x1b[0m");
    eprintln!();

    // ── 1. Ensure self-miner AppData directory layout ───────────────
    let sm_paths = paths::ensure_self_miner_layout()?;
    step("Directories ready");

    // ── 2. Set up file-based logging (keeps console clean) ──────────
    let log_file_path = sm_paths.log_dir.join("self-miner.log");
    {
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file_path)
            .with_context(|| format!("Cannot open log file {}", log_file_path.display()))?;
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
            .with_target(false)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(log_file))
            .init();
    }
    step("Logging to file");

    // ── CPU detection (standalone, not from core) ──────────────────
    let cpu = cpu_info::detect_cpu();
    step(&format!(
        "CPU: {} ({} cores / {} threads)",
        cpu.display_name(),
        cpu.cores,
        cpu.threads,
    ));

    // ── 3. Load or create config ─────────────────────────────────────
    let config_target = cli.config.as_deref().unwrap_or(&sm_paths.config_file);
    let (mut config, config_path) =
        load_or_create_config(Some(config_target))?;

    config.telemetry_file =
        Some(sm_paths.telemetry_file.to_string_lossy().to_string());
    if config.private_key_path == "keys/miner.key"
        || config.private_key_path.contains("TemporalGradientMiner")
    {
        config.private_key_path =
            sm_paths.key_dir.join("miner.key").to_string_lossy().to_string();
    }

    apply_self_miner_defaults(&mut config, &sm_paths);
    sanitize_dev_values(&mut config);

    if let Some(limit) = cli.exit_after_solutions {
        config.exit_after_solutions = Some(limit);
    }

    config.save_to_path(&config_path)?;
    step("Config loaded");

    // Remove stale trust-seal (binary fingerprint changes on rebuild)
    let trust_seal = sm_paths.log_dir.join("miner-trust-seal.json");
    if trust_seal.exists() {
        let _ = std::fs::remove_file(&trust_seal);
    }

    // ── 4. Auto-generate private key if missing ───────────────────────
    let key_path = resolve_key_path(&config, &config_path);
    let is_first_run = !key_path.exists();
    if is_first_run {
        let (_key_hex, address) = keygen::generate_key(&key_path)
            .with_context(|| format!("Failed to generate key at {}", key_path.display()))?;
        config.private_key_path = key_path.to_string_lossy().to_string();
        config.save_to_path(&config_path)?;
        eprintln!();
        eprintln!("  \x1b[33m┌──────────────────────────────────────────────────┐\x1b[0m");
        eprintln!("  \x1b[33m│  FIRST RUN — New wallet created                  │\x1b[0m");
        eprintln!("  \x1b[33m└──────────────────────────────────────────────────┘\x1b[0m");
        eprintln!("  \x1b[1mWallet:\x1b[0m {address}");
        eprintln!();
        eprintln!("  \x1b[90mTo submit solutions on-chain:\x1b[0m");
        eprintln!("  \x1b[90m  1. Fund this wallet with ETH on Arbitrum One (chain 42161)\x1b[0m");
        eprintln!("  \x1b[90m  2. Set your own Arbitrum One RPC in the config file\x1b[0m");
        eprintln!("  \x1b[90m  Mining runs in local mode until the wallet is funded.\x1b[0m");
        eprintln!();
        eprintln!("  \x1b[90mConfig: {}\x1b[0m", mask_user_path(&config_path));
        eprintln!("  \x1b[90mKey:    {}\x1b[0m", mask_user_path(&key_path));
        eprintln!();
    }

    // Derive wallet address from key file (needed for dashboard display)
    let wallet_address = keygen::address_from_key_file(&key_path)
        .unwrap_or_else(|_| String::new());
    if wallet_address.is_empty() {
        step_warn("Wallet address unavailable");
    } else {
        step(&format!("Wallet ready: {wallet_address}"));
    }

    let telemetry_path = config.telemetry_path()?;
    let control_path = config.control_file_path()?;
    if let Some(parent) = telemetry_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Warn if still on the default public RPC (rate-limited, not ideal for mining)
    let using_default_rpc = config.rpc_url == DEFAULT_RPC_URL
        || config.rpc_url == "http://localhost:8545";
    if !is_first_run && using_default_rpc {
        step_warn("Using default public RPC \u{2014} this is rate-limited");
        eprintln!("        Set a private Arbitrum One (chain 42161) RPC in:");
        eprintln!("        {}", mask_user_path(&config_path));
    }

    // ── 5. Spawn miner ────────────────────────────────────────────────
    let handle = spawn_miner(config.clone())?;
    let shutdown = handle.shutdown_token();
    step(&format!("Miner started ({} threads)", config.threads));

    let latest: Arc<RwLock<Option<TelemetrySnapshot>>> = Arc::new(RwLock::new(None));

    // ── 6. Writer task — append telemetry to JSONL + update latest ───
    let mut rx = handle.subscribe();
    let writer_latest = Arc::clone(&latest);
    let writer_telemetry_path = telemetry_path.clone();
    let writer_shutdown = shutdown.clone();
    let writer_task = tokio::spawn(async move {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&writer_telemetry_path)
            .with_context(|| {
                format!("Failed to open {}", writer_telemetry_path.display())
            })?;

        let mut line_count: u64 = 0;
        const ROTATE_EVERY: u64 = 5000;
        const ROTATE_MAX_BYTES: u64 = 5 * 1024 * 1024;
        const ROTATE_KEEP_LINES: usize = 500;

        loop {
            tokio::select! {
                _ = writer_shutdown.cancelled() => break,
                result = rx.recv() => {
                    match result {
                        Ok(snapshot) => {
                            let line = serde_json::to_string(&snapshot)?;
                            writeln!(file, "{}", line)?;
                            *writer_latest.write().await = Some(snapshot);

                            line_count += 1;
                            if line_count % ROTATE_EVERY == 0 {
                                if let Ok(meta) = std::fs::metadata(&writer_telemetry_path) {
                                    if meta.len() > ROTATE_MAX_BYTES {
                                        drop(file);
                                        rotate_telemetry(&writer_telemetry_path, ROTATE_KEEP_LINES);
                                        file = OpenOptions::new()
                                            .create(true)
                                            .append(true)
                                            .open(&writer_telemetry_path)?;
                                    }
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    });

    // ── 7. Embedded heartbeat + ransomware watchdog ─────────────────
    let heartbeat = heartbeat::HeartbeatMonitor::new(
        Arc::clone(&latest),
        vec![
            sm_paths.config_dir.clone(),
            sm_paths.key_dir.clone(),
            sm_paths.log_dir.clone(),
        ],
    );
    let heartbeat_status = heartbeat.status.clone();
    let hb_shutdown = shutdown.clone();
    let heartbeat_task = tokio::spawn(async move {
        heartbeat.run(hb_shutdown).await;
    });
    step("Heartbeat active");

    // ── 8. HTTP server task ─────────────────────────────────────────
    let app_state = AppState {
        handle: Arc::new(handle),
        telemetry_path: telemetry_path.clone(),
        control_path: control_path.clone(),
        latest: Arc::clone(&latest),
        heartbeat_status,
        shutdown: shutdown.clone(),
        wallet_address: wallet_address.clone(),
        rpc_url: config.rpc_url.clone(),
        contract_address: config.contract_address.clone(),
        pool_id: config.pool_id,
        config_path: config_path.clone(),
    };

    let port = cli.port;
    let http_task = tokio::spawn(async move {
        if let Err(e) = server::run_server(app_state, port).await {
            tracing::error!("HTTP server error: {e:#}");
        }
    });
    step(&format!("Dashboard live at \x1b[4mhttp://127.0.0.1:{port}\x1b[0m"));

    // ── Footer ──────────────────────────────────────────────────────
    eprintln!();
    eprintln!("  \x1b[90mLogs:   {}\x1b[0m", mask_user_path(&log_file_path));
    eprintln!("  \x1b[90mConfig: {}\x1b[0m", mask_user_path(&config_path));
    eprintln!();
    eprintln!("  \x1b[1mMining — press Ctrl+C to stop.\x1b[0m");
    eprintln!("  \x1b[90m───────────────────────────────────────────────────────\x1b[0m");

    // ── 9. Open browser ─────────────────────────────────────────────
    if !cli.no_browser {
        let url = format!("http://127.0.0.1:{}", port);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            let _ = open_browser(&url);
        });
    }

    // ── 10. Console status reporter (every 30s) ─────────────────────
    let status_latest = Arc::clone(&latest);
    let status_shutdown = shutdown.clone();
    let start_instant = std::time::Instant::now();
    let status_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.tick().await; // skip first immediate tick
        loop {
            tokio::select! {
                _ = status_shutdown.cancelled() => break,
                _ = interval.tick() => {
                    let snap = status_latest.read().await;
                    if let Some(ref s) = *snap {
                        let elapsed = fmt_elapsed(start_instant.elapsed().as_secs());
                        eprintln!(
                            "  \x1b[90m{elapsed}\x1b[0m  \x1b[36m{}\x1b[0m \x1b[90m│\x1b[0m {} solutions \x1b[90m│\x1b[0m {} on-chain \x1b[90m│\x1b[0m \x1b[33m{:.2} TGBT\x1b[0m",
                            fmt_hashrate(s.hashrate_hs),
                            s.solutions,
                            s.accepted_submissions,
                            s.total_rewards_estimate,
                        );
                    }
                }
            }
        }
    });

    // ── 11. Ctrl+C handler ──────────────────────────────────────────
    let ctrl_shutdown = shutdown.clone();
    tokio::spawn(async move {
        if signal::ctrl_c().await.is_ok() {
            eprintln!();
            eprintln!("  \x1b[33m[!] Ctrl+C received — shutting down...\x1b[0m");
            ctrl_shutdown.cancel();
        }
    });

    // ── 12. Wait for shutdown ───────────────────────────────────────
    shutdown.cancelled().await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    if !writer_task.is_finished() { writer_task.abort(); }
    http_task.abort();
    heartbeat_task.abort();
    status_task.abort();

    let total = fmt_elapsed(start_instant.elapsed().as_secs());
    eprintln!("  \x1b[32m[+] Stopped after {total}. Goodbye!\x1b[0m");
    eprintln!();
    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Print a green step indicator to the console.
fn step(msg: &str) {
    eprintln!("  \x1b[32m[+]\x1b[0m {msg}");
}

/// Print a yellow warning step.
fn step_warn(msg: &str) {
    eprintln!("  \x1b[33m[!]\x1b[0m {msg}");
}

/// Format elapsed seconds as human-readable duration.
fn fmt_elapsed(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

/// Format hashrate for display (auto-scale to kH/s, MH/s).
fn fmt_hashrate(h: f64) -> String {
    if h >= 1_000_000.0 {
        format!("{:.1} MH/s", h / 1_000_000.0)
    } else if h >= 1_000.0 {
        format!("{:.1} kH/s", h / 1_000.0)
    } else {
        format!("{:.0} H/s", h)
    }
}

/// Set the console window title (Windows only).
#[cfg(windows)]
fn set_console_title(title: &str) {
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    extern "system" {
        fn SetConsoleTitleW(title: *const u16) -> i32;
    }
    unsafe { SetConsoleTitleW(wide.as_ptr()); }
}

/// Enable UTF-8 console output and virtual terminal processing (Windows only).
#[cfg(windows)]
fn init_windows_console() {
    extern "system" {
        fn SetConsoleOutputCP(cp: u32) -> i32;
        fn GetStdHandle(nStdHandle: u32) -> isize;
        fn GetConsoleMode(hConsoleHandle: isize, lpMode: *mut u32) -> i32;
        fn SetConsoleMode(hConsoleHandle: isize, dwMode: u32) -> i32;
    }
    unsafe {
        // UTF-8 code page
        SetConsoleOutputCP(65001);
        // Enable ANSI / VT100 escape sequence processing
        let handle = GetStdHandle(0xFFFF_FFF4); // STD_ERROR_HANDLE
        if handle != -1 {
            let mut mode: u32 = 0;
            if GetConsoleMode(handle, &mut mode) != 0 {
                SetConsoleMode(handle, mode | 0x0004); // ENABLE_VIRTUAL_TERMINAL_PROCESSING
            }
        }
    }
}

/// Apply production defaults for self-miner when the config has placeholder values.
fn apply_self_miner_defaults(config: &mut MinerConfig, paths: &SelfMinerPaths) {
    // Set production contract if still placeholder
    if config.contract_address == "0xYourContractAddress" {
        config.contract_address = DEFAULT_CONTRACT.to_string();
    }
    // Set RPC URL if default localhost
    if config.rpc_url == "http://localhost:8545" {
        config.rpc_url = DEFAULT_RPC_URL.to_string();
    }
    // API key: users must provide their own via config
    // (no default key — each miner needs its own NativeBTC API key)
    // Set pool ID
    if config.pool_id == 0 {
        config.pool_id = DEFAULT_POOL_ID;
    }
    // Auto-detect thread count (only when unset / zero — respect explicit threads=1)
    if config.threads == 0 {
        config.threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
    }
    // Ensure key path is set
    if config.private_key_path == "keys/miner.key" {
        let key_path = paths.key_dir.join("miner.key");
        config.private_key_path = key_path.to_string_lossy().to_string();
    }
}

/// Strip any developer-specific values that may have leaked into persisted configs.
/// This runs on every startup so stale dev configs from earlier builds get cleaned.
fn sanitize_dev_values(config: &mut MinerConfig) {
    // Remove developer API key
    if let Some(ref key) = config.rpc_api_key {
        if key.starts_with(DEV_API_KEY_PREFIX) {
            config.rpc_api_key = None;
        }
    }
    // Reset developer RPC URL to public default
    if config.rpc_url.contains("nativebtc.org") {
        config.rpc_url = DEFAULT_RPC_URL.to_string();
    }
    // Reset developer pool ID to community pool
    if config.pool_id == 3 {
        config.pool_id = DEFAULT_POOL_ID;
    }
}

/// Resolve absolute path to the private key file.
fn resolve_key_path(config: &MinerConfig, config_path: &PathBuf) -> PathBuf {
    let p = PathBuf::from(&config.private_key_path);
    if p.is_absolute() {
        p
    } else {
        config_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join(p)
    }
}

/// Open the default browser to the given URL.
fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        if std::process::Command::new("explorer.exe")
            .arg(url)
            .spawn()
            .is_err()
        {
            std::process::Command::new("rundll32.exe")
                .args(["url.dll,FileProtocolHandler", url])
                .spawn()?;
        }
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
    }
    Ok(())
}

/// Rotate telemetry.jsonl — keep only the last `keep` lines.
fn rotate_telemetry(path: &PathBuf, keep: usize) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let lines: Vec<&str> = content.lines().collect();
    let tail = if lines.len() > keep {
        &lines[lines.len() - keep..]
    } else {
        &lines[..]
    };
    let _ = std::fs::write(path, tail.join("\n") + "\n");
}

/// Replace the OS user directory with ~ for display.
/// "C:\Users\comar\AppData\..." → "~\AppData\..."
fn mask_user_path(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    if let Some(home) = dirs_home() {
        let home_str = home.to_string_lossy();
        if let Some(rest) = s.strip_prefix(home_str.as_ref()) {
            return format!("~{rest}");
        }
    }
    s.to_string()
}

fn dirs_home() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE").ok().map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
}
