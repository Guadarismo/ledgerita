mod helpers;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::{Signer, SigningKey};
use helpers::TempTestDirs;
use ledgerita_core::crypto::Address;
use ledgerita_core::network::{NetworkId, QuorumNodeConfig, run_p2p_node_with_quorum};
use ledgerita_core::quorum::{QuorumCertificate, ValidatorSet, sign_qc_vote};
use ledgerita_core::state::{CHECKPOINT_INTERVAL, State};
use ledgerita_core::storage::Storage;
use ledgerita_core::transaction::Transaction;
use libp2p::{Multiaddr, PeerId, identity};
use tokio::sync::Mutex;

#[tokio::test]
async fn test_ciclo_completo_bft_cierre_4096_poda_y_tx_4097_p2p() {
    let dirs = TempTestDirs::new();
    let net = NetworkId::Testnet;

    let v_keys = [
        SigningKey::from_bytes(&[0xA1; 32]),
        SigningKey::from_bytes(&[0xA2; 32]),
        SigningKey::from_bytes(&[0xA3; 32]),
    ];
    let v_pubkeys: Vec<[u8; 32]> = v_keys
        .iter()
        .map(|k| k.verifying_key().to_bytes())
        .collect();
    let vset = ValidatorSet::from_pubkeys(&v_pubkeys, 2).unwrap();

    let sk_sender = SigningKey::from_bytes(&[0xB1; 32]);
    let addr_sender = Address::from_signing_key(&sk_sender);
    let addr_recv = Address::from_signing_key(&SigningKey::from_bytes(&[0xB2; 32]));

    // ── Seed: época 0 -> cierre en 4096 con QC 2-de-3 -> poda -> TX 4097 ──
    let storage_seed = Arc::new(Storage::open(&dirs.a.join("state")).unwrap());
    let expected_root = {
        let mut s = State::load(&storage_seed).unwrap();
        s.mint(addr_sender.clone(), 1_000_000);
        s.tx_count = CHECKPOINT_INTERVAL - 1;

        let mut tx_close = Transaction::new(addr_sender.clone(), addr_recv.clone(), 100, 5, 1)
            .with_chain_id(net.chain_id());
        let msg_close = tx_close.to_bytes();
        tx_close.sign(sk_sender.sign(&msg_close));
        s.apply_transaction(&tx_close).unwrap();
        assert!(s.is_checkpoint_due());

        let cp1_root = s.finalize_checkpoint();
        assert_eq!(s.epoch, 1);
        assert!(s.epoch_txs.is_empty(), "diario debe podarse al cerrar época");
        assert_eq!(s.last_checkpoint_root, Some(cp1_root));

        let qc = QuorumCertificate {
            chain_id: net.chain_id(),
            epoch: 1,
            state_root: cp1_root,
            signatures: vec![
                sign_qc_vote(&v_keys[0], net.chain_id(), 1, &cp1_root, 0),
                sign_qc_vote(&v_keys[1], net.chain_id(), 1, &cp1_root, 1),
            ],
        };
        s.latest_qc = Some(qc);

        // TX 4097 intra-época sobre época 1
        let mut tx_4097 = Transaction::new(addr_sender, addr_recv, 250, 10, 2)
            .with_chain_id(net.chain_id());
        let msg_4097 = tx_4097.to_bytes();
        tx_4097.sign(sk_sender.sign(&msg_4097));
        s.apply_transaction(&tx_4097).unwrap();
        assert_eq!(s.tx_count, CHECKPOINT_INTERVAL + 1);
        assert_eq!(s.epoch_txs.len(), 1);
        s.flush(&storage_seed).unwrap();
        s.generate_state_root()
    };

    let state_seed = Arc::new(Mutex::new(State::load(&storage_seed).unwrap()));
    let kp_seed = identity::Keypair::generate_ed25519();
    let peer_seed = PeerId::from(kp_seed.public());
    let seed_port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };

    let seed_cfg = QuorumNodeConfig {
        validator_set: Some(vset.clone()),
        local_validator: None,
        ..Default::default()
    };
    let s_seed = state_seed.clone();
    let st_seed = storage_seed.clone();
    tokio::spawn(async move {
        let _ = run_p2p_node_with_quorum(
            kp_seed,
            vec![],
            s_seed,
            st_seed,
            Address::from_hash_bytes([1u8; 32]),
            Arc::new(Mutex::new(HashSet::new())),
            vec![],
            Arc::new(Mutex::new(Vec::new())),
            seed_port,
            net,
            seed_cfg,
        )
        .await;
    });

    // Esperar apertura del puerto P2P del seed
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", seed_port))
            .await
            .is_ok()
        {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "seed no abrió puerto");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // ── Cliente fresh con QuorumNodeConfig 2-de-3 activa ──
    let storage_client = Arc::new(Storage::open(&dirs.b.join("state")).unwrap());
    let state_client = Arc::new(Mutex::new(State::load(&storage_client).unwrap()));
    let kp_client = identity::Keypair::generate_ed25519();
    let seed_ma: Multiaddr = format!("/ip4/127.0.0.1/tcp/{seed_port}/p2p/{peer_seed}")
        .parse()
        .unwrap();

    let client_cfg = QuorumNodeConfig {
        validator_set: Some(vset),
        local_validator: None,
        ..Default::default()
    };
    let s_client = state_client.clone();
    let st_client = storage_client.clone();
    tokio::spawn(async move {
        let _ = run_p2p_node_with_quorum(
            kp_client,
            vec![seed_ma],
            s_client,
            st_client,
            Address::from_hash_bytes([2u8; 32]),
            Arc::new(Mutex::new(HashSet::new())),
            vec![],
            Arc::new(Mutex::new(Vec::new())),
            0,
            net,
            client_cfg,
        )
        .await;
    });

    let sync_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        {
            let sc = state_client.lock().await;
            if sc.tx_count == CHECKPOINT_INTERVAL + 1 && sc.generate_state_root() == expected_root
            {
                assert_eq!(sc.epoch, 1);
                assert_eq!(sc.epoch_txs.len(), 1);
                assert!(sc.latest_qc.is_some());
                break;
            }
        }
        assert!(
            tokio::time::Instant::now() < sync_deadline,
            "el cliente no sincronizó TX 4097 con quórum BFT 2-de-3"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn test_e2e_votacion_bft_viva_2_de_3_cierra_checkpoint_y_propaga_qc() {
    use helpers::{alloc_loopback_port, make_test_vset_2_de_3, spawn_real_p2p_node, wait_tcp_port_open};
    use ledgerita_core::network::DeferredSend;

    let dirs = TempTestDirs::new();
    let net = NetworkId::Testnet;
    let (v_keys, vset) = make_test_vset_2_de_3(0xB1);

    let sk_funder = SigningKey::from_bytes(&[0xC1; 32]);
    let addr_funder = Address::from_signing_key_d2(&sk_funder);
    let addr_recv = Address::from_signing_key_d2(&SigningKey::from_bytes(&[0xC2; 32]));

    // Estado idéntico en los 3 nodos al borde tx_count = 4095 (falta 1 TX para el cierre 4096)
    let storage_seed = Arc::new(Storage::open(&dirs.a.join("state")).unwrap());
    let storage_val1 = Arc::new(Storage::open(&dirs.b.join("state")).unwrap());
    let storage_obs = Arc::new(Storage::open(&dirs.c.join("state")).unwrap());
    for st in [&storage_seed, &storage_val1, &storage_obs] {
        let mut s = State::load(st).unwrap();
        s.mint(addr_funder.clone(), 100_000_000);
        s.emitido_total = 100_000_000;
        s.tx_count = CHECKPOINT_INTERVAL - 1;
        s.flush(st).unwrap();
    }

    let state_seed = Arc::new(Mutex::new(State::load(&storage_seed).unwrap()));
    let state_val1 = Arc::new(Mutex::new(State::load(&storage_val1).unwrap()));
    let state_obs = Arc::new(Mutex::new(State::load(&storage_obs).unwrap()));

    let kp_seed = identity::Keypair::generate_ed25519();
    let peer_seed = PeerId::from(kp_seed.public());
    let port_seed = alloc_loopback_port();
    let _h_seed = spawn_real_p2p_node(
        kp_seed,
        vec![],
        state_seed.clone(),
        storage_seed.clone(),
        Address::from_hash_bytes([1u8; 32]),
        vec![],
        port_seed,
        net,
        QuorumNodeConfig {
            validator_set: Some(vset.clone()),
            local_validator: Some((0, v_keys[0].clone())),
            ..Default::default()
        },
    );
    wait_tcp_port_open(port_seed).await;
    let seed_ma: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port_seed}/p2p/{peer_seed}")
        .parse()
        .unwrap();

    // Observador conecta primero al Seed para recibir TX 4096 + votos BFT por gossip
    let _h_obs = spawn_real_p2p_node(
        identity::Keypair::generate_ed25519(),
        vec![seed_ma.clone()],
        state_obs.clone(),
        storage_obs.clone(),
        Address::from_hash_bytes([3u8; 32]),
        vec![],
        0,
        net,
        QuorumNodeConfig {
            validator_set: Some(vset.clone()),
            local_validator: None,
            ..Default::default()
        },
    );
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Validador 1 conecta al Seed y tras sync inicial dispara TX #4096 (cierre) + MSG_VOTE 1
    let _h_val1 = spawn_real_p2p_node(
        identity::Keypair::generate_ed25519(),
        vec![seed_ma],
        state_val1.clone(),
        storage_val1.clone(),
        Address::from_hash_bytes([2u8; 32]),
        vec![],
        0,
        net,
        QuorumNodeConfig {
            validator_set: Some(vset),
            local_validator: Some((1, v_keys[1].clone())),
            testnet_faucet: None,
            deferred_sends: vec![DeferredSend {
                signing_key: sk_funder,
                receiver: addr_recv,
                amount: 1_000_000,
                fee: 5,
                force_legacy_d1: false,
            }],
        },
    );

    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let ss = state_seed.lock().await;
            let sv = state_val1.lock().await;
            let so = state_obs.lock().await;
            let ready = ss.epoch == 1
                && sv.epoch == 1
                && so.epoch == 1
                && ss.latest_qc.is_some()
                && sv.latest_qc.is_some()
                && so.latest_qc.is_some();
            if ready {
                let r_seed = ss.generate_state_root();
                assert_eq!(r_seed, sv.generate_state_root());
                assert_eq!(r_seed, so.generate_state_root());
                assert_eq!(ss.tx_count, CHECKPOINT_INTERVAL);
                assert!(ss.epoch_txs.is_empty());
                assert!(sv.epoch_txs.is_empty());
                assert!(so.epoch_txs.is_empty());
                assert_eq!(ss.latest_qc.as_ref().unwrap().signatures.len(), 2);
                assert_eq!(so.latest_qc.as_ref().unwrap().signatures.len(), 2);
                break;
            }
            drop(ss);
            drop(sv);
            drop(so);
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("Votación BFT viva 2-de-3 debió cerrar época 1 y propagar QC a los 3 nodos reales");
}
