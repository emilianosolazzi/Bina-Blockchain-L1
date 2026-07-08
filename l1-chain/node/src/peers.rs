use rand::seq::IteratorRandom;
use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, RwLock};

#[derive(Clone)]
pub struct PeerList {
    peers: Arc<RwLock<HashSet<SocketAddr>>>,
    max_peers: usize,
    allow_loopback: bool,
    allow_private: bool,
}

impl PeerList {
    pub fn new(max_peers: usize) -> Self {
        Self {
            peers: Arc::new(RwLock::new(HashSet::new())),
            max_peers,
            allow_loopback: cfg!(debug_assertions),
            allow_private: cfg!(debug_assertions),
        }
    }

    pub fn with_policy(max_peers: usize, allow_loopback: bool, allow_private: bool) -> Self {
        Self {
            peers: Arc::new(RwLock::new(HashSet::new())),
            max_peers,
            allow_loopback,
            allow_private,
        }
    }

    pub fn add(&self, addr: SocketAddr) -> bool {
        if !self.is_allowed_addr(&addr) {
            return false;
        }

        let mut peers = self.peers.write().unwrap_or_else(|e| e.into_inner());

        if peers.len() >= self.max_peers && !peers.contains(&addr) {
            return false;
        }

        peers.insert(addr)
    }

    pub fn remove(&self, addr: &SocketAddr) -> bool {
        self.peers
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(addr)
    }

    pub fn contains(&self, addr: &SocketAddr) -> bool {
        self.peers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains(addr)
    }

    pub fn all(&self) -> Vec<SocketAddr> {
        self.peers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .copied()
            .collect()
    }

    pub fn sample(&self, n: usize) -> Vec<SocketAddr> {
        let mut rng = rand::thread_rng();
        self.peers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .copied()
            .choose_multiple(&mut rng, n)
    }

    pub fn count(&self) -> usize {
        self.peers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    fn is_allowed_addr(&self, addr: &SocketAddr) -> bool {
        if addr.port() == 0 {
            return false;
        }

        match addr.ip() {
            IpAddr::V4(ip) => {
                if ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() {
                    return false;
                }
                if ip.is_loopback() && !self.allow_loopback {
                    return false;
                }
                if is_private_v4(&ip) && !self.allow_private {
                    return false;
                }
                true
            }
            IpAddr::V6(ip) => {
                if ip.is_unspecified() || ip.is_multicast() {
                    return false;
                }
                if ip.is_loopback() && !self.allow_loopback {
                    return false;
                }
                true
            }
        }
    }
}

fn is_private_v4(ip: &std::net::Ipv4Addr) -> bool {
    ip.is_private() || ip.is_link_local()
}