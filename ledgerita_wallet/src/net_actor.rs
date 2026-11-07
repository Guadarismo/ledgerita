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

use ledgerita_core::faucet::HARD_CAP_SAT;
use ledgerita_core::network::{
    build_msg, split_msg, Mempool, NetworkId, RawCodec, MSG_SYNC, MSG_TX,
};
use ledgerita_core::defense::{PeerGuard, StrikeKind};
use ledgerita_core::serialize::{decode_transaction, encode_transaction, hash_tx};
use ledgerita_core::state::{State, SyncResult};
use ledgerita_core::storage::Storage;
use ledgerita_core::transaction::Transaction;

/// Multiaddr del seed de producción (kettle03) en MainNet.
pub const SEED_MAINNET: &str =
    "/ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1";

/// Multiaddr del seed de producción (kettle03) en TestNet.
pub const SEED_TESTNET: &str =
    "/ip4/192.168.68.61/tcp/46565/p2p/12D3KooWNjCgE866sbqjr2626KEC3MGYKvYuVM6MvkuG5VhHvaaS";

/// Re-dial de peers no conectados y sync periódico.
pub const RECONNECT_SECS: u64 = 30;
/// Ventana de espera de acks de un broadcast.
pub const BROADCAST_WINDOW: Duration = Duration::from_secs(10);
/// Timeout de cada request libp2p (igual que el core).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Seed por defecto de cada red (editable en la UI).
pub fn default_seed(network: NetworkId) -> &'static str {
    match network {
        NetworkId::Mainnet => SEED_MAINNET,
        NetworkId::Testnet => SEED_TESTNET,
    }
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

fn root_prefix(state: &State) -> String {
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
}

/// Órdenes de la GUI hacia el actor.
#[derive(Debug, Clone)]
pub enum NodeCmd {
    /// TX ya firmada por la GUI, lista para broadcast-first.
    BroadcastTx(Transaction),
    /// Reemplaza la lista de peers/seed a los que conectarse.
    ChangePeers(Vec<Multiaddr>),
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
}

/// Handles que la GUI conserva para hablar con el actor.
pub struct NodeHandles {
    pub shared: Arc<Mutex<NodeShared>>,
    pub send_cmd: mpsc::UnboundedSender<NodeCmd>,
}

/// Lanza el actor de red en su propio hilo con runtime tokio.
pub fn spawn_wallet_node(
    network: NetworkId,
    identity: identity::Keypair,
    state: Arc<AsyncMutex<State>>,
    storage: Arc<Storage>,
    peers: Vec<Multiaddr>,
    listen_port: u16,
) -> Result<NodeHandles, String> {
    let shared = Arc::new(Mutex::new(NodeShared {
        network: network.tag().to_string(),
        started: None,
        peers: HashMap::new(),
        last_broadcast: None,
        retry_pending: false,
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
            if let Err(e) = swarm.dial(addr.clone()) {
                let mut st = self.shared.lock().unwrap();
                if let Some(pv) = st.peers.get_mut(pid) {
                    pv.status = format!("Dial error: {e:?}");
                }
            }
        }

        let mut jobs: Vec<BroadcastJob> = Vec::new();
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
                                let _ = swarm.dial(addr.clone());
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
                    self.request_sync_all(&mut swarm, network);
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(NodeCmd::BroadcastTx(tx)) => {
                            self.start_broadcast(&mut swarm, network, tx, &mut jobs);
                        }
                        Some(NodeCmd::ChangePeers(peers)) => {
                            self.change_peers(&mut swarm, peers, &mut targets);
                        }
                        // El sender murió (la GUI se cierra): el actor termina.
                        None => return Ok(()),
                    }
                }
                ev = swarm.select_next_some() => {
                    match ev {
                        SwarmEvent::NewListenAddr { .. } => {}
                        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                            {
                                let mut st = self.shared.lock().unwrap();
                                if let Some(pv) = st.peers.get_mut(&peer_id) {
                                    pv.connected = true;
                                    pv.status = "Sincronizando…".to_string();
                                }
                            }
                            // Prueba de Tiempo (unit C): primera conexión
                            // (las re-conexiones NO retroceden).
                            self.guard.lock().unwrap().note_conexion(&peer_id, Instant::now());
                            self.request_sync(&mut swarm, network, peer_id);
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
                                    pv.status = format!("Reintentando… ({error:?})");
                                }
                            }
                        }
                        SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                            let mut st = self.shared.lock().unwrap();
                            if let Some(pv) = st.peers.get_mut(&peer_id) {
                                pv.connected = false;
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
                                &mut swarm, network, peer, request, channel,
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
                                network, peer, request_id, response, &mut jobs,
                            ).await;
                        }
                        SwarmEvent::Behaviour(WalletBehaviourEvent::Rr(
                            request_response::Event::OutboundFailure {
                                peer, request_id, error, ..
                            },
                        )) => {
                            // Respuesta que no llega ≠ rechazo: no cuenta como
                            // error de contenido; si nadie responde, el job
                            // cierra Offline.
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
        self.mempool.lock().await.insert(hash);
        s.flush(&self.storage).map_err(|e| e.to_string())?;
        if s.is_checkpoint_due() {
            s.finalize_checkpoint();
            s.flush(&self.storage).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Peer management
    // ------------------------------------------------------------------

    /// Reemplaza la lista de peers objetivo y diala los nuevos no conectados.
    fn change_peers(
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
                if let Err(e) = swarm.dial(addr) {
                    let mut st = self.shared.lock().unwrap();
                    if let Some(pv) = st.peers.get_mut(&pid) {
                        pv.status = format!("Dial error: {e:?}");
                    }
                }
            }
        }
        *targets = nuevo;
    }

    // ------------------------------------------------------------------
    // Sync (la wallet-node valida por sí misma: Prueba de Fórmula zero-trust)
    // ------------------------------------------------------------------

    fn request_sync(&self, swarm: &mut Swarm<WalletBehaviour>, network: NetworkId, peer: PeerId) {
        let msg = build_msg(network, MSG_SYNC, &[0u8; 32]);
        swarm.behaviour_mut().rr.send_request(&peer, msg);
    }

    fn request_sync_all(&self, swarm: &mut Swarm<WalletBehaviour>, network: NetworkId) {
        let connected: Vec<PeerId> = self
            .peers_sin_baneados(swarm.connected_peers().copied().collect());
        for pid in connected {
            self.request_sync(swarm, network, pid);
        }
    }

    /// Adopta el snapshot de un peer y actualiza su `PeerView`.
    async fn adopt_sync_snapshot(
        &self,
        peer: PeerId,
        payload: &[u8],
        network: NetworkId,
    ) {
        let remote = match State::decode_snapshot(payload, network) {
            Ok(s) => s,
            Err(e) => {
                let mut st = self.shared.lock().unwrap();
                if let Some(pv) = st.peers.get_mut(&peer) {
                    pv.status = format!("Snapshot inválido: {e:?}");
                }
                return;
            }
        };
        let mut s = self.state.lock().await;
        let result = s.merge_if_fresher(&remote, &self.storage);
        let mut st = self.shared.lock().unwrap();
        if let Some(pv) = st.peers.get_mut(&peer) {
            let supply = remote.total_supply();
            pv.connected = true;
            pv.synced = true;
            pv.supply_ok = supply <= HARD_CAP_SAT;
            pv.supply_lit = format_amount(supply);
            pv.root = root_prefix(&remote);
            pv.fee_pool_lit = format_amount(remote.fee_pool);
            pv.tx_count = remote.tx_count;
            pv.epoch = remote.epoch;
            pv.accounts = remote.balances.len();
            pv.status = match result {
                Ok(SyncResult::RemoteAdopted) => {
                    // Gate B+C+G aprobado del lado wallet también: el snapshot
                    // traía su prueba de invariancia por delante.
                    pv.synced = true;
                    "Synced ✓".to_string()
                }
                Ok(SyncResult::LocalNewer) => "Local más nuevo".to_string(),
                Ok(SyncResult::AlreadySynced) => "En sincronía ✓".to_string(),
                Ok(SyncResult::Divergent) => "DIVERGENTE 🚨".to_string(),
                Ok(SyncResult::RejectedPorGate(gate)) => {
                    // Zero-trust: la wallet-node NO se traga snapshots que
                    // violan invariantes (decisión §5-② del diseño). NO se
                    // adopta, NO se reenvía, y el estado local queda intacto.
                    // Strike GRAVE: ban inmediato de 60 min (§2, §5-①).
                    let outcome = self
                        .guard
                        .lock()
                        .unwrap()
                        .note_strike(&peer, StrikeKind::SnapshotInvalido, Instant::now());
                    pv.synced = false;
                    eprintln!(
                        "[wallet-net] peer {peer} envió snapshot INVÁLIDO (gate: {gate:?}) — rechazado, strike grave ⇒ {outcome:?}"
                    );
                    format!("🚨 Snapshot rechazado (gate: {gate:?})")
                }
                Err(e) => format!("Error al persistir sync: {e:?}"),
            };
        }
    }

    // ------------------------------------------------------------------
    // Mensajes recibidos (la wallet-node actúa como nodo full: valida, aplica,
    // reenvía y responde ASCII plano, compat con el core — hallazgo H)
    // ------------------------------------------------------------------

    async fn handle_incoming_request(
        &self,
        swarm: &mut Swarm<WalletBehaviour>,
        network: NetworkId,
        peer: PeerId,
        request: Vec<u8>,
        channel: request_response::ResponseChannel<Vec<u8>>,
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
                        self.mempool.lock().await.insert(hash);
                        // Contribución válida del peer (unit C).
                        self.guard.lock().unwrap().note_tx_aceptada(&peer);
                        if let Err(e) = s.flush(&self.storage) {
                            eprintln!("[wallet-net] error persistiendo TX recibida: {e:?}");
                        }
                        if s.is_checkpoint_due() {
                            s.finalize_checkpoint();
                            if let Err(e) = s.flush(&self.storage) {
                                eprintln!("[wallet-net] error persistiendo checkpoint: {e:?}");
                            }
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
                    let _ = swarm.behaviour_mut().rr.send_response(
                        channel,
                        b"INVALID_SYNC".to_vec(),
                    );
                    return;
                }
                let s = self.state.lock().await;
                let snapshot = s.encode_snapshot(network);
                let resp = build_msg(network, MSG_SYNC, &snapshot);
                // Contribución: snapshot servido válido (unit C).
                self.guard.lock().unwrap().note_snapshot_servido(&peer);
                let _ = swarm.behaviour_mut().rr.send_response(channel, resp);
            }
            _ => {
                let _ = swarm.behaviour_mut().rr.send_response(
                    channel,
                    b"UNKNOWN".to_vec(),
                );
            }
        }
    }

    /// Respuesta a un request NUESTRO: ack de broadcast o snapshot de sync.
    async fn handle_response(
        &self,
        network: NetworkId,
        peer: PeerId,
        request_id: request_response::OutboundRequestId,
        response: Vec<u8>,
        jobs: &mut Vec<BroadcastJob>,
    ) {
        // 1) ¿Es el ack de un broadcast propio?
        if let Some(job) = jobs.iter_mut().find(|j| j.open_ids.contains(&request_id)) {
            job.open_ids.remove(&request_id);
            let status = classify_tx_response(network, &response);
            job.mark_response(status);
            return;
        }
        // 2) Si no: snapshot de sync (framed).
        if let Some((MSG_SYNC, payload)) = split_msg(network, &response) {
            self.adopt_sync_snapshot(peer, payload, network).await;
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
}

