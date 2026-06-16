use libp2p::{identity, noise, ping, swarm::NetworkBehaviour, swarm::SwarmEvent, tcp, yamux, Multiaddr};
use std::error::Error;
use tokio::time::Duration;
use libp2p::futures::StreamExt;

#[derive(NetworkBehaviour)]
pub struct LedgeritaBehaviour {
    pub ping: ping::Behaviour,
}

pub async fn run_p2p_node(
    local_key: identity::Keypair,
    connect_peers: Vec<Multiaddr>,
) -> Result<(), Box<dyn Error>> {
    // 1. Identidad criptográfica persistente (en lugar de generar una aleatoria cada vez)
    let local_peer_id = libp2p::PeerId::from(local_key.public());
    println!("🔌 Ledgerita Node Inicializado");
    println!("Mi Identidad P2P (PeerId): {:?}", local_peer_id);

    // 2. Construir el Swarm P2P (Administrador de Enjambre)
    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(local_key)
        .with_tokio() // Runtime asíncrono hiperrápido
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new, // Noise Protocol para Encriptación Zero-Trust P2P
            yamux::Config::default, // Multiplexación de streams
        )?
        .with_behaviour(|_| LedgeritaBehaviour {
            ping: ping::Behaviour::default(),
        })?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    // 3. Poner el nodo a escuchar en un puerto libre al azar
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    println!("Buscando vecinos y escuchando conexiones...");

    // 4. Conectar activamente a los pares conocidos
    for peer in &connect_peers {
        println!("🔗 Intentando conectar a: {}", peer);
        match swarm.dial(peer.clone()) {
            Ok(()) => {}
            Err(e) => eprintln!("⚠️  Error al conectar a {}: {:?}", peer, e),
        }
    }

    // 5. El Loop Eterno (Ciclo de Vida del Nodo P2P)
    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                println!("📡 Nodo Ledgerita escuchando activamente en: {}", address);
            }
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                println!("✅ Conexión establecida con: {:?}", peer_id);
            }
            SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                println!("🔴 Desconectado de {:?}: {:?}", peer_id, cause);
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                if let Some(pid) = peer_id {
                    eprintln!("⚠️  Error de conexión saliente a {:?}: {:?}", pid, error);
                } else {
                    eprintln!("⚠️  Error de conexión saliente: {:?}", error);
                }
            }
            SwarmEvent::Behaviour(LedgeritaBehaviourEvent::Ping(ping_event)) => {
                println!("🔄 Apretón de Manos (Ping): {:?}", ping_event);
            }
            _ => {}
        }
    }
}
