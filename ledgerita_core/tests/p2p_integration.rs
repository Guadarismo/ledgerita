use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use libp2p::request_response::{self, Message};
use libp2p::swarm::SwarmEvent;
use libp2p::{Multiaddr, PeerId, identity};
use tokio::sync::Mutex;

use ledgerita_core::crypto::Address;
use ledgerita_core::network::{MSG_SYNC, MSG_TX, NetworkId, build_msg, split_msg};
use ledgerita_core::serialize::encode_transaction;
use ledgerita_core::state::State;
use ledgerita_core::storage::Storage;
use ledgerita_core::transaction::Transaction;

mod helpers;
use helpers::{TempTestDirs, build_test_swarm, wait_for_listen_addr};

const NET: NetworkId = NetworkId::Mainnet;

#[tokio::test]
async fn test_p2p_sync_and_transaction() {
    let dirs = TempTestDirs::new();

    let sk_a = ed25519_dalek::SigningKey::from_bytes(&[1u8; 32]);
    let addr_a = Address::from_signing_key(&sk_a);
    let addr_b = Address::from_signing_key(&ed25519_dalek::SigningKey::from_bytes(&[2u8; 32]));

    // ── State A: mint + dummy TX (bump freshness) ──
    let storage_a = Arc::new(Storage::open(&dirs.a.join("state")).unwrap());
    {
        let mut s = State::load(&storage_a).unwrap();
        s.mint(addr_a.clone(), 5000);
        let mut d = Transaction::new(addr_a.clone(), addr_a.clone(), 1, 0, 1);
        d.sign(ed25519_dalek::Signer::sign(&sk_a, &d.to_bytes()));
        s.apply_transaction(&d).unwrap();
        s.flush(&storage_a).unwrap();
    }
    let state_a = Arc::new(Mutex::new(State::load(&storage_a).unwrap()));

    // Build TX (not pre-applied to A)
    let real_tx = {
        let mut rt = Transaction::new(addr_a.clone(), addr_b.clone(), 100, 5, 2);
        rt.sign(ed25519_dalek::Signer::sign(&sk_a, &rt.to_bytes()));
        rt
    };
    let real_tx_enc = encode_transaction(&real_tx);

    // ── State B: empty ──
    let storage_b = Arc::new(Storage::open(&dirs.b.join("state")).unwrap());
    let state_b = Arc::new(Mutex::new(State::load(&storage_b).unwrap()));

    // ── P2P keys ──
    let key_a = identity::Keypair::generate_ed25519();
    let key_b = identity::Keypair::generate_ed25519();
    let peer_a = PeerId::from(key_a.public());
    let peer_b = PeerId::from(key_b.public());

    // ── Build and get addresses ──
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

    // ── Channel: B signals TX applied ──
    let (b_done, mut b_done_rx) = tokio::sync::watch::channel(false);

    // ── Task A: respond MSG_SYNC, send + apply TX after sync ──
    let a_state = state_a.clone();
    let a_storage = storage_a.clone();
    let a_peer = peer_b.clone();
    let a_enc = real_tx_enc;
    let a_tx_local = real_tx.clone();
    tokio::spawn(async move {
        let mut synced = false;
        let mut sent = false;
        loop {
            let ev = swarm_a.select_next_some().await;
            match ev {
                SwarmEvent::Behaviour(helpers::TestBehaviourEvent::Rr(
                    request_response::Event::Message {
                        peer,
                        message:
                            Message::Request {
                                request, channel, ..
                            },
                        ..
                    },
                )) if peer == a_peer
                    && split_msg(NET, &request).map(|(t, _)| t) == Some(MSG_SYNC) =>
                {
                    let s = a_state.lock().await;
                    let snap = s.encode_snapshot(NET);
                    drop(s);
                    let resp = build_msg(NET, MSG_SYNC, &snap);
                    let _ = swarm_a.behaviour_mut().rr.send_response(channel, resp);
                    synced = true;
                }
                _ => {}
            }
            // Send TX only AFTER sync handshake is complete
            if synced && !sent {
                let msg = build_msg(NET, MSG_TX, &a_enc);
                swarm_a.behaviour_mut().rr.send_request(&a_peer, msg);
                let mut s = a_state.lock().await;
                s.apply_transaction(&a_tx_local).unwrap();
                s.flush(&a_storage).unwrap();
                sent = true;
            }
        }
    });

    // ── Task B: dial, sync, receive TX ──
    let b_state = state_b.clone();
    let b_storage = storage_b.clone();
    let b_peer = peer_a.clone();
    let b_addr = addr_a_full;
    let b_done_w = b_done.clone();
    tokio::spawn(async move {
        swarm_b.dial(b_addr).ok();
        loop {
            let ev = swarm_b.select_next_some().await;
            match ev {
                SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == b_peer => {
                    let msg = build_msg(NET, MSG_SYNC, &[0u8; 32]);
                    swarm_b.behaviour_mut().rr.send_request(&peer_id, msg);
                }
                SwarmEvent::Behaviour(helpers::TestBehaviourEvent::Rr(
                    request_response::Event::Message {
                        peer,
                        message: Message::Response { response, .. },
                        ..
                    },
                )) if peer == b_peer
                    && split_msg(NET, &response).map(|(t, _)| t) == Some(MSG_SYNC) =>
                {
                    if let Some((_, payload)) = split_msg(NET, &response) {
                        if let Ok(remote) = State::decode_snapshot(payload, NET) {
                            let mut s = b_state.lock().await;
                            s.merge_if_fresher(
                                NET,
                                &remote,
                                &b_storage,
                            )
                            .ok();
                        }
                    }
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
                )) if peer == b_peer
                    && split_msg(NET, &request).map(|(t, _)| t) == Some(MSG_TX) =>
                {
                    let tx_bytes = split_msg(NET, &request).unwrap().1;
                    let decoded = ledgerita_core::serialize::decode_transaction(tx_bytes).unwrap();
                    let mut s = b_state.lock().await;
                    s.apply_transaction(&decoded).unwrap();
                    s.flush(&b_storage).unwrap();
                    let _ = swarm_b
                        .behaviour_mut()
                        .rr
                        .send_response(channel, b"OK".to_vec());
                    b_done_w.send(true).ok();
                }
                _ => {}
            }
        }
    });

    // ── Wait for B to apply TX ──
    tokio::time::timeout(Duration::from_secs(15), async {
        while !*b_done_rx.borrow_and_update() {
            b_done_rx.changed().await.ok();
        }
    })
    .await
    .expect("Timeout waiting for B to apply TX");

    // Let A finish applying locally
    tokio::time::sleep(Duration::from_millis(200)).await;

    // ── Assertions ──
    let fa = state_a.lock().await;
    let fb = state_b.lock().await;
    assert_eq!(
        fa.generate_state_root(),
        fb.generate_state_root(),
        "State roots should match after sync + TX"
    );
    assert_eq!(
        *fa.balances.get(&addr_a).unwrap_or(&0),
        5000 - 105,
        "A balance = 5000 - 100 - 5"
    );
    assert_eq!(
        *fa.balances.get(&addr_b).unwrap_or(&0),
        100,
        "B balance = 100"
    );
    assert_eq!(fa.tx_count, 2, "A tx_count should be 2");
    assert_eq!(fb.tx_count, 2, "B tx_count should be 2");
    assert_eq!(fa.fee_pool, 5, "A fee_pool should be 5");
    assert_eq!(fa.fee_pool, fb.fee_pool, "Fee pools should match");
}

#[tokio::test]
async fn test_e2e_real_node_rechaza_tx_0_0_y_faucet_firmado_pasa_g7() {
    use helpers::{alloc_loopback_port, make_test_vset_2_de_3, spawn_real_p2p_node, wait_tcp_port_open};
    use ledgerita_core::network::{MSG_FAUCET, QuorumNodeConfig, encode_faucet_request};
    use ledgerita_core::quorum::{QuorumCertificate, sign_qc_vote};
    use ledgerita_core::state::CHECKPOINT_INTERVAL;

    let dirs = TempTestDirs::new();
    let net = NetworkId::Testnet;
    let (v_keys, vset) = make_test_vset_2_de_3(0xA1);

    let sk_faucet = ed25519_dalek::SigningKey::from_bytes(&[0xF1; 32]);
    let addr_faucet = Address::from_signing_key(&sk_faucet);
    let sk_zero = ed25519_dalek::SigningKey::from_bytes(&[0xE1; 32]);
    let addr_zero = Address::from_signing_key_d2(&sk_zero);
    let sk_dest = ed25519_dalek::SigningKey::from_bytes(&[0xD2; 32]);
    let addr_dest = Address::from_signing_key_d2(&sk_dest);

    let storage_a = Arc::new(Storage::open(&dirs.a.join("state")).unwrap());
    let base_root = {
        let mut s = State::load(&storage_a).unwrap();
        s.mint(addr_faucet.clone(), 200_000_000);
        s.emitido_total = 200_000_000;
        s.tx_count = CHECKPOINT_INTERVAL;
        let cp_root = s.finalize_checkpoint();
        s.latest_qc = Some(QuorumCertificate {
            chain_id: net.chain_id(),
            epoch: s.epoch,
            state_root: cp_root,
            signatures: vec![
                sign_qc_vote(&v_keys[0], net.chain_id(), s.epoch, &cp_root, 0),
                sign_qc_vote(&v_keys[1], net.chain_id(), s.epoch, &cp_root, 1),
            ],
        });
        s.flush(&storage_a).unwrap();
        s.generate_state_root()
    };
    let state_a = Arc::new(Mutex::new(State::load(&storage_a).unwrap()));

    let kp_a = identity::Keypair::generate_ed25519();
    let peer_a = PeerId::from(kp_a.public());
    let port_a = alloc_loopback_port();
    let _node_a = spawn_real_p2p_node(
        kp_a,
        vec![],
        state_a.clone(),
        storage_a.clone(),
        addr_faucet.clone(),
        vec![],
        port_a,
        net,
        QuorumNodeConfig {
            validator_set: Some(vset.clone()),
            local_validator: None,
            testnet_faucet: Some((sk_faucet.clone(), 50_000_000)),
            deferred_sends: Vec::new(),
        },
    );
    wait_tcp_port_open(port_a).await;

    let seed_ma: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port_a}/p2p/{peer_a}")
        .parse()
        .unwrap();

    // Nodo B real arranca vacío y sincroniza época 1 con QC 2-de-3
    let storage_b = Arc::new(Storage::open(&dirs.b.join("state")).unwrap());
    let state_b = Arc::new(Mutex::new(State::load(&storage_b).unwrap()));
    let _node_b = spawn_real_p2p_node(
        identity::Keypair::generate_ed25519(),
        vec![seed_ma.clone()],
        state_b.clone(),
        storage_b.clone(),
        addr_dest.clone(),
        vec![],
        0,
        net,
        QuorumNodeConfig {
            validator_set: Some(vset),
            ..Default::default()
        },
    );

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if state_b.lock().await.generate_state_root() == base_root {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("Nodo B debe sincronizar época 1 con g7 antes de enviar TX");

    // Cliente atacante/usuario envía: (1) TX 0/0 rechazada y (2) MSG_FAUCET firmado aprobado por g7
    let mut swarm_cli = build_test_swarm(identity::Keypair::generate_ed25519(), net);
    swarm_cli.dial(seed_ma).unwrap();

    let mut tx_zero = Transaction::new_d2(
        &sk_zero.verifying_key(),
        addr_dest.clone(),
        0,
        0,
        1,
    )
    .with_chain_id(net.chain_id());
    tx_zero.sign(ed25519_dalek::Signer::sign(&sk_zero, &tx_zero.to_bytes()));
    let enc_zero = encode_transaction(&tx_zero);
    let faucet_req = encode_faucet_request(&addr_dest, 10_000_000);

    let mut rejected_zero = false;
    let mut faucet_ok = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !(rejected_zero && faucet_ok) {
        let ev = tokio::time::timeout_at(deadline, swarm_cli.select_next_some())
            .await
            .expect("Timeout esperando respuestas MSG_TX (0/0) y MSG_FAUCET");
        match ev {
            SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == peer_a => {
                swarm_cli
                    .behaviour_mut()
                    .rr
                    .send_request(&peer_id, build_msg(net, MSG_TX, &enc_zero));
                swarm_cli
                    .behaviour_mut()
                    .rr
                    .send_request(&peer_id, build_msg(net, MSG_FAUCET, &faucet_req));
            }
            SwarmEvent::Behaviour(helpers::TestBehaviourEvent::Rr(
                request_response::Event::Message {
                    message: Message::Response { response, .. },
                    ..
                },
            )) => {
                if let Some((MSG_TX, payload)) = split_msg(net, &response)
                    && payload.starts_with(b"REJECT:")
                {
                    rejected_zero = true;
                }
                if let Some((MSG_FAUCET, b"OK")) = split_msg(net, &response) {
                    faucet_ok = true;
                }
            }
            _ => {}
        }
    }

    assert!(rejected_zero, "TX 0/0 debe rechazarse explícitamente");
    assert!(faucet_ok, "MSG_FAUCET firmado debe aprobarse");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let sa = state_a.lock().await;
            let sb = state_b.lock().await;
            if sa.tx_count == CHECKPOINT_INTERVAL + 1
                && sb.tx_count == CHECKPOINT_INTERVAL + 1
                && sa.generate_state_root() == sb.generate_state_root()
            {
                assert_eq!(sa.balances.get(&addr_zero), None);
                assert_eq!(sa.balances.get(&addr_dest).copied().unwrap_or(0), 10_000_000);
                assert_eq!(sb.balances.get(&addr_dest).copied().unwrap_or(0), 10_000_000);
                assert_eq!(sa.epoch_txs.len(), 1);
                break;
            }
            drop(sa);
            drop(sb);
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("Nodo B debe adoptar Faucet firmado bajo g7");
}
