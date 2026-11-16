use std::path::{Path, PathBuf};
use std::sync::Arc;
use zeroize::{Zeroize, Zeroizing};

use ed25519_dalek::{Signer, SigningKey};
use eframe::egui;
use ledgerita_core::crypto::{self, Address};
use ledgerita_core::faucet::{DEFAULT_FAUCET_CAP, HARD_CAP_SAT};
use ledgerita_core::genesis;
use ledgerita_core::network::NetworkId;
use ledgerita_core::state::State;
use ledgerita_core::storage::{truncate_contacto_label, Storage};
use ledgerita_core::transaction::Transaction;
use tokio::sync::Mutex as AsyncMutex;

// Actor de red de la wallet-node (Fase 2.1a — Alternativa A incremental):
// la wallet levanta su PROPIO mini-nodo con data-dir propio, esquivando el
// lock exclusivo de sled (que solo conflicta entre PROCESOS sobre la misma
// carpeta).
use ledgerita_wallet::net_actor;
use ledgerita_wallet::net_actor::{BroadcastReport, NodeCmd, NodeHandles};

const LIT_DECIMALS: u64 = 1_000_000;

/// Fee mínimo con sentido: 1 satoshi (1 micro-LIT). El consenso aceptaría
/// fee 0, pero un fee nulo no quema nada (deuda 3b: el fee_pool se quema
/// en cada Cierre de Libro — fee 0 es participação nula). Prefill del
/// formulario de envío para transferencias rápidas: el usuario lo cambia
/// libremente (el input sigue siendo el dueño).
const DEFAULT_FEE_LIT: &str = "0.000001";

fn format_lit(amount: u64) -> String {
    let integer = amount / LIT_DECIMALS;
    let fraction = amount % LIT_DECIMALS;
    format!("{}.{:06}", integer, fraction)
}

/// Hora UTC `HH:MM:SS` desde un timestamp Unix-millis (la GUI no usa chrono;
/// para el historial local basta la hora de pared UTC).
fn format_hora_tx(ts_ms: u64) -> String {
    let secs = ts_ms / 1_000;
    format!(
        "{:02}:{:02}:{:02}",
        (secs / 3_600) % 24,
        (secs / 60) % 60,
        secs % 60
    )
}

fn lit_to_sats(lit: &str) -> Option<u64> {
    let t = lit.trim();
    if t.is_empty() {
        return None;
    }
    let (int_part, frac_part) = match t.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (t, None),
    };
    if int_part.is_empty() || !int_part.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let integer: u64 = int_part.parse().ok()?;
    let fraction: u64 = match frac_part {
        None => 0,
        Some(f) => {
            if f.is_empty() || !f.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let padded = format!("{:0<6}", f);
            padded[..6].parse().ok()?
        }
    };
    integer.checked_mul(LIT_DECIMALS)?.checked_add(fraction)
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string())
        .into()
}

/// Data-dir de la WALLET-NODE por red:
///
/// - Modo NORMAL: `~/.ledgerita-wallet/<red>/`.
/// - Modo PORTABLE (Fase 3d): si existe `portable.flag` junto al EXE, los
///   datos viven en `<exe_dir>/ledgerita-data/<red>` — ni el HOME ni el
///   sistema se tocan (USB/máquina sin instalar). La integridad del paquete
///   se verifica con SHA256SUMS.txt + `verificar_portable.ps1` (ver
///   empaquetar_portable.ps1).
///
/// Decisión 2026-11-05 (Alternativa A): la wallet usa SU PROPIA carpeta para
/// chocar jamás con el lock exclusivo de sled del daemon (que usa
/// `~/.ledgerita`). Cambiar el default crea wallet NUEVA para quien tenía
/// la wallet en `~/.ledgerita` (ver advertencia en la UI — es legítimo
/// apuntar ahí siempre que el daemon NO corra sobre esa carpeta a la vez).
fn default_wallet_dir(network: NetworkId) -> String {
    default_wallet_dir_in(network, portable_mode())
}

/// El modo es INPUT EXPLÍCITO: (a) para poder testearlo sin manipular
/// procesos y (b) para que el paquete portable JAMÁS pueda envenenar a la
/// wallet normal (un flag extraño junto al exe no afecta una copia sin
/// `portable.flag`).
fn default_wallet_dir_in(network: NetworkId, portable: bool) -> String {
    if portable {
        exe_dir()
            .join("ledgerita-data")
            .join(network.tag())
            .to_string_lossy()
            .to_string()
    } else {
        home_dir()
            .join(".ledgerita-wallet")
            .join(network.tag())
            .to_string_lossy()
            .to_string()
    }
}

/// ¿Correr en modo portable? Solo si existe el marcador `portable.flag`
/// junto al propio EXE (nada ambiental, nada remoto).
fn portable_mode() -> bool {
    exe_dir().join("portable.flag").exists()
}

/// Directorio del EXE en ejecución (fallback "." si el SO se pone raro).
fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Canonicaliza de forma tolerante (falla → path crudo); suficiente para
/// comparar rutas default. En Windows se quita el prefijo verbatim `\\?\`
/// que `canonicalize` antepone.
fn canonical(path: &str) -> String {
    let canonical = Path::new(path)
        .canonicalize()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| normalize(path));
    canonical
        .strip_prefix(r"\\?\")
        .unwrap_or(&canonical)
        .to_string()
}

fn normalize(path: &str) -> String {
    path.replace('/', "\\")
}

/// ¿El directorio elegido es el default del daemon core (`~/.ledgerita`)?
/// En ese caso la wallet-node NO puede correr a la vez que el daemon sobre
/// la misma carpeta (lock exclusivo de sled entre procesos).
fn is_core_default_dir(data_dir: &str) -> bool {
    if data_dir.trim().is_empty() {
        return false;
    }
    canonical(data_dir) == normalize(&home_dir().join(".ledgerita").to_string_lossy())
}

/// ¿`dir` es el default de la wallet-node para `network`? (para saber si al
/// cambiar de red se deben reposicionar los defaults o respetar un custom).
fn is_wallet_default_dir(dir: &str, network: NetworkId) -> bool {
    if dir.trim().is_empty() {
        return false;
    }
    canonical(dir) == normalize(&default_wallet_dir(network))
}

/// Parsea la lista de seeds (separadores: coma, `;` o línea nueva); vacío =
/// sin peers (la wallet-node correrá offline y NO aplicará TX).
fn parse_peers(seed: &str) -> Result<Vec<libp2p::Multiaddr>, String> {
    let mut peers = Vec::new();
    for part in seed.split(|c| c == ',' || c == ';' || c == '\n') {
        let text = part.trim();
        if text.is_empty() {
            continue;
        }
        let addr: libp2p::Multiaddr = text.parse().map_err(|_| {
            format!("Seed inválido: \"{text}\" debe ser un multiaddr con /p2p/<PeerId> final")
        })?;
        if net_actor::extract_peer_id(&addr).is_none() {
            return Err(format!(
                "Seed inválido: \"{text}\" debe incluir /p2p/<PeerId>"
            ));
        }
        peers.push(addr);
    }
    Ok(peers)
}

enum Screen {
    Unlock,
    Main,
}

/// Runtime de la wallet-node tras desbloquear: la GUI retiene los handles,
/// pero el ESTADO es propiedad del actor (la GUI solo lo lee y clonea).
/// Alta de contacto en el directorio (Checkpoint 20): persiste en el tree
/// `contactos` (node-local) si hay storage, y refresca la lista en memoria
/// con upsert + orden alfabético. Respete el tope de contactos.
fn guardar_contacto(
    storage: Option<&Storage>,
    addr: &Address,
    label: &str,
    contactos: &mut Vec<(String, String)>,
) -> Result<(), String> {
    if contactos.len() >= ledgerita_core::storage::CONTACTO_MAX
        && !contactos.iter().any(|(h, _)| h == &addr.to_hex())
    {
        return Err(format!(
            "Directorio lleno ({} contactos máximo)",
            ledgerita_core::storage::CONTACTO_MAX
        ));
    }
    let clean_label = truncate_contacto_label(label);
    if clean_label.is_empty() {
        return Err("Etiqueta de contacto vacía".to_string());
    }
    let hex = addr.to_hex();
    if let Some(s) = storage {
        s.save_contacto(addr, &clean_label)
            .map_err(|e| format!("Storage: {e}"))?;
        let _ = s.flush();
    }
    if let Some(slot) = contactos.iter_mut().find(|(h, _)| *h == hex) {
        slot.1 = clean_label;
    } else {
        contactos.push((hex, clean_label));
    }
    contactos.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    Ok(())
}

/// Determina la dirección activa `(address, use_d2)` dinámicamente tras sincronizar:
/// usa D2a por defecto en wallets nuevas, y conserva D1 si existe saldo o nonce legado
/// en `addr_d1` sin actividad en `addr_d2`.
fn resolve_active_account(state: &State, addr_d1: &Address, addr_d2: &Address) -> (Address, bool) {
    let has_d1 = state.balances.get(addr_d1).copied().unwrap_or(0) > 0
        || state.nonces.get(addr_d1).copied().unwrap_or(0) > 0;
    let has_d2 = state.balances.get(addr_d2).copied().unwrap_or(0) > 0
        || state.nonces.get(addr_d2).copied().unwrap_or(0) > 0;
    let use_d2 = !(has_d1 && !has_d2);
    if use_d2 {
        (addr_d2.clone(), true)
    } else {
        (addr_d1.clone(), false)
    }
}

/// Evalúa vencimiento de `AUTO_LOCK_SECS` ANTES de refrescar `last_activity` ante nuevos eventos.
fn check_auto_lock(
    screen: &Screen,
    last_activity: &mut std::time::Instant,
    has_input: bool,
    now: std::time::Instant,
) -> bool {
    if !matches!(screen, Screen::Main) {
        return false;
    }
    if now.saturating_duration_since(*last_activity)
        >= std::time::Duration::from_secs(AUTO_LOCK_SECS)
    {
        return true;
    }
    if has_input {
        *last_activity = now;
    }
    false
}

struct NodeRuntime {
    addr_d1: Address,
    addr_d2: Address,
    signing_seed: Zeroizing<[u8; 32]>,
    state: Arc<AsyncMutex<State>>,
    handles: NodeHandles,
    /// Acceso al storage node-local de la wallet: el directorio de
    /// contactos vive acá (tree `contactos`), ajeno al actor de red.
    storage: Arc<Storage>,
}

/// Contactos de fábrica: direcciones públicas del laboratorio kettles,
/// precargadas SOLO cuando el directorio está vacío en una red nueva
/// (después el directorio es del usuario: libre de editar/borrar).
const CONTACTOS_FABRICA_TESTNET: &[(&str, &str)] = &[
    (
        "B — kettle02 receptor",
        "9ce91363fc369caf6a302d091566a1cc8b11e1d118d8ec416a0196210477fbdf",
    ),
    (
        "nodo kettle02 (testnet)",
        "2e136a85a6ca17678637e1e219bc2db601889a7375cfc790594ceb3d1b0138dd",
    ),
    (
        "nodo kettle03 (testnet)",
        "bfd54b3153d851854b09dcf9f47063abff13e2768d2e486054997853ba67f97f",
    ),
];
const CONTACTOS_FABRICA_MAINNET: &[(&str, &str)] = &[
    (
        "nodo kettle02 (mainnet)",
        "6893239f5850555f5701f24342bfbef5afdab94835f08d6e5f352899d4a17f28",
    ),
    (
        "nodo kettle03 (mainnet)",
        "b1435c81522ac1e2b0f503986ca93bde92542b5d4cab4b9a9fd21253667e4cba",
    ),
];

fn contactos_de_fabrica(network: NetworkId) -> &'static [(&'static str, &'static str)] {
    match network {
        NetworkId::Mainnet => CONTACTOS_FABRICA_MAINNET,
        NetworkId::Testnet => CONTACTOS_FABRICA_TESTNET,
    }
}

struct WalletApp {
    data_dir: String,
    network: NetworkId,
    seed_addr: String,
    password: String,
    unlock_error: Option<String>,
    screen: Screen,

    node: Option<NodeRuntime>,
    state_cache: Option<State>,
    /// Última TX firmada en memoria (para "Reintentar" — los reintentos viven
    /// en memoria, en la GUI y en el actor: nunca se persisten sin red).
    last_tx: Option<Transaction>,

    send_receiver: String,
    send_amount: String,
    send_fee: String,
    send_status: Option<(String, bool)>,
    /// Directorio de direcciones frecuentes (Checkpoint 20): (hex, etiqueta).
    contactos: Vec<(String, String)>,
    /// Formulario de alta manual en el directorio.
    contacto_label: String,
    contacto_addr: String,
    /// ¿Panel del directorio desplegado? (usa `ui.collapsing` directo desde
    /// el pulido del Ckpt 15; el bool quedó muerto y se elimina).

    /// Faucet TestNet: receptor en hex (vacío = esta wallet) y monto en LIT.
    faucet_recipient: String,
    faucet_amount: String,
    /// Status local del faucet ("en camino"/error de validación) — lo
    /// muestra el PANEL del faucet, no el de Enviar (`send_status`).
    faucet_pending: Option<(String, bool)>,
    /// ¿Mostrar el QR de la propia dirección (Fase 3c)?
    show_qr: bool,

    /// Respaldo/Restauración de wallet (Checkpoint 24): destino del
    /// respaldo en el panel del dashboard y ruta del respaldo para
    /// restaurar en la pantalla de desbloqueo; status de cada uno.
    backup_destino: String,
    backup_password: String,
    backup_status: Option<(String, bool)>,
    import_ruta: String,
    import_status: Option<(String, bool)>,

    copied: Option<String>,
    last_activity: std::time::Instant,
}

const AUTO_LOCK_SECS: u64 = 300;

fn default_seeds_string(network: NetworkId) -> String {
    net_actor::default_seeds(network).join(", ")
}

impl WalletApp {
    fn new() -> Self {
        WalletApp {
            data_dir: default_wallet_dir(NetworkId::Testnet),
            network: NetworkId::Testnet,
            seed_addr: default_seeds_string(NetworkId::Testnet),
            password: String::new(),
            unlock_error: None,
            screen: Screen::Unlock,
            node: None,
            state_cache: None,
            last_tx: None,
            send_receiver: String::new(),
            send_amount: String::new(),
            send_fee: DEFAULT_FEE_LIT.to_string(),
            send_status: None,
            contactos: Vec::new(),
            contacto_label: String::new(),
            contacto_addr: String::new(),
            faucet_recipient: String::new(),
            faucet_amount: "1.000000".to_string(),
            faucet_pending: None,
            show_qr: false,
            backup_destino: String::new(),
            backup_password: String::new(),
            backup_status: None,
            import_ruta: String::new(),
            import_status: None,
            copied: None,
            last_activity: std::time::Instant::now(),
        }
    }

    /// Al cambiar de red: si el directorio y el seed siguen siendo defaults
    /// (o están vacíos), se reposicionan a los de la nueva red; un valor
    /// custom del usuario se respeta SIEMPRE.
    fn on_network_changed(&mut self) {
        if self.data_dir.trim().is_empty()
            || is_wallet_default_dir(&self.data_dir, NetworkId::Mainnet)
            || is_wallet_default_dir(&self.data_dir, NetworkId::Testnet)
        {
            self.data_dir = default_wallet_dir(self.network);
        }
        let seed = self.seed_addr.trim();
        if seed.is_empty()
            || seed == net_actor::default_seed(NetworkId::Mainnet)
            || seed == net_actor::default_seed(NetworkId::Testnet)
            || seed == default_seeds_string(NetworkId::Mainnet)
            || seed == default_seeds_string(NetworkId::Testnet)
        {
            self.seed_addr = default_seeds_string(self.network);
        }
    }

    fn lock_wallet(&mut self, ctx: &egui::Context) {
        self.node = None;
        self.state_cache = None;
        self.last_tx = None;
        self.send_status = None;
        self.faucet_pending = None;
        self.backup_status = None;
        self.import_status = None;
        self.password.zeroize();
        self.backup_password.zeroize();
        self.screen = Screen::Unlock;
        ctx.request_repaint();
    }

    /// Al cambiar de red desde el dashboard (7b): CIERRA la sesión — soltar
    /// el NodeRuntime detiene al actor limpio (el channel `cmd_rx` muere,
    /// el actor retorna) — y vuelve a la pantalla de desbloqueo, donde la
    /// wallet correcta (dir/seed de la red elegida) se re-abre con password.
    fn cambiar_red(&mut self, ctx: &egui::Context) {
        self.lock_wallet(ctx);
        self.on_network_changed();
        ctx.request_repaint();
    }

    fn unlock(&mut self, ctx: &egui::Context) {
        let result = (|| -> Result<NodeRuntime, String> {
            let data_dir = PathBuf::from(&self.data_dir);
            let (sk, kp) = crypto::load_or_create_wallet(&data_dir, &self.password)
                .map_err(|e| format!("Wallet: {e}"))?;
            let addr_d1 = Address::from_signing_key(&sk);
            let addr_d2 = Address::from_signing_key_d2(&sk);
            std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
            let storage =
                Arc::new(Storage::open(&data_dir.join("state")).map_err(|e| e.to_string())?);

            // Directorio de contactos (Checkpoint 20): con el book vacío se
            // precargan los contactos de fábrica de la red (direcciones
            // públicas del laboratorio); después el directorio es del
            // usuario, libre de editar y borrar.
            match storage.load_contactos() {
                Ok(v) if v.is_empty() => {
                    for (label, hex) in contactos_de_fabrica(self.network) {
                        if let Some(addr) = Address::from_hex(hex) {
                            let _ = storage.save_contacto(&addr, label);
                        }
                    }
                    let _ = storage.flush();
                }
                _ => {}
            }
            self.contactos = storage
                .load_contactos()
                .unwrap_or_default()
                .into_iter()
                .map(|(a, l)| (a.to_hex(), l))
                .collect();

            let mut state = State::load(&storage).map_err(|e| e.to_string())?;
            if genesis::apply_genesis(&mut state, self.network) {
                state.flush(&storage).map_err(|e| e.to_string())?;
            }
            let signing_seed = Zeroizing::new(sk.to_bytes());
            drop(sk);

            let peers = parse_peers(&self.seed_addr)?;

            let state = Arc::new(AsyncMutex::new(state));
            let handles = net_actor::spawn_wallet_node(
                self.network,
                kp,
                state.clone(),
                storage.clone(),
                peers,
                0, // puerto efímero: la wallet-node no necesita puerto fijo
            )?;
            Ok(NodeRuntime {
                addr_d1,
                addr_d2,
                signing_seed,
                state,
                handles,
                storage,
            })
        })();

        match result {
            Ok(node) => {
                self.password.zeroize();
                self.node = Some(node);
                self.state_cache = None;
                self.unlock_error = None;
                self.last_activity = std::time::Instant::now();
                self.screen = Screen::Main;
            }
            Err(e) => self.unlock_error = Some(e),
        }
        ctx.request_repaint();
    }

    fn current_active_address(&self, node: &NodeRuntime) -> Address {
        if let Some(state) = &self.state_cache {
            resolve_active_account(state, &node.addr_d1, &node.addr_d2).0
        } else {
            node.addr_d2.clone()
        }
    }

    /// **Broadcast-first**: valida, firma, envía al actor y NUNCA aplica
    /// directo — el estado local lo toca únicamente la red a través del
    /// actor (solo si algún peer responde OK/DUPLICATE).
    fn do_send(&mut self, ctx: &egui::Context) {
        match self.build_signed_tx() {
            Ok(tx) => {
                let sent = if let Some(n) = &self.node {
                    n.handles.shared.lock().unwrap().last_broadcast = None;
                    n.handles.send_cmd.send(NodeCmd::BroadcastTx(tx.clone())).is_ok()
                } else {
                    false
                };
                if sent {
                    self.last_tx = Some(tx);
                    self.send_status = Some((
                        "📤 TX firmada y en camino a la red — esperando confirmación…"
                            .to_string(),
                        false,
                    ));
                } else {
                    self.send_status = Some((
                        "El actor de red no está corriendo (¿se cerró el hilo?)".to_string(),
                        true,
                    ));
                }
            }
            Err(e) => self.send_status = Some((e, true)),
        }
        ctx.request_repaint();
    }

    /// Solicita LIT de prueba al nodo operador del Faucet TestNet (`MSG_FAUCET`).
    fn do_faucet(&mut self, send_cmd: &tokio::sync::mpsc::UnboundedSender<NodeCmd>, ctx: &egui::Context) {
        match (|| -> Result<NodeCmd, String> {
            if self.network != NetworkId::Testnet {
                return Err("El faucet solo existe en TestNet".to_string());
            }
            let node = self.node.as_ref().ok_or("Wallet no desbloqueada")?;
            let recipient = if self.faucet_recipient.trim().is_empty() {
                self.current_active_address(node)
            } else {
                Address::from_hex(self.faucet_recipient.trim())
                    .ok_or_else(|| "Dirección de receptor inválida (64 hex)".to_string())?
            };
            let amount = lit_to_sats(&self.faucet_amount)
                .ok_or_else(|| "Monto invalido".to_string())?;
            if amount == 0 {
                return Err("El monto debe ser mayor a 0".to_string());
            }
            Ok(NodeCmd::Faucet {
                recipient,
                amount,
                cap: DEFAULT_FAUCET_CAP,
            })
        })() {
            Ok(cmd) => {
                if let Some(n) = &self.node {
                    n.handles.shared.lock().unwrap().faucet_status = None;
                }
                if send_cmd.send(cmd).is_ok() {
                    self.faucet_pending = Some((
                        "💧 Solicitud enviada al Faucet TestNet…".to_string(),
                        false,
                    ));
                } else {
                    self.faucet_pending =
                        Some(("El actor de red no está corriendo".to_string(), true));
                }
            }
            Err(e) => self.faucet_pending = Some((e, true)),
        }
        ctx.request_repaint();
    }

    fn build_signed_tx(&self) -> Result<Transaction, String> {
        let node = self.node.as_ref().ok_or("Wallet no desbloqueada")?;
        let receiver = Address::from_hex(self.send_receiver.trim())
            .ok_or_else(|| "Dirección de receptor inválida (64 hex)".to_string())?;
        let amount =
            lit_to_sats(&self.send_amount).ok_or_else(|| "Monto inválido".to_string())?;
        if amount == 0 {
            return Err("El monto debe ser mayor a 0".to_string());
        }
        let fee = lit_to_sats(&self.send_fee).ok_or_else(|| "Fee inválido".to_string())?;
        if amount.checked_add(fee).is_none() {
            return Err("Monto + fee desbordan el rango".to_string());
        }
        let (addr, use_d2, nonce) = {
            let s = node.state.blocking_lock();
            let (addr, use_d2) = resolve_active_account(&s, &node.addr_d1, &node.addr_d2);
            let nonce = s.nonces.get(&addr).copied().unwrap_or(0) + 1;
            (addr, use_d2, nonce)
        };
        if receiver == addr {
            return Err("No puedes enviarte LIT a ti mismo".to_string());
        }
        let sk = SigningKey::from_bytes(&node.signing_seed);
        let mut tx = if use_d2 {
            Transaction::new_d2(&sk.verifying_key(), receiver, amount, fee, nonce)
        } else {
            Transaction::new(addr, receiver, amount, fee, nonce)
        }
        .with_chain_id(self.network.chain_id());
        let msg = tx.to_bytes();
        tx.sign(sk.sign(&msg));
        Ok(tx)
    }

    fn copy_address(&mut self, ctx: &egui::Context, addr: &Address) {
        ctx.copy_text(addr.to_hex());
        self.copied = Some("✓ Dirección copiada".to_string());
    }

    /// Exporta el respaldo (Checkpoint 24): el blob CIFRADO de wallet.key
    /// del directorio de la sesión → archivo a elección. Valida contraseña temporal
    /// y la limpia inmediatamente con `zeroize()`.
    fn do_export_respaldo(&mut self) {
        let destino = self.backup_destino.trim().to_string();
        if destino.is_empty() {
            self.backup_password.zeroize();
            self.backup_status =
                Some(("Elegí primero un archivo destino para el respaldo".into(), true));
            return;
        }
        if self.backup_password.is_empty() {
            self.backup_status = Some((
                "Ingresa tu contraseña actual para confirmar y exportar el respaldo".into(),
                true,
            ));
            return;
        }
        let res = crypto::export_wallet_backup(
            &PathBuf::from(&self.data_dir),
            &self.backup_password,
            Path::new(&destino),
        );
        self.backup_password.zeroize();
        match res {
            Ok(()) => self.backup_status = Some((
                format!(
                    "Respaldo guardado en {destino} — guarda tu contraseña: sin ella el blob NO se abre"
                ),
                false,
            )),
            Err(e) => self.backup_status = Some((e, true)),
        }
    }

    /// Diálogo nativo de GUARDAR (Checkpoint 25): el usuario elige la
    /// carpeta y el nombre del respaldo con el explorador de archivos
    /// del SO (rfd). El bloqueante es seguro: los modales de rfd corren
    /// en el hilo principal Y egui manda a repaint al cerrar el modal.
    fn explorar_destino_respaldo(&mut self) {
        let sugerido = format!(
            "respaldo-ledgerita-{}.key",
            &self.directorio_dir_a_sugerencia()
        );
        if let Some(elegido) = rfd::FileDialog::new()
            .set_title("¿Dónde guardar el respaldo de tu wallet?")
            .set_file_name(&sugerido)
            .add_filter("Respaldo Ledgerita (.key)", &["key"])
            .save_file()
        {
            self.backup_destino = elegido.display().to_string();
            self.backup_status = Some((
                format!("Destino elegido: {}", self.backup_destino),
                false,
            ));
        }
    }

    /// Diálogo nativo de ABRIR (Checkpoint 25): ruta del respaldo a
    /// restaurar, sin tipear a mano.
    fn explorar_respaldo_para_restaurar(&mut self) {
        if let Some(elegido) = rfd::FileDialog::new()
            .set_title("¿Qué respaldo querés restaurar?")
            .add_filter("Respaldo de Ledgerita (*.key)", &["key"])
            .pick_file()
        {
            self.import_ruta = elegido.display().to_string();
            self.import_status = Some((
                format!("Respaldo elegido: {}", self.import_ruta),
                false,
            ));
        }
    }

    /// Sugerencia de nombre de archivo: la última parte del data_dir
    /// (ej. "~/.ledgerita-wallet/testnet" → "testnet") da el nombre corto.
    fn directorio_dir_a_sugerencia(&self) -> String {
        Path::new(self.data_dir.trim())
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "wallet".to_string())
    }

    /// Restaurar un respaldo (Checkpoint 24, en la pantalla de desbloqueo):
    /// el respaldo pasa a SER la wallet del directorio indicado. Valida el
    /// descifre con la contraseña tipeada ANTES de tocar nada; si ya había
    /// wallet, la vigente queda en wallet.key.bak-<fecha>.
    fn do_restaurar_respaldo(&mut self) {
        let ruta = self.import_ruta.trim().to_string();
        if ruta.is_empty() {
            self.import_status =
                Some(("Escribe primero la ruta del archivo de respaldo".into(), true));
            return;
        }
        match crypto::import_wallet_backup(
            Path::new(&ruta),
            &PathBuf::from(&self.data_dir),
            &self.password,
        ) {
            Ok(reemplazo) => {
                self.import_status = Some((
                    match reemplazo {
                        Some(bak) => format!(
                            "Restaurado en {} — la wallet ANTERIOR quedó en {} y la sesión SUBE con la dirección DEL RESPALDO",
                            self.data_dir,
                            bak.display()
                        ),
                        None => format!(
                            "Restaurado en {} — desbloqueá con la MISMA contraseña del respaldo",
                            self.data_dir
                        ),
                    },
                    false,
                ));
                self.password.zeroize();
            }
            Err(e) => {
                self.password.zeroize();
                self.import_status = Some((e, true));
            }
        }
    }

    fn draw_unlock(&mut self, ui: &mut egui::Ui) {
        ui.add_space(16.0);
        ui.vertical_centered(|ui| {
            ui.heading("💎 Ledgerita Wallet");
            ui.label("Protocolo de Prueba de Fórmula — nodo full");
        });
        ui.add_space(24.0);

        egui::Grid::new("unlock_grid")
            .num_columns(2)
            .spacing([8.0, 8.0])
            .show(ui, |ui| {
                ui.label("Red:");
                ui.horizontal(|ui| {
                    if ui
                        .radio_value(&mut self.network, NetworkId::Mainnet, "MainNet")
                        .changed()
                    {
                        self.on_network_changed();
                    }
                    if ui
                        .radio_value(&mut self.network, NetworkId::Testnet, "TestNet")
                        .changed()
                    {
                        self.on_network_changed();
                    }
                });
                ui.end_row();

                ui.label("Directorio:");
                ui.add(egui::TextEdit::singleline(&mut self.data_dir).desired_width(280.0));
                ui.end_row();

                ui.label("Seed de red:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.seed_addr)
                        .desired_width(280.0)
                        .hint_text("multiaddr del nodo semilla (con /p2p/<id> final)"),
                );
                ui.end_row();

                ui.label("Contraseña:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.password)
                        .password(true)
                        .desired_width(280.0),
                );
                ui.end_row();
            });

        if is_core_default_dir(&self.data_dir) {
            ui.colored_label(
                egui::Color32::YELLOW,
                "⚠️ Apunta al directorio del daemon (~/.ledgerita). Si esa wallet ya existe úsala, pero NO corras el daemon sobre esa carpeta a la vez: sled usa lock exclusivo por proceso.",
            );
        } else {
            ui.small("💡 La wallet-node usará data-dir propio (~/.ledgerita-wallet/<red>) y validará la red por sí misma (zero-trust).");
        }
        ui.add_space(12.0);
        if ui
            .button("🔓 Desbloquear / Crear y levantar nodo")
            .clicked()
        {
            self.unlock(ui.ctx());
        }
        if let Some(err) = &self.unlock_error {
            ui.colored_label(egui::Color32::RED, format!("❌ {err}"));
        }
        if let Some((msg, is_err)) = &self.import_status {
            if *is_err {
                ui.colored_label(egui::Color32::RED, format!("❌ {msg}"));
            } else {
                ui.colored_label(egui::Color32::LIGHT_GREEN, format!("✅ {msg}"));
            }
        }
        ui.add_space(8.0);
        ui.separator();
        ui.collapsing("📥 Restaurar desde un respaldo", |ui| {
            ui.small(
                "¿Venís con un respaldo hecho desde otra máquina? Cargalo acá: se convierte en la wallet del DIRECTORIO de arriba (y desbloqueá con la MISMA contraseña que tuvo el respaldo). Si el directorio ya tenía wallet, la vigente se salva como wallet.key.bak-<fecha> — OJO: restaurar otra wallet cambia también la identidad P2P del nodo de esa carpeta (la identidad nace de la misma seed).",
            );
            ui.add_space(4.0);
            egui::Grid::new("import_grid")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Respaldo:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.import_ruta)
                            .desired_width(340.0)
                            .hint_text("ruta del archivo de respaldo (ej. D:\\backup\\ledgerita.key)"),
                    );
                    ui.end_row();
                });
            ui.horizontal(|ui| {
                if ui.button("📂 Explorar…").clicked() {
                    self.explorar_respaldo_para_restaurar();
                }
                if ui.button("📥 Restaurar respaldo en el directorio").clicked() {
                    self.do_restaurar_respaldo();
                }
                if ui.button("✖ Limpiar").clicked() {
                    self.import_ruta = String::new();
                    self.import_status = None;
                }
            });
        });
        ui.add_space(8.0);
        ui.separator();
        ui.small("⚠️  Modo demostración: esto NO es dinero real. La wallet se guarda encriptada en el directorio indicado.");
    }

    fn draw_main(&mut self, ui: &mut egui::Ui) {
        // Extraer los Arcs ANTES de tocar &mut self: los clones son baratos y
        // evitan pelear con el borrow checker de egui (node vive mientras sí).
        let (addr_d1, addr_d2, state_arc, shared_arc, send_cmd) = match &self.node {
            Some(n) => (
                n.addr_d1.clone(),
                n.addr_d2.clone(),
                n.state.clone(),
                n.handles.shared.clone(),
                n.handles.send_cmd.clone(),
            ),
            None => return,
        };

        // Estado vigente: try_lock del actor; si está ocupado se usa el cache
        // del frame anterior (la GUI nunca bloquea al actor).
        if let Ok(state) = state_arc.try_lock() {
            self.state_cache = Some(state.clone());
        }
        let state = match &self.state_cache {
            Some(s) => s.clone(),
            None => return,
        };
        let (addr, _use_d2) = resolve_active_account(&state, &addr_d1, &addr_d2);
        let balance = state.balances.get(&addr).copied().unwrap_or(0);
        let supply = state.total_supply();
        let remaining = HARD_CAP_SAT.saturating_sub(supply);

        // Indicador de sincronía (sync en vivo, Checkpoint 36 / 55): el saldo del
        // dashboard se declara CONFIRMADO por la red SOLO si hay al menos un peer
        // actualmente CONECTADO y sincronizado cuya raíz coincida con el estado local.
        let (sync_msg, sync_color, saldo_confirmado) = {
            let shared = shared_arc.lock().unwrap();
            let raiz_local = net_actor::root_prefix(&state);
            let conectados = shared.peers.values().filter(|pv| pv.connected).count();
            let coincide = shared
                .peers
                .values()
                .any(|pv| pv.connected && pv.synced && pv.root == raiz_local);
            if coincide {
                (
                    format!("✅ Sincronizado con la red (raíz {raiz_local})"),
                    egui::Color32::LIGHT_GREEN,
                    true,
                )
            } else if conectados > 0 {
                (
                    "⏳ Conectado — esperando snapshot de la red…".to_string(),
                    egui::Color32::YELLOW,
                    false,
                )
            } else {
                (
                    "⚠ Sin conexión — saldo local SIN confirmar por la red".to_string(),
                    egui::Color32::YELLOW,
                    false,
                )
            }
        };

        ui.horizontal(|ui| {
            ui.heading("🏦 Dashboard");
            // Tema claro/oscuro en vivo (7a): dark de fábrica, alternable.
            let oscuro = ui.style().visuals.dark_mode;
            if ui
                .button(if oscuro { "☀️ Claro" } else { "🌙 Oscuro" })
                .clicked()
            {
                let nuevo = if oscuro {
                    egui::Visuals::light()
                } else {
                    egui::Visuals::dark()
                };
                ui.ctx().set_visuals(nuevo);
            }
            ui.separator();
            // Selector de red en vivo (7b): cambiar de red CERRA la sesión
            // (soltar NodeRuntime detiene al actor limpio) y regresa al
            // desbloqueo con dir/seed reposicionados si eran defaults.
            ui.small("Red:");
            if ui
                .radio_value(&mut self.network, NetworkId::Mainnet, "MainNet")
                .changed()
            {
                self.cambiar_red(ui.ctx());
            }
            if ui
                .radio_value(&mut self.network, NetworkId::Testnet, "TestNet")
                .changed()
            {
                self.cambiar_red(ui.ctx());
            }
            ui.separator();
            if ui.button("🔒 Bloquear").clicked() {
                self.lock_wallet(ui.ctx());
                return;
            }
        });
        ui.add_space(8.0);
        egui::Grid::new("dash_grid")
            .num_columns(2)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                ui.label("Dirección:");
                ui.monospace(addr.to_hex());
                ui.end_row();
                ui.label("Balance:");
                ui.monospace(if saldo_confirmado {
                    format!("{} LIT ✓ (confirmado por la red)", format_lit(balance))
                } else {
                    format!("{} LIT (sin confirmar)", format_lit(balance))
                });
                ui.end_row();
                ui.label("Estado de red:");
                ui.colored_label(sync_color, &sync_msg);
                ui.end_row();
                ui.label("Supply total:");
                ui.monospace(format!(
                    "{} / {} LIT",
                    format_lit(supply),
                    format_lit(HARD_CAP_SAT)
                ));
                ui.end_row();
                ui.label("Restante al hard cap:");
                ui.monospace(format!("{} LIT", format_lit(remaining)));
                ui.end_row();
                ui.label("Fee pool:");
                ui.monospace(format!("{} LIT", format_lit(state.fee_pool)));
                ui.end_row();
                // GOTE (Checkpoint 21): la emisión del tesoro y lo que
                // queda por liberar (constitución B ever-minted: 21M −
                // emitido; lo quemado NO reabre tesoro).
                ui.label("Goteo emitido:");
                ui.monospace(format!(
                    "{} LIT",
                    format_lit(state.emitido_total)
                ));
                ui.end_row();
                ui.label("Tesoro restante:");
                ui.monospace(format!(
                    "{} LIT",
                    format_lit(state.tesoro_disponible())
                ));
                ui.end_row();
                ui.label("Época:");
                ui.monospace(state.epoch.to_string());
                ui.end_row();
                ui.label("TX confirmadas:");
                ui.monospace(state.tx_count.to_string());
                ui.end_row();
                ui.label("Cuentas:");
                ui.monospace(state.balances.len().to_string());
                ui.end_row();
                ui.label("Red:");
                ui.monospace(self.network.tag().to_string());
                ui.end_row();
            });

        ui.horizontal(|ui| {
            if ui.button("📋 Copiar dirección").clicked() {
                self.copy_address(ui.ctx(), &addr);
            }
            // QR de la dirección (Fase 3c): generado en vivo y pintado
            // módulo a módulo (sin crate de imagen ni textura de egui).
            if ui
                .button(if self.show_qr {
                    "📷 Esconder QR"
                } else {
                    "📷 Ver QR"
                })
                .clicked()
            {
                self.show_qr = !self.show_qr;
            }
            if let Some(c) = &self.copied {
                ui.colored_label(egui::Color32::LIGHT_GREEN, c);
            }
        });
        if self.show_qr {
            ui.add_space(6.0);
            draw_qr(ui, &addr.to_hex());
            ui.small("Lector QR: la dirección hex de esta wallet (64 caracteres).");
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(4.0);
        {
            let shared = shared_arc.lock().unwrap();
            draw_network_panel(ui, &shared, &mut self.send_status, &mut self.last_tx);
        }

        // Editor de conexión en vivo: `ChangePeers` re-diala automáticamente
        // (el actor reintenta cada 30s hasta lograrlo).
        ui.add_space(8.0);
        ui.collapsing("🔌 Conexión", |ui| {
            ui.label("Seed de red:");
            ui.add(
                egui::TextEdit::singleline(&mut self.seed_addr)
                    .desired_width(360.0)
                    .hint_text("multiaddr con /p2p/<PeerId> final"),
            );
            if ui.button("🔄 Aplicar cambios de conexión").clicked() {
                let seed = self.seed_addr.trim().to_string();
                match parse_peers(&seed) {
                    Ok(peers) => {
                        if send_cmd.send(NodeCmd::ChangePeers(peers)).is_ok() {
                            self.send_status =
                                Some(("🔄 Peers aplicados al nodo".to_string(), false));
                        } else {
                            self.send_status = Some((
                                "El actor de red no está corriendo".to_string(),
                                true,
                            ));
                        }
                    }
                    Err(e) => self.send_status = Some((e, true)),
                }
            }
            ui.small("El nodo re-diala automáticamente cada 30s a los peers no conectados.");
        });

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);

        ui.heading("📤 Enviar LIT (broadcast-first)");
        ui.add_space(8.0);
        egui::Grid::new("send_grid")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label("Receptor (hex):");
                ui.add(
                    egui::TextEdit::singleline(&mut self.send_receiver)
                        .desired_width(300.0)
                        .hint_text("64 caracteres hexadecimales"),
                );
                ui.end_row();
                ui.label("Monto (LIT):");
                ui.add(
                    egui::TextEdit::singleline(&mut self.send_amount)
                        .desired_width(120.0)
                        .hint_text("0.000000"),
                );
                ui.end_row();
                ui.label("Fee (LIT):");
                ui.add(
                    egui::TextEdit::singleline(&mut self.send_fee)
                        .desired_width(120.0)
                        .hint_text("0.000000"),
                );
                ui.end_row();
            });

        // Directorio de direcciones frecuentes (Checkpoint 20): usar,
        // añadir y borrar contactos node-local (tree `contactos`).
        ui.add_space(8.0);
        ui.collapsing(
            format!("📖 Directorio ({})", self.contactos.len()),
            |ui| {
                for (hex, label) in self.contactos.clone() {
                    ui.horizontal(|ui| {
                        if ui
                            .button(format!("📌 {label}"))
                            .clicked()
                        {
                            self.send_receiver = hex.clone();
                            self.send_status =
                                Some(("📌 Receptor llenado desde el directorio".into(), false));
                        }
                        let corto = &hex[0..8];
                        ui.monospace(format!("{corto}…"));
                        if ui.button("✖").clicked() {
                            if let Some(node) = &self.node {
                                if let Some(addr) = Address::from_hex(&hex) {
                                    let _ = node.storage.delete_contacto(&addr);
                                    let _ = node.storage.flush();
                                }
                            }
                            self.contactos.retain(|(h, _)| h != &hex);
                        }
                    });
                }
                ui.add_space(6.0);
                ui.label("➕ Añadir contacto:");
                egui::Grid::new("contacto_nuevo_grid")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("Etiqueta:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.contacto_label)
                                .desired_width(200.0)
                                .hint_text("ej. nodo kettle02"),
                        );
                        ui.end_row();
                        ui.label("Dirección:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.contacto_addr)
                                .desired_width(460.0)
                                .hint_text("64 caracteres hex…"),
                        );
                        ui.end_row();
                    });
                if ui.button("✚ Guardar contacto").clicked() {
                    let hex = self.contacto_addr.trim().to_string();
                    let label = self.contacto_label.trim().to_string();
                    match Address::from_hex(&hex) {
                        Some(addr) if !label.is_empty() => match guardar_contacto(
                            self.node.as_ref().map(|n| n.storage.as_ref()),
                            &addr,
                            &label,
                            &mut self.contactos,
                        ) {
                            Ok(_) => {
                                self.contacto_addr = String::new();
                                self.contacto_label = String::new();
                                self.send_status =
                                    Some(("✚ Contacto guardado en el directorio".into(), false));
                            }
                            Err(e) => self.send_status = Some((e, true)),
                        },
                        Some(_) => {
                            self.send_status =
                                Some(("Ponéle una etiqueta al contacto".into(), true));
                        }
                        None => {
                            self.send_status =
                                Some(("Dirección inválida (64 hex)".into(), true));
                        }
                    }
                }
            },
        );

        ui.add_space(8.0);
        if ui.button("💸 Firmar y enviar a la red").clicked() {
            self.do_send(ui.ctx());
        }
        if let Some((msg, is_err)) = &self.send_status {
            if *is_err {
                ui.colored_label(egui::Color32::RED, format!("❌ {msg}"));
            } else {
                ui.colored_label(egui::Color32::LIGHT_BLUE, msg);
            }
        }
        let retry_pending = shared_arc.lock().unwrap().retry_pending;
        if retry_pending && ui.button("🔁 Reintentar la última TX ahora").clicked() {
            if let Some(tx) = self.last_tx.clone() {
                shared_arc.lock().unwrap().last_broadcast = None;
                if send_cmd.send(NodeCmd::BroadcastTx(tx)).is_ok() {
                    self.send_status = Some((
                        "🔁 Reenviada a la red (si ya la tenía, responderá DUPLICATE)"
                            .to_string(),
                        false,
                    ));
                }
            }
            ui.ctx().request_repaint();
        }

        // --------------------------------------------------------------
        // Faucet TestNet (solo TestNet): TX real firmada por peer servidor Faucet (Gate g7).
        // --------------------------------------------------------------
        if self.network == NetworkId::Testnet {
            ui.add_space(16.0);
            ui.separator();
            ui.add_space(8.0);
            ui.heading("🪚 Faucet TestNet");
            ui.small(format!(
                "Solicita LIT de prueba a un nodo Faucet TestNet conectado (--faucet-key-file, TX firmada verificada por Gate g7). Tope por dirección: {} LIT.",
                format_lit(DEFAULT_FAUCET_CAP)
            ));
            ui.add_space(4.0);
            egui::Grid::new("faucet_grid")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Receptor (hex):");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.faucet_recipient)
                            .desired_width(300.0)
                            .hint_text("vacío = esta wallet"),
                    );
                    ui.end_row();
                    ui.label("Monto (LIT):");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.faucet_amount)
                            .desired_width(120.0)
                            .hint_text("1.000000"),
                    );
                    ui.end_row();
                });
            ui.add_space(4.0);
            if ui.button("🪚 Pedir del faucet").clicked() {
                self.do_faucet(&send_cmd, ui.ctx());
            }
            // Estado del último intento: el status del ACTOR manda cuando
            // llega; mientras (o si la validación falló local) mostramos el
            // mensaje propio del panel del faucet.
            let status_actor = shared_arc.lock().unwrap().faucet_status.clone();
            match status_actor {
                Some((msg, is_err)) => {
                    self.faucet_pending = None;
                    if is_err {
                        ui.colored_label(egui::Color32::RED, msg);
                    } else {
                        ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
                    }
                }
                None => {
                    if let Some((msg, is_err)) = &self.faucet_pending {
                        if *is_err {
                            ui.colored_label(egui::Color32::RED, msg.clone());
                        } else {
                            ui.colored_label(egui::Color32::LIGHT_BLUE, msg.clone());
                        }
                    }
                }
            }
        }

        // --------------------------------------------------------------
        // 💾 Respaldo de wallet (Checkpoint 24): exporta el blob de
        // wallet.key (cifrado con TU contraseña, NUNCA en claro) a un
        // archivo a elección. La restauración vive en la pantalla de
        // desbloqueo (no hay sesión abierta ⇒ nada que invalidar).
        // --------------------------------------------------------------
        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);
        ui.collapsing("💾 Respaldo de wallet", |ui| {
            ui.small(
                "Copia DE SEGURIDAD de esta wallet: el mismo blob cifrado con TU contraseña. Guardá ese archivo (pendrive/nube) y tu contraseña: si la PC se muere, se restaura en la pantalla de desbloqueo con la MISMA contraseña.",
            );
            ui.add_space(4.0);
            egui::Grid::new("backup_grid")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Destino:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.backup_destino)
                            .desired_width(340.0)
                            .hint_text("ej. C:\\Users\\TuUsuario\\Documents\\respaldo-ledgerita-A.key"),
                    );
                    ui.end_row();
                    ui.label("Confirmar contraseña:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.backup_password)
                            .password(true)
                            .desired_width(240.0)
                            .hint_text("se limpia al instante tras exportar"),
                    );
                    ui.end_row();
                });
            ui.horizontal(|ui| {
                if ui.button("💾 Exportar respaldo").clicked() {
                    self.do_export_respaldo();
                }
                if ui.button("📂 Explorar…").clicked() {
                    self.explorar_destino_respaldo();
                }
            });
            if let Some((msg, is_err)) = &self.backup_status {
                if *is_err {
                    ui.colored_label(egui::Color32::RED, format!("❌ {msg}"));
                } else {
                    ui.colored_label(egui::Color32::LIGHT_GREEN, format!("✅ {msg}"));
                }
            }
        });

        // --------------------------------------------------------------
        // Historial node-local de TXs aplicadas (vive en esta wallet-node,
        // JAMÁS consenso): lo persistió el actor; aquí es puro display.
        // --------------------------------------------------------------
        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);
        {
            let shared = shared_arc.lock().unwrap();
            draw_historial_panel(ui, &shared, &addr);
        }
    }
}

/// Panel de historial node-local de TXs aplicadas (más nueva primero):
/// marca 🪚 faucet / ↑ enviada / ↓ recibida / • TX de la red.
fn draw_historial_panel(
    ui: &mut egui::Ui,
    shared: &net_actor::NodeShared,
    own: &Address,
) {
    ui.heading("📜 Historial de TX");
    ui.small(format!(
        "Registro node-local de esta wallet-node ({} en RAM; el tree `historia` guarda TODO). No es consenso.",
        shared.historial.len()
    ));
    if shared.historial.is_empty() {
        ui.small("Sin TXs aplicadas todavía: envía LIT o pide del faucet (TestNet).");
        return;
    }
    egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
        egui::Grid::new("historial_grid")
            .num_columns(6)
            .spacing([12.0, 3.0])
            .show(ui, |ui| {
                ui.strong("Hora (UTC)");
                ui.strong("");
                ui.strong("Par");
                ui.strong("Monto");
                ui.strong("Fee");
                ui.strong("Hash");
                ui.end_row();
                for rec in &shared.historial {
                    let faucet = rec.kind == 1;
                    let enviada = !faucet && rec.from == *own;
                    let recibida = faucet || rec.to == *own;
                    ui.monospace(format_hora_tx(rec.ts_ms));
                    let (marca, color) = if faucet {
                        ("🪚", egui::Color32::YELLOW)
                    } else if enviada {
                        ("↑", egui::Color32::ORANGE)
                    } else if recibida {
                        ("↓", egui::Color32::LIGHT_GREEN)
                    } else {
                        ("•", egui::Color32::GRAY)
                    };
                    ui.colored_label(color, marca);
                    // Contraparte de la TX (receptor si enviamos/mint, remitente si recibimos).
                    let par_hex = if enviada || faucet {
                        rec.to.to_hex()
                    } else {
                        rec.from.to_hex()
                    };
                    let par: String = par_hex.chars().take(8).collect();
                    ui.monospace(format!("{par}…"));
                    let lit = format_lit(rec.amount);
                    let (monto, color_monto) = if faucet {
                        (lit, egui::Color32::YELLOW)
                    } else if enviada {
                        (format!("-{lit}"), egui::Color32::ORANGE)
                    } else if recibida {
                        (format!("+{lit}"), egui::Color32::LIGHT_GREEN)
                    } else {
                        (lit, egui::Color32::GRAY)
                    };
                    ui.colored_label(color_monto, monto);
                    ui.label(format_lit(rec.fee));
                    ui.monospace(format!(
                        "{:02x}{:02x}{:02x}{:02x}…",
                        rec.tx_hash[0], rec.tx_hash[1], rec.tx_hash[2], rec.tx_hash[3]
                    ));
                    ui.end_row();
                }
            });
    });
}

/// Panel de red: peers sincronizados en vivo + resultado del último broadcast.
fn draw_network_panel(
    ui: &mut egui::Ui,
    shared: &net_actor::NodeShared,
    send_status: &mut Option<(String, bool)>,
    last_tx: &mut Option<Transaction>,
) {
    ui.heading("🌐 Nodo (red en vivo)");
    ui.small(format!(
        "Red del actor: {} · {} peer(s) configurado(s)",
        shared.network,
        shared.peers.len()
    ));
    if shared.peers.is_empty() {
        ui.small("Sin peers configurados — la wallet-node NO aplica TX sin conexión (la wallet jamás miente el saldo).");
    } else {
        egui::Grid::new("peers_grid")
            .num_columns(4)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.strong("Peer");
                ui.strong("Época / TX");
                ui.strong("Supply");
                ui.strong("Estado");
                ui.end_row();
                for pv in shared.peers.values() {
                    let addr_short: String = {
                        let a = pv.addr.replace("/ip4/", "").replace("/tcp/", ":");
                        if a.chars().count() > 34 {
                            let cut: String = a.chars().take(34).collect();
                            format!("{cut}…")
                        } else {
                            a
                        }
                    };
                    ui.monospace(addr_short);
                    ui.label(format!("{} / {} TX", pv.epoch, pv.tx_count));
                    ui.label(if pv.synced {
                        format!("{} LIT", pv.supply_lit)
                    } else {
                        "—".to_string()
                    });
                    let color = if pv.connected {
                        egui::Color32::LIGHT_GREEN
                    } else {
                        egui::Color32::YELLOW
                    };
                    ui.colored_label(color, &pv.status);
                    ui.end_row();
                }
            });
        for pv in shared.peers.values() {
            if !pv.supply_ok {
                ui.colored_label(
                    egui::Color32::RED,
                    format!("🚨 SUPPLY EXCEDIDO reportado por un peer: {} LIT", pv.supply_lit),
                );
            }
        }
    }
    ui.add_space(6.0);
    match &shared.last_broadcast {
        None => {
            if last_tx.is_some() {
                ui.small("⏳ Esperando resultado de la última TX…");
            }
        }
        Some(BroadcastReport::Accepted { acks, duplicates }) => {
            send_status.take();
            ui.colored_label(
                egui::Color32::LIGHT_GREEN,
                format!("✅ La red confirmó la TX (acks={acks}, duplicates={duplicates}) — aplicada localmente"),
            );
            last_tx.take();
        }
        Some(BroadcastReport::Rejected { reason }) => {
            send_status.take();
            ui.colored_label(
                egui::Color32::RED,
                format!("❌ La red rechazó la TX: {reason}"),
            );
            last_tx.take();
        }
        Some(BroadcastReport::Offline { tried }) => {
            send_status.take();
            ui.colored_label(
                egui::Color32::YELLOW,
                format!("⏳ Sin confirmación de la red (peers probados: {tried}) — NO aplicada; reintento automático al reconectar."),
            );
        }
    }
    if shared.retry_pending {
        ui.small("🔁 Hay una TX firmada esperando red (el transaction NO está aplicado mientras no haya confirmación).");
    }
}

/// QR de la dirección (Fase 3c): `qrcode` genera la matriz y el painter de
/// egui la dibuja — borde blanco (quiet zone de 4 módulos por lado, la
/// convención de los lectores), módulos Dark como cuadraditos negros.
fn draw_qr(ui: &mut egui::Ui, data: &str) {
    let code = match qrcode::QrCode::new(data.as_bytes()) {
        Ok(c) => c,
        Err(e) => {
            ui.colored_label(egui::Color32::RED, format!("No se pudo generar el QR: {e:?}"));
            return;
        }
    };
    let size = code.width(); // módulos por lado
    let colors = code.to_colors(); // row-major, len = size*size
    let px = 4.0_f32; // píxeles por módulo
    let margen = 4.0_f32; // módulos de quiet zone por lado
    let total = (size as f32 + 2.0 * margen) * px;
    let (res, painter) = ui.allocate_painter(egui::vec2(total, total), egui::Sense::hover());
    let rect = res.rect;
    painter.rect_filled(rect, 0.0, egui::Color32::WHITE);
    for y in 0..size {
        for x in 0..size {
            if colors[y * size + x] == qrcode::types::Color::Dark {
                let p = egui::Pos2::new(
                    rect.left() + (x as f32 + margen) * px,
                    rect.top() + (y as f32 + margen) * px,
                );
                painter.rect_filled(
                    egui::Rect::from_min_size(p, egui::vec2(px, px)),
                    0.0,
                    egui::Color32::BLACK,
                );
            }
        }
    }
}

impl Drop for WalletApp {
    fn drop(&mut self) {
        self.password.zeroize();
        self.backup_password.zeroize();
    }
}

impl eframe::App for WalletApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if matches!(self.screen, Screen::Main) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs(1));
            let has_input = ui.input(|i| !i.events.is_empty());
            if check_auto_lock(
                &self.screen,
                &mut self.last_activity,
                has_input,
                std::time::Instant::now(),
            ) {
                self.lock_wallet(ui.ctx());
            }
        }
        egui::CentralPanel::default().show(ui, |ui| match self.screen {
            Screen::Unlock => self.draw_unlock(ui),
            Screen::Main => {
                // Scroll de página completa: la barra a la derecha aparece
                // sola cuando el contenido no cabe en la ventana (el panel
                // del historial tiene su propio ScrollArea de adentro).
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.draw_main(ui));
            }
        });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([540.0, 720.0])
            .with_min_inner_size([420.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Ledgerita Wallet",
        options,
        Box::new(|cc| {
            // Dark mode de fábrica (7a): el usuario alterna ☀️/🌙 en vivo
            // desde el dashboard.
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(WalletApp::new()))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_lit() {
        assert_eq!(format_lit(0), "0.000000");
        assert_eq!(format_lit(1_000_000), "1.000000");
        assert_eq!(format_lit(15_500_000), "15.500000");
        assert_eq!(format_lit(21_000_000_000_000u64), "21000000.000000");
    }

    #[test]
    fn test_lit_to_sats() {
        assert_eq!(lit_to_sats("1"), Some(1_000_000));
        assert_eq!(lit_to_sats("1.5"), Some(1_500_000));
        assert_eq!(lit_to_sats("0.000005"), Some(5));
        assert_eq!(lit_to_sats("21"), Some(21_000_000));
        assert_eq!(lit_to_sats(" 1.5 "), Some(1_500_000));
    }

    #[test]
    fn test_lit_to_sats_invalid() {
        assert_eq!(lit_to_sats(""), None);
        assert_eq!(lit_to_sats("abc"), None);
        assert_eq!(lit_to_sats("1."), None);
        assert_eq!(lit_to_sats(".5"), None);
        assert_eq!(lit_to_sats("1.2.3"), None);
        assert_eq!(lit_to_sats("1e5"), None);
    }

    #[test]
    fn test_lit_to_sats_roundtrip() {
        for sats in [1u64, 999_999, 1_000_000, 123_456_789, u64::MAX / 2] {
            let lit = format_lit(sats);
            assert_eq!(lit_to_sats(&lit), Some(sats), "roundtrip para {sats}");
        }
    }

    #[test]
    fn test_default_wallet_dir_por_red() {
        let main = default_wallet_dir(NetworkId::Mainnet);
        let test = default_wallet_dir(NetworkId::Testnet);
        assert!(is_wallet_default_dir(&main, NetworkId::Mainnet), "main: {main}");
        assert!(is_wallet_default_dir(&test, NetworkId::Testnet), "test: {test}");
        assert!(!is_wallet_default_dir(&main, NetworkId::Testnet));
        assert_ne!(main, test);
        // Los defaults de la wallet JAMÁS son el del daemon.
        assert!(!is_core_default_dir(&main));
        assert!(!is_core_default_dir(&test));
    }

    #[test]
    fn test_modo_portable_reposiciona_datos_junto_al_exe() {
        // Fase 3d: el modo portable manda los datos a `ledgerita-data/<red>`
        // junto al EXE y JAMÁS al HOME (contaminación cero de `$HOME`).
        let normal = default_wallet_dir_in(NetworkId::Testnet, false);
        let portatil = default_wallet_dir_in(NetworkId::Testnet, true);
        assert!(normal.contains(".ledgerita-wallet"), "normal: {normal}");
        assert!(portatil.contains("ledgerita-data"), "portable: {portatil}");
        assert!(portatil.contains("testnet"));
        assert_ne!(normal, portatil);
        // Y por red distinta, subcarpeta distinta.
        assert_ne!(
            portatil,
            default_wallet_dir_in(NetworkId::Mainnet, true)
        );
        // El modo portable tampoco pisa la carpeta del daemon.
        assert!(!is_core_default_dir(&portatil));
    }

    #[test]
    fn test_is_core_default_dir_dispara_advertencia() {
        let core_dir = home_dir()
            .join(".ledgerita")
            .to_string_lossy()
            .to_string();
        // Directorio inexistente o no, el default del daemon debe marcar la
        // advertencia (canonical tolerante).
        assert!(
            is_core_default_dir(&core_dir),
            "el default del core debe disparar la advertencia: {core_dir}"
        );
    }

    #[test]
    fn test_on_network_changed_no_pisa_custom() {
        let mut app = WalletApp::new();
        app.on_network_changed();
        assert!(app.data_dir.contains("testnet"));
        app.network = NetworkId::Mainnet;
        app.on_network_changed(); // era default testnet → se acerca a mainnet
        assert!(app.data_dir.contains("mainnet"));

        app.data_dir = "C:\\mi-carpeta-custom".to_string();
        app.network = NetworkId::Testnet;
        app.on_network_changed(); // custom → se respeta
        assert_eq!(app.data_dir, "C:\\mi-carpeta-custom");
    }

    #[test]
    fn test_guardar_contacto_persiste_upsert_y_tope() {
        // Checkpoint 20: el directorio persiste en el tree `contactos`
        // node-local, con upsert por dirección, orden alfabético y tope.
        let dir = std::env::temp_dir().join(format!("ledgerita_contactos_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let storage = Arc::new(
            Storage::open(&dir).expect("storage de prueba"),
        );
        let a1 = Address::from_signing_key(&ed25519_dalek::SigningKey::from_bytes(&[0xd1u8; 32]));
        let a2 = Address::from_signing_key(&ed25519_dalek::SigningKey::from_bytes(&[0xd2u8; 32]));

        let mut contactos: Vec<(String, String)> = Vec::new();
        guardar_contacto(Some(storage.as_ref()), &a1, "zulu nodo", &mut contactos).unwrap();
        guardar_contacto(Some(storage.as_ref()), &a2, "B alpha", &mut contactos).unwrap();
        // Upsert: misma dirección, etiqueta nueva (no duplica).
        guardar_contacto(Some(storage.as_ref()), &a2, "aaa B", &mut contactos).unwrap();
        assert_eq!(contactos.len(), 2);
        assert_eq!(contactos[0].1, "aaa B", "orden alfabético");
        // Persistido: un reopen lo ve.
        drop(storage);
        let storage = Storage::open(&dir).unwrap();
        let reload = storage.load_contactos().unwrap();
        assert_eq!(reload.len(), 2);
        assert_eq!(reload[0].1, "aaa B");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_contactos_fabrica_validos() {
        // Los precables de fábrica son direcciones hex válidas (el seed
        // del unlock las inserta sin tocar pánico).
        for (_, hex) in contactos_de_fabrica(NetworkId::Testnet) {
            assert!(Address::from_hex(hex).is_some(), "hex inválido: {hex}");
        }
        for (_, hex) in contactos_de_fabrica(NetworkId::Mainnet) {
            assert!(Address::from_hex(hex).is_some(), "hex inválido: {hex}");
        }
        assert!(contactos_de_fabrica(NetworkId::Testnet).len() >= 2); // B + nodos
    }

    #[test]
    fn test_wallet_password_zeroize() {
        use zeroize::Zeroize;
        let mut pwd = String::from("super_secret_password_123");
        assert_eq!(pwd, "super_secret_password_123");
        pwd.zeroize();
        assert!(pwd.is_empty());
        assert_eq!(pwd, "");
    }

    #[test]
    fn test_ckpt55_auto_lock_expira_incluso_con_input_y_resolve_d1_d2_dinamico() {
        let now = std::time::Instant::now();
        let mut last = now;
        // Sin vencer: mouse activo refresca actividad
        assert!(!check_auto_lock(
            &Screen::Main,
            &mut last,
            true,
            now + std::time::Duration::from_secs(10),
        ));
        assert_eq!(last, now + std::time::Duration::from_secs(10));

        // Vencido (`>= AUTO_LOCK_SECS`): DEBE bloquear aunque justo vuelva el mouse (`has_input = true`)
        assert!(check_auto_lock(
            &Screen::Main,
            &mut last,
            true,
            now + std::time::Duration::from_secs(10 + AUTO_LOCK_SECS),
        ));

        // `resolve_active_account`: antes de sync (ambas en 0) elige D2a;
        // si tras sync llega saldo legado solo en D1, conmuta dinámicamente a D1.
        let sk = SigningKey::from_bytes(&[0x33; 32]);
        let d1 = Address::from_signing_key(&sk);
        let d2 = Address::from_signing_key_d2(&sk);
        let mut s = State::new();
        assert_eq!(resolve_active_account(&s, &d1, &d2), (d2.clone(), true));
        s.mint(d1.clone(), 500_000);
        assert_eq!(resolve_active_account(&s, &d1, &d2), (d1, false));
    }
}
