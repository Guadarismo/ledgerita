use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::defense::{PeerGuard, StrikeKind};

use async_trait::async_trait;
use futures::io::{AsyncReadExt, AsyncWriteExt};
use libp2p::futures::StreamExt;
use libp2p::identity;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{self, Codec, Message, ProtocolSupport};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::tcp;
use libp2p::yamux;
use libp2p::{Multiaddr, noise};
use tracing::{debug, error, info, trace, warn};

use crate::crypto::Address;
use crate::quorum::{
    QuorumCertificate, ValidatorSet, VoteCollector, VotePayload, sign_qc_vote,
    verify_vote_signature,
};
use crate::serialize::{decode_transaction, encode_transaction, hash_tx};
use crate::state::{ForkResolution, State, SyncResult};
use crate::storage::Storage;
use crate::transaction::Transaction;

pub type Mempool = Arc<Mutex<HashSet<[u8; 32]>>>;

pub const MSG_TX: u8 = 0x01;
pub const MSG_SYNC: u8 = 0x02;
pub const MSG_VOTE: u8 = 0x03;
pub const MSG_SYNC_ALREADY: u8 = 0x04;

/// Lista ordenada de semillas de fallback por red (Checkpoint 44 / 49).
/// Incluye la IP actual del seed (`kettle03` .60) y el respaldo secundario (`kettle02` .64),
/// ambos autenticados con `/p2p/<PeerId>`.
pub const FALLBACK_SEEDS_MAINNET: &[&str] = &[
    "/ip4/192.168.68.60/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1",
    "/ip4/192.168.68.64/tcp/36565/p2p/12D3KooWGrahZ1gW5pFBu9Q1U1QEryPyEUGh8rCGGJJzuuPoHLdu",
];

pub const FALLBACK_SEEDS_TESTNET: &[&str] = &[
    "/ip4/192.168.68.60/tcp/46565/p2p/12D3KooWNjCgE866sbqjr2626KEC3MGYKvYuVM6MvkuG5VhHvaaS",
    "/ip4/192.168.68.64/tcp/46565/p2p/12D3KooWCvE6SWUeqx3j5US3ya3Gs9Mh3yKZLYubxEqxDVtAymit",
];

/// Devuelve las semillas de fallback configuradas para la red indicada.
pub fn default_fallback_seeds(network: NetworkId) -> &'static [&'static str] {
    match network {
        NetworkId::Mainnet => FALLBACK_SEEDS_MAINNET,
        NetworkId::Testnet => FALLBACK_SEEDS_TESTNET,
    }
}

/// Parsea las semillas de fallback de `network` excluyendo el propio `local_peer_id`
/// para evitar auto-marcados al arrancar sin `--connect` (Checkpoint 49).
pub fn fallback_seeds_for_peer(
    network: NetworkId,
    local_peer_id: Option<&libp2p::PeerId>,
) -> Vec<Multiaddr> {
    default_fallback_seeds(network)
        .iter()
        .filter_map(|raw| raw.parse::<Multiaddr>().ok())
        .filter(|ma| match (extract_peer_id(ma), local_peer_id) {
            (Some(pid), Some(self_pid)) => &pid != self_pid,
            _ => true,
        })
        .collect()
}

/// Resuelve multiaddrs DNS (`/dns/...`, `/dns4/...`, `/dns6/...`) de forma asíncrona
/// con timeout de 5 segundos hacia sus equivalentes `/ip4/...` o `/ip6/...`, preservando
/// puerto TCP y `/p2p/<PeerId>` (Checkpoint 44 / 49).
/// Si el multiaddr ya es IP, falla o vence el timeout, devuelve `[addr.clone()]`.
pub async fn resolve_multiaddr_dns(addr: &Multiaddr) -> Vec<Multiaddr> {
    use std::net::IpAddr;

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum DnsKind {
        Any,
        V4,
        V6,
    }

    let mut dns_host: Option<(DnsKind, String)> = None;
    let mut tcp_port: Option<u16> = None;
    let mut p2p_peer: Option<libp2p::PeerId> = None;

    for proto in addr.iter() {
        match proto {
            Protocol::Dns(h) => dns_host = Some((DnsKind::Any, h.to_string())),
            Protocol::Dns4(h) => dns_host = Some((DnsKind::V4, h.to_string())),
            Protocol::Dns6(h) => dns_host = Some((DnsKind::V6, h.to_string())),
            Protocol::Tcp(p) => tcp_port = Some(p),
            Protocol::P2p(pid) => p2p_peer = Some(pid),
            _ => {}
        }
    }

    let (Some((kind, host)), Some(port)) = (dns_host, tcp_port) else {
        return vec![addr.clone()];
    };

    let lookup = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::lookup_host((host.as_str(), port)),
    )
    .await;
    let addrs = match lookup {
        Ok(Ok(iter)) => iter,
        Ok(Err(_)) | Err(_) => {
            warn!(host = %host, port, "No se pudo resolver DNS para multiaddr");
            return vec![addr.clone()];
        }
    };

    let mut resolved = Vec::new();
    for sock in addrs {
        let mut ma = Multiaddr::empty();
        match (kind, sock.ip()) {
            (DnsKind::V4, IpAddr::V4(ip)) | (DnsKind::Any, IpAddr::V4(ip)) => {
                ma.push(Protocol::Ip4(ip));
            }
            (DnsKind::V6, IpAddr::V6(ip)) | (DnsKind::Any, IpAddr::V6(ip)) => {
                ma.push(Protocol::Ip6(ip));
            }
            _ => continue,
        }
        ma.push(Protocol::Tcp(sock.port()));
        if let Some(pid) = p2p_peer {
            ma.push(Protocol::P2p(pid));
        }
        if !resolved.contains(&ma) {
            resolved.push(ma);
        }
    }

    if resolved.is_empty() {
        vec![addr.clone()]
    } else {
        resolved
    }
}

/// Construye la respuesta óptima para un `MSG_SYNC` (Checkpoint 49):
/// si el `remote_root` coincide con nuestro root local actual, responde solo 2 bytes
/// (`MSG_SYNC_ALREADY`); en caso contrario serializa el snapshot completo.
pub fn build_sync_response(
    network: NetworkId,
    state: &State,
    remote_root: &[u8],
) -> (Vec<u8>, bool) {
    if remote_root.len() == 32 && state.generate_state_root().as_slice() == remote_root {
        (build_msg(network, MSG_SYNC_ALREADY, &[]), true)
    } else {
        let snapshot = state.encode_snapshot(network);
        (build_msg(network, MSG_SYNC, &snapshot), false)
    }
}

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
/// Prueba de Tiempo (unidad C): vuelca la reputación del guard al tree
/// `peers` de Storage. Llamar con baja frecuencia (tick de 30 s): los
/// upsert van bufferizados por sled y el flush() real ocurre en los
/// checkpoints — el peor caso es perder los últimos segundos de contadores.
fn export_peers_node_local(storage: &Storage, guard: &PeerGuard) {
    let registros = guard.export_records(Instant::now(), crate::defense::unix_millis_now());
    let mut guardados = 0usize;
    for (pid, rec) in &registros {
        if let Err(e) = storage.save_peer(pid, rec) {
            warn!(error = %e, "Error persistiendo reputación de peer (unidad C)");
        } else {
            guardados += 1;
        }
    }
    if guardados > 0 {
        let _ = storage.flush();
    }
}

/// Configuración opcional de quórum BFT ligero para el nodo P2P.
#[derive(Clone, Default)]
pub struct QuorumNodeConfig {
    /// Conjunto de validadores autorizados para verificar Gate `g7` y votos `MSG_VOTE`.
    pub validator_set: Option<ValidatorSet>,
    /// Identidad de validador local `(validator_id, signing_key)` si este nodo firma checkpoints.
    pub local_validator: Option<(u8, ed25519_dalek::SigningKey)>,
}

/// Procesa un payload `MSG_VOTE` entrante: valida el voto contra `ValidatorSet`,
/// lo agrega al `VoteCollector` y, si completa quórum para la época actual de
/// `state`, adjunta el `QuorumCertificate` resultante. Ante voto inválido registra
/// strike en `PeerGuard`.
#[allow(clippy::too_many_arguments)]
pub fn process_incoming_vote(
    payload: &[u8],
    network: NetworkId,
    peer: &libp2p::PeerId,
    validator_set: Option<&ValidatorSet>,
    collector: &mut VoteCollector,
    state: &mut State,
    guard: &mut PeerGuard,
    now: Instant,
) -> Result<Option<QuorumCertificate>, ()> {
    let vote_payload = match VotePayload::decode(payload) {
        Ok(v) => v,
        Err(_) => {
            guard.note_strike(peer, StrikeKind::SnapshotInvalido, now);
            return Err(());
        }
    };
    let Some(vset) = validator_set else {
        return Ok(None);
    };
    if vote_payload.chain_id != network.chain_id() {
        guard.note_strike(peer, StrikeKind::SnapshotInvalido, now);
        return Err(());
    }
    // Protección DoS intra-época (Ckpt 49): validar firma siempre y descartar
    // sin llenar `VoteCollector` si la época está fuera de `[state.epoch, state.epoch + 1]`.
    let max_epoch = state.epoch.saturating_add(1);
    if vote_payload.epoch < state.epoch || vote_payload.epoch > max_epoch {
        if verify_vote_signature(
            vote_payload.chain_id,
            vote_payload.epoch,
            &vote_payload.state_root,
            &vote_payload.vote,
            vset,
        )
        .is_err()
        {
            guard.note_strike(peer, StrikeKind::SnapshotInvalido, now);
            return Err(());
        }
        return Ok(None);
    }
    match collector.add_vote(&vote_payload, network.chain_id(), vset) {
        Ok(Some(qc)) => {
            let matches_checkpoint =
                qc.epoch == state.epoch && state.last_checkpoint_root == Some(qc.state_root);
            if qc.epoch >= state.epoch
                && (matches_checkpoint || state.generate_state_root() == qc.state_root)
            {
                state.latest_qc = Some(qc.clone());
            }
            Ok(Some(qc))
        }
        Ok(None) => Ok(None),
        Err(_) => {
            guard.note_strike(peer, StrikeKind::SnapshotInvalido, now);
            Err(())
        }
    }
}

async fn flush_pending_broadcast(
    swarm: &mut libp2p::Swarm<LedgeritaBehaviour>,
    guard: &PeerGuard,
    pending_broadcast: &Arc<Mutex<Vec<Vec<u8>>>>,
    network: NetworkId,
) {
    let pending = {
        let mut pb = pending_broadcast.lock().await;
        if pb.is_empty() {
            return;
        }
        let txs = pb.clone();
        pb.clear();
        txs
    };
    let now = Instant::now();
    let connected: Vec<_> = swarm
        .connected_peers()
        .cloned()
        .filter(|pid| guard.is_banned(pid, now).is_none())
        .collect();
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

fn complete_checkpoint_vote(
    s: &mut State,
    network: NetworkId,
    quorum_cfg: &QuorumNodeConfig,
    vote_collector: &mut VoteCollector,
    seen_votes: &mut HashSet<(u64, [u8; 32], u8)>,
    storage: &Storage,
) -> Option<Vec<u8>> {
    if s.epoch < 1 || s.latest_qc.is_some() {
        return None;
    }
    let cp_root = s.last_checkpoint_root?;
    if let Some(ref vset) = quorum_cfg.validator_set
        && let Some(qc) = vote_collector.try_certificate(network.chain_id(), s.epoch, &cp_root, vset)
    {
        s.latest_qc = Some(qc);
    }
    let Some((vid, ref sk)) = quorum_cfg.local_validator else {
        if s.latest_qc.is_some() {
            let _ = s.flush(storage);
        }
        return None;
    };
    let vote = VotePayload {
        chain_id: network.chain_id(),
        epoch: s.epoch,
        state_root: cp_root,
        vote: sign_qc_vote(sk, network.chain_id(), s.epoch, &cp_root, vid),
    };
    seen_votes.insert((s.epoch, cp_root, vid));
    if s.latest_qc.is_none()
        && let Some(ref vset) = quorum_cfg.validator_set
        && let Ok(Some(qc)) = vote_collector.add_vote(&vote, network.chain_id(), vset)
    {
        s.latest_qc = Some(qc);
    }
    if s.latest_qc.is_some() {
        let _ = s.flush(storage);
    }
    Some(vote.encode().to_vec())
}

fn prune_vote_state(
    min_epoch: u64,
    vote_collector: &mut VoteCollector,
    seen_votes: &mut HashSet<(u64, [u8; 32], u8)>,
) {
    vote_collector.prune_below(min_epoch);
    seen_votes.retain(|&(ep, _, _)| ep >= min_epoch);
}

fn broadcast_vote_to_peers(
    swarm: &mut libp2p::Swarm<LedgeritaBehaviour>,
    guard: &PeerGuard,
    network: NetworkId,
    vote_bytes: &[u8],
    exclude_peer: Option<&libp2p::PeerId>,
) {
    let now = Instant::now();
    let connected: Vec<_> = swarm.connected_peers().cloned().collect();
    for pid in connected.into_iter().filter(|pid| {
        exclude_peer != Some(pid) && guard.is_banned(pid, now).is_none()
    }) {
        let vmsg = build_msg(network, MSG_VOTE, vote_bytes);
        swarm.behaviour_mut().rr.send_request(&pid, vmsg);
    }
}

pub async fn run_p2p_node(
    local_key: identity::Keypair,
    connect_peers: Vec<Multiaddr>,
    state: Arc<Mutex<State>>,
    storage: Arc<Storage>,
    // Reservado: identidad del nodo para la reputación/baneo de la unidad
    // B+C. Hoy NO entra al consenso (ver finalize_checkpoint — park 2026-11-05).
    _wallet_address: Address,
    mempool: Mempool,
    pending_txs: Vec<Transaction>,
    pending_broadcast: Arc<Mutex<Vec<Vec<u8>>>>,
    listen_port: u16,
    network: NetworkId,
) -> Result<(), Box<dyn Error>> {
    run_p2p_node_with_quorum(
        local_key,
        connect_peers,
        state,
        storage,
        _wallet_address,
        mempool,
        pending_txs,
        pending_broadcast,
        listen_port,
        network,
        QuorumNodeConfig::default(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_p2p_node_with_quorum(
    local_key: identity::Keypair,
    connect_peers: Vec<Multiaddr>,
    state: Arc<Mutex<State>>,
    storage: Arc<Storage>,
    _wallet_address: Address,
    mempool: Mempool,
    pending_txs: Vec<Transaction>,
    pending_broadcast: Arc<Mutex<Vec<Vec<u8>>>>,
    listen_port: u16,
    network: NetworkId,
    quorum_cfg: QuorumNodeConfig,
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
            let protocols = vec![(network.protocol_id(), ProtocolSupport::Full)];
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
        for resolved in resolve_multiaddr_dns(peer).await {
            match swarm.dial(resolved.clone()) {
                Ok(()) => {}
                Err(e) => warn!(peer = %resolved, error = %e, "Error al conectar"),
            }
        }
    }

    let mut retry_interval = tokio::time::interval(Duration::from_secs(30));
    retry_interval.tick().await;

    // Deuda 11 (hallazgo del burst 21c): el intercambio de snapshots SOLO
    // corría en ConnectionEstablished — un nodo con conexión estable podría
    // atrasarse para siempre si su peer avanzó sin él y ninguna
    // desconexión lo despertó. Tick de re-sync: como el propio connect,
    // cada peer conectado recibe MSG_SYNC (nuestro root) y responde con su
    // snapshot completo (o MSG_SYNC_ALREADY en 2 bytes si ya empatamos).
    let mut resync_interval = tokio::time::interval(Duration::from_secs(30));
    resync_interval.tick().await;

    // PeerGuard (unidad B): strikes + bans node-local, JAMÁS en consenso.
    let mut guard = PeerGuard::new();
    let mut vote_collector = VoteCollector::new();
    let mut seen_votes: HashSet<(u64, [u8; 32], u8)> = HashSet::new();
    // Prueba de Tiempo (unidad C): la antigüedad SIEMPRE sobrevive al reinicio.
    match storage.load_peers() {
        Ok(datos) => {
            guard.import_records(datos, Instant::now(), crate::defense::unix_millis_now());
            info!(
                peers = guard.antiq_count(),
                "Reputación node-local de peers cargada (unidad C)"
            );
        }
        Err(e) => {
            warn!(error = %e, "No se pudo cargar la reputación node-local de peers (unidad C)")
        }
    }

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
                        let root = s.finalize_checkpoint();
                        info!(epoch = s.epoch, "Checkpoint completado (TX local)");
                        let _ = root;
                        prune_vote_state(s.epoch, &mut vote_collector, &mut seen_votes);
                        let _ = complete_checkpoint_vote(
                            &mut s,
                            network,
                            &quorum_cfg,
                            &mut vote_collector,
                            &mut seen_votes,
                            &storage,
                        );
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
            info!(
                count = pending_len,
                "TX(s) lista(s) para broadcast a la red"
            );
        }
    }

    loop {
        tokio::select! {
            _ = retry_interval.tick() => {
                // Prueba de Tiempo (unit C): primer los VETERANOS, y el
                // export del guard al tree peers (persistencia 30 s).
                for peer_id in guard.prioridad_dial(
                    &pending_peers.keys().copied().collect::<Vec<_>>(),
                    Instant::now(),
                ) {
                    if guard.is_banned(&peer_id, Instant::now()).is_some() {
                        warn!(%peer_id, "Peer baneado: salteando dial de reintento (PeerGuard)");
                        continue;
                    }
                    if let Some(addr) = pending_peers.get(&peer_id) {
                        warn!(%peer_id, "Reintentando conexión P2P");
                        for resolved in resolve_multiaddr_dns(addr).await {
                            let _ = swarm.dial(resolved);
                        }
                    }
                }
                guard.prune(Instant::now());
                export_peers_node_local(&storage, &guard);
            }
            _ = resync_interval.tick() => {
                // Deuda 11: re-sync con cada peer conectado (mismo flujo que
                // el handshake de ConnectionEstablished). Solo el root viaja
                // en el REQUEST; la respuesta devuelve snapshot o MSG_SYNC_ALREADY.
                let (root, pending_vote) = {
                    let mut s = state.lock().await;
                    let pv = complete_checkpoint_vote(
                        &mut s,
                        network,
                        &quorum_cfg,
                        &mut vote_collector,
                        &mut seen_votes,
                        &storage,
                    );
                    (s.generate_state_root(), pv)
                };
                let msg = build_msg(network, MSG_SYNC, &root);
                for peer_id in swarm.connected_peers().copied().collect::<Vec<_>>() {
                    if guard.is_banned(&peer_id, Instant::now()).is_some() {
                        continue;
                    }
                    swarm.behaviour_mut().rr.send_request(&peer_id, msg.clone());
                }
                if let Some(ref vote_bytes) = pending_vote {
                    broadcast_vote_to_peers(&mut swarm, &guard, network, vote_bytes, None);
                }
            }
            ev = swarm.select_next_some() => match ev {
                SwarmEvent::NewListenAddr { address, .. } => {
                    info!(%address, "Nodo escuchando activamente");
                }
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    info!(%peer_id, "Conexión establecida");
                    // Prueba de Tiempo (unit C): primera conexión conocida
                    // (las re-conexiones NO retroceden la antigüedad).
                    guard.note_conexion(&peer_id, Instant::now());
                    if pending_peers.remove(&peer_id).is_some() {
                        info!(%peer_id, "Peer retirado de lista de reintentos");
                    }

                    let (root, pending_vote) = {
                        let mut s = state.lock().await;
                        let pv = complete_checkpoint_vote(
                            &mut s,
                            network,
                            &quorum_cfg,
                            &mut vote_collector,
                            &mut seen_votes,
                            &storage,
                        );
                        (s.generate_state_root(), pv)
                    };
                    let msg = build_msg(network, MSG_SYNC, &root);
                    swarm.behaviour_mut().rr.send_request(&peer_id, msg);
                    if let Some(vote_bytes) = pending_vote {
                        let vmsg = build_msg(network, MSG_VOTE, &vote_bytes);
                        swarm.behaviour_mut().rr.send_request(&peer_id, vmsg);
                    }
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
                        libp2p::ping::Event { peer, result: Ok(rtt), .. } => {
                            info!(?rtt, "Ping exitoso");
                            // uptime_ratio node-local (unit C, §3).
                            guard.note_ping(&peer, true);
                        }
                        libp2p::ping::Event { peer, result: Err(e), .. } => {
                            warn!(%peer, error = %e, "Ping error");
                            guard.note_ping(&peer, false);
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
                    // BANEADO (PeerGuard §2): respuesta mínima, SIN decode,
                    // SIN apply, SIN tocar el estado. Mínimo CPU por request.
                    if guard.is_banned(&peer, Instant::now()).is_some() {
                        let _ = swarm.behaviour_mut().rr.send_response(channel, b"NO".to_vec());
                        continue;
                    }
                    if request.len() < 2 {
                        let outcome = guard.note_strike(&peer, StrikeKind::RequestSpam, Instant::now());
                        warn!(%peer, ?outcome, "Request malformado (<2 bytes) — strike grave (flood)");
                        let _ = swarm.behaviour_mut().rr.send_response(channel, b"EMPTY".to_vec());
                        continue;
                    }
                    let (msg_type, payload) = match split_msg(network, &request) {
                        Some(x) => x,
                        None => {
                            let outcome = guard.note_strike(&peer, StrikeKind::ChainIdSpam, Instant::now());
                            warn!(%peer, ?outcome, "Solicitud de otra red (chain_id inválido) — strike leve");
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
                                    let outcome = guard.note_strike(&peer, StrikeKind::TxInvalid, Instant::now());
                                    warn!(%peer, error = %e, ?outcome, "TX inválida recibida — strike leve");
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
                                let outcome = guard.note_strike(&peer, StrikeKind::TxInvalid, Instant::now());
                                warn!(%peer, ?outcome, "Firma inválida en TX recibida — strike leve");
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
                                    // Contribución válida del peer (unit C).
                                    guard.note_tx_aceptada(&peer);
                                    if let Err(e) = s.flush(&storage) {
                                        error!(error = %e, "Error persistiendo estado tras TX");
                                    }
                                    let mut checkpoint_vote: Option<Vec<u8>> = None;
                                    if s.is_checkpoint_due() {
                                        let root = s.finalize_checkpoint();
                                        let root_hex: String = root[..4]
                                            .iter()
                                            .map(|b| format!("{:02x}", b))
                                            .collect();
                                        info!(
                                            epoch = s.epoch,
                                            root = %root_hex,
                                            "Checkpoint completado"
                                        );
                                        prune_vote_state(
                                            s.epoch,
                                            &mut vote_collector,
                                            &mut seen_votes,
                                        );
                                        checkpoint_vote = complete_checkpoint_vote(
                                            &mut s,
                                            network,
                                            &quorum_cfg,
                                            &mut vote_collector,
                                            &mut seen_votes,
                                            &storage,
                                        );
                                        if let Err(e) = s.flush(&storage) {
                                            error!(error = %e, "Error persistiendo checkpoint");
                                        }
                                    }
                                    drop(s);
                                    let _ = swarm.behaviour_mut().rr.send_response(
                                        channel,
                                        b"OK".to_vec(),
                                    );
                                    info!(%peer, "Transacción aplicada");

                                    let connected: Vec<_> =
                                        swarm.connected_peers().cloned().collect();
                                    for pid in connected.into_iter().filter(|pid| {
                                        if pid == &peer {
                                            return false;
                                        }
                                        // Sin prisioneros en el gossip (PeerGuard §2).
                                        guard.is_banned(pid, Instant::now()).is_none()
                                    }) {
                                        let fwd = build_msg(network, MSG_TX, tx_bytes);
                                        swarm.behaviour_mut().rr.send_request(&pid, fwd);
                                    }
                                    if let Some(ref vote_bytes) = checkpoint_vote {
                                        broadcast_vote_to_peers(
                                            &mut swarm,
                                            &guard,
                                            network,
                                            vote_bytes,
                                            None,
                                        );
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
                                guard.note_strike(
                                    &peer,
                                    StrikeKind::SnapshotInvalido,
                                    Instant::now(),
                                );
                                let _ = swarm.behaviour_mut().rr.send_response(
                                    channel,
                                    b"INVALID_SYNC".to_vec(),
                                );
                                continue;
                            }
                            let s = state.lock().await;
                            let (resp, already_synced) =
                                build_sync_response(network, &s, &payload[..32]);
                            let count = s.balances.len();
                            // Contribución: snapshot/sync servido válido (unit C).
                            guard.note_snapshot_servido(&peer);
                            let _ = swarm.behaviour_mut().rr.send_response(channel, resp);
                            if already_synced {
                                debug!(%peer, "Peer ya sincronizado (MSG_SYNC_ALREADY)");
                            } else {
                                info!(%peer, count, "Snapshot de estado enviado");
                            }
                            drop(s);

                            flush_pending_broadcast(
                                &mut swarm,
                                &guard,
                                &pending_broadcast,
                                network,
                            )
                            .await;
                        }
                        MSG_VOTE => {
                            let mut s = state.lock().await;
                            let prev_qc = s.latest_qc.clone();
                            let vote_res = process_incoming_vote(
                                payload,
                                network,
                                &peer,
                                quorum_cfg.validator_set.as_ref(),
                                &mut vote_collector,
                                &mut s,
                                &mut guard,
                                Instant::now(),
                            );
                            match vote_res {
                                Ok(qc_opt) => {
                                    if s.latest_qc != prev_qc
                                        && let Err(e) = s.flush(&storage)
                                    {
                                        error!(error = %e, "Error persistiendo QuorumCertificate");
                                    }
                                    if let Some(qc) = qc_opt {
                                        info!(
                                            %peer,
                                            epoch = qc.epoch,
                                            sigs = qc.signatures.len(),
                                            "QuorumCertificate formado vía MSG_VOTE"
                                        );
                                    }
                                    let should_gossip = VotePayload::decode(payload)
                                        .ok()
                                        .filter(|vp| {
                                            vp.epoch >= s.epoch
                                                && vp.epoch <= s.epoch.saturating_add(1)
                                        })
                                        .is_some_and(|vp| {
                                            if seen_votes.len() >= 2048 {
                                                seen_votes.retain(|&(ep, _, _)| ep >= s.epoch);
                                            }
                                            seen_votes.insert((
                                                vp.epoch,
                                                vp.state_root,
                                                vp.vote.validator_id,
                                            ))
                                        });
                                    drop(s);
                                    let _ = swarm
                                        .behaviour_mut()
                                        .rr
                                        .send_response(channel, b"OK_VOTE".to_vec());
                                    if should_gossip {
                                        broadcast_vote_to_peers(
                                            &mut swarm,
                                            &guard,
                                            network,
                                            payload,
                                            Some(&peer),
                                        );
                                    }
                                }
                                Err(()) => {
                                    drop(s);
                                    warn!(%peer, "MSG_VOTE inválido rechazado — strike en PeerGuard");
                                    let _ = swarm
                                        .behaviour_mut()
                                        .rr
                                        .send_response(channel, b"INVALID_VOTE".to_vec());
                                }
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
                            let outcome = guard.note_strike(&peer, StrikeKind::ChainIdSpam, Instant::now());
                            warn!(%peer, ?outcome, "Respuesta de otra red (chain_id inválido) — strike leve");
                            continue;
                        }
                    };
                    match msg_type {
                        MSG_SYNC => {
                            let snapshot_data = payload;
                            let mut s = state.lock().await;
                            let prev_epoch = s.epoch;
                            match State::decode_snapshot(snapshot_data, network) {
                                Ok(remote) => {
                                    match s.merge_if_fresher_with_quorum(
                                        network,
                                        &remote,
                                        &storage,
                                        quorum_cfg.validator_set.as_ref(),
                                    ) {
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
                                            // debug!: con el tick de re-sync
                                            // (Deuda 11) este caso es el
                                            // STEADY STATE del intervalo (el
                                            // local va adelante o empatado) —
                                            // info! lo inundaría el journal.
                                            debug!(
                                                %peer,
                                                local_epoch = s.epoch,
                                                local_tx = s.tx_count,
                                                remote_epoch = remote.epoch,
                                                remote_tx = remote.tx_count,
                                                "Estado local más reciente — conservando"
                                            );
                                        }
                                        Ok(SyncResult::AlreadySynced) => {
                                            debug!(%peer, "Estado ya sincronizado");
                                        }
                                        Ok(SyncResult::Divergent) => {
                                            match s.resolve_fork_with_reapplied(
                                                network,
                                                &remote,
                                                &storage,
                                                quorum_cfg.validator_set.as_ref(),
                                            ) {
                                                Ok((ForkResolution::KeptLocal, _)) => {
                                                    info!(
                                                        %peer,
                                                        epoch = s.epoch,
                                                        tx_count = s.tx_count,
                                                        "Fork Choice: estado local gana desempate determinista"
                                                    );
                                                }
                                                Ok((
                                                    ForkResolution::AdoptedRemote {
                                                        rolled_back_tx,
                                                        reapplied,
                                                    },
                                                    reapplied_txs,
                                                )) => {
                                                    info!(
                                                        %peer,
                                                        epoch = s.epoch,
                                                        tx_count = s.tx_count,
                                                        had_rollback = rolled_back_tx.is_some(),
                                                        reapplied,
                                                        reapplied_count = reapplied_txs.len(),
                                                        "Fork Choice: adoptado estado remoto ganador tras desempate"
                                                    );
                                                    if !reapplied_txs.is_empty() {
                                                        let mut pb = pending_broadcast.lock().await;
                                                        for tx in &reapplied_txs {
                                                            pb.push(encode_transaction(tx));
                                                        }
                                                    }
                                                }
                                                Ok((ForkResolution::RejectedPorGate(gate), _)) => {
                                                    let outcome = guard.note_strike(
                                                        &peer,
                                                        StrikeKind::SnapshotInvalido,
                                                        Instant::now(),
                                                    );
                                                    warn!(
                                                        %peer,
                                                        ?gate,
                                                        ?outcome,
                                                        "🚨 Snapshot divergente rechazado por Gate en Fork Choice"
                                                    );
                                                }
                                                Ok((ForkResolution::NotDivergent(_), _)) => {}
                                                Err(e) => {
                                                    error!(error = %e, "Error resolviendo fork determinista");
                                                }
                                            }
                                        }
                                        Ok(SyncResult::RejectedPorGate(gate)) => {
                                            // Gate B+C+G (§1): snapshot vencía
                                            // la frescura pero violó una
                                            // invariante de consenso — NO se
                                            // adopta ni reenvía. Strike GRAVE:
                                            // ban inmediato de 60 min (§2,
                                            // decisión §5-① del diseño).
                                            let outcome = guard.note_strike(
                                                &peer,
                                                StrikeKind::SnapshotInvalido,
                                                Instant::now(),
                                            );
                                            warn!(
                                                %peer,
                                                ?gate,
                                                ?outcome,
                                                count = remote.balances.len(),
                                                tx_count = remote.tx_count,
                                                "🚨 Snapshot RECHAZADO por el gate de invariantes: no se adopta ni reenvía"
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
                            if s.epoch > prev_epoch {
                                prune_vote_state(s.epoch, &mut vote_collector, &mut seen_votes);
                            }
                            drop(s);

                            flush_pending_broadcast(
                                &mut swarm,
                                &guard,
                                &pending_broadcast,
                                network,
                            )
                            .await;
                        }
                        MSG_SYNC_ALREADY => {
                            trace!(%peer, "Peer confirmó estado idéntico (MSG_SYNC_ALREADY)");
                            flush_pending_broadcast(
                                &mut swarm,
                                &guard,
                                &pending_broadcast,
                                network,
                            )
                            .await;
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

    #[test]
    fn test_msg_vote_forma_qc_al_alcanzar_umbral() {
        use ed25519_dalek::SigningKey;
        let keys = [
            SigningKey::from_bytes(&[1u8; 32]),
            SigningKey::from_bytes(&[2u8; 32]),
            SigningKey::from_bytes(&[3u8; 32]),
        ];
        let pubkeys: Vec<[u8; 32]> = keys.iter().map(|k| k.verifying_key().to_bytes()).collect();
        let vset = ValidatorSet::from_pubkeys(&pubkeys, 2).unwrap();

        let mut state = State::new();
        state.epoch = 1;
        state.tx_count = 4096;
        let root = state.generate_state_root();
        let net = NetworkId::Testnet;
        let peer1 = libp2p::PeerId::random();
        let peer2 = libp2p::PeerId::random();

        let mut collector = VoteCollector::new();
        let mut guard = PeerGuard::new();
        let now = Instant::now();

        let v0 = VotePayload {
            chain_id: net.chain_id(),
            epoch: 1,
            state_root: root,
            vote: sign_qc_vote(&keys[0], net.chain_id(), 1, &root, 0),
        };
        let res1 = process_incoming_vote(
            &v0.encode(),
            net,
            &peer1,
            Some(&vset),
            &mut collector,
            &mut state,
            &mut guard,
            now,
        );
        assert_eq!(res1, Ok(None));
        assert!(state.latest_qc.is_none());

        let v1 = VotePayload {
            chain_id: net.chain_id(),
            epoch: 1,
            state_root: root,
            vote: sign_qc_vote(&keys[1], net.chain_id(), 1, &root, 1),
        };
        let res2 = process_incoming_vote(
            &v1.encode(),
            net,
            &peer2,
            Some(&vset),
            &mut collector,
            &mut state,
            &mut guard,
            now,
        );
        let qc = res2.expect("segundo voto válido debe formar QC").expect("QC presente");
        assert_eq!(qc.signatures.len(), 2);
        assert_eq!(state.latest_qc, Some(qc));
        assert!(guard.is_banned(&peer1, now).is_none());
        assert!(guard.is_banned(&peer2, now).is_none());
    }

    #[test]
    fn test_msg_vote_invalido_penaliza_peer() {
        use ed25519_dalek::SigningKey;
        let keys = [
            SigningKey::from_bytes(&[1u8; 32]),
            SigningKey::from_bytes(&[2u8; 32]),
            SigningKey::from_bytes(&[3u8; 32]),
        ];
        let pubkeys: Vec<[u8; 32]> = keys.iter().map(|k| k.verifying_key().to_bytes()).collect();
        let vset = ValidatorSet::from_pubkeys(&pubkeys, 2).unwrap();

        let mut state = State::new();
        state.epoch = 1;
        let root = state.generate_state_root();
        let net = NetworkId::Testnet;
        let bad_peer = libp2p::PeerId::random();

        let mut collector = VoteCollector::new();
        let mut guard = PeerGuard::new();
        let now = Instant::now();

        let mut v_bad = VotePayload {
            chain_id: net.chain_id(),
            epoch: 1,
            state_root: root,
            vote: sign_qc_vote(&keys[0], net.chain_id(), 1, &root, 0),
        };
        v_bad.vote.signature[0] ^= 0xff; // firma alterada

        let res = process_incoming_vote(
            &v_bad.encode(),
            net,
            &bad_peer,
            Some(&vset),
            &mut collector,
            &mut state,
            &mut guard,
            now,
        );
        assert_eq!(res, Err(()));
        assert!(guard.is_banned(&bad_peer, now).is_some());
    }

    #[test]
    fn test_sync_con_qc_falso_banea_peer() {
        use ed25519_dalek::SigningKey;
        let keys = [
            SigningKey::from_bytes(&[1u8; 32]),
            SigningKey::from_bytes(&[2u8; 32]),
            SigningKey::from_bytes(&[3u8; 32]),
        ];
        let pubkeys: Vec<[u8; 32]> = keys.iter().map(|k| k.verifying_key().to_bytes()).collect();
        let vset = ValidatorSet::from_pubkeys(&pubkeys, 2).unwrap();

        let dir = std::env::temp_dir().join(format!(
            "ledgerita_test_qc_ban_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let storage = Storage::open(&dir).unwrap();
        let mut local = State::new();
        let mut remote = State::new();
        remote.epoch = 1;
        remote.tx_count = 4096;
        remote.latest_qc = None; // sin QC -> Gate g7a

        let mut guard = PeerGuard::new();
        let attacker = libp2p::PeerId::random();
        let now = Instant::now();

        let res = local
            .merge_if_fresher_with_quorum(NetworkId::Testnet, &remote, &storage, Some(&vset))
            .unwrap();
        assert!(matches!(res, SyncResult::RejectedPorGate(_)));
        if let SyncResult::RejectedPorGate(_) = res {
            guard.note_strike(&attacker, StrikeKind::SnapshotInvalido, now);
        }
        assert!(guard.is_banned(&attacker, now).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_resolve_multiaddr_dns_ip4_passthrough() {
        let ma: Multiaddr =
            "/ip4/192.168.68.60/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1"
                .parse()
                .unwrap();
        let resolved = resolve_multiaddr_dns(&ma).await;
        assert_eq!(resolved, vec![ma]);
    }

    #[tokio::test]
    async fn test_resolve_multiaddr_dns4_localhost() {
        let ma: Multiaddr =
            "/dns4/localhost/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1"
                .parse()
                .unwrap();
        let resolved = resolve_multiaddr_dns(&ma).await;
        assert!(!resolved.is_empty());
        let expected: Multiaddr =
            "/ip4/127.0.0.1/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1"
                .parse()
                .unwrap();
        assert!(resolved.contains(&expected));
    }

    #[test]
    fn test_default_fallback_seeds_por_red() {
        assert!(!default_fallback_seeds(NetworkId::Mainnet).is_empty());
        assert!(!default_fallback_seeds(NetworkId::Testnet).is_empty());
        assert!(default_fallback_seeds(NetworkId::Mainnet)[0].contains("36565"));
        assert!(default_fallback_seeds(NetworkId::Testnet)[0].contains("46565"));
    }

    #[test]
    fn test_build_sync_response_already_synced_vs_snapshot() {
        let mut state = State::new();
        state.epoch = 1;
        state.tx_count = 4096;
        let matching_root = state.generate_state_root();
        let (resp_ok, already) = build_sync_response(NetworkId::Testnet, &state, &matching_root);
        assert!(already);
        assert_eq!(resp_ok.len(), 2);
        assert_eq!(split_msg(NetworkId::Testnet, &resp_ok), Some((MSG_SYNC_ALREADY, &[][..])));

        let other_root = [0xAAu8; 32];
        let (resp_snap, already2) = build_sync_response(NetworkId::Testnet, &state, &other_root);
        assert!(!already2);
        let (mtype, payload) = split_msg(NetworkId::Testnet, &resp_snap).unwrap();
        assert_eq!(mtype, MSG_SYNC);
        let decoded = State::decode_snapshot(payload, NetworkId::Testnet).unwrap();
        assert_eq!(decoded.generate_state_root(), matching_root);
    }

    #[test]
    fn test_process_incoming_vote_despues_de_tx_4097_y_ventana_dos() {
        use ed25519_dalek::{Signer, SigningKey};
        let keys = [
            SigningKey::from_bytes(&[1u8; 32]),
            SigningKey::from_bytes(&[2u8; 32]),
            SigningKey::from_bytes(&[3u8; 32]),
        ];
        let pubkeys: Vec<[u8; 32]> = keys.iter().map(|k| k.verifying_key().to_bytes()).collect();
        let vset = ValidatorSet::from_pubkeys(&pubkeys, 2).unwrap();

        let sk_sender = SigningKey::from_bytes(&[0x91; 32]);
        let sender = Address::from_signing_key(&sk_sender);
        let recv = Address::from_signing_key(&SigningKey::from_bytes(&[0x92; 32]));
        let mut state = State::new();
        state.mint(sender.clone(), 100_000);
        state.tx_count = crate::state::CHECKPOINT_INTERVAL - 1;
        let mut tx_close = Transaction::new(sender.clone(), recv.clone(), 100, 1, 1);
        let m1 = tx_close.to_bytes();
        tx_close.sign(sk_sender.sign(&m1));
        state.apply_transaction(&tx_close).unwrap();
        let cp_root = state.finalize_checkpoint();
        assert_eq!(state.epoch, 1);

        // TX 4097 avanza el root actual distinto del root del checkpoint
        let mut tx_4097 = Transaction::new(sender, recv, 50, 1, 2);
        let m2 = tx_4097.to_bytes();
        tx_4097.sign(sk_sender.sign(&m2));
        state.apply_transaction(&tx_4097).unwrap();
        assert_ne!(state.generate_state_root(), cp_root);
        assert!(state.latest_qc.is_none());

        let net = NetworkId::Testnet;
        let peer1 = libp2p::PeerId::random();
        let peer2 = libp2p::PeerId::random();
        let attacker = libp2p::PeerId::random();
        let mut collector = VoteCollector::new();
        let mut guard = PeerGuard::new();
        let now = Instant::now();

        // Protección DoS: voto futuro fuera de ventana con firma válida se ignora sin strike,
        // pero con firma falsa recibe strike.
        let future_valid = VotePayload {
            chain_id: net.chain_id(),
            epoch: 5,
            state_root: cp_root,
            vote: sign_qc_vote(&keys[0], net.chain_id(), 5, &cp_root, 0),
        };
        assert_eq!(
            process_incoming_vote(
                &future_valid.encode(),
                net,
                &peer1,
                Some(&vset),
                &mut collector,
                &mut state,
                &mut guard,
                now,
            ),
            Ok(None)
        );
        assert!(guard.is_banned(&peer1, now).is_none());

        let mut future_bad = future_valid.clone();
        future_bad.vote.signature[0] ^= 0xFF;
        assert_eq!(
            process_incoming_vote(
                &future_bad.encode(),
                net,
                &attacker,
                Some(&vset),
                &mut collector,
                &mut state,
                &mut guard,
                now,
            ),
            Err(())
        );
        assert!(guard.is_banned(&attacker, now).is_some());

        // Los votos de los validadores llegan DESPUÉS de TX 4097 y forman el QC via last_checkpoint_root
        let v0 = VotePayload {
            chain_id: net.chain_id(),
            epoch: 1,
            state_root: cp_root,
            vote: sign_qc_vote(&keys[0], net.chain_id(), 1, &cp_root, 0),
        };
        let v1 = VotePayload {
            chain_id: net.chain_id(),
            epoch: 1,
            state_root: cp_root,
            vote: sign_qc_vote(&keys[1], net.chain_id(), 1, &cp_root, 1),
        };
        assert_eq!(
            process_incoming_vote(
                &v0.encode(),
                net,
                &peer1,
                Some(&vset),
                &mut collector,
                &mut state,
                &mut guard,
                now,
            ),
            Ok(None)
        );
        let qc = process_incoming_vote(
            &v1.encode(),
            net,
            &peer2,
            Some(&vset),
            &mut collector,
            &mut state,
            &mut guard,
            now,
        )
        .unwrap()
        .expect("debe completar quórum aunque TX 4097 ya se haya aplicado");
        assert_eq!(qc.state_root, cp_root);
        assert_eq!(state.latest_qc, Some(qc));
    }

    #[test]
    fn test_fallback_seeds_excluye_peer_local() {
        let seed0: Multiaddr = default_fallback_seeds(NetworkId::Mainnet)[0].parse().unwrap();
        let seed0_pid = extract_peer_id(&seed0).expect("seed0 debe incluir PeerId");
        let filtered = fallback_seeds_for_peer(NetworkId::Mainnet, Some(&seed0_pid));
        assert_eq!(filtered.len(), 1);
        assert!(filtered[0].to_string().contains("192.168.68.64"));
        assert!(extract_peer_id(&filtered[0]).is_some());

        let testnet_all = fallback_seeds_for_peer(NetworkId::Testnet, None);
        assert_eq!(testnet_all.len(), 2);
        assert!(testnet_all.iter().all(|m| extract_peer_id(m).is_some()));
    }
}
