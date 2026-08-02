use std::time::{Duration, Instant};

use futures::StreamExt;
use libp2p::request_response::{self, Message};
use libp2p::swarm::SwarmEvent;
use libp2p::{identity, Multiaddr, PeerId};
use ledgerita_core::network::{build_msg, MSG_SYNC, NetworkId};

mod helpers;
use helpers::{build_test_swarm, wait_for_listen_addr};

/// Nodos de redes distintas no pueden intercambiar estado del ledger: aunque el
/// transporte TCP se establece, la negociación del protocolo
/// `/ledgerita/1/{red}` fracasa y la solicitud `MSG_SYNC` termina en
/// `OutboundFailure` (nunca llega una respuesta).
#[tokio::test]
async fn test_network_isolation_p2p_mainnet_vs_testnet() {
    let key_main = identity::Keypair::generate_ed25519();
    let key_test = identity::Keypair::generate_ed25519();
    let peer_main = PeerId::from(key_main.public());

    let mut swarm_main = build_test_swarm(key_main, NetworkId::Mainnet);
    let mut swarm_test = build_test_swarm(key_test, NetworkId::Testnet);

    swarm_main
        .listen_on("/ip4/127.0.0.1/tcp/0".parse().unwrap())
        .unwrap();
    let la = wait_for_listen_addr(&mut swarm_main).await;
    let addr_main_full: Multiaddr =
        format!("{}/p2p/{}", la, peer_main.to_base58()).parse().unwrap();

    tokio::spawn(async move {
        let mut s = swarm_main;
        loop {
            let _ = s.select_next_some().await;
        }
    });

    let _ = swarm_test.dial(addr_main_full);

    // 1) La conexión de transporte se establece (aún sin protocolo de red).
    let connected = wait_until(&mut swarm_test, |ev| {
        matches!(ev, SwarmEvent::ConnectionEstablished { peer_id, .. } if *peer_id == peer_main)
    })
    .await;
    assert!(connected, "El transporte TCP debe establecerse entre los nodos");

    // 2) La solicitud de sync fracasa: no se negocia el protocolo de la otra red.
    let req = build_msg(NetworkId::Testnet, MSG_SYNC, &[0u8; 32]);
    swarm_test.behaviour_mut().rr.send_request(&peer_main, req);

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if Instant::now() > deadline {
            panic!("se esperaba OutboundFailure al sync entre redes");
        }
        match tokio::time::timeout(Duration::from_millis(200), swarm_test.select_next_some()).await {
            Ok(ev) => match ev {
                SwarmEvent::Behaviour(helpers::TestBehaviourEvent::Rr(
                    request_response::Event::OutboundFailure { peer, .. },
                )) if peer == peer_main => {
                    break; // El sync entre redes fue rechazado: aislamiento correcto.
                }
                SwarmEvent::Behaviour(helpers::TestBehaviourEvent::Rr(
                    request_response::Event::Message {
                        peer,
                        message: Message::Response { .. },
                        ..
                    },
                )) if peer == peer_main => {
                    panic!("no debe existir respuesta de sync entre redes distintas");
                }
                _ => {}
            },
            Err(_) => {}
        }
    }
}

/// Control: dos swarms de la MISMA red se conectan en el transporte.
#[tokio::test]
async fn test_same_network_connects() {
    let key_a = identity::Keypair::generate_ed25519();
    let key_b = identity::Keypair::generate_ed25519();
    let peer_a = PeerId::from(key_a.public());

    let mut swarm_a = build_test_swarm(key_a, NetworkId::Testnet);
    let mut swarm_b = build_test_swarm(key_b, NetworkId::Testnet);

    swarm_a
        .listen_on("/ip4/127.0.0.1/tcp/0".parse().unwrap())
        .unwrap();
    let la = wait_for_listen_addr(&mut swarm_a).await;
    let addr_a_full: Multiaddr =
        format!("{}/p2p/{}", la, peer_a.to_base58()).parse().unwrap();

    let _ = swarm_b.dial(addr_a_full);

    tokio::spawn(async move {
        let mut s = swarm_a;
        loop {
            let _ = s.select_next_some().await;
        }
    });

    let connected = wait_until(&mut swarm_b, |ev| {
        matches!(ev, SwarmEvent::ConnectionEstablished { peer_id, .. } if *peer_id == peer_a)
    })
    .await;
    assert!(connected, "Dos nodos de la misma red deben conectarse");
}

async fn wait_until<F>(swarm: &mut libp2p::Swarm<helpers::TestBehaviour>, mut pred: F) -> bool
where
    F: FnMut(&SwarmEvent<helpers::TestBehaviourEvent>) -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), swarm.select_next_some()).await {
            Ok(ev) => {
                if pred(&ev) {
                    return true;
                }
            }
            Err(_) => {}
        }
    }
    false
}
