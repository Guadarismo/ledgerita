use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use libp2p::request_response::{self, Message, ProtocolSupport};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::tcp;
use libp2p::yamux;
use libp2p::{identity, noise, Multiaddr, PeerId};

use ledgerita_core::faucet::HARD_CAP_SAT;
use ledgerita_core::network::{build_msg, split_msg, NetworkId, RawCodec};
use ledgerita_core::state::State;

const MSG_SYNC: u8 = 0x02;
const REFRESH_DEFAULT_SECS: u64 = 5;

#[derive(NetworkBehaviour)]
struct MonitorBehaviour {
    rr: request_response::Behaviour<RawCodec>,
}

fn format_amount(amount: u64) -> String {
    let integer = amount / 1_000_000;
    let fraction = amount % 1_000_000;
    format!("{}.{:06}", integer, fraction)
}

fn root_prefix(state: &State) -> String {
    let root = state.generate_state_root();
    root[..8].iter().map(|b| format!("{:02x}", b)).collect()
}

fn check_supply_limit(state: &State) -> bool {
    state.total_supply() <= HARD_CAP_SAT
}

fn extract_peer_id(addr: &Multiaddr) -> Option<PeerId> {
    let s = addr.to_string();
    let needle = "/p2p/";
    if let Some(pos) = s.rfind(needle) {
        s[pos + needle.len()..].parse().ok()
    } else {
        None
    }
}

#[derive(Clone, Default)]
struct PeerView {
    addr: String,
    connected: bool,
    supply_ok: bool,
    supply_lit: String,
    root: String,
    fee_pool_lit: String,
    goteo_lit: String,
    tesoro_lit: String,
    tx_count: u64,
    epoch: u64,
    accounts: usize,
    status: String,
}

#[derive(Default)]
struct SharedState {
    peers: HashMap<PeerId, PeerView>,
    refresh_secs: u64,
    network: String,
    started: Option<Instant>,
}

struct NetworkActor {
    shared: Arc<Mutex<SharedState>>,
}

impl NetworkActor {
    async fn run(
        self,
        connect_peers: Vec<Multiaddr>,
        network: NetworkId,
        refresh_secs: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut targets: HashMap<PeerId, Multiaddr> = HashMap::new();
        for addr in &connect_peers {
            let pid = extract_peer_id(addr).ok_or_else(|| {
                format!("Error: cada --connect debe incluir /p2p/<PeerId> al final: {}", addr)
            })?;
            targets.insert(pid, addr.clone());
        }

        let local_key = identity::Keypair::generate_ed25519();
        let mut swarm = libp2p::SwarmBuilder::with_existing_identity(local_key)
            .with_tokio()
            .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
            .with_behaviour(|_| {
                let protocols = vec![(network.protocol_id(), ProtocolSupport::Full)];
                MonitorBehaviour {
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

        swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

        {
            let mut shared = self.shared.lock().unwrap();
            shared.refresh_secs = refresh_secs;
            shared.network = network.tag().to_string();
            shared.started = Some(Instant::now());
            for (pid, addr) in &targets {
                shared.peers.insert(
                    pid.clone(),
                    PeerView {
                        addr: addr.to_string(),
                        connected: false,
                        supply_ok: true,
                        supply_lit: "—".to_string(),
                        root: "—".to_string(),
                        fee_pool_lit: "—".to_string(),
                        goteo_lit: "—".to_string(),
                        tesoro_lit: "—".to_string(),
                        tx_count: 0,
                        epoch: 0,
                        accounts: 0,
                        status: "Conectando…".to_string(),
                    },
                );
            }
        }

        for addr in targets.values() {
            if let Err(e) = swarm.dial(addr.clone()) {
                let mut shared = self.shared.lock().unwrap();
                if let Some(pid) = extract_peer_id(addr) {
                    if let Some(pv) = shared.peers.get_mut(&pid) {
                        pv.status = format!("Dial error: {:?}", e);
                    }
                }
            }
        }

        let mut last_sync = Instant::now();

        loop {
            if last_sync.elapsed() >= Duration::from_secs(refresh_secs) {
                let connected: Vec<PeerId> = {
                    let shared = self.shared.lock().unwrap();
                    shared
                        .peers
                        .iter()
                        .filter(|(_, pv)| pv.connected)
                        .map(|(pid, _)| pid.clone())
                        .collect()
                };
                for pid in connected {
                    let msg = build_msg(network, MSG_SYNC, &[0u8; 32]);
                    swarm.behaviour_mut().rr.send_request(&pid, msg);
                }
                last_sync = Instant::now();
            }

            let event = match tokio::time::timeout(Duration::from_millis(500), swarm.select_next_some()).await {
                Ok(e) => e,
                Err(_) => continue,
            };

            match event {
                SwarmEvent::NewListenAddr { .. } => {}
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    let mut shared = self.shared.lock().unwrap();
                    if let Some(pv) = shared.peers.get_mut(&peer_id) {
                        pv.connected = true;
                        pv.status = "Sincronizando…".to_string();
                    }
                    drop(shared);
                    let msg = build_msg(network, MSG_SYNC, &[0u8; 32]);
                    swarm.behaviour_mut().rr.send_request(&peer_id, msg);
                }
                SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                    if let Some(pid) = peer_id {
                        let mut shared = self.shared.lock().unwrap();
                        if let Some(pv) = shared.peers.get_mut(&pid) {
                            pv.connected = false;
                            pv.status = format!("Reintentando… ({:?})", error);
                        }
                    }
                }
                SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                    let mut shared = self.shared.lock().unwrap();
                    if let Some(pv) = shared.peers.get_mut(&peer_id) {
                        pv.connected = false;
                        pv.status = format!("Desconectado: {:?}", cause);
                    }
                }
                SwarmEvent::Behaviour(MonitorBehaviourEvent::Rr(
                    request_response::Event::Message {
                        message: Message::Request { channel, .. },
                        ..
                    },
                )) => {
                    let _ = swarm.behaviour_mut().rr.send_response(channel, b"MONITOR".to_vec());
                }
                SwarmEvent::Behaviour(MonitorBehaviourEvent::Rr(
                    request_response::Event::Message {
                        peer,
                        message: Message::Response { response, .. },
                        ..
                    },
                )) => {
                    let mut shared = self.shared.lock().unwrap();
                    let pv = match shared.peers.get_mut(&peer) {
                        Some(pv) => pv,
                        None => continue,
                    };
                    let (msg_type, payload) = match split_msg(network, &response) {
                        Some(x) => x,
                        None => {
                            pv.status = "Respuesta de otra red".to_string();
                            continue;
                        }
                    };
                    if msg_type != MSG_SYNC {
                        continue;
                    }
                    match State::decode_snapshot(payload, network) {
                        Ok(state) => {
                            let supply = state.total_supply();
                            pv.supply_ok = check_supply_limit(&state);
                            pv.supply_lit = format_amount(supply);
                            pv.root = root_prefix(&state);
                            pv.fee_pool_lit = format_amount(state.fee_pool);
                            // GOTE (Checkpoint 21): emisión histórica del
                            // tesoro y disponible remanente (D7 ever-minted).
                            pv.goteo_lit = format_amount(state.emitido_total);
                            pv.tesoro_lit = format_amount(state.tesoro_disponible());
                            pv.tx_count = state.tx_count;
                            pv.epoch = state.epoch;
                            pv.accounts = state.balances.len();
                            pv.connected = true;
                            pv.status = if pv.supply_ok {
                                "OK ✓".to_string()
                            } else {
                                "SUPPLY EXCEDIDO 🚨".to_string()
                            };
                        }
                        Err(e) => {
                            pv.status = format!("Snapshot inválido: {:?}", e);
                        }
                    }
                }
                SwarmEvent::Behaviour(MonitorBehaviourEvent::Rr(
                    request_response::Event::OutboundFailure { peer, error, .. },
                )) => {
                    let mut shared = self.shared.lock().unwrap();
                    if let Some(pv) = shared.peers.get_mut(&peer) {
                        pv.status = format!("{:?}", error);
                    }
                }
                _ => {}
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    let mut connect_peers: Vec<Multiaddr> = Vec::new();
    let mut refresh_secs: u64 = REFRESH_DEFAULT_SECS;
    let mut network = NetworkId::Mainnet;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--connect" | "-c" => {
                i += 1;
                if i < args.len() {
                    connect_peers.push(args[i].parse()?);
                } else {
                    eprintln!("Error: --connect requiere una dirección multiaddr");
                    std::process::exit(1);
                }
            }
            "--refresh" | "-r" => {
                i += 1;
                if i < args.len() {
                    refresh_secs = args[i].parse().unwrap_or_else(|_| {
                        eprintln!("Error: --refresh debe ser un número entero (segundos)");
                        std::process::exit(1);
                    });
                }
            }
            "--network" => {
                i += 1;
                if i < args.len() {
                    network = NetworkId::from_str(&args[i]).unwrap_or_else(|| {
                        eprintln!("Error: red desconocida: {}. Usa mainnet o testnet.", args[i]);
                        std::process::exit(1);
                    });
                }
            }
            "--help" | "-h" => {
                println!("Uso: ledgerita_monitor_tui --connect <multiaddr>... [--refresh <secs>] [--network <mainnet|testnet>]");
                println!("  --connect, -c  Dirección multiaddr del peer (repetible)");
                println!("  --refresh, -r  Segundos entre sondeos (default: 5)");
                println!("  --network, -n  Red del peer a monitorear (default: mainnet)");
                return Ok(());
            }
            other => {
                eprintln!("Argumento desconocido: {}", other);
                eprintln!("Uso: ledgerita_monitor_tui --connect <multiaddr>... [--refresh <secs>] [--network <mainnet|testnet>]");
                std::process::exit(1);
            }
        }
        i += 1;
    }

    if connect_peers.is_empty() {
        eprintln!("Error: se requiere al menos un --connect");
        std::process::exit(1);
    }

    let shared: Arc<Mutex<SharedState>> = Arc::new(Mutex::new(SharedState::default()));
    let actor = NetworkActor { shared: shared.clone() };

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build tokio runtime");
        let net = network;
        rt.block_on(actor.run(connect_peers, net, refresh_secs))
            .expect("Network actor failed");
    });

    let mut terminal = ratatui::init();
    let res = run_ui(&mut terminal, shared);
    ratatui::restore();
    res
}

fn run_ui(
    terminal: &mut ratatui::DefaultTerminal,
    shared: Arc<Mutex<SharedState>>,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        terminal.draw(|frame| {
            use ratatui::layout::{Constraint, Layout};
            use ratatui::style::{Color, Modifier, Style};
            use ratatui::symbols::border;
            use ratatui::text::{Line, Span};
            use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};

            let area = frame.area();
            let chunks = Layout::vertical([
                Constraint::Length(4),
                Constraint::Length(2),
                Constraint::Min(4),
                Constraint::Length(3),
            ])
            .split(area);

            let snapshot = shared.lock().unwrap();
            let title = format!(
                "💎 Ledgerita Monitor TUI — red {} — refresh {}s (q: salir)",
                snapshot.network, snapshot.refresh_secs
            );
            let block = Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_set(border::ROUNDED);
            frame.render_widget(block, chunks[0]);

            let started = snapshot.started.map(|s| s.elapsed().as_secs()).unwrap_or(0);
            let summary = format!(
                "Peers: {}  ·  Uptime: {}s  ·  Tecla 'q' para salir",
                snapshot.peers.len(),
                started
            );
            frame.render_widget(Paragraph::new(summary), chunks[1]);

            let widths = [
                Constraint::Length(20),
                Constraint::Length(30),
                Constraint::Length(14),
                Constraint::Length(22),
                Constraint::Length(14),
                Constraint::Length(14),
                Constraint::Length(14),
                Constraint::Length(8),
                Constraint::Length(8),
                Constraint::Length(8),
                Constraint::Length(24),
            ];
            let header = Row::new(vec![
                "Peer ID", "Dirección", "Supply (LIT)", "State Root", "Fee Pool (LIT)",
                "Goteo (LIT)", "Tesoro (LIT)", "TX", "Época", "Cuentas", "Estado",
            ])
            .style(Style::default().add_modifier(Modifier::BOLD));

            let mut rows: Vec<Row> = Vec::new();
            for (pid, pv) in &snapshot.peers {
                let status_style = if !pv.supply_ok {
                    Style::default().fg(Color::Red)
                } else if pv.connected {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Yellow)
                };
                let supply = if pv.supply_ok {
                    format!("{} / {}", pv.supply_lit, format_amount(HARD_CAP_SAT))
                } else {
                    format!("🚨 {}", pv.supply_lit)
                };
                rows.push(Row::new(vec![
                    Cell::from(pid.to_base58()[..18].to_string()),
                    Cell::from(pv.addr.clone()),
                    Cell::from(supply),
                    Cell::from(pv.root.clone()),
                    Cell::from(pv.fee_pool_lit.clone()),
                    Cell::from(pv.goteo_lit.clone()),
                    Cell::from(pv.tesoro_lit.clone()),
                    Cell::from(pv.tx_count.to_string()),
                    Cell::from(pv.epoch.to_string()),
                    Cell::from(pv.accounts.to_string()),
                    Cell::from(pv.status.clone()).style(status_style),
                ]));
            }
            let table = Table::new(rows, widths).header(header).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Estado de nodos")
                    .border_set(border::ROUNDED),
            );
            frame.render_widget(table, chunks[2]);

            let footer = Line::from(vec![
                Span::raw("LIT = 1,000,000 unidades · Hard cap 21M LIT · "),
                Span::styled("Estado de red de demostración (no es dinero real)", Style::default().fg(Color::DarkGray)),
            ]);
            frame.render_widget(Paragraph::new(footer), chunks[3]);
        })?;

        if crossterm::event::poll(Duration::from_millis(250))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                if key.kind == crossterm::event::KeyEventKind::Press
                    && (key.code == crossterm::event::KeyCode::Char('q')
                        || key.code == crossterm::event::KeyCode::Esc)
                {
                    break;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ledgerita_core::crypto::Address;
    use ed25519_dalek::SigningKey;
    use std::str::FromStr;

    #[test]
    fn test_format_amount() {
        assert_eq!(format_amount(0), "0.000000");
        assert_eq!(format_amount(1_000_000), "1.000000");
        assert_eq!(format_amount(21_000_000_000_000u64), "21000000.000000");
        assert_eq!(format_amount(15_500_000), "15.500000");
    }

    #[test]
    fn test_root_prefix() {
        let mut state = State::new();
        let addr = Address::from_signing_key(&SigningKey::from_bytes(&[42u8; 32]));
        state.mint(addr, 1000);
        let prefix = root_prefix(&state);
        assert_eq!(prefix.len(), 16, "root_prefix debe ser 16 chars hex (8 bytes)");
        assert!(prefix.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_extract_peer_id() {
        let addr = Multiaddr::from_str("/ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1").unwrap();
        let pid = extract_peer_id(&addr);
        assert!(pid.is_some());
        assert_eq!(pid.unwrap().to_base58(), "12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1");
    }

    #[test]
    fn test_extract_peer_id_no_p2p() {
        let addr = Multiaddr::from_str("/ip4/192.168.68.61/tcp/36565").unwrap();
        assert!(extract_peer_id(&addr).is_none());
    }

    #[test]
    fn test_check_supply_limit() {
        let mut state = State::new();
        let addr = Address::from_signing_key(&SigningKey::from_bytes(&[43u8; 32]));
        state.mint(addr, HARD_CAP_SAT);
        assert!(check_supply_limit(&state));
        let mut state2 = State::new();
        let addr2 = Address::from_signing_key(&SigningKey::from_bytes(&[44u8; 32]));
        state2.mint(addr2, HARD_CAP_SAT + 1);
        assert!(!check_supply_limit(&state2));
    }
}