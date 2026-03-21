use anyhow::{Result, Context, anyhow};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH, Duration};
use tokio::fs;
use tokio::io::AsyncWriteExt; // Required for write_all
use ring::{digest, signature};
use tracing::{info, warn, error, debug};
use hex; // Use the hex crate directly
use std::sync::atomic::{AtomicBool, Ordering};

// Add platform-specific modules
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

// Add global update state
static UPDATING: AtomicBool = AtomicBool::new(false);

// --- Manifest Definition ---

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UpdateManifest {
    pub version: String,
    pub timestamp: u64,
    pub size: u64,
    pub sha256: String,
    pub signature: String, // Hex-encoded Ed25519 signature
    pub download_url: String, // URL to download the update binary
}

// --- Update Verifier ---

#[derive(Debug)]
pub struct UpdateVerifier {
    public_key_bytes: Vec<u8>,
    current_version: String,
}

impl UpdateVerifier {
    pub fn new(public_key_bytes: Vec<u8>, current_version: &str) -> Result<Self> {
        // Basic validation of key format (length for Ed25519)
        if public_key_bytes.len() != 32 {
            return Err(anyhow!("Invalid Ed25519 public key length: {}", public_key_bytes.len()));
        }
        Ok(Self {
            public_key_bytes,
            current_version: current_version.to_string(),
        })
    }

    /// Checks the update server for a new manifest and verifies it.
    pub async fn check_for_updates(&self, update_server_url: &str) -> Result<Option<UpdateManifest>> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15)) // Increased timeout slightly
            .build()?;

        let manifest_url = format!("{}/manifest.json", update_server_url.trim_end_matches('/'));
        debug!("Checking for updates at: {}", manifest_url);

        let response = client.get(&manifest_url).send().await
            .context("Failed to send request to update server")?;

        if !response.status().is_success() {
            warn!("Failed to fetch update manifest: Status {}", response.status());
            return Err(anyhow!("Failed to fetch update manifest (Status: {})", response.status()));
        }

        let manifest: UpdateManifest = response.json().await
            .context("Failed to parse update manifest JSON")?;
        debug!("Received manifest for version: {}", manifest.version);

        // Verify signature first
        self.verify_manifest(&manifest)?;
        info!("Update manifest signature verified successfully.");

        // Then check if version is newer
        if self.is_newer_version(&manifest.version) {
            info!("Newer version {} found (current: {}).", manifest.version, self.current_version);
            Ok(Some(manifest))
        } else {
            debug!("Current version {} is up-to-date.", self.current_version);
            Ok(None)
        }
    }

    /// Downloads the update binary specified in the manifest.
    pub async fn download_update(
        &self,
        manifest: &UpdateManifest,
        output_path: &Path,
    ) -> Result<()> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120)) // Longer timeout for download
            .build()?;

        info!("Downloading update from: {}", manifest.download_url);
        let mut response = client.get(&manifest.download_url).send().await
             .context("Failed to send download request")?;

        if !response.status().is_success() {
            return Err(anyhow!("Failed to download update (Status: {})", response.status()));
        }

        // Ensure parent directory exists
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).await.context("Failed to create directory for update file")?;
        }

        let mut file = fs::File::create(output_path).await
            .context(format!("Failed to create update file at {:?}", output_path))?;
        let mut hasher = digest::Context::new(&digest::SHA256);
        let mut bytes_downloaded = 0u64;

        while let Some(chunk) = response.chunk().await.context("Failed to read download chunk")? {
            file.write_all(&chunk).await.context("Failed to write chunk to update file")?;
            hasher.update(&chunk);
            bytes_downloaded += chunk.len() as u64;

            // Optional: Add progress reporting here

            // Verify we're not exceeding expected size (with a small buffer for safety)
            if bytes_downloaded > manifest.size + 1024 { // Allow 1KB buffer
                warn!("Download size ({}) exceeded manifest size ({}), aborting.", bytes_downloaded, manifest.size);
                // Clean up partial file
                drop(file); // Close the file before removing
                fs::remove_file(output_path).await.ok();
                return Err(anyhow!("Download size exceeds manifest size"));
            }
        }

        // Final size check
        if bytes_downloaded != manifest.size {
             warn!("Final download size ({}) does not match manifest size ({}), aborting.", bytes_downloaded, manifest.size);
             drop(file);
             fs::remove_file(output_path).await.ok();
             return Err(anyhow!("Final download size mismatch"));
        }

        // Verify hash
        let actual_hash = hex::encode(hasher.finish().as_ref());
        if actual_hash.to_lowercase() != manifest.sha256.to_lowercase() {
            warn!("Hash mismatch after download. Expected: {}, Got: {}", manifest.sha256, actual_hash);
            drop(file);
            fs::remove_file(output_path).await.ok();
            return Err(anyhow!("Hash mismatch after download"));
        }

        info!("Update downloaded successfully and hash verified.");
        Ok(())
    }

    /// Verifies the signature and timestamp of the update manifest.
    fn verify_manifest(&self, manifest: &UpdateManifest) -> Result<()> {
        let public_key = signature::UnparsedPublicKey::new(
            &signature::ED25519,
            &self.public_key_bytes
        );

        // The message signed on the server should match this format exactly
        let message = format!(
            "{}{}{}{}",
            manifest.version, manifest.timestamp, manifest.size, manifest.sha256
        );

        let signature_bytes = hex::decode(&manifest.signature)
            .context("Invalid signature encoding in manifest (must be hex)")?;

        public_key.verify(message.as_bytes(), &signature_bytes)
            .context("Manifest signature verification failed")?;

        // Verify timestamp is recent and not in the future
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs();

        // Allow manifest to be valid for ~1 day, but not in the future (+5 min skew)
        if manifest.timestamp > now + 300 {
            return Err(anyhow!("Manifest timestamp is too far in the future"));
        }
        if manifest.timestamp < now.saturating_sub(86400 * 2) { // Allow ~2 days old max
             return Err(anyhow!("Manifest timestamp is too old"));
        }

        Ok(())
    }

    /// Compares the manifest version with the current application version.
    fn is_newer_version(&self, new_version_str: &str) -> bool {
        // Use version_compare crate for robust comparison
        match version_compare::compare(&self.current_version, new_version_str) {
            Ok(cmp) => cmp == std::cmp::Ordering::Less,
            Err(e) => {
                warn!("Failed to compare versions ('{}' vs '{}'): {}", self.current_version, new_version_str, e);
                false // Treat parse errors as not newer
            }
        }
    }

    /// Enhanced verification with binary compatibility check
    async fn verify_binary_update(&self, path: &Path) -> Result<()> {
        let metadata = tokio::fs::metadata(path).await?;
        if metadata.len() > 1024 * 1024 * 100 { // 100MB max
            return Err(anyhow!("Update file too large"));
        }

        #[cfg(target_family = "unix")]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = metadata.permissions();
            if perms.mode() & 0o111 == 0 {
                return Err(anyhow!("Update binary not executable"));
            }
        }

        Ok(())
    }
}

// --- Update Application Logic ---

/// Improved update application with platform-specific handling
pub async fn apply_update(update_path: &Path) -> Result<()> {
    if !UPDATING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        return Err(anyhow!("Update already in progress"));
    }

    let _guard = scopeguard::guard((), |_| {
        UPDATING.store(false, Ordering::SeqCst);
    });

    // Create backup
    let backup_path = create_backup().await?;

    let result = match std::env::consts::OS {
        "windows" => apply_update_windows(update_path).await,
        "linux" => apply_update_linux(update_path).await,
        "macos" => apply_update_macos(update_path).await,
        _ => Err(anyhow!("Unsupported platform")),
    };

    if result.is_err() {
        // Restore from backup
        restore_from_backup(&backup_path).await?;
    }

    result
}

#[cfg(target_os = "windows")]
async fn apply_update_windows(update_path: &Path) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    use windows::Win32::System::WindowsProgramming;

    let current_exe = std::env::current_exe()?;
    let temp_path = current_exe.with_extension("old");

    // Use Windows API for atomic rename
    windows::rename_with_backup(&current_exe, &temp_path)?;
    windows::copy_with_progress(update_path, &current_exe)?;
    
    // Schedule old exe for deletion on reboot
    windows::schedule_deletion(&temp_path)?;

    Ok(())
}

async fn create_backup() -> Result<PathBuf> {
    let current_exe = std::env::current_exe()?;
    let backup_path = current_exe.with_extension("backup");
    tokio::fs::copy(&current_exe, &backup_path).await?;
    Ok(backup_path)
}

/// Restarts the application. Placeholder implementation.
/// This function is platform-specific.
pub fn restart_application() -> Result<()> {
    warn!("restart_application: Placeholder function executed. Real restart logic needed.");

    // --- Platform-Specific Implementation Required ---
    // - Get the path to the current executable.
    // - Spawn a new process using that path.
    // - Exit the current process.
    // - Ensure the new process detaches correctly.

    let current_exe = std::env::current_exe()
        .context("Failed to get current executable path")?;

    // Basic restart attempt (might not work reliably across platforms or in all scenarios)
    match std::process::Command::new(&current_exe).spawn() {
        Ok(_) => {
            info!("Successfully spawned new process. Exiting current process.");
            std::process::exit(0); // Exit the current process cleanly
        }
        Err(e) => {
            error!("Failed to spawn new process: {}", e);
            Err(anyhow!("Failed to restart application: {}", e))
        }
    }
}

/// Enhanced restart with platform detection
pub fn restart_application() -> Result<()> {
    let current_exe = std::env::current_exe()?;
    
    #[cfg(target_family = "unix")]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&current_exe)
            .before_exec(|| {
                unsafe { libc::daemon(0, 0) };
                Ok(())
            })
            .spawn()?;
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new(&current_exe)
            .creation_flags(windows::Win32::System::Threading::DETACHED_PROCESS)
            .spawn()?;
    }

    std::process::exit(0);
}
