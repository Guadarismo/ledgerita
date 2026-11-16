use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use futures::StreamExt;
use libp2p::request_response::{self, Message, ProtocolSupport};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::tcp;
use libp2p::yamux;
use libp2p::{Multiaddr, PeerId, identity, noise};

use ledgerita_core::faucet::HARD_CAP_SAT;
use ledgerita_core::network::{NetworkId, RawCodec, build_msg, resolve_multiaddr_dns, split_msg};
use ledgerita_core::quorum::{ValidatorSet, default_validator_set};
use ledgerita_core::state::State;
use ledgerita_core::storage::Storage;

const MSG_SYNC: u8 = 0x02;

#[derive(NetworkBehaviour)]
struct MonitorBehaviour {
    rr: request_response::Behaviour<RawCodec>,
}

fn format_amount(amount: u64) -> String {
    let integer = amount / 1_000_000;
    let fraction = amount % 1_000_000;
    format!("{}.{:06}", integer, fraction)
}

fn short_peer_id(b58: &str) -> &str {
    &b58[..b58.len().min(18)]
}

fn escape_json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn root_prefix(state: &State) -> String {
    let root = state.generate_state_root();
    root[..8].iter().map(|b| format!("{:02x}", b)).collect()
}

fn fmt_addr(addr: &Multiaddr) -> String {
    let s = addr.to_string();
    if s.chars().count() > 65 {
        let cut: String = s.chars().take(62).collect();
        format!("{}...", cut)
    } else {
        s
    }
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

fn check_supply_limit(state: &State) -> bool {
    state.total_supply() <= HARD_CAP_SAT
}

fn cross_validate_roots(roots: &[[u8; 32]]) -> bool {
    if roots.len() < 2 {
        return true;
    }
    let first = &roots[0];
    roots.iter().all(|r| r == first)
}

struct PeerState {
    addr: Multiaddr,
    state: Option<State>,
    error: Option<String>,
    gate_error: Option<String>,
}

struct MonitorSummary {
    healthy: u32,
    unreachable: u32,
    anomalies: u32,
    roots_match: bool,
}

impl MonitorSummary {
    fn status_str(&self) -> &'static str {
        if self.healthy == 0 {
            "unhealthy"
        } else if self.anomalies == 0 {
            "healthy"
        } else {
            "anomalies_detected"
        }
    }
}

fn evaluate_monitor_peers(
    peers: &mut HashMap<PeerId, PeerState>,
    baseline: &mut Option<State>,
    network: NetworkId,
    vset: &ValidatorSet,
) -> MonitorSummary {
    let mut all_roots: Vec<[u8; 32]> = Vec::new();
    let mut healthy = 0u32;
    let mut unreachable = 0u32;
    let mut anomalies = 0u32;

    let mut ordered_keys: Vec<PeerId> = peers.keys().copied().collect();
    ordered_keys.sort();
    for peer_id in ordered_keys {
        if let Some(ps) = peers.get_mut(&peer_id) {
            if let Some(ref state) = ps.state {
                if !check_supply_limit(state) {
                    anomalies += 1;
                    ps.gate_error = Some("SupplyExcedeHardCap".to_string());
                } else if let Err(gate) =
                    State::validate_for_monitor(baseline, network, state, vset)
                {
                    anomalies += 1;
                    ps.gate_error = Some(format!("{:?}", gate));
                } else {
                    all_roots.push(state.generate_state_root());
                    healthy += 1;
                }
            } else {
                unreachable += 1;
                anomalies += 1;
            }
        }
    }

    let roots_match = cross_validate_roots(&all_roots);
    if healthy > 1 && !roots_match {
        anomalies += 1;
    }

    MonitorSummary {
        healthy,
        unreachable,
        anomalies,
        roots_match,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    let mut connect_peers: Vec<Multiaddr> = Vec::new();
    let mut timeout_secs: u64 = 30;
    let mut json_output = false;
    let mut network = NetworkId::Mainnet;
    let mut baseline_dir: Option<PathBuf> = None;

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
            "--timeout" | "-t" => {
                i += 1;
                if i < args.len() {
                    timeout_secs = args[i].parse().unwrap_or_else(|_| {
                        eprintln!("Error: --timeout debe ser un número entero");
                        std::process::exit(1);
                    });
                }
            }
            "--json" | "-j" => {
                json_output = true;
            }
            "--network" | "-n" => {
                i += 1;
                if i < args.len() {
                    network = NetworkId::from_str(&args[i]).unwrap_or_else(|| {
                        eprintln!(
                            "Error: red desconocida: {}. Usa mainnet o testnet.",
                            args[i]
                        );
                        std::process::exit(1);
                    });
                } else {
                    eprintln!("Error: --network requiere un valor: mainnet | testnet");
                    std::process::exit(1);
                }
            }
            "--baseline-state" => {
                i += 1;
                if i < args.len() {
                    baseline_dir = Some(PathBuf::from(&args[i]));
                } else {
                    eprintln!("Error: --baseline-state requiere una ruta data-dir");
                    std::process::exit(1);
                }
            }
            "--help" | "-h" => {
                println!(
                    "Uso: remote_monitor --connect <multiaddr>... [--timeout <secs>] [--json] [--network <mainnet|testnet>] [--baseline-state <data-dir>]"
                );
                println!("  --connect, -c       Dirección multiaddr del peer (repetible)");
                println!("  --timeout, -t       Timeout total en segundos (default: 30)");
                println!("  --json, -j          Salida en formato JSON");
                println!("  --network, -n       Red del peer a monitorear (default: mainnet)");
                println!("  --baseline-state    Directorio data-dir local de referencia (opcional)");
                return Ok(());
            }
            other => {
                eprintln!("Argumento desconocido: {}", other);
                eprintln!(
                    "Uso: remote_monitor --connect <multiaddr>... [--timeout <secs>] [--json] [--network <mainnet|testnet>] [--baseline-state <data-dir>]"
                );
                std::process::exit(1);
            }
        }
        i += 1;
    }

    if connect_peers.is_empty() {
        eprintln!("Error: se requiere al menos un --connect");
        std::process::exit(1);
    }

    let mut baseline: Option<State> = if let Some(ref dir) = baseline_dir {
        let storage = Storage::open(&dir.join("state"))?;
        Some(State::load(&storage)?)
    } else {
        None
    };

    let mut peers: HashMap<PeerId, PeerState> = HashMap::new();
    for addr in &connect_peers {
        let peer_id = match extract_peer_id(addr) {
            Some(pid) => pid,
            None => {
                eprintln!("Error: cada --connect debe incluir /p2p/<PeerId> al final");
                eprintln!("  Ejemplo: /ip4/1.2.3.4/tcp/36565/p2p/12D3KooW...");
                std::process::exit(1);
            }
        };
        peers.insert(
            peer_id,
            PeerState {
                addr: addr.clone(),
                state: None,
                error: None,
                gate_error: None,
            },
        );
    }

    let local_key = identity::Keypair::generate_ed25519();
    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
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
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    if !json_output {
        println!("===========================================");
        println!(" 🔍  Ledgerita Remote Monitor");
        println!("===========================================");
        println!();
        println!("🌐 Conectando a {} peer(s)...", peers.len());
    }

    let dial_targets: Vec<(PeerId, Multiaddr)> = peers
        .iter()
        .map(|(id, ps)| (id.clone(), ps.addr.clone()))
        .collect();
    for (peer_id, addr) in &dial_targets {
        if !json_output {
            println!("   → {}  ({})", peer_id.to_base58(), fmt_addr(addr));
        }
        for resolved in resolve_multiaddr_dns(addr).await {
            if let Err(e) = swarm.dial(resolved)
                && let Some(ps) = peers.get_mut(peer_id)
            {
                ps.error = Some(format!("Error al dial: {:?}", e));
            }
        }
    }
    if !json_output {
        println!();
    }

    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);

    'event_loop: loop {
        let total = peers.len();
        let resolved = peers
            .values()
            .filter(|p| p.state.is_some() || p.error.is_some())
            .count();
        if resolved >= total {
            break 'event_loop;
        }
        if start.elapsed() > timeout {
            break 'event_loop;
        }

        let remaining = timeout.saturating_sub(start.elapsed());
        let event = tokio::time::timeout(remaining, swarm.select_next_some()).await;
        let event = match event {
            Ok(e) => e,
            Err(_) => break 'event_loop,
        };

        match event {
            SwarmEvent::NewListenAddr { .. } => {}
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                if peers.contains_key(&peer_id) {
                    let msg = build_msg(network, MSG_SYNC, &[0u8; 32]);
                    swarm.behaviour_mut().rr.send_request(&peer_id, msg);
                }
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                if let Some(pid) = peer_id {
                    if let Some(ps) = peers.get_mut(&pid) {
                        ps.error = Some(format!("{:?}", error));
                    }
                }
            }
            SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                if let Some(ps) = peers.get_mut(&peer_id) {
                    if ps.state.is_none() && ps.error.is_none() {
                        ps.error = Some(format!("Conexión cerrada: {:?}", cause));
                    }
                }
            }
            SwarmEvent::Behaviour(MonitorBehaviourEvent::Rr(
                request_response::Event::Message {
                    message: Message::Request { channel, .. },
                    ..
                },
            )) => {
                let _ = swarm
                    .behaviour_mut()
                    .rr
                    .send_response(channel, b"MONITOR".to_vec());
            }
            SwarmEvent::Behaviour(MonitorBehaviourEvent::Rr(
                request_response::Event::Message {
                    peer,
                    message: Message::Response { response, .. },
                    ..
                },
            )) => {
                if response.len() < 2 {
                    continue;
                }
                let (msg_type, payload) = match split_msg(network, &response) {
                    Some(x) => x,
                    None => {
                        if let Some(ps) = peers.get_mut(&peer) {
                            if ps.state.is_none() && ps.error.is_none() {
                                ps.error = Some("Respuesta de otra red".to_string());
                            }
                        }
                        continue;
                    }
                };
                if msg_type != MSG_SYNC {
                    continue;
                }
                match State::decode_snapshot(payload, network) {
                    Ok(state) => {
                        if let Some(ps) = peers.get_mut(&peer) {
                            ps.state = Some(state);
                        }
                    }
                    Err(e) => {
                        if let Some(ps) = peers.get_mut(&peer) {
                            ps.error = Some(format!("Snapshot inválido: {:?}", e));
                        }
                    }
                }
            }
            SwarmEvent::Behaviour(MonitorBehaviourEvent::Rr(
                request_response::Event::OutboundFailure { peer, error, .. },
            )) => {
                if let Some(ps) = peers.get_mut(&peer) {
                    if ps.state.is_none() && ps.error.is_none() {
                        ps.error = Some(format!("{:?}", error));
                    }
                }
            }
            _ => {}
        }
    }

    // ── Collect data ──
    let vset = default_validator_set(network);
    let summary = evaluate_monitor_peers(&mut peers, &mut baseline, network, &vset);

    // ── Print report ──
    if json_output {
        let mut json_peers: Vec<String> = Vec::new();
        for (peer_id, ps) in &peers {
            let mut obj = format!(
                r#"{{"peer_id":"{}","address":"{}""#,
                peer_id.to_base58(),
                escape_json_str(&ps.addr.to_string())
            );
            if let Some(ref state) = ps.state {
                let root_hex: String = state.generate_state_root()[..8]
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect();
                obj.push_str(&format!(
                    r#","state":{{"total_supply":{},"total_supply_lit":"{}","hard_cap":{},"supply_within_limit":{},"gates_ok":{},"state_root":"{}","fee_pool":{},"fee_pool_lit":"{}","goteo_emitido":{},"goteo_emitido_lit":"{}","tesoro_restante":{},"tesoro_restante_lit":"{}","tx_count":{},"epoch":{},"account_count":{}}}"#,
                    state.total_supply(),
                    format_amount(state.total_supply()),
                    HARD_CAP_SAT,
                    check_supply_limit(state),
                    ps.gate_error.is_none(),
                    root_hex,
                    state.fee_pool,
                    format_amount(state.fee_pool),
                    state.emitido_total,
                    format_amount(state.emitido_total),
                    state.tesoro_disponible(),
                    format_amount(state.tesoro_disponible()),
                    state.tx_count,
                    state.epoch,
                    state.balances.len(),
                ));
                if let Some(ref gerr) = ps.gate_error {
                    obj.push_str(&format!(r#","gate_error":"{}""#, escape_json_str(gerr)));
                }
            } else if let Some(ref err) = ps.error {
                obj.push_str(&format!(r#","error":"{}""#, escape_json_str(err)));
            } else {
                obj.push_str(r#","error":"timeout""#);
            }
            obj.push('}');
            json_peers.push(obj);
        }

        let status = summary.status_str();
        println!(
            r#"{{"peers":[{}],"cross_validation":{{"peers_compared":{},"all_match":{}}},"summary":{{"total_peers":{},"healthy":{},"unreachable":{},"anomalies":{},"status":"{}"}}}}"#,
            json_peers.join(","),
            summary.healthy,
            summary.roots_match,
            peers.len(),
            summary.healthy,
            summary.unreachable,
            summary.anomalies,
            status,
        );
    } else {
        println!("📊 Resultados por peer:\n");

        for (peer_id, ps) in &peers {
            let b58 = peer_id.to_base58();
            let sid = short_peer_id(&b58);
            println!("  [{}]", sid);

            if let Some(ref state) = ps.state {
                let supply = state.total_supply();
                if check_supply_limit(state) {
                    println!(
                        "    ✅ Supply     : {} / {} LIT",
                        format_amount(supply),
                        format_amount(HARD_CAP_SAT)
                    );
                } else {
                    println!(
                        "    🚨 Supply     : {} LIT  —  EXCEDE EL LÍMITE DE {} LIT!",
                        format_amount(supply),
                        format_amount(HARD_CAP_SAT)
                    );
                }
                if let Some(ref gerr) = ps.gate_error {
                    println!("    🚨 Gates g1-7 : RECHAZADO ({})", gerr);
                } else {
                    println!("    ✅ Gates g1-7 : OK (quórum BFT verificado)");
                }

                println!("    ✅ State Root : {}...", root_prefix(state));
                println!("    📊 Fee Pool   : {} LIT", format_amount(state.fee_pool));
                // GOTE (Checkpoint 21): estado del tesoro virtual.
                println!(
                    "    💧 Goteo emit  : {} LIT",
                    format_amount(state.emitido_total)
                );
                println!(
                    "    🏦 Tesoro rest : {} LIT",
                    format_amount(state.tesoro_disponible())
                );
                println!("    📊 TX Count   : {}", state.tx_count);
                println!("    📊 Época      : {}", state.epoch);
                println!("    📊 Cuentas    : {}", state.balances.len());
            } else if let Some(ref err) = ps.error {
                println!("    ❌ Error: {}", err);
            } else {
                println!("    ⚠️  Sin respuesta (timeout)");
            }
            println!();
        }

        println!("📊 Cross-Validation:");
        if summary.healthy > 1 {
            if summary.roots_match {
                println!(
                    "  ✅ Todos los peers ({}) comparten el mismo State Root",
                    summary.healthy
                );
            } else {
                println!("  🚨 DISCREPANCIA — Los State Roots no coinciden entre peers");
            }
        } else if summary.healthy == 1 {
            println!("  ℹ️  Solo 1 peer verificado (no hay con quién comparar)");
        } else {
            println!("  ⚠️  Ningún peer respondió con estado válido");
        }
        println!();

        println!("Resumen:");
        if summary.anomalies == 0 && summary.healthy > 0 {
            println!(
                "  ✅ Red saludable — {} peer(s) verificado(s), 0 anomalías",
                summary.healthy
            );
        } else {
            println!(
                "  🚨 Estado {}: {} saludable(s), {} inaccesible(s), {} anomalía(s)",
                summary.status_str(),
                summary.healthy,
                summary.unreachable,
                summary.anomalies
            );
        }
        println!("===========================================");
    }

    if summary.anomalies > 0 || summary.healthy == 0 {
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use ledgerita_core::crypto::Address;
    use std::str::FromStr;

    #[test]
    fn test_format_amount() {
        assert_eq!(format_amount(0), "0.000000");
        assert_eq!(format_amount(1_000_000), "1.000000");
        assert_eq!(format_amount(21_000_000_000_000u64), "21000000.000000");
        assert_eq!(format_amount(15_500_000), "15.500000");
        assert_eq!(format_amount(123_456_789), "123.456789");
        assert_eq!(format_amount(999), "0.000999");
    }

    #[test]
    fn test_format_amount_edge_cases() {
        assert_eq!(format_amount(1), "0.000001");
        assert_eq!(format_amount(999_999), "0.999999");
        assert_eq!(format_amount(1_000_001), "1.000001");
        assert_eq!(format_amount(100_000_000), "100.000000");
    }

    #[test]
    fn test_root_prefix() {
        let mut state = State::new();
        let addr = Address::from_signing_key(&SigningKey::from_bytes(&[42u8; 32]));
        state.mint(addr, 1000);
        let prefix = root_prefix(&state);
        assert_eq!(
            prefix.len(),
            16,
            "root_prefix debe ser 16 chars hex (8 bytes)"
        );
        assert!(
            prefix.chars().all(|c| c.is_ascii_hexdigit()),
            "root_prefix debe ser hexadecimal"
        );
    }

    #[test]
    fn test_root_prefix_determinism() {
        let addr1 = Address::from_signing_key(&SigningKey::from_bytes(&[1u8; 32]));
        let addr2 = Address::from_signing_key(&SigningKey::from_bytes(&[2u8; 32]));

        let mut state_a = State::new();
        state_a.mint(addr1.clone(), 100);
        state_a.mint(addr2.clone(), 200);

        let mut state_b = State::new();
        state_b.mint(addr2, 200);
        state_b.mint(addr1.clone(), 100);

        assert_eq!(
            root_prefix(&state_a),
            root_prefix(&state_b),
            "mismo estado debe dar mismo prefix"
        );

        let mut state_c = State::new();
        state_c.mint(addr1, 101);
        assert_ne!(
            root_prefix(&state_a),
            root_prefix(&state_c),
            "estado diferente debe dar prefix diferente"
        );
    }

    #[test]
    fn test_extract_peer_id() {
        let addr = Multiaddr::from_str(
            "/ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1",
        )
        .unwrap();
        let pid = extract_peer_id(&addr);
        assert!(pid.is_some());
        assert_eq!(
            pid.unwrap().to_base58(),
            "12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1"
        );
    }

    #[test]
    fn test_extract_peer_id_no_p2p() {
        let addr = Multiaddr::from_str("/ip4/192.168.68.61/tcp/36565").unwrap();
        let pid = extract_peer_id(&addr);
        assert!(pid.is_none());
    }

    #[test]
    fn test_fmt_addr_short() {
        let addr = Multiaddr::from_str("/ip4/127.0.0.1/tcp/9999").unwrap();
        let s = fmt_addr(&addr);
        assert_eq!(s, "/ip4/127.0.0.1/tcp/9999");
        assert!(!s.contains("..."));
    }

    #[test]
    fn test_fmt_addr_long() {
        let long =
            "/ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1";
        let addr = Multiaddr::from_str(long).unwrap();
        let s = fmt_addr(&addr);
        assert!(s.len() < long.len(), "debe truncar la dirección larga");
        assert!(s.ends_with("..."), "debe terminar con ...");
    }

    #[test]
    fn test_check_supply_limit_under() {
        let mut state = State::new();
        let addr = Address::from_signing_key(&SigningKey::from_bytes(&[42u8; 32]));
        state.mint(addr, 1_000_000);
        assert!(check_supply_limit(&state));
    }

    #[test]
    fn test_check_supply_limit_exact() {
        let mut state = State::new();
        let addr = Address::from_signing_key(&SigningKey::from_bytes(&[43u8; 32]));
        state.mint(addr, HARD_CAP_SAT);
        assert!(check_supply_limit(&state));
    }

    #[test]
    fn test_check_supply_limit_exceeded() {
        let mut state = State::new();
        let addr = Address::from_signing_key(&SigningKey::from_bytes(&[44u8; 32]));
        state.mint(addr, HARD_CAP_SAT + 1);
        assert!(!check_supply_limit(&state));
    }

    #[test]
    fn test_cross_validate_roots_all_match() {
        let root1 = [0xaa; 32];
        let root2 = [0xaa; 32];
        let root3 = [0xaa; 32];
        assert!(cross_validate_roots(&[root1, root2, root3]));
    }

    #[test]
    fn test_cross_validate_roots_discrepancy() {
        let root1 = [0xaa; 32];
        let root2 = [0xbb; 32];
        let root3 = [0xaa; 32];
        assert!(!cross_validate_roots(&[root1, root2, root3]));
    }

    #[test]
    fn test_cross_validate_roots_single() {
        let root1 = [0xaa; 32];
        assert!(cross_validate_roots(&[root1]));
    }

    #[test]
    fn test_cross_validate_roots_empty() {
        assert!(cross_validate_roots(&[]));
    }

    #[test]
    fn test_short_peer_id_y_escape_json_control_chars() {
        assert_eq!(short_peer_id("abc"), "abc");
        assert_eq!(
            short_peer_id("12D3KooWMkKuW9NTuhxe8mdg"),
            "12D3KooWMkKuW9NTuh"
        );
        let escaped = escape_json_str("a\"b\\c\nd\x07");
        assert_eq!(escaped, "a\\\"b\\\\c\\nd\\u0007");
    }

    #[test]
    fn test_ckpt56_evaluate_peers_rechaza_timeouts_y_fallas_gate_sin_sumar_healthy() {
        let vset = default_validator_set(NetworkId::Testnet);
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/46565".parse().unwrap();

        // 1) Todos timeout -> healthy == 0, unhealthy, anomalies > 0
        let mut peers = HashMap::new();
        peers.insert(
            PeerId::random(),
            PeerState {
                addr: addr.clone(),
                state: None,
                error: None,
                gate_error: None,
            },
        );
        let mut baseline = None;
        let s1 = evaluate_monitor_peers(&mut peers, &mut baseline, NetworkId::Testnet, &vset);
        assert_eq!(s1.healthy, 0);
        assert_eq!(s1.unreachable, 1);
        assert_eq!(s1.anomalies, 1);
        assert_eq!(s1.status_str(), "unhealthy");

        // 2) Peer con epoch=99 sin QC -> NO suma healthy, registra gate_error
        let mut forged = State::new();
        forged.epoch = 99;
        forged.tx_count = 99 * ledgerita_core::state::CHECKPOINT_INTERVAL;
        let pid_bad = PeerId::random();
        let mut peers2 = HashMap::new();
        peers2.insert(
            pid_bad,
            PeerState {
                addr,
                state: Some(forged),
                error: None,
                gate_error: None,
            },
        );
        let s2 = evaluate_monitor_peers(&mut peers2, &mut baseline, NetworkId::Testnet, &vset);
        assert_eq!(s2.healthy, 0, "un peer rechazado por Gate no puede contar como healthy");
        assert_eq!(s2.anomalies, 1);
        assert_eq!(
            peers2.get(&pid_bad).unwrap().gate_error.as_deref(),
            Some("FaltaCertificadoQuorum")
        );
    }
}
