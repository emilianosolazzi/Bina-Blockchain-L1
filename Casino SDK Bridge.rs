// Casino SDK Bridge - Native Layer for Randomness Integration
// Written in Rust with optional C ABI for legacy slot machine support

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use once_cell::sync::Lazy;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize}; // Added Serialize
use rand::RngCore; // Added for better entropy

static RNG_ENDPOINT: &str = "http://localhost:3000/api/v1/slot-spin";
static HTTP_CLIENT: Lazy<Client> = Lazy::new(Client::new);
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

// Modified function signature to accept optional auth details
#[no_mangle]
pub extern "C" fn request_random_spin(
    num_reels: c_int,
    symbols_per_reel: c_int,
    api_key: *const c_char,      // Optional API Key
    signature: *const c_char,   // Optional Signature
    signer_address: *const c_char // Optional Signer Address
) -> c_int {
    // Generate a better seed
    let seed = generate_entropy_seed();

    let request_body = serde_json::json!({
        "numReels": num_reels,
        "symbolsPerReel": symbols_per_reel,
        "seed": seed,
        // Only include signature/address in body if API expects them there
        // "signature": if !signature.is_null() { Some(unsafe { CStr::from_ptr(signature).to_string_lossy().into_owned() }) } else { None },
        // "signerAddress": if !signer_address.is_null() { Some(unsafe { CStr::from_ptr(signer_address).to_string_lossy().into_owned() }) } else { None },
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


    let response_result = request_builder.send();
    let mut last = LAST_RESPONSE.lock().unwrap(); // Lock mutex once

    match response_result {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                match resp.json::<SpinResponse>() {
                    Ok(body) => {
                        // Store successful result
                        let result_str = format!("{:?}", body.reelPositions);
                        *last = Some(result_str);
                        return 0; // Success
                    }
                    Err(e) => {
                        // Store JSON parsing error
                        *last = Some(format!("Error: Failed to parse success response - {}", e));
                        return -2; // JSON parsing error
                    }
                }
            } else {
                // Attempt to parse API error response
                let error_text = match resp.json::<ErrorResponse>() {
                    Ok(error_body) => format!("API Error {}: {} {}",
                        status.as_u16(),
                        error_body.error,
                        error_body.details.unwrap_or_default()
                    ),
                    Err(_) => format!("API Error {}: {}", status.as_u16(), status.canonical_reason().unwrap_or("Unknown Status")),
                };
                *last = Some(error_text);
                return -4; // API returned an error status
            }
        }
        Err(e) => {
            // Store request sending error
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

// Build with: cargo build --release --target x86_64-pc-windows-gnu
// Or use cbindgen to generate a header for C integration
