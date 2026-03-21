// Casino SDK Bridge - Native Layer for Randomness Integration
// Written in Rust with optional C ABI for legacy slot machine support

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::sync::{Mutex, Arc};
use std::time::{SystemTime, UNIX_EPOCH, Duration}; // Added Duration
use std::thread; // Added for sleep
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering}; // For request counters
use std::collections::HashMap;
use std::collections::VecDeque; // For connection pool
use once_cell::sync::Lazy;
use reqwest::blocking::{Client, ClientBuilder}; // Updated for connection pool
use reqwest::header::{HeaderMap, HeaderValue}; // For advanced headers
use serde::{Deserialize, Serialize};
use rand::{RngCore, thread_rng, SeedableRng};
use rand::rngs::StdRng; // For deterministic generation
use log::{info, warn, error, debug, LevelFilter}; // Added logging macros
use env_logger::Builder; // Added logger builder
use crossbeam_channel::{bounded, Sender, Receiver}; // For multi-threaded request handling

// --- Configuration ---
// Replace localhost endpoint with Entropy blockchain's randomness API
static RNG_ENDPOINT: &str = "https://api.entropy-chain.io/v1/randomness";
const ENTROPY_CHAIN_ID: u64 = 1337; // Entropy blockchain ID
const ENTROPY_CONTRACT_ADDRESS: &str = "0x1234567890123456789012345678901234567890"; // Temporal Gradient Beacon address
const MAX_RETRIES: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_millis(500);

// Advanced configuration for 1M user throughput
const CONNECTION_POOL_SIZE: usize = 100; // High connection count
const REQUEST_QUEUE_SIZE: usize = 1000; // Large request queue
const WORKER_THREADS: usize = 16; // Use many worker threads
const BATCH_SIZE: usize = 50; // Batch requests when possible
const CACHE_CAPACITY: usize = 10_000; // Large LRU cache size
const STATS_REPORTING_INTERVAL: Duration = Duration::from_secs(60); // Report stats every minute
// --- End Configuration ---

// Add Entropy blockchain specific authentication
struct EntropyAuth {
    token: String,
    private_key: Option<String>,
}

static ENTROPY_AUTH: Lazy<Mutex<EntropyAuth>> = Lazy::new(|| {
    Mutex::new(EntropyAuth {
        token: std::env::var("ENTROPY_API_TOKEN").unwrap_or_default(),
        private_key: std::env::var("ENTROPY_SIGNER_KEY").ok(),
    })
});

// --- Connection Pool ---
struct HttpConnection {
    client: Client,
    last_used: SystemTime,
    request_count: usize,
}

struct ConnectionPool {
    connections: Mutex<VecDeque<HttpConnection>>,
    max_size: usize,
    total_created: AtomicUsize,
    total_reused: AtomicUsize,
}

impl ConnectionPool {
    fn new(max_size: usize) -> Self {
        let mut connections = VecDeque::with_capacity(max_size);
        
        // Pre-warm with some initial connections
        for _ in 0..max_size.min(10) {
            if let Ok(client) = Self::create_client() {
                connections.push_back(HttpConnection {
                    client,
                    last_used: SystemTime::now(),
                    request_count: 0,
                });
            }
        }
        
        Self {
            connections: Mutex::new(connections),
            max_size,
            total_created: AtomicUsize::new(connections.len()),
            total_reused: AtomicUsize::new(0),
        }
    }
    
    fn create_client() -> Result<Client, reqwest::Error> {
        // Create client with optimal settings for high throughput
        ClientBuilder::new()
            .timeout(Duration::from_secs(10))
            .tcp_nodelay(true)
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .pool_max_idle_per_host(CONNECTION_POOL_SIZE)
            .build()
    }
    
    fn get_connection(&self) -> Client {
        let mut pool = self.connections.lock().unwrap();
        
        if let Some(mut conn) = pool.pop_front() {
            // Reuse existing connection
            conn.last_used = SystemTime::now();
            conn.request_count += 1;
            self.total_reused.fetch_add(1, Ordering::SeqCst);
            
            // Return client, keep connection metadata for later
            let client = conn.client.clone();
            pool.push_back(conn);
            client
        } else if self.total_created.load(Ordering::SeqCst) < self.max_size {
            // Create new connection if below capacity
            match Self::create_client() {
                Ok(client) => {
                    let client_clone = client.clone();
                    pool.push_back(HttpConnection {
                        client,
                        last_used: SystemTime::now(),
                        request_count: 1,
                    });
                    self.total_created.fetch_add(1, Ordering::SeqCst);
                    client_clone
                },
                Err(_) => {
                    // Fallback to a simple new client on failure
                    warn!("Failed to create pooled HTTP client, using default");
                    Client::new()
                }
            }
        } else {
            // At capacity, create temporary client
            warn!("Connection pool at capacity, creating temporary client");
            Client::new()
        }
    }
    
    fn stats(&self) -> (usize, usize, usize) {
        let pool = self.connections.lock().unwrap();
        let current_size = pool.len();
        let created = self.total_created.load(Ordering::SeqCst);
        let reused = self.total_reused.load(Ordering::SeqCst);
        
        (current_size, created, reused)
    }
}

static HTTP_POOL: Lazy<ConnectionPool> = Lazy::new(|| {
    // Initialize logger early
    init_logger();
    info!("Initializing HTTP connection pool with size {}", CONNECTION_POOL_SIZE);
    ConnectionPool::new(CONNECTION_POOL_SIZE)
});

// Statistics tracking
static REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static REQUESTS_SUCCESS: AtomicU64 = AtomicU64::new(0);
static REQUESTS_FAILED: AtomicU64 = AtomicU64::new(0);
static REQUESTS_CACHED: AtomicU64 = AtomicU64::new(0);
static LAST_RESPONSE: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

// --- Request Queue for Async Processing ---
struct RandomnessRequest {
    signer_address: Option<String>,
    callback: Option<Box<dyn Fn(Result<SpinResponse, String>) + Send>>,
    priority: u8, // Lower is higher priority
}

struct RequestProcessor {
    sender: Sender<RandomnessRequest>,
    _receiver: Receiver<RandomnessRequest>, // Keep a reference to prevent dropping
}

impl RequestProcessor {
    fn new() -> Self {
        let (sender, receiver) = bounded(REQUEST_QUEUE_SIZE);
        let receiver_clone = receiver.clone();
        
        // Spawn worker threads to process requests
        for worker_id in 0..WORKER_THREADS {
            let worker_receiver = receiver.clone();
            thread::spawn(move || {
                Self::worker_thread(worker_id, worker_receiver);
            });
        }
        
        Self {
            sender,
            _receiver: receiver_clone,
        }
    }
    
    fn worker_thread(worker_id: usize, receiver: Receiver<RandomnessRequest>) {
        info!("Starting request worker thread {}", worker_id);
        
        let mut batch = Vec::with_capacity(BATCH_SIZE);
        let mut batch_timer = SystemTime::now();
        
        loop {
            // Wait for a request
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(request) => {
                    batch.push(request);
                },
                Err(_) => {
                    // No requests in 100ms, continue to batch check
                }
            }
            
            // Process batch if it's full or if timer expired
            let batch_time_elapsed = SystemTime::now().duration_since(batch_timer).unwrap_or_default();
            if batch.len() >= BATCH_SIZE || (batch.len() > 0 && batch_time_elapsed >= Duration::from_millis(50)) {
                debug!("Worker {} processing batch of {} requests", worker_id, batch.len());
                
                // Process requests together if batch optimization is possible
                if batch.len() > 1 {
                    // Could implement batch API request if supported
                    // For now, process individually
                    for req in batch.drain(..) {
                        let result = Self::process_single_request(req.signer_address);
                        if let Some(callback) = req.callback {
                            callback(result);
                        }
                    }
                } else if batch.len() == 1 {
                    let req = batch.remove(0);
                    let result = Self::process_single_request(req.signer_address);
                    if let Some(callback) = req.callback {
                        callback(result);
                    }
                }
                
                // Reset batch timer
                batch_timer = SystemTime::now();
            }
            
            // Yield to other threads periodically
            if batch.is_empty() {
                thread::yield_now();
            }
        }
    }
    
    fn process_single_request(signer_address: Option<String>) -> Result<SpinResponse, String> {
        // Track total requests
        REQUESTS_TOTAL.fetch_add(1, Ordering::SeqCst);
        
        // Get client from connection pool
        let client = HTTP_POOL.get_connection();
        
        // Prepare entropy-specific headers
        let mut headers = HeaderMap::new();
        if let Some(addr) = &signer_address {
            headers.insert("X-Signer-Address", HeaderValue::from_str(addr).unwrap_or_default());
        }
        
        // Add Entropy authentication token
        let auth = ENTROPY_AUTH.lock().unwrap();
        if !auth.token.is_empty() {
            headers.insert("Authorization", HeaderValue::from_str(&format!("Bearer {}", auth.token)).unwrap_or_default());
        }
        
        // Generate unique request ID with timestamp
        let request_nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_string();
        
        // Prepare entropy seed - use Entropy-compatible entropy format
        let entropy_seed = generate_entropy_seed();
        
        // Make API request with retries, properly formatted for Entropy blockchain
        let mut last_error = String::from("Request not attempted");
        for attempt in 1..=MAX_RETRIES {
            match client.post(RNG_ENDPOINT)
                .headers(headers.clone())
                .json(&serde_json::json!({
                    // Entropy blockchain specific parameters
                    "chainId": ENTROPY_CHAIN_ID,
                    "beaconAddress": ENTROPY_CONTRACT_ADDRESS,
                    "requestType": "gameRandom",
                    "numResults": 3, // For 3 reels
                    "resultRange": 10, // For 10 symbols per reel
                    "userSeed": entropy_seed,
                    "nonce": request_nonce,
                    "attestation": generate_attestation(&entropy_seed, &request_nonce),
                }))
                .send() 
            {
                Ok(response) => {
                    if response.status().is_success() {
                        match response.json::<EntropyResponse>() {
                            Ok(entropy_result) => {
                                // Convert Entropy response to SpinResponse format
                                let result = SpinResponse {
                                    reelPositions: entropy_result.results.into_iter()
                                        .map(|r| r as u8)
                                        .collect(),
                                    timestamp: entropy_result.timestamp,
                                    seedUsed: entropy_result.requestHash,
                                    source: "entropy-blockchain",
                                };
                                
                                // Store the successful response
                                let serialized = serde_json::to_string(&result).unwrap_or_default();
                                *LAST_RESPONSE.lock().unwrap() = Some(serialized.clone());
                                
                                // Count success
                                REQUESTS_SUCCESS.fetch_add(1, Ordering::SeqCst);
                                
                                return Ok(result);
                            },
                            Err(e) => {
                                last_error = format!("JSON parse error: {}", e);
                            }
                        }
                    } else {
                        last_error = format!("HTTP error: {}", response.status());
                    }
                },
                Err(e) => {
                    last_error = format!("Request error: {}", e);
                }
            }
            
            // If not last attempt, wait before retrying
            if attempt < MAX_RETRIES {
                let backoff = RETRY_DELAY.mul_f32(1.5_f32.powi(attempt as i32 - 1));
                thread::sleep(backoff);
            }
        }
        
        // Count failure after all retries
        REQUESTS_FAILED.fetch_add(1, Ordering::SeqCst);
        Err(last_error)
    }
    
    fn submit(&self, request: RandomnessRequest) -> Result<(), String> {
        match self.sender.try_send(request) {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("Failed to queue request: {}", e))
        }
    }
}

static REQUEST_PROCESSOR: Lazy<RequestProcessor> = Lazy::new(|| {
    info!("Initializing request processor with {} worker threads", WORKER_THREADS);
    RequestProcessor::new()
});

// --- Results Cache using LRU ---
struct LruCache<K, V> {
    map: Mutex<HashMap<K, (V, SystemTime)>>,
    queue: Mutex<VecDeque<K>>,
    capacity: usize,
}

impl<K: Clone + Eq + std::hash::Hash, V: Clone> LruCache<K, V> {
    fn new(capacity: usize) -> Self {
        Self {
            map: Mutex::new(HashMap::with_capacity(capacity)),
            queue: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }
    
    fn get(&self, key: &K) -> Option<V> {
        let mut map = self.map.lock().unwrap();
        let mut queue = self.queue.lock().unwrap();
        
        if let Some((value, _timestamp)) = map.get(key) {
            // Move key to the back of the queue (most recently used)
            if let Some(pos) = queue.iter().position(|k| k == key) {
                queue.remove(pos);
            }
            queue.push_back(key.clone());
            
            return Some(value.clone());
        }
        None
    }
    
    fn put(&self, key: K, value: V) {
        let mut map = self.map.lock().unwrap();
        let mut queue = self.queue.lock().unwrap();
        
        // Remove oldest entry if at capacity
        if map.len() >= self.capacity && !queue.is_empty() {
            if let Some(old_key) = queue.pop_front() {
                map.remove(&old_key);
            }
        }
        
        // Add new entry
        map.insert(key.clone(), (value, SystemTime::now()));
        queue.push_back(key);
    }
    
    fn stats(&self) -> (usize, usize) {
        let map = self.map.lock().unwrap();
        let capacity = self.capacity;
        let size = map.len();
        (size, capacity)
    }
}

// Initialize the results cache
static RESULTS_CACHE: Lazy<LruCache<String, SpinResponse>> = Lazy::new(|| {
    LruCache::new(CACHE_CAPACITY)
});

// --- Start Stats Collector ---
fn start_stats_collector() {
    thread::spawn(|| {
        loop {
            thread::sleep(STATS_REPORTING_INTERVAL);
            
            // Get connection pool stats
            let (pool_size, created, reused) = HTTP_POOL.stats();
            
            // Get cache stats
            let (cache_size, cache_capacity) = RESULTS_CACHE.stats();
            
            // Get request stats
            let total = REQUESTS_TOTAL.load(Ordering::SeqCst);
            let success = REQUESTS_SUCCESS.load(Ordering::SeqCst);
            let failed = REQUESTS_FAILED.load(Ordering::SeqCst);
            let cached = REQUESTS_CACHED.load(Ordering::SeqCst);
            
            info!("=== Casino SDK Bridge Stats ===");
            info!("Requests: {} total, {} successful, {} failed, {} cached", 
                  total, success, failed, cached);
            info!("Connection pool: {}/{} connections, {} created, {} reused",
                  pool_size, CONNECTION_POOL_SIZE, created, reused);
            info!("Cache: {}/{} entries", cache_size, cache_capacity);
            info!("==============================");
        }
    });
}
// --- End Stats Collector ---

#[derive(Debug, Deserialize, Serialize, Clone)]
struct SpinResponse {
    reelPositions: Vec<u8>,
    timestamp: u64,
    seedUsed: String,
    source: String,
}

// Optional: Structure for API error responses
#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: String,
}

// --- Logger Initialization ---
fn init_logger() {
    let mut builder = Builder::from_default_env();
    
    builder
        .filter(None, LevelFilter::Info)
        .format_timestamp_millis()
        .init();
        
    info!("Casino SDK Bridge logger initialized");
    
    // Start the stats collector thread
    start_stats_collector();
}
// --- End Logger Initialization ---

// Modified function to handle 1 million concurrent users efficiently
#[no_mangle]
pub extern "C" fn request_random_spin(
    signer_address: *const c_char // Optional Signer Address
) -> c_int {
    // Track start time for performance
    let start_time = SystemTime::now();
    
    // Safety check for null pointer
    let signer_addr_string = if !signer_address.is_null() {
        unsafe {
            let c_str = CStr::from_ptr(signer_address);
            match c_str.to_str() {
                Ok(s) => Some(s.to_string()),
                Err(_) => None,
            }
        }
    } else {
        None
    };
    
    // Generate request ID based on address and timestamp
    let request_id = format!("{}:{}", 
        signer_addr_string.as_deref().unwrap_or("anonymous"),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
    );
    
    // Check cache first
    if let Some(cached_result) = RESULTS_CACHE.get(&request_id) {
        debug!("Cache hit for request {}", request_id);
        
        // Store in the last response for retrieval
        let serialized = serde_json::to_string(&cached_result).unwrap_or_default();
        *LAST_RESPONSE.lock().unwrap() = Some(serialized);
        
        // Count cache hit
        REQUESTS_CACHED.fetch_add(1, Ordering::SeqCst);
        
        return 1; // Success
    }
    
    // Submit request to the processor queue
    let request = RandomnessRequest {
        signer_address: signer_addr_string,
        callback: Some(Box::new(move |result| {
            match result {
                Ok(response) => {
                    // Cache successful result
                    RESULTS_CACHE.put(request_id.clone(), response.clone());
                    
                    // Log performance
                    let duration = SystemTime::now().duration_since(start_time).unwrap_or_default();
                    debug!("Request {} completed in {:?}", request_id, duration);
                },
                Err(error) => {
                    warn!("Request {} failed: {}", request_id, error);
                }
            }
        })),
        priority: 1, // Default priority
    };
    
    match REQUEST_PROCESSOR.submit(request) {
        Ok(_) => {
            debug!("Request {} submitted to queue", request_id);
            1 // Success
        },
        Err(e) => {
            error!("Failed to submit request {}: {}", request_id, e);
            0 // Failure
        }
    }
}

#[no_mangle]
pub extern "C" fn get_last_result() -> *const c_char {
    let last_response_guard = LAST_RESPONSE.lock().unwrap();
    match &*last_response_guard {
        Some(response) => {
            let c_string = CString::new(response.clone()).unwrap_or_else(|_| CString::new("{}").unwrap());
            c_string.into_raw()
        },
        None => {
            let c_string = CString::new("{}").unwrap();
            c_string.into_raw()
        }
    }
}

// Use cryptographic RNG for better entropy
fn generate_entropy_seed() -> String {
    let mut rng = thread_rng();
    let mut bytes = [0u8; 32];
    rng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

// Generate attestation for Entropy blockchain requests
fn generate_attestation(seed: &str, nonce: &str) -> String {
    let auth = ENTROPY_AUTH.lock().unwrap();
    
    // If we have a private key, generate a proper signature
    if let Some(key) = &auth.private_key {
        // Hash the request data
        let message = format!("{}:{}", seed, nonce);
        let message_hash = sha256_hash(message.as_bytes());
        
        // Sign using ethers crate or similar functionality
        // This is a placeholder - replace with actual signing code
        format!("0x{}", hex::encode([0u8; 65])) // Placeholder
    } else {
        // No private key available, return empty attestation
        String::from("")
    }
}

fn sha256_hash(data: &[u8]) -> [u8; 32] {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut output = [0u8; 32];
    output.copy_from_slice(&result);
    output
}

// Optional cleanup if used in FFI context
#[no_mangle]
pub extern "C" fn free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe {
            let _ = CString::from_raw(ptr);
        }
    }
}

// --- Serial Port Communication (for physical slot machines) ---
#[cfg(feature = "serial")]
mod serial_comm {
    use serialport::{SerialPort, SerialPortSettings};
    use std::time::Duration;
    use std::io::{Read, Write};
    use std::sync::Mutex;
    use once_cell::sync::Lazy;
    use log::{info, warn, error, debug};

    static SERIAL_PORT: Lazy<Mutex<Option<Box<dyn SerialPort>>>> = Lazy::new(|| {
        Mutex::new(None)
    });

    // CRC16 implementation for message integrity
    fn crc16(data: &[u8]) -> u16 {
        let mut crc = 0xFFFFu16;
        for &byte in data {
            crc ^= byte as u16;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
            }
        }
        crc
    }

    fn encode_message(cmd: u8, payload: &[u8]) -> Vec<u8> {
        let mut message = vec![0x02]; // STX
        message.push(cmd);
        message.push(payload.len() as u8);
        message.extend_from_slice(payload);
        let crc = crc16(&message[1..]);
        message.push((crc >> 8) as u8);
        message.push((crc & 0xFF) as u8);
        message.push(0x03); // ETX
        message
    }

    pub fn open_port(port_name: &str) -> Result<(), String> {
        let settings = serialport::SerialPortSettings {
            baud_rate: 115_200,
            data_bits: serialport::DataBits::Eight,
            flow_control: serialport::FlowControl::None,
            parity: serialport::Parity::None,
            stop_bits: serialport::StopBits::One,
            timeout: Duration::from_millis(1000),
        };

        debug!("Opening serial port: {}", port_name);
        match serialport::open_with_settings(port_name, &settings) {
            Ok(port) => {
                info!("Successfully opened serial port {}", port_name);
                let mut port_guard = SERIAL_PORT.lock().unwrap();
                *port_guard = Some(port);
                Ok(())
            },
            Err(e) => {
                error!("Failed to open serial port {}: {}", port_name, e);
                Err(format!("Failed to open serial port: {}", e))
            }
        }
    }

    pub fn send_randomness(reel_positions: &[u8]) -> Result<(), String> {
        let mut port_guard = SERIAL_PORT.lock().unwrap();
        if let Some(port) = port_guard.as_mut() {
            // Command 0x42: Send Reel Positions
            let message = encode_message(0x42, reel_positions);
            
            debug!("Sending {} bytes to serial port", message.len());
            match port.write_all(&message) {
                Ok(_) => {
                    // Read acknowledgment
                    let mut response = [0u8; 32];
                    match port.read(&mut response) {
                        Ok(bytes_read) if bytes_read > 0 => {
                            debug!("Received {} bytes from serial port", bytes_read);
                            if response[0] == 0x06 { // ACK
                                Ok(())
                            } else {
                                warn!("Invalid acknowledgment received: {:?}", &response[..bytes_read]);
                                Err(String::from("Invalid acknowledgment"))
                            }
                        },
                        Ok(_) => {
                            warn!("Empty response from serial port");
                            Err(String::from("Empty response"))
                        },
                        Err(e) => {
                            error!("Failed to read from serial port: {}", e);
                            Err(format!("Read error: {}", e))
                        }
                    }
                },
                Err(e) => {
                    error!("Failed to write to serial port: {}", e);
                    Err(format!("Write error: {}", e))
                }
            }
        } else {
            Err(String::from("Serial port not open"))
        }
    }
    
    // Enhanced healthcheck for physical machine connectivity
    pub fn check_connection() -> bool {
        let mut port_guard = SERIAL_PORT.lock().unwrap();
        if let Some(port) = port_guard.as_mut() {
            // Command 0x10: Ping/Status
            let message = encode_message(0x10, &[]);
            
            if let Err(e) = port.write_all(&message) {
                error!("Failed to write ping to serial port: {}", e);
                return false;
            }
            
            // Read response
            let mut response = [0u8; 32];
            match port.read(&mut response) {
                Ok(bytes_read) if bytes_read > 0 => {
                    debug!("Received {} bytes from serial ping", bytes_read);
                    // Check for valid status response
                    // Machine-specific protocol details would go here
                    true
                },
                _ => false
            }
        } else {
            false
        }
    }
}
// --- End Serial Port Communication ---

// New: Fallback local RNG implementation for when backend is unavailable
struct FallbackRng {
    rng: Mutex<StdRng>,
    fallback_count: AtomicU64,
}

impl FallbackRng {
    fn new() -> Self {
        // Initialize with a seed derived from system entropy
        let mut seed = [0u8; 32];
        thread_rng().fill_bytes(&mut seed);
        
        Self {
            rng: Mutex::new(StdRng::from_seed(seed)),
            fallback_count: AtomicU64::new(0),
        }
    }
    
    fn generate_spin(&self, reels: usize, symbols_per_reel: usize) -> SpinResponse {
        let mut reel_positions = Vec::with_capacity(reels);
        
        // Lock RNG for consistent generation
        let mut rng = self.rng.lock().unwrap();
        
        // Use QR_HASH_ITERATIONS similar to Entropy blockchain's own approach
        const QR_HASH_ITERATIONS: usize = 3;
        
        // Generate seed with multiple hash iterations for quantum resistance
        let mut seed = [0u8; 32];
        rng.fill_bytes(&mut seed);
        
        // Apply quantum-resistant hashing similar to Entropy blockchain
        let mut qr_hash = seed;
        for _ in 0..QR_HASH_ITERATIONS {
            qr_hash = sha256_hash(&qr_hash);
        }
        
        // Use the QR hash to generate positions
        for i in 0..reels {
            let position = (qr_hash[i % 32] as usize % symbols_per_reel) as u8;
            reel_positions.push(position);
        }
        
        // Increment fallback usage counter
        self.fallback_count.fetch_add(1, Ordering::SeqCst);
        
        // Return formatted response that attributes to Entropy fallback
        SpinResponse {
            reelPositions: reel_positions,
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
            seedUsed: format!("entropy-local-{}", hex::encode(qr_hash)),
            source: "entropy-fallback",
        }
    }
    
    fn usage_count(&self) -> u64 {
        self.fallback_count.load(Ordering::SeqCst)
    }
}

// Static instance of fallback RNG
static FALLBACK_RNG: Lazy<FallbackRng> = Lazy::new(|| {
    info!("Initializing fallback RNG for resilience");
    FallbackRng::new()
});

// New C ABI function: Get spin result with explicit parameters and fallback
#[no_mangle]
pub extern "C" fn generate_spin_with_fallback(
    reels: c_int,
    symbols_per_reel: c_int,
    signer_address: *const c_char,
    allow_fallback: c_int
) -> *const c_char {
    // Validate parameters
    let num_reels = if reels > 0 && reels <= 10 { reels as usize } else { 3 };
    let num_symbols = if symbols_per_reel > 0 && symbols_per_reel <= 100 { symbols_per_reel as usize } else { 10 };
    
    // Try primary randomness generation
    let result = request_random_spin(signer_address);
    
    // If primary method failed and fallback is allowed
    if result == 0 && allow_fallback != 0 {
        debug!("Primary randomness failed, using fallback RNG");
        
        // Generate from fallback RNG
        let response = FALLBACK_RNG.generate_spin(num_reels, num_symbols);
        
        // Store as last response
        let serialized = serde_json::to_string(&response).unwrap_or_default();
        *LAST_RESPONSE.lock().unwrap() = Some(serialized.clone());
        
        // Return the serialized result
        let c_string = CString::new(serialized).unwrap_or_else(|_| CString::new("{}").unwrap());
        return c_string.into_raw();
    }
    
    // Return either the primary result or empty object if both methods failed
    get_last_result()
}

// New: Get system status - useful for monitoring in high-volume scenarios
#[no_mangle]
pub extern "C" fn get_system_status() -> *const c_char {
    // Connection pool stats
    let (pool_size, created, reused) = HTTP_POOL.stats();
    
    // Cache stats
    let (cache_size, cache_capacity) = RESULTS_CACHE.stats();
    
    // Request stats
    let total = REQUESTS_TOTAL.load(Ordering::SeqCst);
    let success = REQUESTS_SUCCESS.load(Ordering::SeqCst);
    let failed = REQUESTS_FAILED.load(Ordering::SeqCst);
    let cached = REQUESTS_CACHED.load(Ordering::SeqCst);
    
    // Fallback usage
    let fallback_count = FALLBACK_RNG.usage_count();
    
    // Build status JSON
    let status = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "uptime": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        "requests": {
            "total": total,
            "success": success,
            "failed": failed,
            "cached": cached,
            "fallback": fallback_count,
        },
        "connections": {
            "current": pool_size,
            "capacity": CONNECTION_POOL_SIZE,
            "created": created,
            "reused": reused,
        },
        "cache": {
            "size": cache_size,
            "capacity": cache_capacity,
            "hit_rate": if total > 0 { (cached as f64 * 100.0) / total as f64 } else { 0.0 },
        },
        "workers": {
            "count": WORKER_THREADS,
            "queue_size": REQUEST_QUEUE_SIZE,
        }
    });
    
    // Convert to C string
    let serialized = status.to_string();
    let c_string = CString::new(serialized).unwrap_or_else(|_| CString::new("{}").unwrap());
    c_string.into_raw()
}

// New struct for Entropy blockchain API responses
#[derive(Debug, Deserialize)]
struct EntropyResponse {
    requestId: String,
    requestHash: String,
    results: Vec<u32>,
    timestamp: u64,
    blockNumber: u64,
    beaconOutput: String,
    signature: String,
}

// --- Build Comments ---
// Build with: cargo build --release
// Optional: Build for specific target: cargo build --release --target x86_64-pc-windows-gnu
//
// --- cbindgen Integration ---
// 1. Install cbindgen: cargo install cbindgen
// 2. Create cbindgen.toml in the project root (optional, for configuration)
// 3. Generate header: cbindgen --config cbindgen.toml --crate casino_sdk_bridge --output include/casino_sdk_bridge.h
//    (Adjust crate name and output path as needed)
// 4. Consider adding a build script (build.rs) to automate header generation during build.
// --- End cbindgen Integration ---
