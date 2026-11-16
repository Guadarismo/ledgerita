//! Ensayo en vivo (Checkpoint 41) contra TestNet (`kettle03`).
//!
//! Marcado `#[ignore]` para no depender de red LAN en `cargo test` normal.
//! Ejecutar explícitamente con:
//! `cargo test --release --test p2p_quorum_live -- --ignored --nocapture`

use std::time::Duration;

use ed25519_dalek::SigningKey;
use libp2p::futures::StreamExt;
use libp2p::request_response::{self, Message, ProtocolSupport};
use libp2p::swarm::SwarmEvent;
use libp2p::{Multiaddr, PeerId, noise, tcp, yamux};

use ledgerita_core::network::{MSG_SYNC, MSG_VOTE, NetworkId, RawCodec, build_msg, split_msg};
use ledgerita_core::quorum::{VotePayload, sign_qc_vote};
use ledgerita_core::state::State;

#[derive(libp2p::swarm::NetworkBehaviour)]
struct AttackBehaviour {
    rr: request_response::Behaviour<RawCodec>,
}

#[tokio::test]
#[ignore]
async fn test_live_testnet_rechaza_voto_y_snapshot_falso() {
    let target: Multiaddr =
        "/ip4/192.168.68.60/tcp/46565/p2p/12D3KooWNjCgE866sbqjr2626KEC3MGYKvYuVM6MvkuG5VhHvaaS"
            .parse()
            .unwrap();
    let target_peer: PeerId = "12D3KooWNjCgE866sbqjr2626KEC3MGYKvYuVM6MvkuG5VhHvaaS"
        .parse()
        .unwrap();

    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .unwrap()
        .with_behaviour(|_| {
            let protocols = vec![(NetworkId::Testnet.protocol_id(), ProtocolSupport::Full)];
            AttackBehaviour {
                rr: request_response::Behaviour::with_codec(
                    RawCodec,
                    protocols,
                    request_response::Config::default().with_request_timeout(Duration::from_secs(10)),
                ),
            }
        })
        .unwrap()
        .build();

    swarm.dial(target).unwrap();

    let mut got_invalid_vote = false;
    let mut sent_forged_snapshot = false;
    let deadline = tokio::time::sleep(Duration::from_secs(8));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            _ = &mut deadline => break,
            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == target_peer => {
                        // 1) Enviar MSG_VOTE falso firmado por clave no autorizada
                        let rogue = SigningKey::from_bytes(&[0x99u8; 32]);
                        let fake_vote = VotePayload {
                            chain_id: NetworkId::Testnet.chain_id(),
                            epoch: 99,
                            state_root: [0xABu8; 32],
                            vote: sign_qc_vote(&rogue, NetworkId::Testnet.chain_id(), 99, &[0xABu8; 32], 0),
                        };
                        let msg = build_msg(NetworkId::Testnet, MSG_VOTE, &fake_vote.encode());
                        swarm.behaviour_mut().rr.send_request(&peer_id, msg);
                    }
                    SwarmEvent::Behaviour(AttackBehaviourEvent::Rr(
                        request_response::Event::Message {
                            peer,
                            message: Message::Response { response, .. },
                            ..
                        }
                    )) if peer == target_peer => {
                        let rejected_vote = split_msg(NetworkId::Testnet, &response)
                            == Some((MSG_VOTE, b"INVALID_VOTE".as_slice()))
                            || response == b"INVALID_VOTE";
                        if rejected_vote {
                            got_invalid_vote = true;
                            println!("✅ kettle03 rechazó MSG_VOTE falso con INVALID_VOTE");
                        }
                    }
                    SwarmEvent::Behaviour(AttackBehaviourEvent::Rr(
                        request_response::Event::Message {
                            peer,
                            message: Message::Request { request, channel, .. },
                            ..
                        }
                    )) if peer == target_peer => {
                        if let Some((MSG_SYNC, _)) = split_msg(NetworkId::Testnet, &request) {
                            // 2) Inyectar snapshot falso con epoch=99 sin QC
                            let mut forged = State::new();
                            forged.epoch = 99;
                            forged.tx_count = 99 * 4096;
                            let snap = forged.encode_snapshot(NetworkId::Testnet);
                            let resp = build_msg(NetworkId::Testnet, MSG_SYNC, &snap);
                            let _ = swarm.behaviour_mut().rr.send_response(channel, resp);
                            sent_forged_snapshot = true;
                            println!("📤 Snapshot falso (epoch=99 sin QC) enviado a kettle03");
                        }
                    }
                    _ => {}
                }
            }
        }
        if got_invalid_vote && sent_forged_snapshot {
            tokio::time::sleep(Duration::from_millis(500)).await;
            break;
        }
    }

    assert!(got_invalid_vote, "kettle03 debe rechazar MSG_VOTE falso");
    assert!(sent_forged_snapshot, "debió enviarse el snapshot falso tras MSG_SYNC");
}
