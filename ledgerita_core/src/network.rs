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
) -> Result<(), Box<dyn Error>> {
    let local_peer_id = libp2p::PeerId::from(local_key.public());
    info!("Inicializando nodo Ledgerita");
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
                ("/ledgerita/1".to_string(), ProtocolSupport::Full),
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
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(60)))
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
                    let mut msg = vec![MSG_SYNC];
                    msg.extend_from_slice(&root);
                    swarm.behaviour_mut().rr.send_request(&peer_id, msg);
                }
                SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                    info!(%peer_id, ?cause, "Desconectado");
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
                    if request.is_empty() {
                        let _ = swarm.behaviour_mut().rr.send_response(channel, b"EMPTY".to_vec());
                        continue;
                    }
                    match request[0] {
                        MSG_TX => {
                            let tx_bytes = &request[1..];
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
                                            let mut fwd = vec![MSG_TX];
                                            fwd.extend_from_slice(tx_bytes);
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
                            if request.len() < 33 {
                                let _ = swarm.behaviour_mut().rr.send_response(
                                    channel,
                                    b"INVALID_SYNC".to_vec(),
                                );
                                continue;
                            }
                            let s = state.lock().await;
                            let snapshot = s.encode_snapshot();
                            let mut resp = vec![MSG_SYNC];
                            resp.extend_from_slice(&snapshot);
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
                                        let mut msg = vec![MSG_TX];
                                        msg.extend_from_slice(tx_bytes);
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
                    if response.is_empty() {
                        continue;
                    }
                    match response[0] {
                        MSG_SYNC => {
                            let snapshot_data = &response[1..];
                            let mut s = state.lock().await;
                            match State::decode_snapshot(snapshot_data) {
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
                                        let mut msg = vec![MSG_TX];
                                        msg.extend_from_slice(tx_bytes);
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
                            let status = String::from_utf8_lossy(&response[1..]);
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
        let mut msg = vec![MSG_SYNC];
        msg.extend_from_slice(&root);
        assert_eq!(msg.len(), 33);
        assert_eq!(msg[0], MSG_SYNC);
        assert_eq!(&msg[1..], &root[..]);
    }
}
