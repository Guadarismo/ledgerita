use std::time::Duration;

use libp2p::identity;
use ledgerita_core::network::NetworkId;

mod helpers;
use helpers::{wait_for_listen_addrs, wait_for_listen_addr, TempTestDirs};

const NET: NetworkId = NetworkId::Mainnet;

#[tokio::test]
async fn test_wait_for_listen_addr_ipv6() {
    let mut swarm = helpers::build_test_swarm(identity::Keypair::generate_ed25519(), NET);
    swarm
        .listen_on("/ip6/::1/tcp/0".parse().unwrap())
        .unwrap();

    let addr = wait_for_listen_addr(&mut swarm).await;
    assert!(
        addr.to_string().contains("::1"),
        "Expected IPv6 loopback, got {}",
        addr
    );
}

#[tokio::test]
async fn test_wait_for_listen_addrs_multiple() {
    let mut swarm = helpers::build_test_swarm(identity::Keypair::generate_ed25519(), NET);
    swarm
        .listen_on("/ip4/127.0.0.1/tcp/0".parse().unwrap())
        .unwrap();
    swarm
        .listen_on("/ip6/::1/tcp/0".parse().unwrap())
        .unwrap();

    let addrs = tokio::time::timeout(
        Duration::from_secs(10),
        wait_for_listen_addrs(&mut swarm),
    )
    .await
    .expect("timed out waiting for listen addrs");

    let joined: Vec<String> = addrs.iter().map(|a| a.to_string()).collect();
    assert!(
        joined.iter().any(|s| s.contains("127.0.0.1")),
        "expected an IPv4 loopback addr in {:?}",
        joined
    );
    assert!(
        joined.iter().any(|s| s.contains("::1")),
        "expected an IPv6 loopback addr in {:?}",
        joined
    );
}

#[test]
fn test_temp_dirs_with_labels() {
    let dirs = TempTestDirs::with_labels(["seed", "peer1", "peer2", "extra"]);

    for label in ["seed", "peer1", "peer2", "extra", "a", "b", "c"] {
        let dir = dirs.get(label);
        assert!(dir.exists(), "dir for '{}' should exist at {}", label, dir.display());
    }

    assert_ne!(
        dirs.get("seed"),
        dirs.get("peer1"),
        "labeled dirs should differ"
    );
    assert_eq!(dirs.get("a"), &dirs.a, "legacy field 'a' should match label 'a'");

    let _ = dirs;
}

#[test]
fn test_temp_dirs_default_legacy_fields() {
    let dirs = TempTestDirs::new();
    assert!(dirs.a.exists());
    assert!(dirs.b.exists());
    assert!(dirs.c.exists());
    assert_ne!(dirs.a, dirs.b);
    assert_ne!(dirs.b, dirs.c);

    let _ = dirs;
}
