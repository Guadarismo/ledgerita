use std::path::{Path, PathBuf};
use std::sync::Arc;

use ed25519_dalek::Signer;
use eframe::egui;
use ledgerita_core::crypto::{self, Address};
use ledgerita_core::faucet::{DEFAULT_FAUCET_CAP, HARD_CAP_SAT};
use ledgerita_core::genesis;
use ledgerita_core::network::NetworkId;
use ledgerita_core::state::State;
use ledgerita_core::storage::Storage;
use ledgerita_core::transaction::Transaction;
use tokio::sync::Mutex as AsyncMutex;

// Actor de red de la wallet-node (Fase 2.1a — Alternativa A incremental):
// la wallet levanta su PROPIO mini-nodo con data-dir propio, esquivando el
// lock exclusivo de sled (que solo conflicta entre PROCESOS sobre la misma
// carpeta).
use ledgerita_wallet::net_actor;
use ledgerita_wallet::net_actor::{BroadcastReport, NodeCmd, NodeHandles};

const LIT_DECIMALS: u64 = 1_000_000;

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

/// Data-dir de la WALLET-NODE por red: `~/.ledgerita-wallet/<red>/`.
///
/// Decisión 2026-11-05 (Alternativa A): la wallet usa SU PROPIA carpeta para
/// chocar jamás con el lock exclusivo de sled del daemon (que usa
/// `~/.ledgerita`). Cambiar el default crea wallet NUEVA para quien tenía
/// la wallet en `~/.ledgerita` (ver advertencia en la UI — es legítimo
/// apuntar ahí siempre que el daemon NO corra sobre esa carpeta a la vez).
fn default_wallet_dir(network: NetworkId) -> String {
    home_dir()
        .join(".ledgerita-wallet")
        .join(network.tag())
        .to_string_lossy()
        .to_string()
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
struct NodeRuntime {
    address: Address,
    state: Arc<AsyncMutex<State>>,
    handles: NodeHandles,
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

    /// Faucet TestNet: receptor en hex (vacío = esta wallet) y monto en LIT.
    faucet_recipient: String,
    faucet_amount: String,

    copied: Option<String>,
}

impl WalletApp {
    fn new() -> Self {
        WalletApp {
            data_dir: default_wallet_dir(NetworkId::Testnet),
            network: NetworkId::Testnet,
            seed_addr: net_actor::default_seed(NetworkId::Testnet).to_string(),
            password: String::new(),
            unlock_error: None,
            screen: Screen::Unlock,
            node: None,
            state_cache: None,
            last_tx: None,
            send_receiver: String::new(),
            send_amount: String::new(),
            send_fee: String::new(),
            send_status: None,
            faucet_recipient: String::new(),
            faucet_amount: "1.000000".to_string(),
            copied: None,
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
        {
            self.seed_addr = net_actor::default_seed(self.network).to_string();
        }
    }

    fn unlock(&mut self, ctx: &egui::Context) {
        let result = (|| -> Result<NodeRuntime, String> {
            let data_dir = PathBuf::from(&self.data_dir);
            let (sk, kp) = crypto::load_or_create_wallet(&data_dir, &self.password)
                .map_err(|e| format!("Wallet: {e}"))?;
            let address = Address::from_signing_key(&sk);
            std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
            let storage =
                Arc::new(Storage::open(&data_dir.join("state")).map_err(|e| e.to_string())?);
            let mut state = State::load(&storage).map_err(|e| e.to_string())?;
            if genesis::apply_genesis(&mut state, self.network) {
                state.flush(&storage).map_err(|e| e.to_string())?;
            }

            let peers = parse_peers(&self.seed_addr)?;

            let state = Arc::new(AsyncMutex::new(state));
            let handles = net_actor::spawn_wallet_node(
                self.network,
                kp,
                state.clone(),
                storage,
                peers,
                0, // puerto efímero: la wallet-node no necesita puerto fijo
            )?;
            Ok(NodeRuntime {
                address,
                state,
                handles,
            })
        })();

        match result {
            Ok(node) => {
                self.node = Some(node);
                self.state_cache = None;
                self.unlock_error = None;
                self.screen = Screen::Main;
            }
            Err(e) => self.unlock_error = Some(e),
        }
        ctx.request_repaint();
    }

    /// **Broadcast-first**: valida, firma, envía al actor y NUNCA aplica
    /// directo — el estado local lo toca únicamente la red a través del
    /// actor (solo si algún peer responde OK/DUPLICATE).
    fn do_send(&mut self, ctx: &egui::Context) {
        match self.build_signed_tx() {
            Ok(tx) => {
                let sent = self
                    .node
                    .as_ref()
                    .map(|n| n.handles.send_cmd.send(NodeCmd::BroadcastTx(tx.clone())).is_ok())
                    .unwrap_or(false);
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

    /// Faucet TestNet node-local: valida monto/receptor y le pasa la orden
    /// al actor (`NodeCmd::Faucet`) — el mint corre IN-PROCESS en la
    /// wallet-node (nunca directo desde la GUI, el estado lo toca solo el actor).
    /// Es de MEJOR ESFUERZO: si una TX concurrente avanza la red mientras
    /// mintea, el mint se pierde (nuestro snapshot pierde frescura).
    fn do_faucet(&mut self, send_cmd: &tokio::sync::mpsc::UnboundedSender<NodeCmd>, ctx: &egui::Context) {
        match (|| -> Result<NodeCmd, String> {
            if self.network != NetworkId::Testnet {
                return Err("El faucet solo existe en TestNet".to_string());
            }
            let node = self.node.as_ref().ok_or("Wallet no desbloqueada")?;
            let recipient = if self.faucet_recipient.trim().is_empty() {
                node.address.clone()
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
                if send_cmd.send(cmd).is_ok() {
                    self.send_status = Some((
                        "🪚 Orden de mint en camino al faucet node-local…".to_string(),
                        false,
                    ));
                } else {
                    self.send_status =
                        Some(("El actor de red no está corriendo".to_string(), true));
                }
            }
            Err(e) => self.send_status = Some((e, true)),
        }
        ctx.request_repaint();
    }

    fn build_signed_tx(&self) -> Result<Transaction, String> {
        let node = self.node.as_ref().ok_or("Wallet no desbloqueada")?;
        let addr = node.address.clone();
        let receiver = Address::from_hex(self.send_receiver.trim())
            .ok_or_else(|| "Dirección de receptor inválida (64 hex)".to_string())?;
        if receiver == addr {
            return Err("No puedes enviarte LIT a ti mismo".to_string());
        }
        let amount =
            lit_to_sats(&self.send_amount).ok_or_else(|| "Monto inválido".to_string())?;
        if amount == 0 {
            return Err("El monto debe ser mayor a 0".to_string());
        }
        let fee = lit_to_sats(&self.send_fee).ok_or_else(|| "Fee inválido".to_string())?;
        if amount.checked_add(fee).is_none() {
            return Err("Monto + fee desbordan el rango".to_string());
        }
        // Nonce desde la vista local del estado (consenso replicado; el actor
        // lo mantiene al día con la red).
        let nonce = {
            let s = node.state.blocking_lock();
            s.nonces.get(&addr).copied().unwrap_or(0) + 1
        };
        let mut tx = Transaction::new(addr, receiver, amount, fee, nonce);
        let msg = tx.to_bytes();
        let sk = crypto::load_or_create_wallet(&PathBuf::from(&self.data_dir), &self.password)
            .map_err(|e| format!("Wallet: {e}"))?
            .0;
        tx.sign(sk.sign(&msg));
        Ok(tx)
    }

    fn copy_address(&mut self, ctx: &egui::Context) {
        if let Some(node) = &self.node {
            ctx.copy_text(node.address.to_hex());
            self.copied = Some("✓ Dirección copiada".to_string());
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
        ui.add_space(8.0);
        ui.separator();
        ui.small("⚠️  Modo demostración: esto NO es dinero real. La wallet se guarda encriptada en el directorio indicado.");
    }

    fn draw_main(&mut self, ui: &mut egui::Ui) {
        // Extraer los Arcs ANTES de tocar &mut self: los clones son baratos y
        // evitan pelear con el borrow checker de egui (node vive mientras sí).
        let (addr, state_arc, shared_arc, send_cmd) = match &self.node {
            Some(n) => (
                n.address.clone(),
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
        let balance = state.balances.get(&addr).copied().unwrap_or(0);
        let supply = state.total_supply();
        let remaining = HARD_CAP_SAT.saturating_sub(supply);

        ui.heading("🏦 Dashboard");
        ui.add_space(8.0);
        egui::Grid::new("dash_grid")
            .num_columns(2)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                ui.label("Dirección:");
                ui.monospace(addr.to_hex());
                ui.end_row();
                ui.label("Balance:");
                ui.monospace(format!("{} LIT", format_lit(balance)));
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

        if ui.button("📋 Copiar dirección").clicked() {
            self.copy_address(ui.ctx());
        }
        if let Some(c) = &self.copied {
            ui.colored_label(egui::Color32::LIGHT_GREEN, c);
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
        // Faucet TestNet (solo TestNet): mint node-local in-process.
        // --------------------------------------------------------------
        if self.network == NetworkId::Testnet {
            ui.add_space(16.0);
            ui.separator();
            ui.add_space(8.0);
            ui.heading("🪚 Faucet TestNet");
            ui.small(format!(
                "LIT de prueba minteado por tu PROPIA wallet-node (in-process) — la red lo copia con su pull (~30s). Tope de por vida por dirección: {} LIT.",
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
            // Estado del último intento (lo escribe el actor).
            if let Some((msg, is_err)) = shared_arc.lock().unwrap().faucet_status.clone() {
                if is_err {
                    ui.colored_label(egui::Color32::RED, msg);
                } else {
                    ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
                }
            }
        }

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

impl eframe::App for WalletApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| match self.screen {
            Screen::Unlock => self.draw_unlock(ui),
            Screen::Main => self.draw_main(ui),
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
        Box::new(|_cc| Ok(Box::new(WalletApp::new()))),
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
}
