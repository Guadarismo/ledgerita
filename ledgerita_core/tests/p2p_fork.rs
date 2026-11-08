use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::StreamExt;
use libp2p::request_response::{self, Message};
use libp2p::swarm::SwarmEvent;
use libp2p::{Multiaddr, PeerId, identity};
use tokio::sync::Mutex;

use ledgerita_core::crypto::Address;
use ledgerita_core::network::{MSG_SYNC, NetworkId, build_msg, split_msg};
use ledgerita_core::state::{State, SyncResult};
use ledgerita_core::storage::Storage;
use ledgerita_core::transaction::Transaction;

mod helpers;
use helpers::{TempTestDirs, build_test_swarm, wait_for_listen_addr};

const NET: NetworkId = NetworkId::Mainnet;

#[tokio::test]
async fn test_fork_detection_and_resolution() {
    let dirs = TempTestDirs::new();

    let sk_alice = ed25519_dalek::SigningKey::from_bytes(&[1u8; 32]);
    let sk_bob = ed25519_dalek::SigningKey::from_bytes(&[2u8; 32]);
    let sk_charlie = ed25519_dalek::SigningKey::from_bytes(&[3u8; 32]);
    let addr_alice = Address::from_signing_key(&sk_alice);
    let addr_bob = Address::from_signing_key(&sk_bob);
    let addr_charlie = Address::from_signing_key(&sk_charlie);

    let genesis = || {
        let mut s = State::new();
        s.mint(addr_alice.clone(), 10_000);
        s.mint(addr_bob.clone(), 10_000);
        s.mint(addr_charlie.clone(), 10_000);
        s
    };

    let storage_a = Arc::new(Storage::open(&dirs.a.join("state")).unwrap());
    {
        let mut s = genesis();
        let mut tx = Transaction::new(addr_alice.clone(), addr_bob.clone(), 100, 0, 1);
        tx.sign(ed25519_dalek::Signer::sign(&sk_alice, &tx.to_bytes()));
        s.apply_transaction(&tx).unwrap();
        s.flush(&storage_a).unwrap();
    }
    let state_a = Arc::new(Mutex::new(State::load(&storage_a).unwrap()));

    let storage_b = Arc::new(Storage::open(&dirs.b.join("state")).unwrap());
    {
        let mut s = genesis();
        let mut tx = Transaction::new(addr_charlie.clone(), addr_bob.clone(), 200, 0, 1);
        tx.sign(ed25519_dalek::Signer::sign(&sk_charlie, &tx.to_bytes()));
        s.apply_transaction(&tx).unwrap();
        s.flush(&storage_b).unwrap();
    }
    let state_b = Arc::new(Mutex::new(State::load(&storage_b).unwrap()));

    {
        let sa = state_a.lock().await;
        let sb = state_b.lock().await;
        assert_eq!(sa.tx_count, 1);
        assert_eq!(sb.tx_count, 1);
        assert_ne!(
            sa.generate_state_root(),
            sb.generate_state_root(),
            "States should be diverged (different TXs)"
        );
    }

    let fresher_tx = {
        let mut tx = Transaction::new(addr_alice.clone(), addr_bob.clone(), 50, 0, 2);
        tx.sign(ed25519_dalek::Signer::sign(&sk_alice, &tx.to_bytes()));
        tx
    };

    let key_a = identity::Keypair::generate_ed25519();
    let key_b = identity::Keypair::generate_ed25519();
    let peer_a = PeerId::from(key_a.public());
    let peer_b = PeerId::from(key_b.public());

    let mut swarm_a = build_test_swarm(key_a, NET);
    let mut swarm_b = build_test_swarm(key_b, NET);
    swarm_a
        .listen_on("/ip4/127.0.0.1/tcp/0".parse().unwrap())
        .unwrap();
    swarm_b
        .listen_on("/ip4/127.0.0.1/tcp/0".parse().unwrap())
        .unwrap();

    let la = wait_for_listen_addr(&mut swarm_a).await;
    let _lb = wait_for_listen_addr(&mut swarm_b).await;
    let addr_a_full: Multiaddr = format!("{}/p2p/{}", la, peer_a.to_base58())
        .parse()
        .unwrap();

    let diverged = Arc::new(AtomicU64::new(0));
    let (resolved_tx, mut resolved_rx) = tokio::sync::watch::channel(false);

    let a_state = state_a.clone();
    let a_storage = storage_a.clone();
    let a_diverged = diverged.clone();
    let a_peer_b = peer_b.clone();
    tokio::spawn(async move {
        let mut applied_fresher = false;

        loop {
            let ev = swarm_a.select_next_some().await;
            match ev {
                SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == a_peer_b => {
                    let s = a_state.lock().await;
                    let root = s.generate_state_root();
                    let msg = build_msg(NET, MSG_SYNC, &root);
                    swarm_a.behaviour_mut().rr.send_request(&peer_id, msg);
                }
                SwarmEvent::Behaviour(helpers::TestBehaviourEvent::Rr(
                    request_response::Event::Message {
                        peer,
                        message:
                            Message::Request {
                                request, channel, ..
                            },
                        ..
                    },
                )) if peer == a_peer_b
                    && split_msg(NET, &request).map(|(t, _)| t) == Some(MSG_SYNC) =>
                {
                    let s = a_state.lock().await;
                    let snap = s.encode_snapshot(NET);
                    drop(s);
                    let resp = build_msg(NET, MSG_SYNC, &snap);
                    let _ = swarm_a.behaviour_mut().rr.send_response(channel, resp);
                }
                SwarmEvent::Behaviour(helpers::TestBehaviourEvent::Rr(
                    request_response::Event::Message {
                        peer,
                        message: Message::Response { response, .. },
                        ..
                    },
                )) if peer == a_peer_b
                    && split_msg(NET, &response).map(|(t, _)| t) == Some(MSG_SYNC) =>
                {
                    if let Ok(remote) = State::decode_snapshot(
                        split_msg(NET, &response).map(|(_, p)| p).unwrap_or(&[]),
                        NET,
                    ) {
                        let mut s = a_state.lock().await;
                        if let Ok(SyncResult::Divergent) = s.merge_if_fresher(
                            ledgerita_core::network::NetworkId::Testnet,
                            &remote,
                            &a_storage,
                        ) {
                            a_diverged.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
                _ => {}
            }

            if a_diverged.load(Ordering::Relaxed) >= 1 && !applied_fresher {
                let mut s = a_state.lock().await;
                s.apply_transaction(&fresher_tx).unwrap();
                s.flush(&a_storage).unwrap();
                let root = s.generate_state_root();
                drop(s);
                applied_fresher = true;

                let msg = build_msg(NET, MSG_SYNC, &root);
                swarm_a.behaviour_mut().rr.send_request(&a_peer_b, msg);
            }
        }
    });

    let b_state = state_b.clone();
    let b_storage = storage_b.clone();
    let b_diverged = diverged.clone();
    let b_peer_a = peer_a.clone();
    let b_addr = addr_a_full.clone();
    let b_resolved = resolved_tx.clone();
    tokio::spawn(async move {
        swarm_b.dial(b_addr).ok();
        let mut trigger_resync = false;

        loop {
            let ev = swarm_b.select_next_some().await;
            match ev {
                SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == b_peer_a => {
                    let s = b_state.lock().await;
                    let root = s.generate_state_root();
                    let msg = build_msg(NET, MSG_SYNC, &root);
                    swarm_b.behaviour_mut().rr.send_request(&peer_id, msg);
                }
                SwarmEvent::Behaviour(helpers::TestBehaviourEvent::Rr(
                    request_response::Event::Message {
                        peer,
                        message:
                            Message::Request {
                                request, channel, ..
                            },
                        ..
                    },
                )) if peer == b_peer_a
                    && split_msg(NET, &request).map(|(t, _)| t) == Some(MSG_SYNC) =>
                {
                    let s = b_state.lock().await;
                    let snap = s.encode_snapshot(NET);
                    drop(s);
                    let resp = build_msg(NET, MSG_SYNC, &snap);
                    let _ = swarm_b.behaviour_mut().rr.send_response(channel, resp);
                }
                SwarmEvent::Behaviour(helpers::TestBehaviourEvent::Rr(
                    request_response::Event::Message {
                        peer,
                        message: Message::Response { response, .. },
                        ..
                    },
                )) if peer == b_peer_a
                    && split_msg(NET, &response).map(|(t, _)| t) == Some(MSG_SYNC) =>
                {
                    if let Ok(remote) = State::decode_snapshot(
                        split_msg(NET, &response).map(|(_, p)| p).unwrap_or(&[]),
                        NET,
                    ) {
                        let mut s = b_state.lock().await;
                        match s.merge_if_fresher(
                            ledgerita_core::network::NetworkId::Testnet,
                            &remote,
                            &b_storage,
                        ) {
                            Ok(SyncResult::Divergent) => {
                                b_diverged.fetch_add(1, Ordering::Relaxed);
                                trigger_resync = true;
                            }
                            Ok(SyncResult::RemoteAdopted) => {
                                b_resolved.send(true).ok();
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }

            if trigger_resync {
                trigger_resync = false;
                let s = b_state.lock().await;
                let root = s.generate_state_root();
                let msg = build_msg(NET, MSG_SYNC, &root);
                swarm_b.behaviour_mut().rr.send_request(&b_peer_a, msg);
            }
        }
    });

    tokio::time::timeout(Duration::from_secs(10), async {
        while diverged.load(Ordering::Relaxed) < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("Timeout: ambos nodos debieron detectar divergencia");

    tokio::time::timeout(Duration::from_secs(10), async {
        while !*resolved_rx.borrow_and_update() {
            resolved_rx.changed().await.ok();
        }
    })
    .await
    .expect("Timeout: B no adoptó el estado de A");

    tokio::time::sleep(Duration::from_millis(300)).await;

    let fa = state_a.lock().await;
    let fb = state_b.lock().await;

    assert_eq!(
        fa.generate_state_root(),
        fb.generate_state_root(),
        "State roots should match after resolution"
    );

    assert_eq!(fa.tx_count, 2, "A tx_count should be 2");
    assert_eq!(fb.tx_count, 2, "B tx_count should be 2 (adopted from A)");

    assert_eq!(
        *fa.balances.get(&addr_alice).unwrap_or(&0),
        10000 - 100 - 50,
        "Alice balance after A's TXs"
    );
    assert_eq!(
        *fa.balances.get(&addr_bob).unwrap_or(&0),
        10000 + 100 + 50,
        "Bob balance after A's TXs"
    );
    assert_eq!(
        *fa.balances.get(&addr_charlie).unwrap_or(&0),
        10000,
        "Charlie should have genesis balance (B's TX reverted after adopting A)"
    );

    assert_eq!(fa.total_supply(), 30000, "Supply must remain at 30000");
}
