use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use libp2p::request_response::{self, Message};
use libp2p::swarm::SwarmEvent;
use libp2p::{Multiaddr, PeerId, identity};
use tokio::sync::Mutex;

use ledgerita_core::crypto::Address;
use ledgerita_core::network::{MSG_SYNC, NetworkId, build_msg, split_msg};
use ledgerita_core::state::State;
use ledgerita_core::storage::Storage;
use ledgerita_core::transaction::Transaction;

mod helpers;
use helpers::{TempTestDirs, build_test_swarm, wait_for_listen_addr};

const NET: NetworkId = NetworkId::Mainnet;

async fn run_sync_client(
    mut swarm: libp2p::Swarm<helpers::TestBehaviour>,
    dial_addr: Multiaddr,
    target_peer: PeerId,
    state: Arc<Mutex<State>>,
    storage: Arc<Storage>,
) {
    let _ = swarm.dial(dial_addr);

    loop {
        let ev = swarm.select_next_some().await;
        match ev {
            SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == target_peer => {
                let s = state.lock().await;
                let root = s.generate_state_root();
                let msg = build_msg(NET, MSG_SYNC, &root);
                swarm.behaviour_mut().rr.send_request(&peer_id, msg);
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
            )) if peer == target_peer
                && split_msg(NET, &request).map(|(t, _)| t) == Some(MSG_SYNC) =>
            {
                let s = state.lock().await;
                let snap = s.encode_snapshot(NET);
                drop(s);
                let resp = build_msg(NET, MSG_SYNC, &snap);
                let _ = swarm.behaviour_mut().rr.send_response(channel, resp);
            }
            SwarmEvent::Behaviour(helpers::TestBehaviourEvent::Rr(
                request_response::Event::Message {
                    peer,
                    message: Message::Response { response, .. },
                    ..
                },
            )) if peer == target_peer
                && split_msg(NET, &response).map(|(t, _)| t) == Some(MSG_SYNC) =>
            {
                if let Ok(remote) = State::decode_snapshot(
                    split_msg(NET, &response).map(|(_, p)| p).unwrap_or(&[]),
                    NET,
                ) {
                    let mut s = state.lock().await;
                    let _ = s.merge_if_fresher(
                        NET,
                        &remote,
                        &storage,
                    );
                }
                return;
            }
            SwarmEvent::Behaviour(helpers::TestBehaviourEvent::Rr(
                request_response::Event::OutboundFailure { peer, error, .. },
            )) if peer == target_peer => {
                panic!("Outbound failure: {:?}", error);
            }
            _ => {}
        }
    }
}

#[tokio::test]
async fn test_p2p_reconnect_resync() {
    let dirs = TempTestDirs::new();

    let sk_a = ed25519_dalek::SigningKey::from_bytes(&[1u8; 32]);
    let addr_a = Address::from_signing_key(&sk_a);
    let addr_b = Address::from_signing_key(&ed25519_dalek::SigningKey::from_bytes(&[2u8; 32]));

    let storage_a = Arc::new(Storage::open(&dirs.a.join("state")).unwrap());
    {
        let mut s = State::new();
        s.mint(addr_a.clone(), 5000);
        s.mint(addr_b.clone(), 100);
        let mut d = Transaction::new(addr_a.clone(), addr_a.clone(), 1, 0, 1);
        d.sign(ed25519_dalek::Signer::sign(&sk_a, &d.to_bytes()));
        s.apply_transaction(&d).unwrap();
        s.flush(&storage_a).unwrap();
    }
    let state_a = Arc::new(Mutex::new(State::load(&storage_a).unwrap()));

    let storage_b = Arc::new(Storage::open(&dirs.b.join("state")).unwrap());
    {
        let mut s = State::new();
        s.mint(addr_a.clone(), 5000);
        s.mint(addr_b.clone(), 100);
        s.flush(&storage_b).unwrap();
    }
    let state_b = Arc::new(Mutex::new(State::load(&storage_b).unwrap()));

    let tx2 = {
        let mut rt = Transaction::new(addr_a.clone(), addr_b.clone(), 100, 5, 2);
        rt.sign(ed25519_dalek::Signer::sign(&sk_a, &rt.to_bytes()));
        rt
    };
    let tx3 = {
        let mut rt = Transaction::new(addr_a.clone(), addr_b.clone(), 50, 3, 3);
        rt.sign(ed25519_dalek::Signer::sign(&sk_a, &rt.to_bytes()));
        rt
    };

    let key_a = identity::Keypair::generate_ed25519();
    let key_b = identity::Keypair::generate_ed25519();
    let peer_a = PeerId::from(key_a.public());
    let peer_b = PeerId::from(key_b.public());

    let mut swarm_a = build_test_swarm(key_a, NET);
    swarm_a
        .listen_on("/ip4/127.0.0.1/tcp/0".parse().unwrap())
        .unwrap();
    let la = wait_for_listen_addr(&mut swarm_a).await;
    let addr_a_full: Multiaddr = format!("{}/p2p/{}", la, peer_a.to_base58())
        .parse()
        .unwrap();

    let (initial_sync_tx, mut initial_sync_rx) = tokio::sync::watch::channel(false);
    let (resync_tx, mut resync_rx) = tokio::sync::watch::channel(false);

    // ── Task A: seed event loop ──
    let a_state = state_a.clone();
    let a_storage = storage_a.clone();
    let a_peer_b = peer_b.clone();
    tokio::spawn(async move {
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
                        let _ = s.merge_if_fresher(
                            NET,
                            &remote,
                            &a_storage,
                        );
                    }
                }
                _ => {}
            }
        }
    });

    // ── Phase 1: Initial sync (B-swarm-1) ──
    {
        let swarm_b1 = build_test_swarm(key_b.clone(), NET);
        let b_state = state_b.clone();
        let b_storage = storage_b.clone();
        let b_dial = addr_a_full.clone();
        let b_target = peer_a.clone();
        let b_signal = initial_sync_tx.clone();
        tokio::spawn(async move {
            run_sync_client(swarm_b1, b_dial, b_target, b_state, b_storage).await;
            b_signal.send(true).ok();
        });
    }

    tokio::time::timeout(Duration::from_secs(15), async {
        while !*initial_sync_rx.borrow_and_update() {
            initial_sync_rx.changed().await.ok();
        }
    })
    .await
    .expect("Timeout: B-swarm-1 no completó sync inicial");

    // After initial sync, B should have the same state as A (tx_count=1)
    {
        let sa = state_a.lock().await;
        let sb = state_b.lock().await;
        assert_eq!(
            sa.generate_state_root(),
            sb.generate_state_root(),
            "State roots should match after initial sync"
        );
        assert_eq!(sa.tx_count, 1, "A tx_count should be 1 after dummy TX");
        assert_eq!(sb.tx_count, 1, "B tx_count should be 1 after sync");
        assert_eq!(*sa.balances.get(&addr_a).unwrap_or(&0), 5000);
        assert_eq!(*sa.balances.get(&addr_b).unwrap_or(&0), 100);
    }

    // ── Phase 2: A applies TXs while B is "disconnected" ──
    {
        let mut s = state_a.lock().await;
        s.apply_transaction(&tx2).unwrap();
        s.apply_transaction(&tx3).unwrap();
        s.flush(&storage_a).unwrap();
    }

    {
        let sa = state_a.lock().await;
        assert_eq!(sa.tx_count, 3, "A should have 3 TXs after phase 2");
        // Alice: 5000 - (0+0) - (100+5) - (50+3) = 4842
        assert_eq!(*sa.balances.get(&addr_a).unwrap_or(&0), 4842);
        assert_eq!(*sa.balances.get(&addr_b).unwrap_or(&0), 250);
        assert_eq!(sa.fee_pool, 8);
    }

    // ── Phase 3: Reconnect (B-swarm-2, same P2P key) ──
    {
        let swarm_b2 = build_test_swarm(key_b, NET);
        let b_state = state_b.clone();
        let b_storage = storage_b.clone();
        let b_dial = addr_a_full;
        let b_target = peer_a;
        let b_signal = resync_tx;
        tokio::spawn(async move {
            run_sync_client(swarm_b2, b_dial, b_target, b_state, b_storage).await;
            b_signal.send(true).ok();
        });
    }

    tokio::time::timeout(Duration::from_secs(15), async {
        while !*resync_rx.borrow_and_update() {
            resync_rx.changed().await.ok();
        }
    })
    .await
    .expect("Timeout: B-swarm-2 no completó resync");

    tokio::time::sleep(Duration::from_millis(200)).await;

    // ── Final assertions ──
    let fa = state_a.lock().await;
    let fb = state_b.lock().await;

    assert_eq!(
        fa.generate_state_root(),
        fb.generate_state_root(),
        "State roots should match after reconnection"
    );

    assert_eq!(fa.tx_count, 3, "A tx_count should be 3");
    assert_eq!(fb.tx_count, 3, "B tx_count should be 3 after re-sync");

    assert_eq!(
        *fa.balances.get(&addr_a).unwrap_or(&0),
        4842,
        "Alice balance after all TXs"
    );
    assert_eq!(
        *fb.balances.get(&addr_a).unwrap_or(&0),
        4842,
        "Alice balance matches on B"
    );

    assert_eq!(
        *fa.balances.get(&addr_b).unwrap_or(&0),
        250,
        "Bob balance after all TXs"
    );
    assert_eq!(
        *fb.balances.get(&addr_b).unwrap_or(&0),
        250,
        "Bob balance matches on B"
    );

    assert_eq!(fa.fee_pool, 8, "A fee_pool should be 8");
    assert_eq!(fa.fee_pool, fb.fee_pool, "Fee pools should match");

    assert_eq!(
        fa.total_supply() + fa.fee_pool,
        5100,
        "Supply (balances + fee_pool) must remain at 5100"
    );
    assert_eq!(fa.total_supply(), fb.total_supply(), "Supply must match");
}
