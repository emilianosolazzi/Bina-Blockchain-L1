use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

#[derive(Clone)]
pub struct PeerList {
    peers: Arc<RwLock<HashSet<SocketAddr>>>,
    max_peers: usize,
}

impl PeerList {
    pub fn new(max_peers: usize) -> Self {
        Self {
            peers: Arc::new(RwLock::new(HashSet::new())),
            max_peers,
        }
    }

    pub fn add(&self, addr: SocketAddr) -> bool {
        let mut peers = self.peers.write().unwrap();
        if peers.len() >= self.max_peers && !peers.contains(&addr) {
            return false;
        }
        peers.insert(addr)
    }

    pub fn all(&self) -> Vec<SocketAddr> {
        self.peers.read().unwrap().iter().copied().collect()
    }

    pub fn count(&self) -> usize {
        self.peers.read().unwrap().len()
    }
}