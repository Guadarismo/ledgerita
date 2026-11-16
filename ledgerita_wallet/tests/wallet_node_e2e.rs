//! Test E2E headless del wallet-node (Fase 2.1a) — prueba en local ANTES de
//! la prueba en vivo contra producción (2026-11-05).
//!
//! Escenario completo del broadcast-first, sobre un demonio core real
//! (`run_p2p_node` in-process):
//! 1. Demonio TestNet con 5 LIT minteados a la wallet.
//! 2. La wallet arranca SIN peers, envía una TX → **NO se aplica** (Offline)
//!    y el estado local no se toca ("la wallet jamás miente el saldo").
//! 3. `ChangePeers` al demonio → sync adopta su snapshot más fresco.
//! 4. El reintento automático (TX en memoria) llega a la red → `OK` → la
//!    wallet la aplica → balances/tx_count reflejan la verdad de la red.

use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ed25519_dalek::Signer;
use ledgerita_core::crypto::{load_or_create_wallet, Address};
use ledgerita_core::network::{run_p2p_node, Mempool, NetworkId};
use ledgerita_core::state::State;
use ledgerita_core::storage::Storage;
use ledgerita_core::transaction::Transaction;
use tokio::sync::Mutex;

use ledgerita_wallet::net_actor::{self, BroadcastReport, NodeCmd};

const NET: NetworkId = NetworkId::Testnet;

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("ledgerita_wallet_test_{label}_{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn find_free_port() -> u16 {
    for _ in 0..20 {
        if let Ok(listener) = std::net::TcpListener::bind("127.0.0.1:0") {
            let port = listener.local_addr().unwrap().port();
            if port > 1024 {
                return port;
            }
        }
    }
    panic!("no free port");
}

/// Espera a que el puerto TCP esté abierto cediendo el executor (tokio::sleep);
/// con `std::thread::sleep` bloquearía el runtime single-threaded del test y
/// el demonio jamás llegaría a correr.
async fn wait_for_port(port: u16, max: Duration) {
    let start = Instant::now();
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        if start.elapsed() > max {
            panic!("el demonio core no abrió el puerto {port} en {max:?}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Reintenta la sonda hasta `Ok(())` o pase `max`; falla con el último motivo.
async fn wait_until(max: Duration, what: &str, mut probe: impl FnMut() -> Result<(), String>) {
    let start = Instant::now();
    let mut last;
    loop {
        last = match probe() {
            Ok(()) => return,
            Err(reason) => reason,
        };
        if start.elapsed() > max {
            panic!("timeout esperando {what}: {last}");
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

#[tokio::test]
async fn test_wallet_node_broadcast_first_e2e() {
    let daemon_dir = unique_temp_dir("daemon");
    let wallet_dir = unique_temp_dir("wallet");
    let port = find_free_port();

    // ── Identidades ──
    // El demonio: keypair FIJO (el PeerId va a la multiaddr del seed).
    let kp_daemon = libp2p::identity::Keypair::generate_ed25519();
    let daemon_peer_id = libp2p::PeerId::from(kp_daemon.public());
    let daemon_wallet_addr = Address::from_signing_key(&ed25519_dalek::SigningKey::from_bytes(
        &[0xd1; 32],
    ));

    // La wallet: la MISMA crypto.load_or_create_wallet que usa la GUI.
    let (sk_wallet, kp_wallet) =
        load_or_create_wallet(&wallet_dir, "test-password-e2e").unwrap();
    let wallet_addr = Address::from_signing_key(&sk_wallet);
    let receiver_addr = daemon_wallet_addr.clone();

    // ── Demonio: estado con fondos (mint estilo --mint del core) ──
    let storage_d = Arc::new(Storage::open(&daemon_dir.join("state")).unwrap());
    let state_d = Arc::new(Mutex::new(State::load(&storage_d).unwrap()));
    {
        let mut s = state_d.lock().await;
        s.mint(wallet_addr.clone(), 5_000_000); // 5 LIT
        s.tx_count += 1; // frescura (así lo hace el CLI --mint)
        s.flush(&storage_d).unwrap();
    }

    // ── Daemon core (run_p2p_node), sin seeds (la wallet lo diala a él) ──
    let mempool: Mempool = Arc::new(Mutex::new(std::collections::HashSet::new()));
    let pending_broadcast = Arc::new(Mutex::new(Vec::new()));
    {
        let state = state_d.clone();
        let storage = storage_d.clone();
        tokio::spawn(async move {
            if let Err(e) = run_p2p_node(
                kp_daemon,
                vec![],
                state,
                storage,
                daemon_wallet_addr,
                mempool,
                vec![],
                pending_broadcast,
                port,
                NET,
            )
            .await
            {
                eprintln!("[e2e-demonio] terminó con error: {e}");
            }
        });
    }
    wait_for_port(port, Duration::from_secs(10)).await;
    eprintln!("[e2e] puerto del demonio abierto: {port}");

    let seed: libp2p::Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}/p2p/{daemon_peer_id}")
        .parse()
        .unwrap();

    // ── Wallet: storage local fresh (sin génesis) + actor SIN peers ──
    let storage_w = Arc::new(Storage::open(&wallet_dir.join("state")).unwrap());
    let state_w = Arc::new(Mutex::new(State::load(&storage_w).unwrap()));
    let handles = net_actor::spawn_wallet_node(
        NET,
        kp_wallet,
        state_w.clone(),
        storage_w,
        vec![], // sin seed: mismo caso que la GUI con seed vacío
        0,
    )
    .expect("spawn del actor");

    let baseline_tx_count = state_w.lock().await.tx_count;
    assert_eq!(baseline_tx_count, 0, "wallet fresca, sin nada aplicado");

    // ── PASO 1: TX sin conexión → Offline y NO se aplica ──
    let tx_enviar = {
        let nonce = 1; // (baseline: la cuenta esta vacia)
        let mut tx = Transaction::new(
            wallet_addr.clone(),
            receiver_addr.clone(),
            1_000, // monto
            10,    // fee
            nonce,
        )
        .with_chain_id(NetworkId::Testnet.chain_id());
        let msg = tx.to_bytes();
        tx.sign(sk_wallet.sign(&msg));
        tx
    };
    handles
        .send_cmd
        .send(NodeCmd::BroadcastTx(tx_enviar))
        .expect("enviar broadcast sin conexión");

    wait_until(Duration::from_secs(5), "reporte Offline", || {
        let st = handles.shared.lock().unwrap();
        match &st.last_broadcast {
            Some(BroadcastReport::Offline { tried }) if *tried == 0 => Ok(()),
            Some(other) => Err(format!("reporte distinto: {other:?}")),
            None => Err("sin reporte aún".to_string()),
        }
    })
    .await;

    // Promesa central: sin conexión, el estado local no se toca.
    assert_eq!(
        state_w.lock().await.tx_count,
        0,
        "sin conexión la TX NO debe aplicar al estado local"
    );

    // ── PASO 2: conectar → sync adopta el snapshot del demonio ──
    handles
        .send_cmd
        .send(NodeCmd::ChangePeers(vec![seed]))
        .expect("change peers");

    wait_until(Duration::from_secs(15), "peer sincronizado", || {
        let st = handles.shared.lock().unwrap();
        match st.peers.values().next() {
            Some(pv) if pv.connected && pv.synced => Ok(()),
            Some(pv) => Err(format!("peer: {}", pv.status)),
            None => Err("sin peers registrados".to_string()),
        }
    })
    .await;

    // Tras adoptar el snapshot más fresco del demonio: el mint (tx_count=1).
    wait_until(Duration::from_secs(5), "snapshot adoptado", || {
        match state_w.try_lock() {
            Ok(s) if s.tx_count >= 1 => Ok(()),
            Ok(_) => Err("la wallet no adoptó el snapshot (tx_count=0)".to_string()),
            Err(_) => Err("estado ocupado (lo lee el actor)".to_string()),
        }
    })
    .await;

    // ── PASO 3: reintento automático al conectar → OK → aplicar ──
    wait_until(Duration::from_secs(20), "broadcast aceptado", || {
        let st = handles.shared.lock().unwrap();
        match &st.last_broadcast {
            Some(BroadcastReport::Accepted { .. }) => Ok(()),
            Some(BroadcastReport::Rejected { reason }) => Err(format!("rechazada: {reason}")),
            other => Err(format!("reporte: {other:?}")),
        }
    })
    .await;

    // El estado local refleja la verdad de la red.
    wait_until(Duration::from_secs(10), "estado local actualizado", || {
        let s = state_w.try_lock();
        let Ok(s) = s else { return Err("estado ocupado (le el actor)".to_string()) };
        let bal = s.balances.get(&wallet_addr).copied().unwrap_or(0);
        let recv = s.balances.get(&receiver_addr).copied().unwrap_or(0);
        if bal == 5_000_000 - 1_000 - 10 && recv == 1_000 {
            Ok(())
        } else {
            Err(format!("balances aún no: emisor={bal} receptor={recv}"))
        }
    })
    .await;

    // El demonio: su estado debe reflejar la misma Tx.
    assert_eq!(
        state_d.lock().await.total_supply(),
        state_w.lock().await.total_supply(),
        "supply del demonio y de la wallet deben coincidir (ambos aplicaron la TX)"
    );
    assert_eq!(
        state_d.lock().await.tx_count,
        2, // mint + nuestra TX
        "el demonio debió aplicar la TX confirmada"
    );

    // Fue un único broadcast (1 ack del seed conectado).
    let report = handles.shared.lock().unwrap().last_broadcast.clone();
    assert!(matches!(
        report,
        Some(BroadcastReport::Accepted { acks: 1, duplicates: 0 })
    ), "reporte final inesperado: {report:?}");

    // ── Limpieza (best effort; sled puede seguir abierto) ──
    let _ = std::fs::remove_dir_all(&daemon_dir);
    let _ = std::fs::remove_dir_all(&wallet_dir);
}
