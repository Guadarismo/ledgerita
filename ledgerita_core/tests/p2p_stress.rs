use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use libp2p::request_response::{self, Message};
use libp2p::swarm::SwarmEvent;
use libp2p::{identity, Multiaddr, PeerId};
use tokio::sync::Mutex;

use ledgerita_core::crypto::Address;
use ledgerita_core::network::{build_msg, split_msg, MSG_TX, MSG_SYNC, NetworkId};
use ledgerita_core::serialize::encode_transaction;
use ledgerita_core::state::State;
use ledgerita_core::storage::Storage;
use ledgerita_core::transaction::Transaction;

mod helpers;
use helpers::{build_test_swarm, wait_for_listen_addr, TempTestDirs};

const NET: NetworkId = NetworkId::Mainnet;

const MINT_AMOUNT: u64 = 10_000_000_000; // 10000 LIT
const TX_AMOUNT: u64 = 100_000_000; // 100 LIT
const TOTAL_TXS: u64 = 10;

fn make_tx(sender: &Address, receiver: &Address, sk: &ed25519_dalek::SigningKey, nonce: u64) -> Transaction {
    let mut tx = Transaction::new(sender.clone(), receiver.clone(), TX_AMOUNT, 0, nonce);
    tx.sign(ed25519_dalek::Signer::sign(sk, &tx.to_bytes()));
    tx
}

struct NodeCtx {
    state: Arc<Mutex<State>>,
    storage: Arc<Storage>,
    txs: Vec<Transaction>,
    encs: Vec<Vec<u8>>,
}

/// Node A – seed. Espera 2 conexiones, luego envía sus 5 TXs a todos los peers conectados.
fn spawn_node_a(
    mut swarm: libp2p::Swarm<helpers::TestBehaviour>,
    ctx: NodeCtx,
    done: tokio::sync::watch::Sender<bool>,
) {
    tokio::spawn(async move {
        let mut connected: Vec<PeerId> = Vec::new();
        let mut sent_idx = 0usize;
        let expected_peers: u64 = 2;

        loop {
            let ev = swarm.select_next_some().await;
            match ev {
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    if !connected.contains(&peer_id) {
                        connected.push(peer_id);
                    }
                }
                SwarmEvent::Behaviour(helpers::TestBehaviourEvent::Rr(
                    request_response::Event::Message {
                        message: Message::Request { request, channel, .. },
                        ..
                    },
                )) if split_msg(NET, &request).map(|(t, _)| t) == Some(MSG_SYNC) => {
                    let s = ctx.state.lock().await;
                    let snap = s.encode_snapshot(NET);
                    drop(s);
                    let resp = build_msg(NET, MSG_SYNC, &snap);
                    let _ = swarm.behaviour_mut().rr.send_response(channel, resp);
                }
                SwarmEvent::Behaviour(helpers::TestBehaviourEvent::Rr(
                    request_response::Event::Message {
                        message: Message::Response { response, .. },
                        ..
                    },
                )) if split_msg(NET, &response).map(|(t, _)| t) == Some(MSG_SYNC) => {
                    if let Ok(remote) = State::decode_snapshot(
                        split_msg(NET, &response).map(|(_, p)| p).unwrap_or(&[]),
                        NET,
                    ) {
                        let mut s = ctx.state.lock().await;
                        let _ = s.merge_if_fresher(&remote, &ctx.storage);
                    }
                }
                SwarmEvent::Behaviour(helpers::TestBehaviourEvent::Rr(
                    request_response::Event::Message {
                        message: Message::Request { request, channel, .. },
                        ..
                    },
                )) if split_msg(NET, &request).map(|(t, _)| t) == Some(MSG_TX) => {
                    if let Ok(decoded) = ledgerita_core::serialize::decode_transaction(
                        split_msg(NET, &request).map(|(_, p)| p).unwrap_or(&[]),
                    ) {
                        let mut s = ctx.state.lock().await;
                        let _ = s.apply_transaction(&decoded);
                        let _ = s.flush(&ctx.storage);
                    }
                    let _ = swarm.behaviour_mut().rr.send_response(channel, b"OK".to_vec());
                }
                _ => {}
            }

            if (connected.len() as u64) >= expected_peers && sent_idx < ctx.txs.len() {
                for pid in &connected {
                    let msg = build_msg(NET, MSG_TX, &ctx.encs[sent_idx]);
                    swarm.behaviour_mut().rr.send_request(pid, msg);
                }
                {
                    let mut s = ctx.state.lock().await;
                    let _ = s.apply_transaction(&ctx.txs[sent_idx]);
                    let _ = s.flush(&ctx.storage);
                }
                sent_idx += 1;
            }

            if sent_idx >= ctx.txs.len() {
                let tx_count = { ctx.state.lock().await.tx_count };
                if tx_count >= TOTAL_TXS {
                    done.send(true).ok();
                }
            }
        }
    });
}

/// Node B or C – dial peers, sync, then broadcast TXs.
fn spawn_node_bc(
    mut swarm: libp2p::Swarm<helpers::TestBehaviour>,
    ctx: NodeCtx,
    done: tokio::sync::watch::Sender<bool>,
    dial_targets: Vec<Multiaddr>,
    expected_syncs: u64,
) {
    tokio::spawn(async move {
        let mut synced = 0u64;
        let mut sent_idx = 0usize;
        let mut connected: Vec<PeerId> = Vec::new();

        for target in dial_targets {
            let _ = swarm.dial(target);
        }

        loop {
            let ev = swarm.select_next_some().await;
            match ev {
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    if !connected.contains(&peer_id) {
                        connected.push(peer_id);
                    }
                    let msg = build_msg(NET, MSG_SYNC, &[0u8; 32]);
                    swarm.behaviour_mut().rr.send_request(&peer_id, msg);
                }
                SwarmEvent::Behaviour(helpers::TestBehaviourEvent::Rr(
                    request_response::Event::Message {
                        message: Message::Request { request, channel, .. },
                        ..
                    },
                )) if split_msg(NET, &request).map(|(t, _)| t) == Some(MSG_SYNC) => {
                    let s = ctx.state.lock().await;
                    let snap = s.encode_snapshot(NET);
                    drop(s);
                    let resp = build_msg(NET, MSG_SYNC, &snap);
                    let _ = swarm.behaviour_mut().rr.send_response(channel, resp);
                }
                SwarmEvent::Behaviour(helpers::TestBehaviourEvent::Rr(
                    request_response::Event::Message {
                        message: Message::Response { response, .. },
                        ..
                    },
                )) if split_msg(NET, &response).map(|(t, _)| t) == Some(MSG_SYNC) => {
                    if let Ok(remote) = State::decode_snapshot(
                        split_msg(NET, &response).map(|(_, p)| p).unwrap_or(&[]),
                        NET,
                    ) {
                        let mut s = ctx.state.lock().await;
                        let _ = s.merge_if_fresher(&remote, &ctx.storage);
                    }
                    synced += 1;
                }
                SwarmEvent::Behaviour(helpers::TestBehaviourEvent::Rr(
                    request_response::Event::Message {
                        message: Message::Request { request, channel, .. },
                        ..
                    },
                )) if split_msg(NET, &request).map(|(t, _)| t) == Some(MSG_TX) => {
                    if let Ok(decoded) = ledgerita_core::serialize::decode_transaction(
                        split_msg(NET, &request).map(|(_, p)| p).unwrap_or(&[]),
                    ) {
                        let mut s = ctx.state.lock().await;
                        let _ = s.apply_transaction(&decoded);
                        let _ = s.flush(&ctx.storage);
                    }
                    let _ = swarm.behaviour_mut().rr.send_response(channel, b"OK".to_vec());
                }
                _ => {}
            }

            if synced >= expected_syncs && sent_idx < ctx.txs.len() {
                for pid in &connected {
                    let msg = build_msg(NET, MSG_TX, &ctx.encs[sent_idx]);
                    swarm.behaviour_mut().rr.send_request(pid, msg);
                }
                {
                    let mut s = ctx.state.lock().await;
                    let _ = s.apply_transaction(&ctx.txs[sent_idx]);
                    let _ = s.flush(&ctx.storage);
                }
                sent_idx += 1;
            }

            if sent_idx >= ctx.txs.len() {
                let tx_count = { ctx.state.lock().await.tx_count };
                if tx_count >= TOTAL_TXS {
                    done.send(true).ok();
                }
            }
        }
    });
}

#[tokio::test]
async fn test_p2p_stress_3_nodes_10_txs() {
    let dirs = TempTestDirs::new();

    // ── Wallet keys and addresses ──
    let sk_a = ed25519_dalek::SigningKey::from_bytes(&[10u8; 32]);
    let sk_b = ed25519_dalek::SigningKey::from_bytes(&[20u8; 32]);
    let sk_c = ed25519_dalek::SigningKey::from_bytes(&[30u8; 32]);
    let addr_a = Address::from_signing_key(&sk_a);
    let addr_b = Address::from_signing_key(&sk_b);
    let addr_c = Address::from_signing_key(&sk_c);

    // ── Same genesis state for all 3 nodes ──
    let storage_a = Arc::new(Storage::open(&dirs.a.join("state")).unwrap());
    let storage_b = Arc::new(Storage::open(&dirs.b.join("state")).unwrap());
    let storage_c = Arc::new(Storage::open(&dirs.c.join("state")).unwrap());

    for storage in [&storage_a, &storage_b, &storage_c] {
        let mut s = State::new();
        s.mint(addr_a.clone(), MINT_AMOUNT);
        s.mint(addr_b.clone(), MINT_AMOUNT);
        s.mint(addr_c.clone(), MINT_AMOUNT);
        s.flush(storage).unwrap();
    }

    let state_a = Arc::new(Mutex::new(State::load(&storage_a).unwrap()));
    let state_b = Arc::new(Mutex::new(State::load(&storage_b).unwrap()));
    let state_c = Arc::new(Mutex::new(State::load(&storage_c).unwrap()));

    // ── Build TXs: A→B (5), B→C (3), C→A (2) ──
    let txs_a: Vec<Transaction> = (0..5).map(|i| make_tx(&addr_a, &addr_b, &sk_a, i + 1)).collect();
    let txs_b: Vec<Transaction> = (0..3).map(|i| make_tx(&addr_b, &addr_c, &sk_b, i + 1)).collect();
    let txs_c: Vec<Transaction> = (0..2).map(|i| make_tx(&addr_c, &addr_a, &sk_c, i + 1)).collect();

    let encs_a: Vec<Vec<u8>> = txs_a.iter().map(encode_transaction).collect();
    let encs_b: Vec<Vec<u8>> = txs_b.iter().map(encode_transaction).collect();
    let encs_c: Vec<Vec<u8>> = txs_c.iter().map(encode_transaction).collect();

    // ── P2P keypairs ──
    let key_a = identity::Keypair::generate_ed25519();
    let key_b = identity::Keypair::generate_ed25519();
    let key_c = identity::Keypair::generate_ed25519();
    let peer_a = PeerId::from(key_a.public());
    let peer_b = PeerId::from(key_b.public());

    // ── Swarms ──
    let mut swarm_a = build_test_swarm(key_a, NET);
    let mut swarm_b = build_test_swarm(key_b, NET);
    let mut swarm_c = build_test_swarm(key_c, NET);
    swarm_a.listen_on("/ip4/127.0.0.1/tcp/0".parse().unwrap()).unwrap();
    swarm_b.listen_on("/ip4/127.0.0.1/tcp/0".parse().unwrap()).unwrap();
    swarm_c.listen_on("/ip4/127.0.0.1/tcp/0".parse().unwrap()).unwrap();

    let la = wait_for_listen_addr(&mut swarm_a).await;
    let lb = wait_for_listen_addr(&mut swarm_b).await;
    let addr_a_full: Multiaddr = format!("{}/p2p/{}", la, peer_a.to_base58()).parse().unwrap();
    let addr_b_full: Multiaddr = format!("{}/p2p/{}", lb, peer_b.to_base58()).parse().unwrap();

    let (done_a_tx, mut done_a_rx) = tokio::sync::watch::channel(false);
    let (done_b_tx, mut done_b_rx) = tokio::sync::watch::channel(false);
    let (done_c_tx, mut done_c_rx) = tokio::sync::watch::channel(false);

    // ── Launch seed node A ──
    spawn_node_a(
        swarm_a,
        NodeCtx { state: state_a.clone(), storage: storage_a.clone(), txs: txs_a, encs: encs_a },
        done_a_tx,
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    // ── Launch B (dials A) ──
    spawn_node_bc(
        swarm_b,
        NodeCtx { state: state_b.clone(), storage: storage_b.clone(), txs: txs_b, encs: encs_b },
        done_b_tx,
        vec![addr_a_full.clone()],
        1,
    );

    tokio::time::sleep(Duration::from_millis(500)).await;

    // ── Launch C (dials A and B) ──
    spawn_node_bc(
        swarm_c,
        NodeCtx { state: state_c.clone(), storage: storage_c.clone(), txs: txs_c, encs: encs_c },
        done_c_tx,
        vec![addr_a_full, addr_b_full],
        2,
    );

    // ── Wait for all 3 nodes ──
    let timeout = Duration::from_secs(30);

    tokio::time::timeout(timeout, async {
        while !*done_a_rx.borrow_and_update() { let _ = done_a_rx.changed().await; }
    }).await.expect("Timeout Nodo A");

    tokio::time::timeout(timeout, async {
        while !*done_b_rx.borrow_and_update() { let _ = done_b_rx.changed().await; }
    }).await.expect("Timeout Nodo B");

    tokio::time::timeout(timeout, async {
        while !*done_c_rx.borrow_and_update() { let _ = done_c_rx.changed().await; }
    }).await.expect("Timeout Nodo C");

    tokio::time::sleep(Duration::from_millis(500)).await;

    // ── Assertions ──
    let fa = state_a.lock().await;
    let fb = state_b.lock().await;
    let fc = state_c.lock().await;

    let root_a = fa.generate_state_root();
    let root_b = fb.generate_state_root();
    let root_c = fc.generate_state_root();

    assert_eq!(root_a, root_b, "State root A != B");
    assert_eq!(root_b, root_c, "State root B != C");

    let expected_a = MINT_AMOUNT - (5 * TX_AMOUNT) + (2 * TX_AMOUNT);
    let expected_b = MINT_AMOUNT + (5 * TX_AMOUNT) - (3 * TX_AMOUNT);
    let expected_c = MINT_AMOUNT + (3 * TX_AMOUNT) - (2 * TX_AMOUNT);

    assert_eq!(*fa.balances.get(&addr_a).unwrap_or(&0), expected_a, "Balance A incorrecto");
    assert_eq!(*fa.balances.get(&addr_b).unwrap_or(&0), expected_b, "Balance B incorrecto");
    assert_eq!(*fa.balances.get(&addr_c).unwrap_or(&0), expected_c, "Balance C incorrecto");

    assert_eq!(fa.tx_count, TOTAL_TXS, "tx_count A debe ser {}", TOTAL_TXS);
    assert_eq!(fb.tx_count, TOTAL_TXS, "tx_count B debe ser {}", TOTAL_TXS);
    assert_eq!(fc.tx_count, TOTAL_TXS, "tx_count C debe ser {}", TOTAL_TXS);

    assert_eq!(fa.total_supply(), 3 * MINT_AMOUNT, "Supply total debe mantenerse en 30000 LIT");
}
