use anyhow::{Result, Context};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{info, warn};
use ring::digest;
use tempfile::TempDir;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
pub struct UpdateHelper {
    backup_paths: Vec<PathBuf>,
    staging_dir: Option<TempDir>,
    progress: AtomicU64,
    checksums: Vec<(PathBuf, [u8; 32])>, // Store SHA256 of backups
}

impl UpdateHelper {
    pub fn new() -> Self {
        Self {
            backup_paths: Vec::new(),
            staging_dir: None,
            progress: AtomicU64::new(0),
            checksums: Vec::new(),
        }
    }

    pub async fn verify_disk_space(&self, required: u64) -> Result<()> {
        let available = available_space()?;
        if available < required * 3 { // Need space for original + backup + new
            return Err(anyhow!("Insufficient disk space"));
        }
        Ok(())
    }
    
    pub async fn create_atomic_rename(from: &Path, to: &Path) -> Result<()> {
        tokio::fs::rename(from, to).await.context("Atomic rename failed")?;
        Ok(())
    }

    pub async fn prepare_update(&mut self) -> Result<PathBuf> {
        // Create secure staging directory
        let staging = tempfile::Builder::new()
            .prefix("update_staging")
            .rand_bytes(16)
            .tempdir()?;
        
        let staging_path = staging.path().to_owned();
        self.staging_dir = Some(staging);
        
        Ok(staging_path)
    }

    pub async fn create_backup(&mut self, path: &Path) -> Result<()> {
        let backup_path = path.with_extension("backup");
        
        // Create secure temporary file
        let mut temp = tempfile::NamedTempFile::new()?;
        tokio::fs::copy(path, temp.path()).await?;
        
        // Calculate checksum
        let mut hasher = digest::Context::new(&digest::SHA256);
        let content = tokio::fs::read(temp.path()).await?;
        hasher.update(&content);
        let checksum = hasher.finish();
        
        // Atomic rename
        temp.persist(&backup_path)?;
        
        self.backup_paths.push(backup_path.clone());
        self.checksums.push((backup_path, checksum.as_ref().try_into()?));
        
        Ok(())
    }

    pub async fn verify_and_cleanup(&mut self) -> Result<()> {
        // Verify all backups
        for (path, checksum) in &self.checksums {
            let content = tokio::fs::read(path).await?;
            let mut hasher = digest::Context::new(&digest::SHA256);
            hasher.update(&content);
            let current = hasher.finish();
            
            if current.as_ref() != checksum.as_slice() {
                return Err(anyhow!("Backup integrity check failed"));
            }
        }

        // Clean staging directory
        if let Some(dir) = self.staging_dir.take() {
            dir.close()?;
        }

        Ok(())
    }

    pub async fn rollback(&self) -> Result<()> {
        for path in self.backup_paths.iter().rev() {
            let original = path.with_extension("");
            self.create_atomic_rename(path, &original).await?;
        }
        Ok(())
    }

    pub fn update_progress(&self, progress: u64) {
        self.progress.store(progress, Ordering::Release);
    }

    pub fn get_progress(&self) -> u64 {
        self.progress.load(Ordering::Acquire)
    }

    // Platform-specific disk space check
    #[cfg(target_os = "windows")]
    async fn get_available_space(path: &Path) -> Result<u64> {
        use windows::Win32::Storage::FileSystem;
        let mut available: u64 = 0;
        unsafe {
            FileSystem::GetDiskFreeSpaceExW(
                path.as_os_str(),
                Some(&mut available),
                None,
                None,
            )?;
        }
        Ok(available)
    }
}

impl Drop for UpdateHelper {
    fn drop(&mut self) {
        if let Some(dir) = self.staging_dir.take() {
            if let Err(e) = dir.close() {
                warn!("Failed to cleanup staging directory: {}", e);
            }
        }
    }
}

#[cfg(target_family = "unix")]
fn available_space() -> Result<u64> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::env::current_exe()?.metadata()?;
    Ok(metadata.blocks() * 512)
}
