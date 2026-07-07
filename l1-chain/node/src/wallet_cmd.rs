//! l1-wallet — Bina Chain wallet CLI  ($BINA)
//!
//! Commands
//! ────────────────────────────────────────────────────────────────────────────
//!   l1-wallet generate [--path <file>]   Generate a new hybrid keypair
//!   l1-wallet show     [--path <file>]   Print address + public key
//!   l1-wallet address  [--path <file>]   Print address only (for scripts)
//!   l1-wallet sign     <message>         Sign a message, print signature hex
//!   l1-wallet verify   <message> <sig>   Verify a hex signature
//!
//! The wallet file is JSON stored at:
//!   %USERPROFILE%\.bina\wallet.json   (Windows)
//!   $HOME/.bina/wallet.json           (Unix)
//!
//! File format:
//!   {
//!     "version": 1,
//!     "address": "<40-char hex>",        ← 20-byte wallet address
//!     "public_key": "<1858-char hex>",   ← 929 bytes: ed25519(32) + falcon_pk(897)
//!     "secret_key": "<4420-char hex>"    ← 2210 bytes: ed_sk(32)+falcon_pk(897)+falcon_sk(1281)
//!   }

use l1_core::crypto::{HybridSignature, WalletKeypair};
use std::path::PathBuf;
use std::{env, fs, process};

// ─── Wallet file ─────────────────────────────────────────────────────────────

fn default_wallet_path() -> PathBuf {
    let home = env::var("USERPROFILE")
        .or_else(|_| env::var("HOME"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".bina").join("wallet.json")
}

fn save_wallet(kp: &WalletKeypair, path: &PathBuf) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("cannot create wallet directory");
    }
    let json = serde_json::json!({
        "version":    1,
        "address":    kp.address_hex(),
        "public_key": hex::encode(kp.public_key().to_bytes()),
        "secret_key": hex::encode(kp.to_secret_bytes()),
    });
    let text = serde_json::to_string_pretty(&json).unwrap();
    fs::write(path, text).expect("cannot write wallet file");
}

fn load_wallet(path: &PathBuf) -> WalletKeypair {
    let text = fs::read_to_string(path).unwrap_or_else(|_| {
        eprintln!("error: wallet file not found: {}", path.display());
        eprintln!("  Run 'l1-wallet generate' to create one.");
        process::exit(1);
    });
    let v: serde_json::Value = serde_json::from_str(&text).expect("wallet file is not valid JSON");
    let sk_hex = v["secret_key"].as_str().expect("missing 'secret_key'");
    let sk_bytes = hex::decode(sk_hex).expect("'secret_key' is not valid hex");
    WalletKeypair::from_secret_bytes(&sk_bytes).unwrap_or_else(|e| {
        eprintln!("error: corrupt wallet file: {e}");
        process::exit(1);
    })
}

// ─── Commands ─────────────────────────────────────────────────────────────────

fn cmd_generate(wallet_path: &PathBuf) {
    if wallet_path.exists() {
        eprintln!("Wallet already exists at {}", wallet_path.display());
        eprintln!("Delete it manually first if you really want to overwrite.");
        process::exit(1);
    }
    println!("Generating hybrid keypair (Ed25519 + Falcon-512)...");
    let kp = WalletKeypair::generate();
    save_wallet(&kp, wallet_path);
    println!();
    println!("  ✓  Wallet created: {}", wallet_path.display());
    println!();
    println!("  Address   : {}", kp.address_hex());
    println!("  Algorithms: Ed25519 (classical) + Falcon-512 (post-quantum)");
    println!("  Key sizes : ed25519 32 B + falcon_pk 897 B + falcon_sk 1281 B");
    println!();
    println!("  ⚠  Keep the wallet file secret. Anyone with it can sign transactions.");
}

fn cmd_show(wallet_path: &PathBuf) {
    let kp = load_wallet(wallet_path);
    let pk_bytes = kp.public_key().to_bytes();
    println!("Wallet: {}", wallet_path.display());
    println!();
    println!("  address    : {}", kp.address_hex());
    println!("  public_key : {}…  ({} bytes)", &hex::encode(&pk_bytes)[..32], pk_bytes.len());
    println!("               ed25519(32 B) + falcon_pk(897 B)");
}

fn cmd_address(wallet_path: &PathBuf) {
    // Silent — just print the address, useful for scripts
    let kp = load_wallet(wallet_path);
    println!("{}", kp.address_hex());
}

fn cmd_sign(wallet_path: &PathBuf, message: &str) {
    let kp  = load_wallet(wallet_path);
    let sig = kp.sign(message.as_bytes());
    let raw = sig.to_bytes();
    println!("address  : {}", kp.address_hex());
    println!("message  : {message}");
    println!("sig_hex  : {}", hex::encode(&raw));
    println!("ed25519  : {}", sig.ed_hex());
    println!("falcon   : {}…  ({} bytes)", &sig.falcon_hex()[..32], sig.falcon.len());
    println!("sig_bytes: {}", raw.len());
}

fn cmd_verify(wallet_path: &PathBuf, message: &str, sig_hex: &str) {
    let kp  = load_wallet(wallet_path);
    let raw = hex::decode(sig_hex).unwrap_or_else(|_| {
        eprintln!("error: signature is not valid hex");
        process::exit(1);
    });
    let sig = HybridSignature::from_bytes(&raw).unwrap_or_else(|e| {
        eprintln!("error: bad signature bytes: {e}");
        process::exit(1);
    });
    match kp.public_key().verify(message.as_bytes(), &sig) {
        Ok(())  => println!("✓  VALID   — both Ed25519 and Falcon-512 verified"),
        Err(e)  => {
            println!("✗  INVALID — {e}");
            process::exit(2);
        }
    }
}

fn cmd_help() {
    println!("l1-wallet — Bina Chain wallet  $BINA  (Ed25519 + Falcon-512 hybrid)");
    println!();
    println!("Usage:");
    println!("  l1-wallet generate [--path <file>]   Generate a new keypair");
    println!("  l1-wallet show     [--path <file>]   Show address + public key");
    println!("  l1-wallet address  [--path <file>]   Print address only");
    println!("  l1-wallet sign     <message>         Sign a message");
    println!("  l1-wallet verify   <message> <sig>   Verify a hex signature");
    println!();
    println!("Default wallet: %USERPROFILE%\\.bina\\wallet.json");
}

// ─── Entry point ─────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = env::args().collect();

    // Parse --path override
    let mut wallet_path = default_wallet_path();
    let mut rest: Vec<&str> = Vec::new();
    let mut i = 1usize;
    while i < args.len() {
        if (args[i] == "--path" || args[i] == "-p") && i + 1 < args.len() {
            wallet_path = PathBuf::from(&args[i + 1]);
            i += 2;
        } else {
            rest.push(&args[i]);
            i += 1;
        }
    }

    let cmd = rest.first().copied().unwrap_or("help");
    match cmd {
        "generate" | "gen"  => cmd_generate(&wallet_path),
        "show"              => cmd_show(&wallet_path),
        "address" | "addr"  => cmd_address(&wallet_path),
        "sign"              => {
            let msg = rest.get(1).copied().unwrap_or_else(|| {
                eprintln!("Usage: l1-wallet sign <message>"); process::exit(1)
            });
            cmd_sign(&wallet_path, msg);
        }
        "verify" => {
            let msg = rest.get(1).copied().unwrap_or_else(|| {
                eprintln!("Usage: l1-wallet verify <message> <sig_hex>"); process::exit(1)
            });
            let sig = rest.get(2).copied().unwrap_or_else(|| {
                eprintln!("Usage: l1-wallet verify <message> <sig_hex>"); process::exit(1)
            });
            cmd_verify(&wallet_path, msg, sig);
        }
        _ => cmd_help(),
    }
}
