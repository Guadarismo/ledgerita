#![allow(dead_code)]
use std::collections::HashMap;
use std::time::Duration;

use futures::StreamExt;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::tcp;
use libp2p::yamux;
use libp2p::{Multiaddr, identity, noise};

use ledgerita_core::network::{NetworkId, RawCodec};

fn is_loopback(addr: &Multiaddr) -> bool {
    let s = addr.to_string();
    s.contains("127.0.0.1") || s.contains("::1")
}

#[derive(NetworkBehaviour)]
pub struct TestBehaviour {
    pub rr: request_response::Behaviour<RawCodec>,
}

pub fn build_test_swarm(
    key: identity::Keypair,
    network: NetworkId,
) -> libp2p::Swarm<TestBehaviour> {
    libp2p::SwarmBuilder::with_existing_identity(key)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .unwrap()
        .with_behaviour(|_| {
            let protocols = vec![(network.protocol_id(), ProtocolSupport::Full)];
            TestBehaviour {
                rr: request_response::Behaviour::with_codec(
                    RawCodec,
                    protocols,
                    request_response::Config::default()
                        .with_request_timeout(Duration::from_secs(10)),
                ),
            }
        })
        .unwrap()
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(60)))
        .build()
}

pub async fn wait_for_listen_addr(swarm: &mut libp2p::Swarm<TestBehaviour>) -> Multiaddr {
    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                if is_loopback(&address) {
                    return address;
                }
            }
            _ => {}
        }
    }
}

pub async fn wait_for_listen_addrs(swarm: &mut libp2p::Swarm<TestBehaviour>) -> Vec<Multiaddr> {
    let mut addrs: Vec<Multiaddr> = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_millis(150), swarm.select_next_some()).await {
            Ok(SwarmEvent::NewListenAddr { address, .. }) => {
                if is_loopback(&address) && !addrs.contains(&address) {
                    addrs.push(address);
                }
            }
            Ok(_) => {}
            Err(_) => {
                if !addrs.is_empty() {
                    return addrs;
                }
            }
        }
    }
}

static TEST_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub struct TempTestDirs {
    pub a: std::path::PathBuf,
    pub b: std::path::PathBuf,
    pub c: std::path::PathBuf,
    pub labels: HashMap<String, std::path::PathBuf>,
}

impl TempTestDirs {
    pub fn new() -> Self {
        Self::with_labels(["a", "b", "c"])
    }

    pub fn with_labels<S, I>(labels: I) -> Self
    where
        S: Into<String>,
        I: IntoIterator<Item = S>,
    {
        let test_id = format!("{}_{}", std::process::id(), TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        let mut map = HashMap::new();
        for label in labels {
            let name: String = label.into();
            let dir = std::env::temp_dir().join(format!("lgr_{}_{}", name, test_id));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            map.insert(name, dir);
        }
        for base in ["a", "b", "c"] {
            map.entry(base.to_string()).or_insert_with(|| {
                let dir = std::env::temp_dir().join(format!("lgr_{}_{}", base, test_id));
                let _ = std::fs::remove_dir_all(&dir);
                std::fs::create_dir_all(&dir).unwrap();
                dir
            });
        }
        let a = map["a"].clone();
        let b = map["b"].clone();
        let c = map["c"].clone();
        TempTestDirs {
            a,
            b,
            c,
            labels: map,
        }
    }

    pub fn get(&self, label: &str) -> &std::path::PathBuf {
        &self.labels[label]
    }
}

impl Default for TempTestDirs {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TempTestDirs {
    fn drop(&mut self) {
        for dir in self.labels.values() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

pub fn alloc_loopback_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

pub async fn wait_tcp_port_open(port: u16) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "puerto P2P 127.0.0.1:{port} no abrió a tiempo"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub fn make_test_vset_2_de_3(
    seed_byte_base: u8,
) -> (
    Vec<ed25519_dalek::SigningKey>,
    ledgerita_core::quorum::ValidatorSet,
) {
    let keys = vec![
        ed25519_dalek::SigningKey::from_bytes(&[seed_byte_base; 32]),
        ed25519_dalek::SigningKey::from_bytes(&[seed_byte_base.wrapping_add(1); 32]),
        ed25519_dalek::SigningKey::from_bytes(&[seed_byte_base.wrapping_add(2); 32]),
    ];
    let pks: Vec<[u8; 32]> = keys.iter().map(|k| k.verifying_key().to_bytes()).collect();
    let vset = ledgerita_core::quorum::ValidatorSet::from_pubkeys(&pks, 2).unwrap();
    (keys, vset)
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_real_p2p_node(
    kp: identity::Keypair,
    connect_peers: Vec<Multiaddr>,
    state: std::sync::Arc<tokio::sync::Mutex<ledgerita_core::state::State>>,
    storage: std::sync::Arc<ledgerita_core::storage::Storage>,
    wallet_addr: ledgerita_core::crypto::Address,
    pending_txs: Vec<ledgerita_core::transaction::Transaction>,
    port: u16,
    network: NetworkId,
    quorum_cfg: ledgerita_core::network::QuorumNodeConfig,
) -> tokio::task::JoinHandle<()> {
    let mempool = std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new()));
    let pending_broadcast = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    tokio::spawn(async move {
        let _ = ledgerita_core::network::run_p2p_node_with_quorum(
            kp,
            connect_peers,
            state,
            storage,
            wallet_addr,
            mempool,
            pending_txs,
            pending_broadcast,
            port,
            network,
            quorum_cfg,
        )
        .await;
    })
}
