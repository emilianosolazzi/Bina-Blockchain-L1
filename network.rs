use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, Duration};
use std::sync::Arc;
use anyhow::{Result, Context, anyhow};
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::{TlsConnector, client::TlsStream};
use rustls::{ClientConfig, RootCertStore, OwnedTrustAnchor, client::ClientSessionMemoryCache}; // Added ClientSessionMemoryCache
use webpki::DnsNameRef; // Correct import for DNS name
use rand::rngs::OsRng;
use rand::RngCore;
use tracing::info;
use url::Url; // Added for better endpoint parsing
use async_trait::async_trait; // Added for the trait

// --- Concrete Implementations ---

async fn connect_with_timeout(endpoint: &str, timeout: Duration) -> Result<TcpStream> {
    tokio::time::timeout(timeout, TcpStream::connect(endpoint))
        .await
        .context("Connection timed out")? // Error if timeout occurs
        .context("Failed to connect to TCP endpoint") // Error if TcpStream::connect fails
}

async fn add_traffic_noise<S: AsyncReadExt + AsyncWriteExt + Unpin>(stream: &mut S) -> Result<()> {
    // Send some initial random padding to mask connection start
    // Adjust size and pattern as needed for desired masking effect
    const NOISE_SIZE: usize = 128;
    let mut noise = vec![0u8; NOISE_SIZE];
    OsRng.fill_bytes(&mut noise);

    stream.write_all(&noise).await.context("Failed to write traffic noise")?;
    stream.flush().await.context("Failed to flush traffic noise")?;
    info!("Sent {} bytes of initial traffic noise.", NOISE_SIZE);
    Ok(())
}

// --- End Concrete Implementations ---

static CONNECTION_ESTABLISHED: AtomicBool = AtomicBool::new(false);

// --- SecureTransport Trait Definition ---
#[async_trait]
pub trait SecureTransport: Send + Sync {
    async fn write_all(&mut self, data: &[u8]) -> Result<()>;
    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<()>;
    async fn shutdown(&mut self) -> Result<()>;
    // Optional: Add check_rotation if needed in the trait
    // fn check_rotation(&mut self) -> Result<()>;
}
// --- End Trait Definition ---

pub struct SecureChannel {
    // Use tokio-rustls TlsStream over tokio's TcpStream
    stream: TlsStream<TcpStream>,
    last_rotated: SystemTime,
}

// --- Updated secure_connect function ---
pub async fn secure_connect(endpoint: &str, custom_root_store: Option<RootCertStore>) -> Result<SecureChannel> {
    // --- Better Endpoint Parsing ---
    // Assume HTTPS if no scheme provided, as TLS is expected.
    let endpoint_url_str = if !endpoint.contains("://") {
        format!("https://{}", endpoint)
    } else {
        endpoint.to_string()
    };
    let url = Url::parse(&endpoint_url_str)
        .context(format!("Failed to parse endpoint URL: {}", endpoint_url_str))?;
    let domain_str = url.host_str().ok_or_else(|| anyhow!("Invalid endpoint: missing domain"))?;
    let port = url.port_or_known_default().unwrap_or(443); // Default to 443 for HTTPS
    let tcp_endpoint = format!("{}:{}", domain_str, port);
    // --- End Better Endpoint Parsing ---

    // Configure TLS Root Certificates
    let root_store = match custom_root_store {
        Some(store) => {
            info!("Using custom root certificate store.");
            store
        }
        None => {
            info!("Loading native root certificates.");
            let mut store = RootCertStore::empty();
            for cert in rustls_native_certs::load_native_certs().context("Failed to load native certificates")? {
                store.add(&rustls::Certificate(cert.0))
                    .map_err(|e| anyhow!("Failed to add native certificate: {}", e))?;
            }
            store
        }
    };

    // --- Build TLS ClientConfig with Session Resumption ---
    let config = ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(root_store)
        .with_no_client_auth()
        .with_client_session_cache(ClientSessionMemoryCache::new(128)); // Enable session resumption cache

    let connector = TlsConnector::from(Arc::new(config));

    // Extract domain name for SNI and verification
    let domain = DnsNameRef::try_from_ascii_str(domain_str)
        .map_err(|_| anyhow!("Invalid DNS name format in endpoint: {}", domain_str))?;

    // Connect TCP stream with timeout
    let stream = connect_with_timeout(&tcp_endpoint, Duration::from_secs(10)).await?;
    stream.set_nodelay(true).context("Failed to set TCP_NODELAY")?;

    // Perform TLS handshake
    let mut tls_stream = connector.connect(domain, stream).await
        .context("TLS handshake failed")?;

    // Add initial traffic pattern masking
    add_traffic_noise(&mut tls_stream).await?;

    CONNECTION_ESTABLISHED.store(true, Ordering::SeqCst);
    info!("Secure connection established to {}", tcp_endpoint);

    Ok(SecureChannel {
        stream: tls_stream,
        last_rotated: SystemTime::now(),
    })
}

// --- SecureChannel Implementation ---
impl SecureChannel {
    // Make async if rotate_keys becomes async
    pub fn check_rotation(&mut self) -> Result<()> {
        if self.last_rotated.elapsed()? > Duration::from_secs(3600) { // Check every hour
            self.rotate_keys()?; // Call the placeholder rotation logic
            self.last_rotated = SystemTime::now();
        }
        Ok(())
    }

    // Placeholder for key rotation logic - remains complex
    fn rotate_keys(&mut self) -> Result<()> {
        info!("Rotating TLS keys (placeholder - requires complex implementation)");
        // Actual implementation for TLS 1.3 key update or renegotiation is non-trivial
        // and depends heavily on the specific rustls features and session state.
        // For now, we just log the intent.
        // If noise is desired after rotation attempt:
        // tokio::spawn(add_traffic_noise(&mut self.stream)); // Needs async context if rotate_keys is async
        Ok(())
    }

    // Add methods to read/write from the secure channel
    pub async fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.stream.write(buf).await.context("Failed to write to secure channel")
    }

    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.stream.read(buf).await.context("Failed to read from secure channel")
    }

    pub async fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.stream.write_all(buf).await.context("Failed to write_all to secure channel")
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        self.stream.shutdown().await.context("Failed to shutdown secure channel")
    }
}

// --- Implement SecureTransport Trait for SecureChannel ---
#[async_trait]
impl SecureTransport for SecureChannel {
    async fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.stream.write_all(buf).await.context("Failed to write_all to secure channel")
    }

    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        self.stream.read_exact(buf).await.context("Failed to read_exact from secure channel")
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.stream.shutdown().await.context("Failed to shutdown secure channel")
    }

    // If check_rotation is needed by consumers via the trait:
    // fn check_rotation(&mut self) -> Result<()> {
    //     self.check_rotation() // Call the inherent method
    // }
}
// --- End Trait Implementation ---
