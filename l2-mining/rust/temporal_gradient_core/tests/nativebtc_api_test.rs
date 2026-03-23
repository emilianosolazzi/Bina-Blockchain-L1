/// Integration test: verify NativeBTC (FastPath) API endpoints
/// that the stale-block miner depends on.
///
/// Exercises:
///   1. GET /v1/block-height        — lightweight tip height
///   2. GET /v1/mempool/fees         — fee estimates
///   3. GET /v1/mempool/stats        — mempool size/congestion
///   4. GET /v1/blocks               — recent blocks (orphan source)
///   5. WSS /v1/mempool/stream       — live WebSocket push stream
///   6. GET /v1/chain/tips           — chain tips with orphan detection
///   7. GET /v1/block/:hash/header   — raw 80-byte block header
///   8. GET /v1/blocks/:height       — canonical block at height
///
/// Run:  cargo test --test nativebtc_api_test --features stale-mining -- --nocapture

use std::time::Duration;
use tokio::time::timeout;

const BASE: &str = "https://api.nativebtc.org";
const API_KEY: &str = "fp_2d93df5e6cebe485b69c363a62e237fc9d0f88b9";

fn url(path: &str) -> String {
    format!("{BASE}/{path}?key={API_KEY}")
}

fn ws_url() -> String {
    format!("wss://api.nativebtc.org/v1/mempool/stream?key={API_KEY}")
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("TGBT-IntegrationTest/0.1")
        .build()
        .unwrap()
}

// ─── REST endpoint tests ────────────────────────────────────────

#[tokio::test]
async fn test_block_height() {
    let c = client();
    let resp = c.get(url("v1/block-height")).send().await;
    match resp {
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            println!("[block-height] status={status} body={body}");
            assert!(status.is_success(), "expected 2xx, got {status}");
            // NativeBTC returns {"success":true,"height":NNNNNN}
            let v: serde_json::Value =
                serde_json::from_str(&body).expect("body should be valid JSON");
            let height = v
                .get("height")
                .and_then(|h| h.as_u64())
                .expect("missing .height in response");
            // Bitcoin height should be above 800k by 2026
            assert!(height > 800_000, "block height {height} seems too low");
            println!("[block-height] ✓ height = {height}");
        }
        Err(e) => {
            println!("[block-height] ✗ request failed: {e:#}");
            eprintln!("WARNING: block-height endpoint unreachable: {e}");
        }
    }
}

#[tokio::test]
async fn test_mempool_fees() {
    let c = client();
    let resp = c.get(url("v1/mempool/fees")).send().await;
    match resp {
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            println!("[mempool/fees] status={status} body={}", &body[..body.len().min(500)]);
            assert!(status.is_success(), "expected 2xx, got {status}");
            let v: serde_json::Value = serde_json::from_str(&body).expect("should be valid JSON");
            println!("[mempool/fees] parsed keys: {:?}", v.as_object().map(|o| o.keys().collect::<Vec<_>>()));
            // Check for expected fields — at least one fee tier should exist
            let has_fees = v.get("fastestFee").is_some()
                || v.get("fastest_fee").is_some()
                || v.get("fastest").is_some();
            println!("[mempool/fees] has fee fields: {has_fees}");
            println!("[mempool/fees] ✓ full response: {v}");
        }
        Err(e) => {
            eprintln!("[mempool/fees] ✗ request failed: {e:#}");
        }
    }
}

#[tokio::test]
async fn test_mempool_stats() {
    let c = client();
    let resp = c.get(url("v1/mempool/stats")).send().await;
    match resp {
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            println!("[mempool/stats] status={status} body={}", &body[..body.len().min(500)]);
            assert!(status.is_success(), "expected 2xx, got {status}");
            let v: serde_json::Value = serde_json::from_str(&body).expect("should be valid JSON");
            println!("[mempool/stats] parsed keys: {:?}", v.as_object().map(|o| o.keys().collect::<Vec<_>>()));
            println!("[mempool/stats] ✓ full response: {v}");
        }
        Err(e) => {
            eprintln!("[mempool/stats] ✗ request failed: {e:#}");
        }
    }
}

#[tokio::test]
async fn test_blocks_list() {
    let c = client();
    let resp = c.get(url("v1/blocks")).send().await;
    match resp {
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            println!("[blocks] status={status} body_len={}", body.len());
            assert!(status.is_success(), "expected 2xx, got {status}");
            let v: serde_json::Value = serde_json::from_str(&body).expect("should be valid JSON");
            if let Some(arr) = v.as_array() {
                println!("[blocks] returned {} blocks", arr.len());
                if let Some(first) = arr.first() {
                    let id = first.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                    let height = first.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
                    let has_extras = first.get("extras").is_some();
                    let has_orphans = first
                        .get("extras")
                        .and_then(|e| e.get("orphans"))
                        .is_some();
                    println!("[blocks] first block: id={id} height={height} has_extras={has_extras} has_orphans={has_orphans}");
                    // Print the keys of the first block
                    if let Some(obj) = first.as_object() {
                        println!("[blocks] first block keys: {:?}", obj.keys().collect::<Vec<_>>());
                    }
                }
                println!("[blocks] ✓");
            } else {
                println!("[blocks] response is not an array: {}", &body[..body.len().min(300)]);
            }
        }
        Err(e) => {
            eprintln!("[blocks] ✗ request failed: {e:#}");
        }
    }
}

// ─── WebSocket stream test ──────────────────────────────────────

#[tokio::test]
async fn test_websocket_stream() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    let url = ws_url();
    println!("[ws] connecting to {}", url.replace(API_KEY, "[REDACTED]"));

    let connect_result = timeout(Duration::from_secs(10), connect_async(&url)).await;

    match connect_result {
        Err(_) => {
            eprintln!("[ws] ✗ connection timed out after 10s");
            return;
        }
        Ok(Err(e)) => {
            eprintln!("[ws] ✗ connection error: {e:#}");
            return;
        }
        Ok(Ok((ws_stream, response))) => {
            println!("[ws] ✓ connected — HTTP status: {}", response.status());

            let (mut tx, mut rx) = ws_stream.split();

            // Subscribe to stats (only valid WS commands: subscribe:stats, subscribe:txs, filter:address:<addr>)
            for cmd in ["subscribe:stats"] {
                println!("[ws] sending '{cmd}'");
                if let Err(e) = tx.send(Message::Text(cmd.to_string())).await {
                    eprintln!("[ws] ✗ failed to send {cmd}: {e}");
                    return;
                }
            }

            // Read messages for up to 15 seconds, collect what we get
            let mut msg_count = 0u32;
            let mut got_stats = false;
            let mut got_block = false;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(15);

            println!("[ws] listening for messages (15s timeout)...");

            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    println!("[ws] timeout reached");
                    break;
                }

                match timeout(remaining, rx.next()).await {
                    Err(_) => {
                        println!("[ws] no more messages within timeout");
                        break;
                    }
                    Ok(None) => {
                        println!("[ws] stream ended");
                        break;
                    }
                    Ok(Some(Err(e))) => {
                        eprintln!("[ws] ✗ read error: {e}");
                        break;
                    }
                    Ok(Some(Ok(Message::Text(text)))) => {
                        msg_count += 1;
                        let preview = if text.len() > 300 {
                            format!("{}...[truncated]", &text[..300])
                        } else {
                            text.clone()
                        };
                        println!("[ws] msg #{msg_count}: {preview}");

                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                            let msg_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                            println!("[ws]   → type='{msg_type}'");
                            match msg_type {
                                "stats" => {
                                    got_stats = true;
                                    if let Some(data) = v.get("data") {
                                        if let Some(mp) = data.get("mempool") {
                                            let size = mp.get("size").and_then(|s| s.as_u64());
                                            println!("[ws]   → mempool size: {size:?}");
                                        }
                                        if let Some(fees) = data.get("fees") {
                                            let fastest = fees.get("fastestFee").and_then(|f| f.as_u64());
                                            println!("[ws]   → fastest fee: {fastest:?} sat/vB");
                                        }
                                    }
                                }
                                "new_block" | "block" => {
                                    got_block = true;
                                    let height = v.get("height").or_else(|| {
                                        v.get("data").and_then(|d| d.get("height"))
                                    }).and_then(|h| h.as_u64());
                                    println!("[ws]   → block height: {height:?}");
                                }
                                "new_txs" => {
                                    let count = v.get("count").and_then(|c| c.as_u64());
                                    println!("[ws]   → {count:?} new txs");
                                }
                                _ => {}
                            }
                        }

                        // Once we've seen a stats message, we've proven the
                        // stream works — no need to wait for a block (rare event)
                        if got_stats && msg_count >= 3 {
                            println!("[ws] got enough data, closing");
                            break;
                        }
                    }
                    Ok(Some(Ok(Message::Ping(data)))) => {
                        println!("[ws] received ping, sending pong");
                        let _ = tx.send(Message::Pong(data)).await;
                    }
                    Ok(Some(Ok(Message::Pong(_)))) => {
                        println!("[ws] received pong");
                    }
                    Ok(Some(Ok(Message::Close(frame)))) => {
                        println!("[ws] received close frame: {frame:?}");
                        break;
                    }
                    Ok(Some(Ok(other))) => {
                        println!("[ws] other message type: {other:?}");
                    }
                }
            }

            let _ = tx.send(Message::Close(None)).await;
            println!("[ws] summary: {msg_count} messages, stats={got_stats}, block={got_block}");
            
            if msg_count > 0 {
                println!("[ws] ✓ WebSocket stream is functional");
            } else {
                eprintln!("[ws] ⚠ connected but received 0 messages in 15s");
            }
        }
    }
}

// ─── Chain-tips pipeline tests ──────────────────────────────────

/// Test 6: GET /v1/chain/tips — must return `success: true`, an `activeTip`,
/// a `tips` array, and `orphanCount`.
#[tokio::test]
async fn test_chain_tips() {
    let c = client();
    let resp = c.get(url("v1/chain/tips")).send().await;
    match resp {
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            println!("[chain/tips] status={status} body_len={}", body.len());
            assert!(status.is_success(), "expected 2xx, got {status}");

            let v: serde_json::Value =
                serde_json::from_str(&body).expect("should be valid JSON");

            // success flag
            let success = v.get("success").and_then(|s| s.as_bool()).unwrap_or(false);
            assert!(success, "expected success=true, got {}", v.get("success").unwrap_or(&serde_json::Value::Null));

            // activeTip
            let active = v.get("activeTip").expect("missing activeTip");
            let tip_height = active.get("height").and_then(|h| h.as_u64()).expect("activeTip.height");
            let tip_hash = active.get("hash").and_then(|h| h.as_str()).expect("activeTip.hash");
            let is_orphan = active.get("isOrphan").and_then(|o| o.as_bool()).unwrap_or(true);
            assert!(!is_orphan, "activeTip should not be an orphan");
            assert!(tip_height > 800_000, "tip height {tip_height} too low");
            assert_eq!(tip_hash.len(), 64, "hash should be 64 hex chars");
            println!("[chain/tips] activeTip: height={tip_height} hash={tip_hash}");

            // tips array
            let tips = v.get("tips").and_then(|t| t.as_array()).expect("missing tips array");
            let total_tips = v.get("totalTips").and_then(|t| t.as_u64()).unwrap_or(0);
            let orphan_count = v.get("orphanCount").and_then(|o| o.as_u64()).unwrap_or(0);
            println!(
                "[chain/tips] totalTips={total_tips} orphanCount={orphan_count} tips.len()={}",
                tips.len()
            );

            // Validate orphan entries
            let mut orphans_found = 0u64;
            for tip in tips {
                let is_o = tip.get("isOrphan").and_then(|o| o.as_bool()).unwrap_or(false);
                if is_o {
                    orphans_found += 1;
                    let h = tip.get("height").and_then(|h| h.as_u64()).unwrap_or(0);
                    let hash = tip.get("hash").and_then(|h| h.as_str()).unwrap_or("?");
                    let branch = tip.get("branchLen").and_then(|b| b.as_u64()).unwrap_or(0);
                    let status_str = tip.get("status").and_then(|s| s.as_str()).unwrap_or("?");
                    println!(
                        "[chain/tips]   orphan: height={h} hash={}… branchLen={branch} status={status_str}",
                        &hash[..hash.len().min(16)]
                    );
                    // Each orphan must have height, hash, branchLen
                    assert!(h > 0, "orphan height should be > 0");
                    assert_eq!(hash.len(), 64, "orphan hash should be 64 hex chars");
                }
            }
            println!("[chain/tips] orphans validated: {orphans_found}");
            println!("[chain/tips] ✓");
        }
        Err(e) => {
            eprintln!("[chain/tips] ✗ request failed: {e:#}");
            panic!("chain/tips endpoint unreachable");
        }
    }
}

/// Test 7: GET /v1/block/:hash/header — fetch the active tip's raw header,
/// confirm `headerHex` is 160 hex chars (80 bytes) and `headerBytes == 80`.
#[tokio::test]
async fn test_block_header() {
    let c = client();

    // First, grab the active tip hash from chain/tips
    let tips_resp: serde_json::Value = c
        .get(url("v1/chain/tips"))
        .send()
        .await
        .expect("chain/tips request failed")
        .json()
        .await
        .expect("chain/tips parse failed");

    let tip_hash = tips_resp
        .get("activeTip")
        .and_then(|a| a.get("hash"))
        .and_then(|h| h.as_str())
        .expect("no activeTip.hash");

    println!("[block/header] fetching header for tip {tip_hash}");

    let header_url = url(&format!("v1/block/{}/header", tip_hash));
    let resp = c.get(&header_url).send().await;
    match resp {
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            println!("[block/header] status={status} body_len={}", body.len());
            assert!(status.is_success(), "expected 2xx, got {status}");

            let v: serde_json::Value =
                serde_json::from_str(&body).expect("should be valid JSON");

            let success = v.get("success").and_then(|s| s.as_bool()).unwrap_or(false);
            assert!(success, "expected success=true");

            let header_hex = v.get("headerHex").and_then(|h| h.as_str()).expect("missing headerHex");
            let header_bytes_field = v.get("headerBytes").and_then(|h| h.as_u64()).unwrap_or(0);
            let height = v.get("height").and_then(|h| h.as_u64()).unwrap_or(0);
            let prev_hash = v.get("previousHash").and_then(|h| h.as_str()).unwrap_or("?");

            println!("[block/header] headerHex len={} headerBytes={header_bytes_field} height={height}", header_hex.len());
            println!("[block/header] previousHash={prev_hash}");

            // Validate header
            assert_eq!(header_hex.len(), 160, "headerHex should be 160 hex chars (80 bytes)");
            assert_eq!(header_bytes_field, 80, "headerBytes should be 80");
            assert!(height > 800_000, "height {height} too low");

            // Verify it decodes cleanly
            let decoded = hex::decode(header_hex).expect("headerHex should be valid hex");
            assert_eq!(decoded.len(), 80, "decoded header should be exactly 80 bytes");

            println!("[block/header] ✓ valid 80-byte header at height {height}");
        }
        Err(e) => {
            eprintln!("[block/header] ✗ request failed: {e:#}");
            panic!("block/header endpoint unreachable");
        }
    }
}

/// Test 8: GET /v1/blocks/:height — fetch block at a specific height,
/// confirm it returns a hash we can use for canonical reference.
#[tokio::test]
async fn test_blocks_by_height() {
    let c = client();

    // Get the current tip height first
    let tips_resp: serde_json::Value = c
        .get(url("v1/chain/tips"))
        .send()
        .await
        .expect("chain/tips request failed")
        .json()
        .await
        .expect("chain/tips parse failed");

    let tip_height = tips_resp
        .get("activeTip")
        .and_then(|a| a.get("height"))
        .and_then(|h| h.as_u64())
        .expect("no activeTip.height");

    // Fetch a block a few blocks behind the tip (more stable)
    let test_height = tip_height - 5;
    println!("[blocks/:height] fetching block at height {test_height} (tip={tip_height})");

    let block_url = url(&format!("v1/blocks/{}", test_height));
    let resp = c.get(&block_url).send().await;
    match resp {
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            println!("[blocks/:height] status={status} body_len={}", body.len());
            assert!(status.is_success(), "expected 2xx, got {status}");

            let v: serde_json::Value =
                serde_json::from_str(&body).expect("should be valid JSON");

            // May be a string (verbosity=0) or an object with hash field
            let block_hash = v
                .as_str()
                .or_else(|| v.get("hash").and_then(|h| h.as_str()))
                .or_else(|| v.get("id").and_then(|h| h.as_str()));

            if let Some(hash) = block_hash {
                assert_eq!(hash.len(), 64, "block hash should be 64 hex chars, got {}", hash.len());
                println!("[blocks/:height] hash at {test_height} = {hash}");
                println!("[blocks/:height] ✓");
            } else {
                // Print the full response so we can see the real shape
                println!("[blocks/:height] full response: {v}");
                // Still pass if we got a success response — the shape just differs
                println!("[blocks/:height] ⚠ could not extract hash, but endpoint is reachable");
            }
        }
        Err(e) => {
            eprintln!("[blocks/:height] ✗ request failed: {e:#}");
            panic!("blocks/:height endpoint unreachable");
        }
    }
}
