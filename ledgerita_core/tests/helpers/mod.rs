use std::time::Duration;

use futures::StreamExt;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::tcp;
use libp2p::yamux;
use libp2p::{identity, noise, Multiaddr};

use ledgerita_core::network::RawCodec;

#[derive(NetworkBehaviour)]
pub struct TestBehaviour {
    pub rr: request_response::Behaviour<RawCodec>,
}

pub fn build_test_swarm(key: identity::Keypair) -> libp2p::Swarm<TestBehaviour> {
    libp2p::SwarmBuilder::with_existing_identity(key)
        .with_tokio()
        .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)
        .unwrap()
        .with_behaviour(|_| {
            let protocols = vec![("/ledgerita/1".to_string(), ProtocolSupport::Full)];
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
                if address.to_string().contains("127.0.0.1") {
                    return address;
                }
            }
            _ => {}
        }
    }
}

pub struct TempTestDirs {
    pub a: std::path::PathBuf,
    pub b: std::path::PathBuf,
    pub c: std::path::PathBuf,
}

impl TempTestDirs {
    pub fn new() -> Self {
        let test_id = std::process::id();
        let a = std::env::temp_dir().join(format!("lgr_a_{}", test_id));
        let b = std::env::temp_dir().join(format!("lgr_b_{}", test_id));
        let c = std::env::temp_dir().join(format!("lgr_c_{}", test_id));
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
        let _ = std::fs::remove_dir_all(&c);
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::create_dir_all(&c).unwrap();
        TempTestDirs { a, b, c }
    }
}

impl Default for TempTestDirs {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TempTestDirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.a);
        let _ = std::fs::remove_dir_all(&self.b);
        let _ = std::fs::remove_dir_all(&self.c);
    }
}
