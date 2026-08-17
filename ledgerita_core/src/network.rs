use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use async_trait::async_trait;
use futures::io::{AsyncReadExt, AsyncWriteExt};
use libp2p::futures::StreamExt;
use libp2p::identity;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{self, Codec, Message, ProtocolSupport};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::tcp;
use libp2p::yamux;
use libp2p::{noise, Multiaddr};
use tracing::{error, info, trace, warn};

use crate::crypto::Address;
use crate::serialize::{decode_transaction, encode_transaction, hash_tx};
use crate::transaction::Transaction;
use crate::state::{State, SyncResult};
use crate::storage::Storage;

pub type Mempool = Arc<Mutex<HashSet<[u8; 32]>>>;

pub const MSG_TX: u8 = 0x01;
pub const MSG_SYNC: u8 = 0x02;

/// Identifica una red Ledgerita. Cada red tiene su propio protocolo libp2p
/// y su propio `chain_id`, de modo que nodos de redes distintas no pueden
/// negociar conexión ni intercambiar mensajes (aislamiento TestNet/MainNet).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetworkId {
    Mainnet,
    Testnet,
}

impl NetworkId {
    pub const CHAIN_ID_MAINNET: u8 = 0x01;
    pub const CHAIN_ID_TESTNET: u8 = 0x02;

    pub fn chain_id(self) -> u8 {
        match self {
            NetworkId::Mainnet => Self::CHAIN_ID_MAINNET,
            NetworkId::Testnet => Self::CHAIN_ID_TESTNET,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            NetworkId::Mainnet => "mainnet",
            NetworkId::Testnet => "testnet",
        }
    }

    /// Identificador del protocolo libp2p (capa de enrutamiento).
    /// Redes distintas no negocian el mismo protocolo → no se conectan.
    pub fn protocol_id(self) -> String {
        format!("/ledgerita/1/{}", self.tag())
    }

    pub fn from_str(s: &str) -> Option<NetworkId> {
        match s.to_ascii_lowercase().as_str() {
            "mainnet" => Some(NetworkId::Mainnet),
            "testnet" => Some(NetworkId::Testnet),
            _ => None,
        }
    }
}

impl std::fmt::Display for NetworkId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.tag())
    }
}

/// Construye un mensaje de red con el framing `[chain_id][msg_type][payload]`.
pub fn build_msg(net: NetworkId, msg_type: u8, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(2 + payload.len());
    buf.push(net.chain_id());
    buf.push(msg_type);
    buf.extend_from_slice(payload);
    buf
}

/// Valida el `chain_id` de un mensaje y devuelve `(msg_type, payload)`.
/// Devuelve `None` si el mensaje es demasiado corto o pertenece a otra red.
pub fn split_msg(net: NetworkId, data: &[u8]) -> Option<(u8, &[u8])> {
    if data.len() < 2 {
        return None;
    }
    if data[0] != net.chain_id() {
        return None;
    }
    Some((data[1], &data[2..]))
}

/// Códec request/response en bruto para libp2p.
///
/// Serializa cada mensaje como una longitud de 4 bytes en big-endian
/// seguida de la carga útil (`Vec<u8>`) sin ningún envoltorio adicional.
/// El protocolo sobre el que se usa (ver [`MSG_TX`] / [`MSG_SYNC`]) define
/// el formato interno de esa carga útil.
#[derive(Default, Clone)]
pub struct RawCodec;

#[async_trait]
impl Codec for RawCodec {
    type Protocol = String;
    type Request = Vec<u8>;
    type Response = Vec<u8>;

    async fn read_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> std::io::Result<Self::Request>
    where
        T: futures::io::AsyncRead + Unpin + Send,
    {
        let mut len_buf = [0u8; 4];
        io.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        io.read_exact(&mut buf).await?;
        Ok(buf)
    }

    async fn read_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> std::io::Result<Self::Response>
    where
        T: futures::io::AsyncRead + Unpin + Send,
    {
        let mut len_buf = [0u8; 4];
        io.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        io.read_exact(&mut buf).await?;
        Ok(buf)
    }

    async fn write_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        req: Self::Request,
    ) -> std::io::Result<()>
    where
        T: futures::io::AsyncWrite + Unpin + Send,
    {
        let len = req.len() as u32;
        io.write_all(&len.to_be_bytes()).await?;
        io.write_all(&req).await?;
        Ok(())
    }

    async fn write_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        res: Self::Response,
    ) -> std::io::Result<()>
    where
        T: futures::io::AsyncWrite + Unpin + Send,
    {
        let len = res.len() as u32;
        io.write_all(&len.to_be_bytes()).await?;
        io.write_all(&res).await?;
        Ok(())
    }
}

#[derive(NetworkBehaviour)]
pub struct LedgeritaBehaviour {
    pub ping: libp2p::ping::Behaviour,
    pub rr: request_response::Behaviour<RawCodec>,
}

fn extract_peer_id(addr: &Multiaddr) -> Option<libp2p::PeerId> {
    for proto in addr.iter() {
        if let Protocol::P2p(peer_id) = proto {
            return Some(peer_id);
        }
    }
    None
}

/// Arranca el nodo P2P de Ledgerita y mantiene su bucle de eventos en ejecución.
///
/// Crea un [`libp2p::Swarm`] con identidad `local_key`, escucha en `listen_port`
/// (TCP) y diala a `connect_peers`. El bucle procesa: transacciones entrantes
/// (broadcast a la red y aplicación al `state`), peticiones de sync (envío del
/// snapshot local), resultados de sync, y eventos de comportamiento de red.
///
/// `network` determina el protocolo libp2p y el `chain_id` de cada mensaje;
/// solo nodos de la misma red pueden conectarse y sincronizar.
///
/// No retorna a menos que ocurra un error fatal; en condiciones normales corre
/// indefinidamente.
pub async fn run_p2p_node(
    local_key: identity::Keypair,
    connect_peers: Vec<Multiaddr>,
    state: Arc<Mutex<State>>,
    storage: Arc<Storage>,
    wallet_address: Address,
    mempool: Mempool,
    pending_txs: Vec<Transaction>,
    pending_broadcast: Arc<Mutex<Vec<Vec<u8>>>>,
    listen_port: u16,
    network: NetworkId,
) -> Result<(), Box<dyn Error>> {
    let local_peer_id = libp2p::PeerId::from(local_key.public());
    info!("Inicializando nodo Ledgerita ({})", network);
    info!(peer_id = %local_peer_id, "Identidad P2P");

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|_| {
            let protocols = vec![
                (network.protocol_id(), ProtocolSupport::Full),
            ];
            LedgeritaBehaviour {
                ping: libp2p::ping::Behaviour::default(),
                rr: request_response::Behaviour::with_codec(
                    RawCodec,
                    protocols,
                    request_response::Config::default()
                        .with_request_timeout(Duration::from_secs(15)),
                ),
            }
        })?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(300)))
        .build();

    let listen_addr = format!("/ip4/0.0.0.0/tcp/{}", listen_port);
    swarm.listen_on(listen_addr.parse()?)?;

    info!("Escuchando conexiones entrantes");

    let mut pending_peers: HashMap<libp2p::PeerId, Multiaddr> = HashMap::new();
    for peer in &connect_peers {
        info!(%peer, "Iniciando conexión saliente");
        if let Some(peer_id) = extract_peer_id(peer) {
            pending_peers.insert(peer_id, peer.clone());
        }
        match swarm.dial(peer.clone()) {
            Ok(()) => {}
            Err(e) => warn!(%peer, error = %e, "Error al conectar"),
        }
    }

    let mut retry_interval = tokio::time::interval(Duration::from_secs(30));
    retry_interval.tick().await;

    if !pending_txs.is_empty() {
        let mut s = state.lock().await;
        for tx in &pending_txs {
            if !tx.is_signature_valid() {
                warn!("TX local omitida: firma inválida");
                continue;
            }
            let hash = hash_tx(tx);
            mempool.lock().await.insert(hash);
            match s.apply_transaction(tx) {
                Ok(()) => {
                    if let Err(e) = s.flush(&storage) {
                        error!(error = %e, "Error persistiendo TX local");
                    }
                    if s.is_checkpoint_due() {
                        let root = s.finalize_checkpoint(std::slice::from_ref(&wallet_address));
                        info!(epoch = s.epoch, "Checkpoint completado (TX local)");
                        let _ = root;
                        if let Err(e) = s.flush(&storage) {
                            error!(error = %e, "Error persistiendo checkpoint");
                        }
                    }
                    pending_broadcast.lock().await.push(encode_transaction(tx));
                    info!(
                        sender = %tx.sender.to_hex()[..16].to_string(),
                        receiver = %tx.receiver.to_hex()[..16].to_string(),
                        amount = tx.amount,
                        nonce = tx.nonce,
                        "TX local aplicada"
                    );
                }
                Err(e) => {
                    warn!("TX local rechazada: {}", e);
                }
            }
        }
        drop(s);
        let pending_len = pending_broadcast.lock().await.len();
        if pending_len > 0 {
            info!(count = pending_len, "TX(s) lista(s) para broadcast a la red");
        }
    }

    loop {
        tokio::select! {
            _ = retry_interval.tick() => {
                for (peer_id, addr) in pending_peers.clone() {
                    warn!(%peer_id, "Reintentando conexión P2P");
                    let _ = swarm.dial(addr);
                }
            }
            ev = swarm.select_next_some() => match ev {
                SwarmEvent::NewListenAddr { address, .. } => {
                    info!(%address, "Nodo escuchando activamente");
                }
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    info!(%peer_id, "Conexión establecida");
                    if pending_peers.remove(&peer_id).is_some() {
                        info!(%peer_id, "Peer retirado de lista de reintentos");
                    }

                    let s = state.lock().await;
                    let root = s.generate_state_root();
                    let msg = build_msg(network, MSG_SYNC, &root);
                    swarm.behaviour_mut().rr.send_request(&peer_id, msg);
                }
                SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                    info!(%peer_id, ?cause, "Desconectado");
                    // Re-agregar el peer a la lista de reintentos: si la conexión
                    // se cae (KeepAliveTimeout, red, etc.), el dial automático de
                    // 30s debe volver a intentar conectarlo. Sin esto, un peer
                    // conectado una vez jamás se reconecta (bug de producción:
                    // la red quedaba silenciosa tras ~60s).
                    if let Some(addr) = connect_peers.iter().find(|a| {
                        extract_peer_id(a).map(|pid| pid == peer_id).unwrap_or(false)
                    }) {
                        pending_peers.insert(peer_id, addr.clone());
                        info!(%peer_id, "Peer re-agregado a lista de reintentos");
                    }
                }
                SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                    if let Some(pid) = peer_id {
                        warn!(%pid, error = %error, "Error de conexión saliente");
                    } else {
                        warn!(error = %error, "Error de conexión saliente");
                    }
                }
                SwarmEvent::Behaviour(LedgeritaBehaviourEvent::Ping(ping_event)) => {
                    match ping_event {
                        libp2p::ping::Event { result: Ok(rtt), .. } => {
                            info!(?rtt, "Ping exitoso");
                        }
                        libp2p::ping::Event { result: Err(e), .. } => {
                            warn!(error = %e, "Ping error");
                        }
                    }
                }
                SwarmEvent::Behaviour(LedgeritaBehaviourEvent::Rr(
                    request_response::Event::Message {
                        peer,
                        connection_id: _,
                        message: Message::Request {
                            request_id: _,
                            request,
                            channel,
                        },
                    },
                )) => {
                    if request.len() < 2 {
                        let _ = swarm.behaviour_mut().rr.send_response(channel, b"EMPTY".to_vec());
                        continue;
                    }
                    let (msg_type, payload) = match split_msg(network, &request) {
                        Some(x) => x,
                        None => {
                            warn!(%peer, "Solicitud de otra red (chain_id inválido)");
                            let _ = swarm.behaviour_mut().rr.send_response(
                                channel,
                                b"BADNET".to_vec(),
                            );
                            continue;
                        }
                    };
                    match msg_type {
                        MSG_TX => {
                            let tx_bytes = payload;
                            let decoded = match decode_transaction(tx_bytes) {
                                Ok(tx) => tx,
                                Err(e) => {
                                    warn!(%peer, error = %e, "TX inválida recibida");
                                    let _ = swarm.behaviour_mut().rr.send_response(
                                        channel,
                                        b"INVALID".to_vec(),
                                    );
                                    continue;
                                }
                            };
                            let hash = hash_tx(&decoded);
                            {
                                let mp = mempool.lock().await;
                                if mp.contains(&hash) {
                                    let _ = swarm.behaviour_mut().rr.send_response(
                                        channel,
                                        b"DUPLICATE".to_vec(),
                                    );
                                    continue;
                                }
                            }
                            if !decoded.is_signature_valid() {
                                warn!(%peer, "Firma inválida en TX recibida");
                                let _ = swarm.behaviour_mut().rr.send_response(
                                    channel,
                                    b"INVALID_SIG".to_vec(),
                                );
                                continue;
                            }
                            let mut s = state.lock().await;
                            match s.apply_transaction(&decoded) {
                                Ok(()) => {
                                    mempool.lock().await.insert(hash);
                                    if let Err(e) = s.flush(&storage) {
                                        error!(error = %e, "Error persistiendo estado tras TX");
                                    }
                                    if s.is_checkpoint_due() {
                                        let root = s.finalize_checkpoint(std::slice::from_ref(&wallet_address));
                                        let root_hex: String = root[..4]
                                            .iter()
                                            .map(|b| format!("{:02x}", b))
                                            .collect();
                                        info!(
                                            epoch = s.epoch,
                                            root = %root_hex,
                                            "Checkpoint completado"
                                        );
                                        if let Err(e) = s.flush(&storage) {
                                            error!(error = %e, "Error persistiendo checkpoint");
                                        }
                                    }
                                    let _ = swarm.behaviour_mut().rr.send_response(
                                        channel,
                                        b"OK".to_vec(),
                                    );
                                    info!(%peer, "Transacción aplicada");

                                    let connected: Vec<_> =
                                        swarm.connected_peers().cloned().collect();
                                    for pid in connected {
                                        if pid != peer {
                                            let fwd = build_msg(network, MSG_TX, tx_bytes);
                                            swarm.behaviour_mut().rr.send_request(&pid, fwd);
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!(%peer, error = %e, "Transacción rechazada");
                                    let _ = swarm.behaviour_mut().rr.send_response(
                                        channel,
                                        format!("REJECT:{}", e).into_bytes(),
                                    );
                                }
                            }
                        }
                        MSG_SYNC => {
                            if payload.len() < 32 {
                                let _ = swarm.behaviour_mut().rr.send_response(
                                    channel,
                                    b"INVALID_SYNC".to_vec(),
                                );
                                continue;
                            }
                            let s = state.lock().await;
                            let snapshot = s.encode_snapshot(network);
                            let resp = build_msg(network, MSG_SYNC, &snapshot);
                            let _ = swarm.behaviour_mut().rr.send_response(channel, resp);
                            let count = s.balances.len();
                            info!(%peer, count, "Snapshot de estado enviado");
                            drop(s);

                            let pending = {
                                let mut pb = pending_broadcast.lock().await;
                                let txs = pb.clone();
                                pb.clear();
                                txs
                            };
                            if !pending.is_empty() {
                                let connected: Vec<_> = swarm.connected_peers().cloned().collect();
                                for pid in &connected {
                                    for tx_bytes in &pending {
                                        let msg = build_msg(network, MSG_TX, tx_bytes);
                                        swarm.behaviour_mut().rr.send_request(pid, msg);
                                    }
                                }
                                info!(
                                    count = pending.len(),
                                    peers = connected.len(),
                                    "TX(s) pendiente(s) enviada(s) tras sync"
                                );
                            }
                        }
                        _ => {
                            let _ = swarm.behaviour_mut().rr.send_response(
                                channel,
                                b"UNKNOWN".to_vec(),
                            );
                        }
                    }
                }
                SwarmEvent::Behaviour(LedgeritaBehaviourEvent::Rr(
                    request_response::Event::Message {
                        peer,
                        connection_id: _,
                        message: Message::Response { response, .. },
                    },
                )) => {
                    if response.len() < 2 {
                        continue;
                    }
                    let (msg_type, payload) = match split_msg(network, &response) {
                        Some(x) => x,
                        None => {
                            warn!(%peer, "Respuesta de otra red (chain_id inválido)");
                            continue;
                        }
                    };
                    match msg_type {
                        MSG_SYNC => {
                            let snapshot_data = payload;
                            let mut s = state.lock().await;
                            match State::decode_snapshot(snapshot_data, network) {
                                Ok(remote) => {
                                    match s.merge_if_fresher(&remote, &storage) {
                                        Ok(SyncResult::RemoteAdopted) => {
                                            info!(
                                                %peer,
                                                count = s.balances.len(),
                                                epoch = s.epoch,
                                                tx_count = s.tx_count,
                                                "Estado sincronizado"
                                            );
                                        }
                                        Ok(SyncResult::LocalNewer) => {
                                            info!(
                                                %peer,
                                                local_epoch = s.epoch,
                                                local_tx = s.tx_count,
                                                remote_epoch = remote.epoch,
                                                remote_tx = remote.tx_count,
                                                "Estado local más reciente — conservando"
                                            );
                                        }
                                        Ok(SyncResult::AlreadySynced) => {
                                            info!(%peer, "Estado ya sincronizado");
                                        }
                                        Ok(SyncResult::Divergent) => {
                                            warn!(
                                                %peer,
                                                epoch = s.epoch,
                                                tx_count = s.tx_count,
                                                "Estado divergente: misma época pero diferente state root"
                                            );
                                        }
                                        Err(e) => {
                                            error!(error = %e, "Error persistiendo estado sincronizado");
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!(%peer, error = %e, "Snapshot inválido recibido");
                                }
                            }
                            drop(s);

                            let pending = {
                                let mut pb = pending_broadcast.lock().await;
                                let txs = pb.clone();
                                pb.clear();
                                txs
                            };
                            if !pending.is_empty() {
                                let connected: Vec<_> = swarm.connected_peers().cloned().collect();
                                for pid in &connected {
                                    for tx_bytes in &pending {
                                        let msg = build_msg(network, MSG_TX, tx_bytes);
                                        swarm.behaviour_mut().rr.send_request(pid, msg);
                                    }
                                }
                                info!(
                                    count = pending.len(),
                                    peers = connected.len(),
                                    "TX(s) pendiente(s) enviada(s) tras sync"
                                );
                            }
                        }
                        MSG_TX => {
                            let status = String::from_utf8_lossy(payload);
                            info!(%peer, %status, "Respuesta TX recibida");
                        }
                        _ => {}
                    }
                }
                SwarmEvent::Behaviour(LedgeritaBehaviourEvent::Rr(
                    request_response::Event::OutboundFailure {
                        peer, request_id, error, ..
                    },
                )) => {
                    warn!(%peer, ?request_id, error = %error, "Error de solicitud");
                }
                SwarmEvent::Behaviour(LedgeritaBehaviourEvent::Rr(
                    request_response::Event::ResponseSent { peer, request_id, .. },
                )) => {
                    trace!(%peer, ?request_id, "Respuesta enviada");
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_request_format() {
        let root = [0xab; 32];
        let msg = build_msg(NetworkId::Mainnet, MSG_SYNC, &root);
        assert_eq!(msg.len(), 34);
        assert_eq!(msg[0], NetworkId::CHAIN_ID_MAINNET);
        let (msg_type, payload) = split_msg(NetworkId::Mainnet, &msg).unwrap();
        assert_eq!(msg_type, MSG_SYNC);
        assert_eq!(payload, &root[..]);
        assert!(
            split_msg(NetworkId::Testnet, &msg).is_none(),
            "un mensaje de mainnet debe ser rechazado en testnet"
        );
    }

    #[test]
    fn test_network_id_protocol_strings_differ() {
        assert_ne!(
            NetworkId::Mainnet.protocol_id(),
            NetworkId::Testnet.protocol_id()
        );
        assert_eq!(NetworkId::Mainnet.protocol_id(), "/ledgerita/1/mainnet");
        assert_eq!(NetworkId::Testnet.protocol_id(), "/ledgerita/1/testnet");
        assert_eq!(NetworkId::Mainnet.chain_id(), 0x01);
        assert_eq!(NetworkId::Testnet.chain_id(), 0x02);
        assert_ne!(NetworkId::Mainnet.chain_id(), NetworkId::Testnet.chain_id());
    }

    #[test]
    fn test_network_id_from_str() {
        assert_eq!(NetworkId::from_str("mainnet"), Some(NetworkId::Mainnet));
        assert_eq!(NetworkId::from_str("testnet"), Some(NetworkId::Testnet));
        assert_eq!(NetworkId::from_str("MAINNET"), Some(NetworkId::Mainnet));
        assert_eq!(NetworkId::from_str("MainNet"), Some(NetworkId::Mainnet));
        assert_eq!(NetworkId::from_str("otro"), None);
        assert_eq!(NetworkId::from_str(""), None);
    }

    #[test]
    fn test_split_msg_too_short_and_badnet() {
        assert!(split_msg(NetworkId::Mainnet, b"").is_none());
        assert!(split_msg(NetworkId::Mainnet, b"\x01").is_none());
        let wrong_net = build_msg(NetworkId::Testnet, MSG_TX, &[1, 2, 3]);
        assert!(split_msg(NetworkId::Mainnet, &wrong_net).is_none());
    }
}
