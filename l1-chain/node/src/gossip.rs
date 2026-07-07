use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::envelope::{unix_secs, BinaMessage, BlockClaimEnvelope, PeerHelloEnvelope};
use crate::peers::PeerList;

const MESSAGE_MAX_AGE_SECS: u64 = 30;

pub struct SeenMessages {
    seen: Arc<RwLock<HashSet<String>>>,
    max_size: usize,
}

impl SeenMessages {
    pub fn new(max_size: usize) -> Self {
        Self {
            seen: Arc::new(RwLock::new(HashSet::new())),
            max_size,
        }
    }

    pub fn mark_seen(&self, message_id: &str) -> bool {
        let mut seen = self.seen.write().unwrap();
        if seen.contains(message_id) {
            return false;
        }
        if seen.len() >= self.max_size {
            let to_remove: Vec<String> = seen.iter().take(self.max_size / 2).cloned().collect();
            for id in to_remove {
                seen.remove(&id);
            }
        }
        seen.insert(message_id.to_string());
        true
    }
}

#[derive(Clone)]
pub struct Gossip {
    peers: Arc<PeerList>,
    seen: Arc<SeenMessages>,
    network: String,
    http_client: reqwest::Client,
}

impl Gossip {
    pub fn new(peers: Arc<PeerList>, network: impl Into<String>) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_millis(700))
            .build()
            .expect("failed to build gossip HTTP client");
        Self {
            peers,
            seen: Arc::new(SeenMessages::new(10_000)),
            network: network.into(),
            http_client,
        }
    }

    pub fn network(&self) -> &str {
        &self.network
    }

    pub fn peers(&self) -> Arc<PeerList> {
        Arc::clone(&self.peers)
    }

    pub async fn broadcast_claim(&self, envelope: BlockClaimEnvelope) {
        self.seen.mark_seen(&envelope.message_id);
        self.broadcast_message(BinaMessage::BlockClaim(envelope), None, 500)
            .await;
    }

    pub async fn relay_message(&self, message: BinaMessage, except: SocketAddr) {
        self.broadcast_message(message, Some(except), 250).await;
    }

    pub async fn handle_incoming(
        &self,
        message: BinaMessage,
        _from_peer: SocketAddr,
    ) -> Option<BinaMessage> {
        match message {
            BinaMessage::BlockClaim(mut envelope) => {
                if envelope.network != self.network {
                    eprintln!("[gossip] wrong network: {}", envelope.network);
                    return None;
                }
                if envelope.ttl == 0 || unix_secs() > envelope.sent_at_unix.saturating_add(MESSAGE_MAX_AGE_SECS) {
                    return None;
                }
                if !self.seen.mark_seen(&envelope.message_id) {
                    return None;
                }
                if let Err(e) = envelope.verify() {
                    eprintln!("[gossip] invalid block claim envelope: {e}");
                    return None;
                }
                envelope.ttl = envelope.ttl.saturating_sub(1);
                Some(BinaMessage::BlockClaim(envelope))
            }
            BinaMessage::PeerHello(hello) => {
                if hello.network != self.network {
                    return None;
                }
                if let Ok(addr) = hello.listen_addr.parse() {
                    self.peers.add(addr);
                }
                Some(BinaMessage::PeerHello(hello))
            }
            BinaMessage::PeerList(list) => {
                for addr in list.peers.iter().filter_map(|peer| peer.parse().ok()) {
                    self.peers.add(addr);
                }
                Some(BinaMessage::PeerList(list))
            }
            BinaMessage::Ping(ping) => Some(BinaMessage::Ping(ping)),
        }
    }

    pub async fn bootstrap(&self, listen_addr: &str, best_height: u64, best_hash: &str) {
        let seeds = self.peers.all();
        for seed in seeds {
            let hello = PeerHelloEnvelope {
                network: self.network.clone(),
                version: 1,
                best_height,
                best_hash: best_hash.to_string(),
                listen_addr: listen_addr.to_string(),
            };

            let hello_url = format!("http://{seed}/p2p/hello");
            let _ = self.http_client.post(&hello_url).json(&hello).send().await;

            let peers_url = format!("http://{seed}/p2p/peers");
            match self.http_client.get(&peers_url).send().await {
                Ok(resp) => match resp.json::<Vec<String>>().await {
                    Ok(peers) => {
                        for peer in peers.into_iter().filter_map(|peer| peer.parse().ok()) {
                            self.peers.add(peer);
                        }
                    }
                    Err(e) => eprintln!("[gossip] seed {seed} peer list decode error: {e}"),
                },
                Err(e) => eprintln!("[gossip] seed {seed} unavailable: {e}"),
            }
        }
    }

    async fn broadcast_message(&self, message: BinaMessage, except: Option<SocketAddr>, timeout_ms: u64) {
        let json = match serde_json::to_string(&message) {
            Ok(json) => json,
            Err(e) => {
                eprintln!("[gossip] serialize error: {e}");
                return;
            }
        };

        let handles: Vec<_> = self
            .peers
            .all()
            .into_iter()
            .filter(|peer| Some(*peer) != except)
            .map(|peer| {
                let client = self.http_client.clone();
                let json = json.clone();
                tokio::spawn(async move {
                    let url = format!("http://{peer}/p2p/message");
                    let result = tokio::time::timeout(
                        Duration::from_millis(timeout_ms),
                        client
                            .post(&url)
                            .header("Content-Type", "application/json")
                            .body(json)
                            .send(),
                    )
                    .await;
                    match result {
                        Ok(Ok(resp)) if resp.status().is_success() => {}
                        Ok(Ok(resp)) => eprintln!("[gossip] peer {peer} rejected: {}", resp.status()),
                        Ok(Err(e)) => eprintln!("[gossip] peer {peer} error: {e}"),
                        Err(_) => eprintln!("[gossip] peer {peer} timeout"),
                    }
                })
            })
            .collect();

        for handle in handles {
            let _ = handle.await;
        }
    }
}