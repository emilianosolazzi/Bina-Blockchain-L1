// Casino SDK Bridge - Native Layer for Randomness Integration
// Written in Rust with optional C ABI for legacy slot machine support

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH, Duration}; // Added Duration
use std::thread; // Added for sleep
use once_cell::sync::Lazy;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use rand::RngCore;
use log::{info, warn, error, debug, LevelFilter}; // Added logging macros
use env_logger::Builder; // Added logger builder

// --- Configuration ---
static RNG_ENDPOINT: &str = "http://localhost:3000/api/v1/slot-spin";
const MAX_RETRIES: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_millis(500);
// --- End Configuration ---

static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
    // Initialize logger (call this early, ideally once)
    init_logger();
    Client::new()
});
static LAST_RESPONSE: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

#[derive(Debug, Deserialize)]
struct SpinResponse {
    reelPositions: Vec<u8>,
    timestamp: u64,
    seedUsed: String,
    source: String,
    // Optional: Add requestId if the API returns it
    // requestId: Option<String>,
}

// Optional: Structure for API error responses
#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: String,
    details: Option<String>,
}

// --- Logger Initialization ---
fn init_logger() {
    let mut builder = Builder::from_default_env();
    // Set default log level if RUST_LOG is not set
    builder.filter_level(LevelFilter::Info);
    // Optionally configure log format, target (e.g., file), etc.
    // builder.target(env_logger::Target::Stdout);
    builder.init(); // Initialize the logger
    info!("Casino SDK Bridge Logger Initialized.");
}
// --- End Logger Initialization ---


// Modified function signature to accept optional auth details
#[no_mangle]
pub extern "C" fn request_random_spin(
    num_reels: c_int,
    symbols_per_reel: c_int,
    api_key: *const c_char,      // Optional API Key
    signature: *const c_char,   // Optional Signature
    signer_address: *const c_char // Optional Signer Address
) -> c_int {
    info!("Received spin request: reels={}, symbols={}", num_reels, symbols_per_reel);
    // Generate a better seed
    let seed = generate_entropy_seed();
    debug!("Generated seed: {}", seed);

    let request_body = serde_json::json!({
        "numReels": num_reels,
        "symbolsPerReel": symbols_per_reel,
        "seed": seed,
    });

    let mut request_builder = HTTP_CLIENT.post(RNG_ENDPOINT).json(&request_body);

    // --- Add Headers for Auth/Signature ---
    // API Key Header
    if !api_key.is_null() {
        if let Ok(key_str) = unsafe { CStr::from_ptr(api_key).to_str() } {
            request_builder = request_builder.header("x-api-key", key_str);
        } else {
             // Handle invalid UTF-8 in key if necessary
             let mut last = LAST_RESPONSE.lock().unwrap();
             *last = Some("Error: Invalid API Key encoding".to_string());
             return -3; // Indicate bad input parameter
        }
    }
    // Signature Headers (adjust header names based on API spec)
    if !signature.is_null() && !signer_address.is_null() {
         if let (Ok(sig_str), Ok(addr_str)) = (
             unsafe { CStr::from_ptr(signature).to_str() },
             unsafe { CStr::from_ptr(signer_address).to_str() }
         ) {
            request_builder = request_builder.header("x-signature", sig_str);
            request_builder = request_builder.header("x-signer-address", addr_str);
         } else {
             let mut last = LAST_RESPONSE.lock().unwrap();
             *last = Some("Error: Invalid Signature/Address encoding".to_string());
             return -3; // Indicate bad input parameter
         }
    }
    // --- End Add Headers ---

    // --- Retry Logic ---
    let mut attempts = 0;
    let response_result = loop {
        attempts += 1;
        debug!("Attempt {} to send request to {}", attempts, RNG_ENDPOINT);

        // Clone the builder before sending, as send consumes it
        let current_request = request_builder.try_clone().expect("Failed to clone request builder");

        match current_request.send() {
            Ok(resp) => {
                // Check if status indicates a potentially retryable server error (e.g., 5xx)
                if resp.status().is_server_error() && attempts < MAX_RETRIES {
                    warn!("Server error ({}), retrying after {:?}... (Attempt {}/{})", resp.status(), RETRY_DELAY, attempts, MAX_RETRIES);
                    thread::sleep(RETRY_DELAY);
                    continue; // Retry the loop
                }
                // If not a retryable server error or retries exhausted, break with the result
                break Ok(resp);
            }
            Err(e) => {
                // Check if the error is potentially recoverable (e.g., network timeout)
                if e.is_timeout() || e.is_connect() || e.is_request() { // Check specific error kinds
                    if attempts < MAX_RETRIES {
                        warn!("Request failed ({}), retrying after {:?}... (Attempt {}/{})", e, RETRY_DELAY, attempts, MAX_RETRIES);
                        thread::sleep(RETRY_DELAY);
                        continue; // Retry the loop
                    } else {
                        error!("Request failed after {} attempts: {}", attempts, e);
                        break Err(e); // Max retries reached, break with error
                    }
                } else {
                     error!("Non-retryable request error: {}", e);
                     break Err(e); // Non-recoverable error, break immediately
                }
            }
        }
    };
    // --- End Retry Logic ---


    let mut last = LAST_RESPONSE.lock().unwrap(); // Lock mutex once

    match response_result {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                match resp.json::<SpinResponse>() {
                    Ok(body) => {
                        info!("Successfully received spin result: {:?}", body.reelPositions);
                        let result_str = format!("{:?}", body.reelPositions);
                        *last = Some(result_str);
                        return 0; // Success
                    }
                    Err(e) => {
                        error!("Failed to parse success response JSON: {}", e);
                        *last = Some(format!("Error: Failed to parse success response - {}", e));
                        return -2; // JSON parsing error
                    }
                }
            } else {
                // Attempt to parse API error response
                let error_text = match resp.text() { // Get text first for better error logging
                    Ok(text) => {
                        match serde_json::from_str::<ErrorResponse>(&text) {
                            Ok(error_body) => format!("API Error {}: {} {}",
                                status.as_u16(),
                                error_body.error,
                                error_body.details.unwrap_or_default()
                            ),
                            Err(_) => format!("API Error {}: {}", status.as_u16(), text), // Use raw text if JSON parse fails
                        }
                    },
                    Err(_) => format!("API Error {}: {}", status.as_u16(), status.canonical_reason().unwrap_or("Unknown Status")),
                };
                error!("API returned error: {}", error_text);
                *last = Some(error_text);
                return -4; // API returned an error status
            }
        }
        Err(e) => {
            // Error already logged during retry logic
            *last = Some(format!("Error: Request failed - {}", e));
            return -1; // Request failed (network issue, etc.)
        }
    }
}

#[no_mangle]
pub extern "C" fn get_last_result() -> *const c_char {
    let last = LAST_RESPONSE.lock().unwrap();
    match &*last {
        // Return success or error message stored previously
        Some(s) => CString::new(s.clone()).unwrap().into_raw(),
        None => CString::new("No result yet").unwrap().into_raw(), // Changed default message
    }
    // IMPORTANT: The caller MUST call free_string on the returned pointer
    //            to avoid memory leaks.
}

// Use cryptographic RNG for better entropy
fn generate_entropy_seed() -> String {
    let mut bytes = [0u8; 16]; // 128 bits of entropy
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes) // Return as hex string
}

// Optional cleanup if used in FFI context
#[no_mangle]
pub extern "C" fn free_string(ptr: *mut c_char) {
    // Ensure this function is called by the C code
    // to release the memory allocated by CString::into_raw().
    if ptr.is_null() { return; }
    unsafe { CString::from_raw(ptr); } // Takes ownership and drops, freeing memory
}

// --- Serial Port Communication (Conceptual - Requires 'serial' feature) ---
#[cfg(feature = "serial")]
mod serial_comm {
    use super::*; // Import necessary items from parent module
    use std::io::{Read, Write}; // For serial port read/write

    // Define a simple serial protocol (example)
    // Request: "SPIN:<reels>:<symbols>:<seed_hex>\n"
    // Response Success: "OK:<pos1>,<pos2>,...\n"
    // Response Error: "ERR:<message>\n"

    #[no_mangle]
    pub extern "C" fn request_random_spin_serial(
        port_name: *const c_char,
        baud_rate: u32,
        num_reels: c_int,
        symbols_per_reel: c_int
        // Note: Authentication via serial is complex and not shown here
    ) -> c_int {
        info!("Received serial spin request: port={:?}, baud={}, reels={}, symbols={}", port_name, baud_rate, num_reels, symbols_per_reel);

        let port_name_str = match unsafe { CStr::from_ptr(port_name).to_str() } {
            Ok(s) => s,
            Err(_) => {
                error!("Invalid port name encoding");
                let mut last = LAST_RESPONSE.lock().unwrap();
                *last = Some("Error: Invalid port name encoding".to_string());
                return -3;
            }
        };

        let seed = generate_entropy_seed(); // Generate seed locally

        // --- Open Serial Port ---
        let mut port = match serialport::new(port_name_str, baud_rate)
            .timeout(Duration::from_secs(5)) // Set read/write timeout
            .open() {
            Ok(p) => p,
            Err(e) => {
                error!("Failed to open serial port '{}': {}", port_name_str, e);
                let mut last = LAST_RESPONSE.lock().unwrap();
                *last = Some(format!("Error: Failed to open port - {}", e));
                return -5; // Specific error code for serial port failure
            }
        };
        info!("Serial port '{}' opened successfully.", port_name_str);

        // --- Send Request ---
        let request_str = format!("SPIN:{}:{}:{}\n", num_reels, symbols_per_reel, seed);
        debug!("Sending serial request: {}", request_str.trim());
        if let Err(e) = port.write_all(request_str.as_bytes()) {
            error!("Failed to write to serial port: {}", e);
            let mut last = LAST_RESPONSE.lock().unwrap();
            *last = Some(format!("Error: Failed to write to port - {}", e));
            return -5;
        }

        // --- Read Response ---
        let mut response_buf = Vec::new();
        let mut byte_buf = [0u8; 1];
        loop {
            match port.read(&mut byte_buf) {
                Ok(1) => {
                    if byte_buf[0] == b'\n' {
                        break; // End of response line
                    }
                    response_buf.push(byte_buf[0]);
                    if response_buf.len() > 1024 { // Prevent buffer overflow
                         error!("Serial response too long");
                         let mut last = LAST_RESPONSE.lock().unwrap();
                         *last = Some("Error: Serial response too long".to_string());
                         return -5;
                    }
                }
                Ok(0) => { // Should not happen with timeout?
                    warn!("Serial read returned 0 bytes.");
                    break;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    error!("Serial read timed out");
                    let mut last = LAST_RESPONSE.lock().unwrap();
                    *last = Some("Error: Serial read timed out".to_string());
                    return -5;
                }
                Err(e) => {
                    error!("Failed to read from serial port: {}", e);
                    let mut last = LAST_RESPONSE.lock().unwrap();
                    *last = Some(format!("Error: Failed to read from port - {}", e));
                    return -5;
                }
            }
        }

        let response_str = String::from_utf8_lossy(&response_buf);
        debug!("Received serial response: {}", response_str);

        // --- Parse Response ---
        let mut last = LAST_RESPONSE.lock().unwrap();
        if response_str.starts_with("OK:") {
            let positions_str = response_str.trim_start_matches("OK:");
            // Basic parsing, assumes comma-separated u8 values
            let positions: Result<Vec<u8>, _> = positions_str
                .split(',')
                .map(|s| s.trim().parse::<u8>())
                .collect();

            match positions {
                Ok(pos) => {
                    info!("Successfully parsed serial spin result: {:?}", pos);
                    *last = Some(format!("{:?}", pos));
                    return 0; // Success
                }
                Err(e) => {
                    error!("Failed to parse serial OK response positions: {}", e);
                    *last = Some(format!("Error: Failed to parse serial positions - {}", e));
                    return -2; // Parsing error
                }
            }
        } else if response_str.starts_with("ERR:") {
            let error_msg = response_str.trim_start_matches("ERR:").trim();
            error!("Received serial error: {}", error_msg);
            *last = Some(format!("Serial Error: {}", error_msg));
            return -4; // Error reported by serial device
        } else {
            error!("Unrecognized serial response format: {}", response_str);
            *last = Some(format!("Error: Unrecognized serial response - {}", response_str));
            return -2; // Parsing/Format error
        }
    }
}
// --- End Serial Port Communication ---


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
