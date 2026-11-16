//! Actor de red de la wallet-node (Fase 2.1a — Alternativa A incremental).
//!
//! La wallet levanta su PROPIO mini-nodo (decisión 2026-11-05): este actor
//! corre en un hilo con runtime tokio (patrón del monitor TUI). El actor es
//! el DUEÑO del estado (`Arc<tokio::sync::Mutex<State>>` + `Arc<Storage>`);
//! la GUI solo lo lee a través de `NodeShared` y le habla por el canal de
//! comandos `NodeCmd`.
//!
//! **Broadcast-first:** la GUI firma → `NodeCmd::BroadcastTx(tx)` → el actor
//! envía `MSG_TX` a todos los peers conectados → solo si ALGÚN peer responde
//! `OK` (o `DUPLICATE`) se aplica localmente y persiste. Sin conexión no se
//! aplica NADA: la wallet jamás miente el saldo. La TX sin confirmación queda
//! en cola de reintento en memoria (se reenvía sola al reconectar).
//!
//! **Compatibilidad (hallazgo H):** el core daemon responde a los `MSG_TX`
//! con status ASCII SIN framing (`OK`, `DUPLICATE`, `REJECT:x`, `BADNET`, ...)
//! y a los `MSG_SYNC` con snapshot SÍ enmarcado. Este actor clasifica ambos
//! (primero framing, si no, ASCII plano) y responde igual que el core (ASCII
//! plano) para que el daemon registre bien sus respuestas.
//!
//! Trampas heredadas del core que aquí se respetan:
//! - Framing de mensajes: `[chain_id][msg_type][payload]` (`build_msg` /
//!   `split_msg`); redes distintas no se conectan (protocolo por red).
//! - Re-dial cada 30s de peers no conectados (patrón core; sin esto un peer
//!   caído jamás se reconecta).
//! - `MSG_SYNC` al conectar y cada 30s (el seed responde su snapshot y el
//!   actor lo adopta vía `merge_if_fresher`).

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use libp2p::request_response::{self, Message, ProtocolSupport};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::tcp;
use libp2p::yamux;
use libp2p::{identity, noise, Multiaddr, PeerId, Swarm};
use tokio::sync::mpsc;
use tokio::sync::Mutex as AsyncMutex;

use ledgerita_core::defense::{PeerGuard, StrikeKind};
use ledgerita_core::faucet::HARD_CAP_SAT;
use ledgerita_core::network::{
    build_msg, encode_faucet_request, process_incoming_vote, split_msg, sync_probe_root, Mempool,
    NetworkId, RawCodec, MAX_MEMPOOL_LEN, MSG_FAUCET, MSG_SYNC, MSG_SYNC_ALREADY, MSG_TX,
    MSG_VOTE,
};
use ledgerita_core::quorum::{default_validator_set, ValidatorSet, VoteCollector, VotePayload};
use ledgerita_core::serialize::{decode_transaction, encode_transaction, hash_tx};
use ledgerita_core::state::{ForkResolution, State, SyncResult};
use ledgerita_core::storage::{Storage, TxHistoryRecord};
use ledgerita_core::transaction::Transaction;

fn insert_mempool(mempool: &mut HashSet<[u8; 32]>, hash: [u8; 32]) {
    if mempool.len() >= MAX_MEMPOOL_LEN {
        mempool.clear();
    }
    mempool.insert(hash);
}

/// Multiaddr del seed de producción (kettle03) en MainNet.
pub const SEED_MAINNET: &str =
    "/ip4/192.168.68.60/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1";

/// Multiaddr del seed de producción (kettle03) en TestNet.
pub const SEED_TESTNET: &str =
    "/ip4/192.168.68.60/tcp/46565/p2p/12D3KooWNjCgE866sbqjr2626KEC3MGYKvYuVM6MvkuG5VhHvaaS";

/// Re-dial de peers no conectados y sync periódico.
pub const RECONNECT_SECS: u64 = 30;
/// Ventana de espera de acks de un broadcast.
pub const BROADCAST_WINDOW: Duration = Duration::from_secs(10);

/// Máximo de registros del historial que el actor mantiene en RAM para la
/// GUI (el tree `historia` de persiste TODAS; solo la vista se recorta).
pub const HISTORIAL_MEM_MAX: usize = 200;
/// Timeout de cada request libp2p (igual que el core).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Seed por defecto de cada red (editable en la UI).
pub fn default_seed(network: NetworkId) -> &'static str {
    match network {
        NetworkId::Mainnet => SEED_MAINNET,
        NetworkId::Testnet => SEED_TESTNET,
    }
}

/// Lista de seeds con fallback para la red (Checkpoint 44).
pub fn default_seeds(network: NetworkId) -> &'static [&'static str] {
    ledgerita_core::network::default_fallback_seeds(network)
}

/// Extrae el PeerId de un multiaddr `/.../p2p/<peer_id>`.
pub fn extract_peer_id(addr: &Multiaddr) -> Option<PeerId> {
    use libp2p::multiaddr::Protocol;
    for proto in addr.iter() {
        if let Protocol::P2p(peer_id) = proto {
            return Some(peer_id);
        }
    }
    None
}

/// Formato LIT con 6 decimales (igual que la GUI).
pub fn format_amount(amount: u64) -> String {
    format!("{}.{:06}", amount / 1_000_000, amount % 1_000_000)
}

/// Prefijo hex de 8 de la raíz de estado — también lo usa la GUI para el
/// indicador de sincronía del dashboard (¿la raíz local ES la de la red?).
pub fn root_prefix(state: &State) -> String {
    state
        .generate_state_root()[..8]
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

// ---------------------------------------------------------------------------
// Clasificación de respuestas TX (compat hallazgo H)
// ---------------------------------------------------------------------------

/// Estado que la red le comunica a una TX firmada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxStatus {
    /// El peer aplicó la TX.
    Accepted,
    /// El peer ya la tenía (es consenso que exista: cuenta como confirmada).
    Duplicate,
    /// El peer la rechazó con motivo (`REJECT:...`).
    Rejected(String),
    /// Cualquier otra cosa (`BADNET`, `INVALID`, basura, ...).
    Other(String),
}

/// ¿Cuenta esta respuesta como "la red confirmó la TX"? (regla broadcast-first:
/// `OK` o `DUPLICATE`).
pub fn tx_status_is_accepted(status: &TxStatus) -> bool {
    matches!(status, TxStatus::Accepted | TxStatus::Duplicate)
}

/// Clasifica el payload crudo de una respuesta a un `MSG_TX`.
///
/// Compat (hallazgo H): el core responde TX SIN framing, así que primero se
/// prueba el framing (para cuando se arregle el core) y si no aplica se lee
/// el payload completo como status ASCII plano.
pub fn classify_tx_response(network: NetworkId, payload: &[u8]) -> TxStatus {
    match split_msg(network, payload) {
        Some((MSG_TX, inner)) => classify_plain(inner),
        Some(_) => TxStatus::Other(format!(
            "respuesta inesperada ({} bytes, msg enmarcado no-TX)",
            payload.len()
        )),
        None => classify_plain(payload),
    }
}

fn classify_plain(raw: &[u8]) -> TxStatus {
    let text = String::from_utf8_lossy(raw);
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case("OK") {
        return TxStatus::Accepted;
    }
    if trimmed.eq_ignore_ascii_case("DUPLICATE") {
        return TxStatus::Duplicate;
    }
    if let Some(reason) = trimmed.strip_prefix("REJECT:") {
        return TxStatus::Rejected(reason.to_string());
    }
    TxStatus::Other(trimmed.to_string())
}

// ---------------------------------------------------------------------------
// Estado compartido con la GUI
// ---------------------------------------------------------------------------

/// Vista de un peer para el dashboard de la GUI.
#[derive(Debug, Clone, Default)]
pub struct PeerView {
    pub addr: String,
    pub connected: bool,
    pub synced: bool,
    pub supply_ok: bool,
    pub supply_lit: String,
    pub root: String,
    pub fee_pool_lit: String,
    pub tx_count: u64,
    pub epoch: u64,
    pub accounts: usize,
    pub status: String,
}

/// Resultado final de un broadcast-first (el último, visible en la GUI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BroadcastReport {
    /// La red confirmó la TX y fue aplicada localmente.
    Accepted { acks: usize, duplicates: usize },
    /// Peers alcanzados y todos la rechazaron.
    Rejected { reason: String },
    /// Sin peers conectados o nadie respondió: NO se aplicó (pasa a reintento).
    Offline { tried: usize },
}

/// Estado que la GUI lee por polling cada frame (std Mutex, lock corto).
#[derive(Debug, Default)]
pub struct NodeShared {
    pub network: String,
    pub started: Option<Instant>,
    pub peers: HashMap<PeerId, PeerView>,
    pub last_broadcast: Option<BroadcastReport>,
    /// Hay una TX firmada esperando reconexión para reintentar.
    pub retry_pending: bool,
    /// Historial node-local de TXs aplicadas (más nueva primero, máx
    /// `HISTORIAL_MEM_MAX`). Lectura de la GUI para el panel 📜.
    pub historial: Vec<TxHistoryRecord>,
    /// Resultado del último intento de faucet (mensaje, error?).
    pub faucet_status: Option<(String, bool)>,
}

/// Órdenes de la GUI hacia el actor.
#[derive(Debug, Clone)]
pub enum NodeCmd {
    /// TX ya firmada por la GUI, lista para broadcast-first.
    BroadcastTx(Transaction),
    /// Reemplaza la lista de peers/seed a los que conectarse.
    ChangePeers(Vec<Multiaddr>),
    /// Mintea LIT de prueba desde el faucet node-local (solo TestNet).
    /// El mint nace en la wallet-node y propagas con el pull de los demás.
    Faucet {
        recipient: ledgerita_core::crypto::Address,
        amount: u64,
        cap: u64,
    },
}

// ---------------------------------------------------------------------------
// Broadcast jobs (recolección de acks dirigida por eventos)
// ---------------------------------------------------------------------------

/// Un broadcast en vuelo: peticiones enviadas y acks recolectados.
#[derive(Debug, Clone)]
pub struct BroadcastJob {
    pub tx: Transaction,
    pub deadline: Instant,
    pub peers_total: usize,
    pub peers_answered: usize,
    pub acks: usize,
    pub duplicates: usize,
    pub last_error: Option<String>,
    /// Ids de requests enviados cuyo response aún no llega.
    pub open_ids: HashSet<request_response::OutboundRequestId>,
}

impl BroadcastJob {
    pub fn new(tx: Transaction, peers_total: usize) -> Self {
        BroadcastJob {
            tx,
            deadline: Instant::now() + BROADCAST_WINDOW,
            peers_total,
            peers_answered: 0,
            acks: 0,
            duplicates: 0,
            last_error: None,
            open_ids: HashSet::new(),
        }
    }

    /// Registra la respuesta de un peer en el job (contabilidad central y
    /// única: acks/duplicados/errores).
    pub fn mark_response(&mut self, status: TxStatus) {
        self.peers_answered += 1;
        if tx_status_is_accepted(&status) {
            // Regla broadcast-first: OK o DUPLICATE confirman la TX.
            match status {
                TxStatus::Accepted => self.acks += 1,
                _ => self.duplicates += 1,
            }
        } else {
            match status {
                TxStatus::Rejected(reason) => self.last_error = Some(reason),
                TxStatus::Other(text) => {
                    self.last_error = Some(format!("respuesta inesperada: {text}"))
                }
                _ => {}
            }
        }
    }

    /// ¿La red ya confirmó la TX (algún OK/DUPLICATE)?
    pub fn accepted(&self) -> bool {
        self.acks + self.duplicates > 0
    }

    /// ¿Se puede cerrar el job? (aceptado, todos respondieron, o se venció la
    /// ventana). El actor SIEMPRE llena `open_ids` antes de pushear el job,
    /// así que "todos respondieron" se mide con respuestas, no con ids vacíos.
    pub fn done(&self) -> bool {
        self.accepted()
            || (self.peers_total > 0 && self.peers_answered >= self.peers_total)
            || Instant::now() >= self.deadline
    }
}

/// Traduce el resultado crudo del job a un reporte final para la GUI.
pub fn report_for_job(job: &BroadcastJob) -> BroadcastReport {
    if job.accepted() {
        BroadcastReport::Accepted {
            acks: job.acks,
            duplicates: job.duplicates,
        }
    } else if let Some(reason) = &job.last_error {
        BroadcastReport::Rejected { reason: reason.clone() }
    } else {
        BroadcastReport::Offline { tried: job.peers_total }
    }
}

// ---------------------------------------------------------------------------
// El actor
// ---------------------------------------------------------------------------

#[derive(NetworkBehaviour)]
struct WalletBehaviour {
    ping: libp2p::ping::Behaviour,
    rr: request_response::Behaviour<RawCodec>,
}

/// Actor: levanta el swarm de la wallet-node y lo mantiene vivo.
pub struct WalletActor {
    shared: Arc<Mutex<NodeShared>>,
    state: Arc<AsyncMutex<State>>,
    storage: Arc<Storage>,
    mempool: Mempool,
    listen_port: u16,
    /// TX firmada sin confirmación que se reenvía al reconectar (en memoria).
    retry_tx: Option<Transaction>,
    /// PeerGuard (unidad B): strikes/bans node-local, NUNCA en el estado de
    /// consenso. Mutex std: los métodos del actor son `&self` y el guard
    /// solo se sostiene guard_lock breve (sin await adentro).
    guard: std::sync::Mutex<PeerGuard>,
    /// Conjunto de validadores BFT para verificar snapshots entrantes (`g7`).
    validator_set: Option<ValidatorSet>,
}

/// Handles que la GUI conserva para hablar con el actor.
pub struct NodeHandles {
    pub shared: Arc<Mutex<NodeShared>>,
    pub send_cmd: mpsc::UnboundedSender<NodeCmd>,
}

/// Lanza el actor de red con el `ValidatorSet` oficial 2-de-3 de `network`.
pub fn spawn_wallet_node(
    network: NetworkId,
    identity: identity::Keypair,
    state: Arc<AsyncMutex<State>>,
    storage: Arc<Storage>,
    peers: Vec<Multiaddr>,
    listen_port: u16,
) -> Result<NodeHandles, String> {
    spawn_wallet_node_with_quorum(
        network,
        identity,
        state,
        storage,
        peers,
        listen_port,
        Some(default_validator_set(network)),
    )
}

/// Variante parametrizada de `spawn_wallet_node` (útil en tests E2E con validadores propios).
pub fn spawn_wallet_node_with_quorum(
    network: NetworkId,
    identity: identity::Keypair,
    state: Arc<AsyncMutex<State>>,
    storage: Arc<Storage>,
    peers: Vec<Multiaddr>,
    listen_port: u16,
    validator_set: Option<ValidatorSet>,
) -> Result<NodeHandles, String> {
    let shared = Arc::new(Mutex::new(NodeShared {
        network: network.tag().to_string(),
        started: None,
        peers: HashMap::new(),
        last_broadcast: None,
        retry_pending: false,
        historial: Vec::new(),
        faucet_status: None,
    }));
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<NodeCmd>();
    let actor = WalletActor {
        shared: shared.clone(),
        state,
        storage,
        mempool: Arc::new(AsyncMutex::new(HashSet::new())),
        listen_port,
        retry_tx: None,
        guard: std::sync::Mutex::new(PeerGuard::new()),
        validator_set,
    };
    std::thread::Builder::new()
        .name("wallet-net-actor".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("runtime tokio del actor de red");
            rt.block_on(async move {
                if let Err(e) = actor.run(network, identity, peers, cmd_rx).await {
                    eprintln!("[wallet-net] actor terminó con error: {e}");
                }
            });
        })
        .map_err(|e| format!("No se pudo crear el hilo del actor de red: {e}"))?;
    Ok(NodeHandles {
        shared,
        send_cmd: cmd_tx,
    })
}

impl WalletActor {
    pub async fn run(
        mut self,
        network: NetworkId,
        local_key: identity::Keypair,
        initial_peers: Vec<Multiaddr>,
        mut cmd_rx: mpsc::UnboundedReceiver<NodeCmd>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut swarm = libp2p::SwarmBuilder::with_existing_identity(local_key)
            .with_tokio()
            .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
            .with_behaviour(|_| {
                let protocols = vec![(network.protocol_id(), ProtocolSupport::Full)];
                WalletBehaviour {
                    ping: libp2p::ping::Behaviour::default(),
                    rr: request_response::Behaviour::with_codec(
                        RawCodec,
                        protocols,
                        request_response::Config::default()
                            .with_request_timeout(REQUEST_TIMEOUT),
                    ),
                }
            })?
            .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(300)))
            .build();

        let listen_addr: Multiaddr =
            format!("/ip4/0.0.0.0/tcp/{}", self.listen_port).parse()?;
        swarm.listen_on(listen_addr)?;

        // Prueba de Tiempo (unit C): la antigüedad SIEMPRE sobrevive al
        // reinicio — cargamos el tree `peers` node-local al arrancar.
        match self.storage.load_peers() {
            Ok(datos) => {
                self.guard.lock().unwrap().import_records(
                    datos,
                    Instant::now(),
                    ledgerita_core::defense::unix_millis_now(),
                );
                eprintln!(
                    "[wallet-net] reputación node-local cargada (unit C): {} peers con historia",
                    self.guard.lock().unwrap().antiq_count()
                );
            }
            Err(e) => eprintln!("[wallet-net] no se pudo cargar la reputación de peers: {e:?}"),
        }
        // Historial node-local de TXs (Fase 2): sobrevive al reinicio.
        match self.storage.load_tx_history(HISTORIAL_MEM_MAX) {
            Ok(hist) => {
                let n = hist.len();
                self.shared.lock().unwrap().historial = hist;
                eprintln!("[wallet-net] historial node-local cargado: {n} TXs aplicadas");
            }
            Err(e) => eprintln!("[wallet-net] no se pudo cargar el historial de TXs: {e:?}"),
        }

        {
            let mut st = self.shared.lock().unwrap();
            st.started = Some(Instant::now());
        }

        // Peers objetivo: NUNCA se retiran (si la conexión cae, el tick de
        // 30s los re-diala; patrón de re-add del core, simplificado).
        let mut targets: HashMap<PeerId, Multiaddr> = HashMap::new();
        for addr in initial_peers {
            let pid = match extract_peer_id(&addr) {
                Some(pid) => pid,
                None => {
                    eprintln!(
                        "[wallet-net] peer sin /p2p/<PeerId> ignorado: {addr}"
                    );
                    continue;
                }
            };
            targets.insert(pid, addr);
        }
        {
            let mut st = self.shared.lock().unwrap();
            for (pid, addr) in &targets {
                st.peers.entry(pid.clone()).or_insert_with(|| PeerView {
                    addr: addr.to_string(),
                    status: "Conectando…".to_string(),
                    ..PeerView::default()
                });
            }
        }
        for (pid, addr) in &targets {
            if self.guard.lock().unwrap().is_banned(pid, Instant::now()).is_some() {
                eprintln!("[wallet-net] peer {pid} baneado: no se diala al arranque (PeerGuard)");
                continue;
            }
            for resolved in ledgerita_core::network::resolve_multiaddr_dns(addr).await {
                if let Err(e) = swarm.dial(resolved) {
                    let mut st = self.shared.lock().unwrap();
                    if let Some(pv) = st.peers.get_mut(pid) {
                        pv.status = format!("Dial error: {e:?}");
                    }
                }
            }
        }

        let mut jobs: Vec<BroadcastJob> = Vec::new();
        let mut vote_collector = VoteCollector::new();
        let mut seen_votes: HashSet<(u64, [u8; 32], u8)> = HashSet::new();
        let mut sync_request_ids: HashSet<request_response::OutboundRequestId> = HashSet::new();
        let mut faucet_request_ids: HashSet<request_response::OutboundRequestId> = HashSet::new();
        let mut retry_interval = tokio::time::interval(Duration::from_secs(RECONNECT_SECS));
        retry_interval.tick().await; // el primer tick es inmediato

        loop {
            tokio::select! {
                _ = retry_interval.tick() => {
                    // Redial de peers caídos + re-sync de los conectados;
                    // el guard se descarta los prisioneros y poda memoria.
                    // Prueba de Tiempo (unit C): primero los VETERANOS.
                    let orden: Vec<PeerId> = {
                        let g = self.guard.lock().unwrap();
                        g.prioridad_dial(
                            &targets.keys().copied().collect::<Vec<_>>(),
                            Instant::now(),
                        )
                    };
                    for pid in orden {
                        if self.guard.lock().unwrap().is_banned(&pid, Instant::now()).is_some() {
                            eprintln!("[wallet-net] peer {pid} baneado: sin redial de reintento (PeerGuard)");
                            continue;
                        }
                        if let Some(addr) = targets.get(&pid) {
                            if !swarm.is_connected(&pid) {
                                for resolved in ledgerita_core::network::resolve_multiaddr_dns(addr).await {
                                    let _ = swarm.dial(resolved);
                                }
                            }
                        }
                    }
                    self.guard.lock().unwrap().prune(Instant::now());
                    // Export de reputación node-local (unit C): tree `peers`.
                    let registros = {
                        let g = self.guard.lock().unwrap();
                        g.export_records(Instant::now(), ledgerita_core::defense::unix_millis_now())
                    };
                    for (pid, rec) in &registros {
                        if let Err(e) = self.storage.save_peer(pid, rec) {
                            eprintln!("[wallet-net] error persistiendo reputación de {pid}: {e:?}");
                        }
                    }
                    let _ = self.storage.flush();
                    self.request_sync_all(&mut swarm, network, &mut sync_request_ids).await;
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(NodeCmd::BroadcastTx(tx)) => {
                            self.start_broadcast(&mut swarm, network, tx, &mut jobs);
                        }
                        Some(NodeCmd::ChangePeers(peers)) => {
                            self.change_peers(&mut swarm, peers, &mut targets).await;
                        }
                        Some(NodeCmd::Faucet { recipient, amount, cap: _ }) => {
                            self.do_faucet(
                                &mut swarm,
                                network,
                                recipient,
                                amount,
                                &mut faucet_request_ids,
                            );
                        }
                        // El sender murió (la GUI se cierra): el actor termina.
                        None => return Ok(()),
                    }
                }
                ev = swarm.select_next_some() => {
                    match ev {
                        SwarmEvent::NewListenAddr { .. } => {}
                        SwarmEvent::ConnectionEstablished { peer_id, connection_id, .. } => {
                            if self.guard.lock().unwrap().is_banned(&peer_id, Instant::now()).is_some() {
                                let _ = swarm.close_connection(connection_id);
                                let mut st = self.shared.lock().unwrap();
                                if let Some(pv) = st.peers.get_mut(&peer_id) {
                                    pv.connected = false;
                                    pv.synced = false;
                                    pv.status = "Baneado (PeerGuard)".to_string();
                                }
                                continue;
                            }
                            {
                                let mut st = self.shared.lock().unwrap();
                                if let Some(pv) = st.peers.get_mut(&peer_id) {
                                    pv.connected = true;
                                    pv.synced = false;
                                    pv.status = "Sincronizando…".to_string();
                                }
                            }
                            // Prueba de Tiempo (unit C): primera conexión
                            // (las re-conexiones NO retroceden).
                            self.guard.lock().unwrap().note_conexion(&peer_id, Instant::now());
                            self.request_sync(&mut swarm, network, peer_id, &mut sync_request_ids).await;
                            // Reintento de broadcast-first: hay TX firmada esperando red.
                            if let Some(tx) = self.retry_tx.take() {
                                self.start_broadcast(&mut swarm, network, tx, &mut jobs);
                            }
                        }
                        SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                            let mut st = self.shared.lock().unwrap();
                            if let Some(pid) = peer_id {
                                if let Some(pv) = st.peers.get_mut(&pid) {
                                    pv.connected = false;
                                    pv.synced = false;
                                    pv.status = format!("Reintentando… ({error:?})");
                                }
                            }
                        }
                        SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                            let mut st = self.shared.lock().unwrap();
                            if let Some(pv) = st.peers.get_mut(&peer_id) {
                                pv.connected = false;
                                pv.synced = false;
                                pv.status = format!("Desconectado: {cause:?}");
                            }
                        }
                        SwarmEvent::Behaviour(WalletBehaviourEvent::Ping(ev)) => {
                            // uptime_ratio node-local (unit C, §3).
                            match ev {
                                libp2p::ping::Event { peer, result: Ok(_), .. } => {
                                    self.guard.lock().unwrap().note_ping(&peer, true);
                                }
                                libp2p::ping::Event { peer, result: Err(_), .. } => {
                                    self.guard.lock().unwrap().note_ping(&peer, false);
                                }
                            }
                        }
                        SwarmEvent::Behaviour(WalletBehaviourEvent::Rr(
                            request_response::Event::Message {
                                peer,
                                message: Message::Request { request, channel, .. },
                                ..
                            },
                        )) => {
                            self.handle_incoming_request(
                                &mut swarm,
                                network,
                                peer,
                                request,
                                channel,
                                &mut vote_collector,
                                &mut seen_votes,
                            ).await;
                        }
                        SwarmEvent::Behaviour(WalletBehaviourEvent::Rr(
                            request_response::Event::Message {
                                peer,
                                message: Message::Response { request_id, response, .. },
                                ..
                            },
                        )) => {
                            self.handle_response(
                                &mut swarm,
                                network,
                                peer,
                                request_id,
                                response,
                                &mut jobs,
                                &mut sync_request_ids,
                                &mut faucet_request_ids,
                            ).await;
                        }
                        SwarmEvent::Behaviour(WalletBehaviourEvent::Rr(
                            request_response::Event::OutboundFailure {
                                peer, request_id, error, ..
                            },
                        )) => {
                            sync_request_ids.remove(&request_id);
                            if faucet_request_ids.remove(&request_id) && faucet_request_ids.is_empty() {
                                let mut st = self.shared.lock().unwrap();
                                if st.faucet_status.is_none() {
                                    st.faucet_status = Some((
                                        format!("No se recibió respuesta del faucet ({error:?})"),
                                        true,
                                    ));
                                }
                            }
                            if jobs.iter_mut().any(|j| j.open_ids.remove(&request_id)) {
                                // (los timeout son silenciosos; el reporte final es Offline)
                            }
                            let mut st = self.shared.lock().unwrap();
                            if let Some(pv) = st.peers.get_mut(&peer) {
                                pv.status = format!("Request falló: {error:?}");
                            }
                        }
                        SwarmEvent::Behaviour(WalletBehaviourEvent::Rr(
                            request_response::Event::ResponseSent { .. },
                        )) => {}
                        _ => {}
                    }
                }
            }

            // Cerrar jobs vencidos/aceptados (fuera del select, con awaits).
            self.service_jobs(&mut jobs).await;
        }
    }

    // ------------------------------------------------------------------
    // Broadcast-first
    // ------------------------------------------------------------------

    /// Filtera los peers baneados: no gastamos requests (ni respuestas) en
    /// prisioneros — la respuesta llegará `NO` y el job esperaría eternamente
    /// por una confirmación que nunca vendrá.
    fn peers_sin_baneados(&self, peers: Vec<PeerId>) -> Vec<PeerId> {
        let guard = self.guard.lock().unwrap();
        peers
            .into_iter()
            .filter(|p| guard.is_banned(p, Instant::now()).is_none())
            .collect()
    }

    /// Envía la TX a todos los peers conectados y arma el job de recolección.
    /// Con cero peers conectados NO se aplica (Offline + cola de reintento).
    fn start_broadcast(
        &mut self,
        swarm: &mut Swarm<WalletBehaviour>,
        network: NetworkId,
        tx: Transaction,
        jobs: &mut Vec<BroadcastJob>,
    ) {
        let connected: Vec<PeerId> = self
            .peers_sin_baneados(swarm.connected_peers().copied().collect());
        if connected.is_empty() {
            eprintln!(
                "[wallet-net] sin peers conectados: la TX NO se aplica (queda en reintento)"
            );
            self.retry_tx = Some(tx);
            let mut st = self.shared.lock().unwrap();
            st.last_broadcast = Some(BroadcastReport::Offline { tried: 0 });
            st.retry_pending = true;
            return;
        }
        self.shared.lock().unwrap().last_broadcast = None;
        let mut job = BroadcastJob::new(tx.clone(), connected.len());
        let msg = build_msg(network, MSG_TX, &encode_transaction(&tx));
        for pid in connected {
            let id = swarm.behaviour_mut().rr.send_request(&pid, msg.clone());
            job.open_ids.insert(id);
        }
        jobs.push(job);
    }

    /// Cierra los jobs ya decididos: aplica la TX confirmada y publica el
    /// reporte en el estado compartido.
    async fn service_jobs(&mut self, jobs: &mut Vec<BroadcastJob>) {
        let mut finished: Vec<BroadcastJob> = Vec::new();
        jobs.retain_mut(|job| {
            if job.done() {
                finished.push(job.clone());
                false
            } else {
                true
            }
        });
        for job in finished {
            let report = report_for_job(&job);
            match &report {
                BroadcastReport::Accepted { .. } => {
                    self.retry_tx = None;
                    if let Err(e) = self.apply_confirmed(&job.tx).await {
                        eprintln!(
                            "[wallet-net] TX confirmada por la red pero el apply local falló \
                             (probablemente ya aplicada por forward): {e}"
                        );
                    }
                }
                BroadcastReport::Rejected { .. } => {
                    self.retry_tx = None;
                    eprintln!("[wallet-net] TX rechazada por la red: no se aplica");
                }
                BroadcastReport::Offline { .. } => {
                    eprintln!(
                        "[wallet-net] broadcast sin confirmación; en cola de reintento"
                    );
                    self.retry_tx = Some(job.tx.clone());
                }
            }
            let mut st = self.shared.lock().unwrap();
            st.last_broadcast = Some(report);
            st.retry_pending = self.retry_tx.is_some();
        }
    }

    /// Aplica (por fin) la TX confirmada por la red al estado local.
    async fn apply_confirmed(&self, tx: &Transaction) -> Result<(), String> {
        let hash = hash_tx(tx);
        let mut s = self.state.lock().await;
        s.apply_transaction(tx).map_err(|e| e.to_string())?;
        {
            let mut mp = self.mempool.lock().await;
            insert_mempool(&mut mp, hash);
        }
        if s.is_checkpoint_due() {
            s.finalize_checkpoint();
            self.mempool.lock().await.clear();
        }
        s.flush(&self.storage).map_err(|e| e.to_string())?;
        // Historial node-local (Fase 2): nuestra TX propia quedó aplicada.
        self.registrar_historial(tx, 0);
        Ok(())
    }

    // ------------------------------------------------------------------
    // Historial node-local (Fase 2) + faucet TestNet
    // ------------------------------------------------------------------

    /// Registra una TX aplicada para el historial de la GUI (RAM + tree
    /// `historia` en sled). Node-local: la misma TX re-registrada es
    /// idempotent por la clave hash.
    fn registrar_historial(&self, tx: &Transaction, kind: u64) {
        let rec = TxHistoryRecord {
            tx_hash: hash_tx(tx),
            from: tx.sender.clone(),
            to: tx.receiver.clone(),
            amount: tx.amount,
            fee: tx.fee,
            nonce: tx.nonce,
            ts_ms: ledgerita_core::defense::unix_millis_now(),
            kind,
        };
        self.persist_historial(rec);
    }

    fn persist_historial(&self, rec: TxHistoryRecord) {
        if let Err(e) = self.storage.save_tx_record(&rec.tx_hash, &rec) {
            eprintln!("[wallet-net] error persistiendo historial: {e:?}");
        }
        let mut st = self.shared.lock().unwrap();
        st.historial.truncate(HISTORIAL_MEM_MAX.saturating_sub(1));
        st.historial.insert(0, rec);
    }

    /// Solicita LIT de prueba al nodo operador del Faucet TestNet (`MSG_FAUCET`),
    /// generando una TX real firmada compatible con Gate `g7` (en vez de mutar el
    /// estado local sin firma y causar auto-baneo contra los validadores).
    fn do_faucet(
        &self,
        swarm: &mut Swarm<WalletBehaviour>,
        network: NetworkId,
        recipient: ledgerita_core::crypto::Address,
        amount: u64,
        faucet_request_ids: &mut HashSet<request_response::OutboundRequestId>,
    ) {
        self.shared.lock().unwrap().faucet_status = None;
        if network != NetworkId::Testnet {
            self.shared.lock().unwrap().faucet_status =
                Some(("El faucet solo está disponible en TestNet".to_string(), true));
            return;
        }
        let connected: Vec<PeerId> =
            self.peers_sin_baneados(swarm.connected_peers().copied().collect());
        if connected.is_empty() {
            self.shared.lock().unwrap().faucet_status = Some((
                "Sin peers conectados — conéctate a TestNet para pedir del faucet".to_string(),
                true,
            ));
            return;
        }
        let payload = encode_faucet_request(&recipient, amount);
        let req = build_msg(network, MSG_FAUCET, &payload);
        for pid in connected {
            let rid = swarm.behaviour_mut().rr.send_request(&pid, req.clone());
            faucet_request_ids.insert(rid);
        }
    }

    // ------------------------------------------------------------------
    // Peer management
    // ------------------------------------------------------------------

    /// Reemplaza la lista de peers objetivo y diala los nuevos no conectados.
    async fn change_peers(
        &mut self,
        swarm: &mut Swarm<WalletBehaviour>,
        peers: Vec<Multiaddr>,
        targets: &mut HashMap<PeerId, Multiaddr>,
    ) {
        let mut nuevo: HashMap<PeerId, Multiaddr> = HashMap::new();
        for addr in peers {
            let pid = match extract_peer_id(&addr) {
                Some(pid) => pid,
                None => {
                    eprintln!("[wallet-net] peer sin /p2p/<PeerId> ignorado: {addr}");
                    continue;
                }
            };
            nuevo.insert(pid, addr);
        }
        {
            let mut st = self.shared.lock().unwrap();
            for (pid, addr) in &nuevo {
                st.peers
                    .entry(pid.clone())
                    .or_insert_with(|| PeerView {
                        addr: addr.to_string(),
                        status: "Conectando…".to_string(),
                        ..PeerView::default()
                    });
            }
        }
        for (pid, addr) in nuevo.clone() {
            if self.guard.lock().unwrap().is_banned(&pid, Instant::now()).is_some() {
                eprintln!("[wallet-net] peer {pid} baneado: ChangePeers NO lo diala (PeerGuard)");
                continue;
            }
            if !swarm.is_connected(&pid) {
                for resolved in ledgerita_core::network::resolve_multiaddr_dns(&addr).await {
                    if let Err(e) = swarm.dial(resolved) {
                        let mut st = self.shared.lock().unwrap();
                        if let Some(pv) = st.peers.get_mut(&pid) {
                            pv.status = format!("Dial error: {e:?}");
                        }
                    }
                }
            }
        }
        *targets = nuevo;
    }

    // ------------------------------------------------------------------
    // Sync (la wallet-node valida por sí misma: Prueba de Fórmula zero-trust)
    // ------------------------------------------------------------------

    async fn request_sync(
        &self,
        swarm: &mut Swarm<WalletBehaviour>,
        network: NetworkId,
        peer: PeerId,
        sync_request_ids: &mut HashSet<request_response::OutboundRequestId>,
    ) {
        let probe = {
            let s = self.state.lock().await;
            sync_probe_root(&s)
        };
        let msg = build_msg(network, MSG_SYNC, &probe);
        let rid = swarm.behaviour_mut().rr.send_request(&peer, msg);
        if sync_request_ids.len() < 4096 {
            sync_request_ids.insert(rid);
        }
    }

    async fn request_sync_all(
        &self,
        swarm: &mut Swarm<WalletBehaviour>,
        network: NetworkId,
        sync_request_ids: &mut HashSet<request_response::OutboundRequestId>,
    ) {
        let connected: Vec<PeerId> =
            self.peers_sin_baneados(swarm.connected_peers().copied().collect());
        if connected.is_empty() {
            return;
        }
        let probe = {
            let s = self.state.lock().await;
            sync_probe_root(&s)
        };
        let msg = build_msg(network, MSG_SYNC, &probe);
        for pid in connected {
            let rid = swarm.behaviour_mut().rr.send_request(&pid, msg.clone());
            if sync_request_ids.len() < 4096 {
                sync_request_ids.insert(rid);
            }
        }
    }

    /// Adopta el snapshot de un peer, actualiza su `PeerView` y devuelve cualquier
    /// TX local reaplicada tras un fork para su re-gossip.
    async fn adopt_sync_snapshot(
        &self,
        peer: PeerId,
        payload: &[u8],
        network: NetworkId,
    ) -> Vec<Transaction> {
        let remote = match State::decode_snapshot(payload, network) {
            Ok(s) => s,
            Err(e) => {
                let outcome = self.guard.lock().unwrap().note_strike(
                    &peer,
                    StrikeKind::SnapshotInvalido,
                    Instant::now(),
                );
                eprintln!(
                    "[wallet-net] peer {peer} envió snapshot corrupto ({e:?}) — strike grave ⇒ {outcome:?}"
                );
                let mut st = self.shared.lock().unwrap();
                if let Some(pv) = st.peers.get_mut(&peer) {
                    pv.synced = false;
                    pv.status = format!("Snapshot inválido: {e:?}");
                }
                return Vec::new();
            }
        };
        let mut reapplied_to_gossip = Vec::new();
        let mut s = self.state.lock().await;
        let prev_epoch = s.epoch;
        let result = s.merge_if_fresher_with_quorum_reapplied(
            network,
            &remote,
            &self.storage,
            self.validator_set.as_ref(),
        );
        let fork_res = match &result {
            Ok((SyncResult::Divergent, _)) => Some(s.resolve_fork_with_reapplied(
                network,
                &remote,
                &self.storage,
                self.validator_set.as_ref(),
            )),
            _ => None,
        };
        if s.epoch > prev_epoch {
            self.mempool.lock().await.clear();
        }
        drop(s);
        let mut st = self.shared.lock().unwrap();
        if let Some(pv) = st.peers.get_mut(&peer) {
            let supply = remote.total_supply();
            pv.connected = true;
            pv.synced = false;
            pv.supply_ok = supply <= HARD_CAP_SAT;
            pv.supply_lit = format_amount(supply);
            pv.root = root_prefix(&remote);
            pv.fee_pool_lit = format_amount(remote.fee_pool);
            pv.tx_count = remote.tx_count;
            pv.epoch = remote.epoch;
            pv.accounts = remote.balances.len();
            pv.status = match result {
                Ok((SyncResult::RemoteAdopted, reapplied)) => {
                    pv.synced = true;
                    reapplied_to_gossip = reapplied;
                    "Synced ✓".to_string()
                }
                Ok((SyncResult::LocalNewer, _)) => "Local más nuevo".to_string(),
                Ok((SyncResult::AlreadySynced, _)) => {
                    pv.synced = true;
                    "En sincronía ✓".to_string()
                }
                Ok((SyncResult::Divergent, _)) => match fork_res {
                    Some(Ok((ForkResolution::KeptLocal, _))) => {
                        pv.synced = true;
                        "Fork resuelto: local prevalece ✓".to_string()
                    }
                    Some(Ok((ForkResolution::AdoptedRemote { .. }, reapplied))) => {
                        pv.synced = true;
                        reapplied_to_gossip = reapplied;
                        "Fork resuelto: remoto adoptado ✓".to_string()
                    }
                    Some(Ok((ForkResolution::RejectedPorGate(gate), _))) => {
                        let outcome = self.guard.lock().unwrap().note_strike(
                            &peer,
                            StrikeKind::SnapshotInvalido,
                            Instant::now(),
                        );
                        eprintln!(
                            "[wallet-net] peer {peer} envió fork INVÁLIDO (gate: {gate:?}) — rechazado, strike grave ⇒ {outcome:?}"
                        );
                        format!("🚨 Fork rechazado (gate: {gate:?})")
                    }
                    Some(Ok((ForkResolution::NotDivergent(sync), reapplied))) => {
                        if matches!(sync, SyncResult::RemoteAdopted | SyncResult::AlreadySynced) {
                            pv.synced = true;
                        }
                        reapplied_to_gossip = reapplied;
                        "En sincronía ✓".to_string()
                    }
                    Some(Err(e)) => format!("Error resolviendo fork: {e:?}"),
                    None => "DIVERGENTE 🚨".to_string(),
                },
                Ok((SyncResult::RejectedPorGate(gate), _)) => {
                    let outcome = self.guard.lock().unwrap().note_strike(
                        &peer,
                        StrikeKind::SnapshotInvalido,
                        Instant::now(),
                    );
                    eprintln!(
                        "[wallet-net] peer {peer} envió snapshot INVÁLIDO (gate: {gate:?}) — rechazado, strike grave ⇒ {outcome:?}"
                    );
                    format!("🚨 Snapshot rechazado (gate: {gate:?})")
                }
                Err(e) => format!("Error al persistir sync: {e:?}"),
            };
        }
        reapplied_to_gossip
    }

    // ------------------------------------------------------------------
    // Mensajes recibidos (la wallet-node actúa como nodo full: valida, aplica,
    // reenvía y responde ASCII plano, compat con el core — hallazgo H)
    // ------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    async fn handle_incoming_request(
        &self,
        swarm: &mut Swarm<WalletBehaviour>,
        network: NetworkId,
        peer: PeerId,
        request: Vec<u8>,
        channel: request_response::ResponseChannel<Vec<u8>>,
        vote_collector: &mut VoteCollector,
        seen_votes: &mut HashSet<(u64, [u8; 32], u8)>,
    ) {
        // BANEADO (PeerGuard §2): respuesta mínima, SIN decode/apply — los
        // requests del prisionero no cuestan CPU ni tocan el estado.
        if self.guard.lock().unwrap().is_banned(&peer, Instant::now()).is_some() {
            let _ = swarm.behaviour_mut().rr.send_response(channel, b"NO".to_vec());
            return;
        }
        if request.len() < 2 {
            let outcome = self.guard.lock().unwrap().note_strike(&peer, StrikeKind::RequestSpam, Instant::now());
            eprintln!("[wallet-net] request malformado de {peer} — strike grave (flood) ⇒ {outcome:?}");
            let _ = swarm.behaviour_mut().rr.send_response(channel, b"EMPTY".to_vec());
            return;
        }
        let (msg_type, payload) = match split_msg(network, &request) {
            Some(x) => x,
            None => {
                let outcome = self.guard.lock().unwrap().note_strike(&peer, StrikeKind::ChainIdSpam, Instant::now());
                eprintln!("[wallet-net] chain_id inválido de {peer} — strike leve ⇒ {outcome:?}");
                let _ = swarm.behaviour_mut().rr.send_response(channel, b"BADNET".to_vec());
                return;
            }
        };
        match msg_type {
            MSG_TX => {
                let decoded = match decode_transaction(payload) {
                    Ok(tx) => tx,
                    Err(_) => {
                        let outcome = self.guard.lock().unwrap().note_strike(&peer, StrikeKind::TxInvalid, Instant::now());
                        eprintln!("[wallet-net] TX inválida de {peer} — strike leve ⇒ {outcome:?}");
                        let _ = swarm.behaviour_mut().rr.send_response(
                            channel,
                            b"INVALID".to_vec(),
                        );
                        return;
                    }
                };
                if decoded.chain_id != network.chain_id() {
                    let outcome = self.guard.lock().unwrap().note_strike(&peer, StrikeKind::TxInvalid, Instant::now());
                    eprintln!("[wallet-net] chain_id inválido en TX de {peer} — strike leve ⇒ {outcome:?}");
                    let _ = swarm.behaviour_mut().rr.send_response(
                        channel,
                        b"INVALID_SIG".to_vec(),
                    );
                    return;
                }
                let hash = hash_tx(&decoded);
                {
                    let mp = self.mempool.lock().await;
                    if mp.contains(&hash) {
                        let _ = swarm.behaviour_mut().rr.send_response(
                            channel,
                            b"DUPLICATE".to_vec(),
                        );
                        return;
                    }
                }
                if !decoded.is_signature_valid() {
                    let outcome = self.guard.lock().unwrap().note_strike(&peer, StrikeKind::TxInvalid, Instant::now());
                    eprintln!("[wallet-net] firma inválida de {peer} — strike leve ⇒ {outcome:?}");
                    let _ = swarm.behaviour_mut().rr.send_response(
                        channel,
                        b"INVALID_SIG".to_vec(),
                    );
                    return;
                }
                let mut s = self.state.lock().await;
                match s.apply_transaction(&decoded) {
                    Ok(()) => {
                        {
                            let mut mp = self.mempool.lock().await;
                            insert_mempool(&mut mp, hash);
                        }
                        // Contribución válida del peer (unit C).
                        self.guard.lock().unwrap().note_tx_aceptada(&peer);
                        // Historial node-local (Fase 2): TX de la red aplicada.
                        self.registrar_historial(&decoded, 0);
                        if s.is_checkpoint_due() {
                            s.finalize_checkpoint();
                            self.mempool.lock().await.clear();
                        }
                        if let Err(e) = s.flush(&self.storage) {
                            eprintln!("[wallet-net] error persistiendo TX recibida: {e:?}");
                        }
                        let _ = swarm.behaviour_mut().rr.send_response(
                            channel,
                            b"OK".to_vec(),
                        );
                        // Reenvío a los demás (patrón core); sin prisioneros.
                        let connected: Vec<PeerId> = self.peers_sin_baneados(
                            swarm.connected_peers().copied().collect(),
                        );
                        for pid in connected {
                            if pid != peer {
                                let fwd = build_msg(network, MSG_TX, payload);
                                swarm.behaviour_mut().rr.send_request(&pid, fwd);
                            }
                        }
                    }
                    Err(e) => {
                        let _ = swarm.behaviour_mut().rr.send_response(
                            channel,
                            format!("REJECT:{e}").into_bytes(),
                        );
                    }
                }
            }
            MSG_SYNC => {
                if payload.len() < 32 {
                    self.guard.lock().unwrap().note_strike(
                        &peer,
                        StrikeKind::SnapshotInvalido,
                        Instant::now(),
                    );
                    let _ = swarm.behaviour_mut().rr.send_response(
                        channel,
                        b"INVALID_SYNC".to_vec(),
                    );
                    return;
                }
                let s = self.state.lock().await;
                let (resp, _) = ledgerita_core::network::build_sync_response(
                    network,
                    &s,
                    &payload[..32],
                );
                // Contribución: snapshot servido válido (unit C).
                self.guard.lock().unwrap().note_snapshot_servido(&peer);
                let _ = swarm.behaviour_mut().rr.send_response(channel, resp);
            }
            MSG_VOTE => {
                let mut s = self.state.lock().await;
                let decoded_vp = VotePayload::decode(payload).ok();
                if let Some(ref vp) = decoded_vp
                    && self.validator_set.is_some()
                    && vp.chain_id == network.chain_id()
                    && vp.epoch >= s.epoch
                    && vp.epoch <= s.epoch.saturating_add(1)
                    && seen_votes.contains(&(vp.epoch, vp.state_root, vp.vote.validator_id))
                {
                    drop(s);
                    let _ = swarm.behaviour_mut().rr.send_response(
                        channel,
                        build_msg(network, MSG_VOTE, b"OK_VOTE"),
                    );
                    return;
                }
                let prev_qc = s.latest_qc.clone();
                let vote_res = {
                    let mut g = self.guard.lock().unwrap();
                    process_incoming_vote(
                        payload,
                        network,
                        &peer,
                        self.validator_set.as_ref(),
                        vote_collector,
                        &mut s,
                        &mut g,
                        Instant::now(),
                    )
                };
                match vote_res {
                    Ok(_) => {
                        if s.latest_qc != prev_qc {
                            let _ = s.flush(&self.storage);
                        }
                        if let Some(vp) = decoded_vp
                            && vp.epoch >= s.epoch
                            && vp.epoch <= s.epoch.saturating_add(1)
                        {
                            if seen_votes.len() >= 2048 {
                                seen_votes.retain(|&(ep, _, _)| ep >= s.epoch);
                                if seen_votes.len() >= 2048 {
                                    seen_votes.clear();
                                }
                            }
                            seen_votes.insert((vp.epoch, vp.state_root, vp.vote.validator_id));
                        }
                        drop(s);
                        let _ = swarm.behaviour_mut().rr.send_response(
                            channel,
                            build_msg(network, MSG_VOTE, b"OK_VOTE"),
                        );
                    }
                    Err(()) => {
                        drop(s);
                        let _ = swarm.behaviour_mut().rr.send_response(
                            channel,
                            build_msg(network, MSG_VOTE, b"INVALID_VOTE"),
                        );
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

    /// Respuesta a un request NUESTRO: ack de broadcast, faucet o snapshot de sync.
    #[allow(clippy::too_many_arguments)]
    async fn handle_response(
        &mut self,
        swarm: &mut Swarm<WalletBehaviour>,
        network: NetworkId,
        peer: PeerId,
        request_id: request_response::OutboundRequestId,
        response: Vec<u8>,
        jobs: &mut Vec<BroadcastJob>,
        sync_request_ids: &mut HashSet<request_response::OutboundRequestId>,
        faucet_request_ids: &mut HashSet<request_response::OutboundRequestId>,
    ) {
        let is_sync_resp = sync_request_ids.remove(&request_id);
        let is_faucet_resp = faucet_request_ids.remove(&request_id);
        if self.guard.lock().unwrap().is_banned(&peer, Instant::now()).is_some() {
            for j in jobs.iter_mut() {
                j.open_ids.remove(&request_id);
            }
            return;
        }
        // 1) ¿Es el ack de un broadcast propio?
        if let Some(job) = jobs.iter_mut().find(|j| j.open_ids.contains(&request_id)) {
            job.open_ids.remove(&request_id);
            let status = classify_tx_response(network, &response);
            job.mark_response(status);
            return;
        }
        // 2) ¿Respuesta a solicitud TestNet Faucet (`MSG_FAUCET`)?
        if is_faucet_resp {
            if let Some((MSG_FAUCET, inner)) = split_msg(network, &response) {
                let text = String::from_utf8_lossy(inner).trim().to_string();
                if text == "OK" {
                    faucet_request_ids.clear();
                    self.shared.lock().unwrap().faucet_status = Some((
                        "💧 Faucet TX emitida por la red — sincronizando saldo…".to_string(),
                        false,
                    ));
                    self.request_sync_all(swarm, network, sync_request_ids).await;
                } else if let Some(reason) = text.strip_prefix("REJECT:") {
                    self.shared.lock().unwrap().faucet_status =
                        Some((format!("❌ Faucet rechazado: {reason}"), true));
                } else if text == "NO_FAUCET" && faucet_request_ids.is_empty() {
                    let mut st = self.shared.lock().unwrap();
                    if st.faucet_status.is_none() {
                        st.faucet_status = Some((
                            "Ningún peer conectado tiene faucet activo (--faucet-key-file)".to_string(),
                            true,
                        ));
                    }
                }
            }
            return;
        }
        // 3) Respuesta de sincronización (`MSG_SYNC` o `MSG_SYNC_ALREADY`).
        match split_msg(network, &response) {
            Some((MSG_SYNC, payload)) => {
                let reapplied = self.adopt_sync_snapshot(peer, payload, network).await;
                for tx in reapplied {
                    self.start_broadcast(swarm, network, tx, jobs);
                }
            }
            Some((MSG_SYNC_ALREADY, _)) if is_sync_resp => {
                let s = self.state.lock().await;
                let root = root_prefix(&s);
                let supply_ok = s.total_supply() <= HARD_CAP_SAT;
                let supply_lit = format_amount(s.total_supply());
                let fee_pool_lit = format_amount(s.fee_pool);
                let tx_count = s.tx_count;
                let epoch = s.epoch;
                let accounts = s.balances.len();
                drop(s);

                let mut st = self.shared.lock().unwrap();
                if let Some(pv) = st.peers.get_mut(&peer) {
                    pv.connected = true;
                    pv.synced = true;
                    pv.supply_ok = supply_ok;
                    pv.supply_lit = supply_lit;
                    pv.root = root;
                    pv.fee_pool_lit = fee_pool_lit;
                    pv.tx_count = tx_count;
                    pv.epoch = epoch;
                    pv.accounts = accounts;
                    pv.status = "En sincronía ✓".to_string();
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use ledgerita_core::crypto::Address;
    use std::str::FromStr;

    fn make_tx(nonce: u64) -> Transaction {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let rk = SigningKey::from_bytes(&[8u8; 32]);
        Transaction::new(
            Address::from_signing_key(&sk),
            Address::from_signing_key(&rk),
            500,
            50,
            nonce,
        )
    }

    #[test]
    fn test_classify_plain_ok_y_duplicate_cuentan_como_aceptado() {
        for status in [b"OK".as_slice(), b"DUPLICATE".as_slice(), b" ok ".as_slice()] {
            let classified = classify_tx_response(NetworkId::Mainnet, status);
            assert!(
                tx_status_is_accepted(&classified),
                "{:?} (de {}) debe contar como aceptado",
                classified,
                String::from_utf8_lossy(status),
            );
        }
        assert_eq!(classify_tx_response(NetworkId::Mainnet, b"OK"), TxStatus::Accepted);
        assert_eq!(
            classify_tx_response(NetworkId::Mainnet, b"DUPLICATE"),
            TxStatus::Duplicate,
        );
    }

    #[test]
    fn test_classify_plain_reject_y_otros() {
        assert_eq!(
            classify_tx_response(NetworkId::Testnet, b"REJECT:nonce no coincide"),
            TxStatus::Rejected("nonce no coincide".to_string()),
        );
        assert_eq!(
            classify_tx_response(NetworkId::Testnet, b"BADNET"),
            TxStatus::Other("BADNET".to_string()),
        );
        assert_eq!(
            classify_tx_response(NetworkId::Testnet, b"INVALID_SIG"),
            TxStatus::Other("INVALID_SIG".to_string()),
        );
        assert!(!tx_status_is_accepted(&TxStatus::Rejected("x".into())));
        assert!(!tx_status_is_accepted(&TxStatus::Other("x".into())));
    }

    #[test]
    fn test_classify_framed_status_compat_futuro() {
        // Si algún día el core responde TX con framing (hallazgo H arreglado),
        // el inner ASCII debe seguir clasificando bien.
        let framed = build_msg(NetworkId::Mainnet, MSG_TX, b"OK");
        assert_eq!(
            classify_tx_response(NetworkId::Mainnet, &framed),
            TxStatus::Accepted,
        );
        // Y un snapshot enmarcado NO debe clasificar como status TX.
        let sync_msg = build_msg(NetworkId::Mainnet, MSG_SYNC, &[0u8; 32]);
        assert!(!tx_status_is_accepted(
            &classify_tx_response(NetworkId::Mainnet, &sync_msg),
        ));
    }

    #[test]
    fn test_status_ascii_de_otra_red_y_basura() {
        // Nuance del hallazgo H: un status ENMARCADO de otra red NO se lee
        // plano (los bytes del framing ensucian el payload) → cae como Other,
        // que es más seguro: la defensa real de red funciona en los REQUESTS
        // (chain_id + protocolo), y lo importado aquí es que un status
        // dudoso JAMÁS se acepta.
        let de_otra_red = build_msg(NetworkId::Testnet, MSG_TX, b"OK");
        assert_eq!(
            classify_tx_response(NetworkId::Mainnet, &de_otra_red),
            TxStatus::Other("\u{2}\u{1}OK".to_string()),
        );
        assert!(!tx_status_is_accepted(
            &classify_tx_response(NetworkId::Mainnet, &de_otra_red),
        ));
        assert_eq!(
            classify_tx_response(NetworkId::Mainnet, b"malo"),
            TxStatus::Other("malo".to_string()),
        );
    }

    #[test]
    fn test_job_nuevo_esta_pendiente_y_contable_de_acks() {
        let mut job = BroadcastJob::new(make_tx(1), 3);
        assert_eq!(job.peers_total, 3);
        assert!(!job.accepted());
        assert!(!job.done(), "job con deadline futuro y 0 respuestas no está listo");
        assert_eq!(report_for_job(&job), BroadcastReport::Offline { tried: 3 });

        job.acks += 1;
        assert_eq!(job.acks, 1);
        assert!(job.accepted());
        assert!(job.done());
    }

    #[test]
    fn test_report_for_job_tres_estados() {
        let mut job = BroadcastJob::new(make_tx(1), 2);
        assert_eq!(report_for_job(&job), BroadcastReport::Offline { tried: 2 });

        job.acks = 1;
        assert_eq!(
            report_for_job(&job),
            BroadcastReport::Accepted { acks: 1, duplicates: 0 },
        );

        let mut job_second = BroadcastJob::new(make_tx(2), 2);
        job_second.last_error = Some("nonce no coincide".to_string());
        assert_eq!(
            report_for_job(&job_second),
            BroadcastReport::Rejected { reason: "nonce no coincide".to_string() },
        );

        let job_dup = BroadcastJob {
            duplicates: 1,
            ..BroadcastJob::new(make_tx(3), 1)
        };
        assert_eq!(
            report_for_job(&job_dup),
            BroadcastReport::Accepted { acks: 0, duplicates: 1 },
        );
    }

    #[test]
    fn test_default_seed_por_red() {
        assert_eq!(default_seed(NetworkId::Mainnet), SEED_MAINNET);
        assert_eq!(default_seed(NetworkId::Testnet), SEED_TESTNET);
        // Puertos distintos por red y peer-id presente.
        let main = Multiaddr::from_str(SEED_MAINNET).unwrap();
        let test = Multiaddr::from_str(SEED_TESTNET).unwrap();
        assert_ne!(main, test);
    }

    #[test]
    fn test_extract_peer_id_en_seeds() {
        let main = Multiaddr::from_str(SEED_MAINNET).unwrap();
        let pid = extract_peer_id(&main).expect("seed mainnet trae /p2p/<id>");
        assert_eq!(
            pid.to_base58(),
            "12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1",
        );
        let sin_p2p = Multiaddr::from_str("/ip4/192.168.68.61/tcp/36565").unwrap();
        assert!(extract_peer_id(&sin_p2p).is_none());
    }

    #[test]
    fn test_format_amount() {
        assert_eq!(format_amount(0), "0.000000");
        assert_eq!(format_amount(5), "0.000005");
        assert_eq!(format_amount(1_000_000), "1.000000");
        assert_eq!(format_amount(21_000_000_000_000u64), "21000000.000000");
    }

    /// El primitivo del indicador de sincronía del dashboard (Ckpt 36):
    /// la GUI declara el saldo confirmado SOLO si la raíz local coincide
    /// con la de un peer sincronizado — estados iguales comparten prefijo,
    /// estados distintos NO (si coincidieran, la wallet mentiría el saldo).
    #[test]
    fn test_root_prefix_distingue_estados() {
        use std::path::PathBuf;
        use std::time::{SystemTime, UNIX_EPOCH};
        fn dir_temp(label: &str) -> PathBuf {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "ledgerita_net_actor_test_{label}_{nanos}"
            ));
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }
        let d1 = dir_temp("p1");
        let d2 = dir_temp("p2");
        let s1 = State::load(&Arc::new(Storage::open(&d1.join("state")).unwrap())).unwrap();
        let mut s2 = State::load(&Arc::new(Storage::open(&d2.join("state")).unwrap())).unwrap();
        // Iguales ⇒ mismo prefijo.
        assert_eq!(root_prefix(&s1), root_prefix(&s2));
        // Distintos ⇒ prefijos distintos (si coincidieran, la GUI mentiría).
        s2.mint(
            Address::from_signing_key(&SigningKey::from_bytes(&[3u8; 32])),
            7_000_000,
        );
        assert_ne!(root_prefix(&s1), root_prefix(&s2));
        // 8 BYTES de raíz = 16 hex (el formato que ve la GUI: p. ej.
        // la raíz de producción b1718aa40b5d085b tiene 16 hex).
        assert_eq!(root_prefix(&s1).len(), 16);
    }

    #[tokio::test]
    async fn test_wallet_rechaza_snapshot_falso_sin_quorum_bft() {
        use ed25519_dalek::SigningKey;
        use std::time::{SystemTime, UNIX_EPOCH};

        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("ledgerita_wallet_bft_reject_{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = Arc::new(Storage::open(&dir.join("state")).unwrap());
        let state = Arc::new(AsyncMutex::new(State::load(&storage).unwrap()));

        let vset = default_validator_set(NetworkId::Testnet);
        let shared = Arc::new(Mutex::new(NodeShared {
            network: NetworkId::Testnet.tag().to_string(),
            started: None,
            peers: HashMap::new(),
            last_broadcast: None,
            retry_pending: false,
            historial: Vec::new(),
            faucet_status: None,
        }));
        let peer = PeerId::random();
        shared.lock().unwrap().peers.insert(peer, PeerView::default());

        let actor = WalletActor {
            shared: shared.clone(),
            state: state.clone(),
            storage,
            mempool: Arc::new(AsyncMutex::new(HashSet::new())),
            listen_port: 0,
            retry_tx: None,
            guard: std::sync::Mutex::new(PeerGuard::new()),
            validator_set: Some(vset),
        };

        // Peer malicioso envía snapshot de epoch=1 SIN QuorumCertificate
        let mut fake = State::new();
        fake.epoch = 1;
        fake.tx_count = 4096;
        let victim = Address::from_signing_key(&SigningKey::from_bytes(&[0x77; 32]));
        fake.mint(victim.clone(), 9_000_000);
        let fake_wire = fake.encode_snapshot(NetworkId::Testnet);

        actor.adopt_sync_snapshot(peer, &fake_wire, NetworkId::Testnet).await;

        assert!(
            state.lock().await.balances.is_empty(),
            "la wallet no debe adoptar saldos sin quórum BFT 2-de-3"
        );
        let st = shared.lock().unwrap();
        let pv = st.peers.get(&peer).unwrap();
        assert!(!pv.synced, "peer rechazado no debe quedar marcado synced=true");
        assert!(pv.status.contains("rechazado"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_ckpt55_wallet_snapshot_corrupto_aplica_strike_y_mempool_acotada() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("ledgerita_wallet_ckpt55_{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = Arc::new(Storage::open(&dir.join("state")).unwrap());
        let state = Arc::new(AsyncMutex::new(State::load(&storage).unwrap()));

        let shared = Arc::new(Mutex::new(NodeShared {
            network: NetworkId::Testnet.tag().to_string(),
            started: None,
            peers: HashMap::new(),
            last_broadcast: None,
            retry_pending: false,
            historial: Vec::new(),
            faucet_status: None,
        }));
        let peer = PeerId::random();
        shared.lock().unwrap().peers.insert(peer, PeerView::default());

        let actor = WalletActor {
            shared,
            state,
            storage,
            mempool: Arc::new(AsyncMutex::new(HashSet::new())),
            listen_port: 0,
            retry_tx: None,
            guard: std::sync::Mutex::new(PeerGuard::new()),
            validator_set: Some(default_validator_set(NetworkId::Testnet)),
        };

        // Snapshot corrupto debe disparar SnapshotInvalido (ban inmediato)
        actor.adopt_sync_snapshot(peer, b"basura_no_es_snapshot", NetworkId::Testnet).await;
        assert!(
            actor.guard.lock().unwrap().is_banned(&peer, Instant::now()).is_some(),
            "decode_snapshot corrupto debe aplicar strike en PeerGuard"
        );

        // insert_mempool acota estrictamente a MAX_MEMPOOL_LEN
        let mut mp = HashSet::new();
        for i in 0..=MAX_MEMPOOL_LEN {
            let mut h = [0u8; 32];
            h[..8].copy_from_slice(&(i as u64).to_be_bytes());
            insert_mempool(&mut mp, h);
        }
        assert_eq!(mp.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

