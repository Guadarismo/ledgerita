use libp2p::{identity, noise, ping, swarm::NetworkBehaviour, swarm::SwarmEvent, tcp, yamux};
use std::error::Error;
use tokio::time::Duration;
use libp2p::futures::StreamExt;

#[derive(NetworkBehaviour)]
pub struct LedgeritaBehaviour {
    pub ping: ping::Behaviour,
}

pub async fn run_p2p_node() -> Result<(), Box<dyn Error>> {
    // 1. Generar identidad criptográfica de capa de red "PeerId"
    let local_key = identity::Keypair::generate_ed25519();
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

    // 3. Poner el nodo a escuchar en un puerto libre al azar en todas las interfaces de red
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    println!("Buscando vecinos y escuchando conexiones...");

    // 4. El Loop Eterno (Ciclo de Vida del Nodo P2P)
    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                println!("📡 Nodo Ledgerita escuchando activamente en: {}", address);
            }
            SwarmEvent::Behaviour(LedgeritaBehaviourEvent::Ping(ping_event)) => {
                println!("🔄 Apretón de Manos (Ping): {:?}", ping_event);
            }
            _ => {}
        }
    }
}
