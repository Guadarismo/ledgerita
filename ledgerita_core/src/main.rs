use ed25519_dalek::SigningKey;
use ledgerita_core::crypto::{self, Address};
use ledgerita_core::faucet::{DEFAULT_FAUCET_AMOUNT, DEFAULT_FAUCET_CAP, HARD_CAP_SAT};
use ledgerita_core::genesis;
use ledgerita_core::network::{
    DeferredSend, Mempool, NetworkId, QuorumNodeConfig, fallback_seeds_for_peer,
    run_p2p_node_with_quorum,
};
use ledgerita_core::quorum::{ValidatorSet, default_validator_set, parse_hex_32};
use ledgerita_core::serialize::{encode_transaction, hash_tx};
use ledgerita_core::state::State;
use ledgerita_core::storage::Storage;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;
use zeroize::Zeroizing;

fn format_lit(amount: u64) -> String {
    let integer = amount / 1_000_000;
    let fraction = amount % 1_000_000;
    format!("{}.{:06}", integer, fraction)
}

fn short_hex(s: &str) -> String {
    s.chars().take(16).collect()
}

fn default_data_dir_in(home: &Path, network: NetworkId) -> PathBuf {
    let base = home.join(".ledgerita");
    let scoped = base.join(network.tag());
    if network == NetworkId::Mainnet && !scoped.exists() && base.join("state").exists() {
        base
    } else {
        scoped
    }
}

fn default_data_dir(network: NetworkId) -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    default_data_dir_in(Path::new(&home), network)
}

fn validate_password_value(pw: String) -> Result<Zeroizing<String>, String> {
    let pw = Zeroizing::new(pw);
    if pw.trim().is_empty() {
        return Err("Error: LEDGERITA_PASSWORD no puede estar vacía.".to_string());
    }
    Ok(pw)
}

fn take_password_from_env() -> Zeroizing<String> {
    let raw = std::env::var("LEDGERITA_PASSWORD").unwrap_or_else(|_| {
        eprintln!("Error: LEDGERITA_PASSWORD no está configurada.");
        eprintln!("Establece la variable de entorno con la contraseña de tu wallet.");
        eprintln!("Ejemplo (PowerShell):  $env:LEDGERITA_PASSWORD=\"tu-clave-segura\"");
        eprintln!("Ejemplo (Linux):       export LEDGERITA_PASSWORD=\"tu-clave-segura\"");
        std::process::exit(1);
    });
    // SAFETY: se ejecuta en `fn main` síncrono antes de crear hilos del runtime Tokio.
    unsafe {
        std::env::remove_var("LEDGERITA_PASSWORD");
    }
    validate_password_value(raw).unwrap_or_else(|msg| {
        eprintln!("{msg}");
        std::process::exit(1);
    })
}

fn load_signing_key_file(path: &Path, flag_name: &str) -> SigningKey {
    let raw = Zeroizing::new(std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("Error leyendo {} {}: {}", flag_name, path.display(), e);
        std::process::exit(1);
    }));
    let seed = Zeroizing::new(parse_hex_32(&raw).unwrap_or_else(|| {
        eprintln!(
            "Error: {} debe contener exactamente 64 caracteres hex (seed ed25519)",
            path.display()
        );
        std::process::exit(1);
    }));
    SigningKey::from_bytes(&seed)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("ledgerita_core=info,warn"));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .init();

    let args: Vec<String> = std::env::args().collect();

    let mut data_dir: Option<PathBuf> = None;
    let mut connect_peers = Vec::new();
    let mut show_only_address = false;
    let mut init_wallet = false;
    let mut legacy_d1 = false;
    let mut pending_send: Vec<(String, u64, u64)> = Vec::new();
    let mut pending_mints: Vec<(String, u64)> = Vec::new();
    let mut pending_faucets: Vec<(String, u64)> = Vec::new();
    let mut faucet_amount: u64 = DEFAULT_FAUCET_AMOUNT;
    let mut faucet_cap: u64 = DEFAULT_FAUCET_CAP;
    let mut faucet_key_file: Option<PathBuf> = None;
    let mut listen_port: u16 = 0;
    let mut network = NetworkId::Mainnet;
    let mut no_fallback = false;
    let mut validators_file: Option<PathBuf> = None;
    let mut validator_id: Option<u8> = None;
    let mut validator_key_file: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--legacy-d1" => {
                legacy_d1 = true;
            }
            "--data-dir" | "-d" => {
                i += 1;
                if i < args.len() {
                    data_dir = Some(PathBuf::from(&args[i]));
                }
            }
            "--connect" | "-c" => {
                i += 1;
                if i < args.len() {
                    connect_peers.push(args[i].parse()?);
                }
            }
            "--show-address" | "-s" => {
                show_only_address = true;
            }
            "--init" => {
                // Crea la wallet (wallet.key) en --data-dir, imprime la
                // dirección y SAL sin correr el nodo. Útil para preparar
                // wallets de la wallet-node (Fase 2.1b) antes del faucet.
                init_wallet = true;
            }
            "--send" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --send requiere <receiver_hex> <amount> [fee]");
                    std::process::exit(1);
                }
                let receiver_hex = args[i].clone();
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --send requiere <amount> después del receiver");
                    std::process::exit(1);
                }
                let amount: u64 = args[i].parse().unwrap_or_else(|_| {
                    eprintln!("Error: amount debe ser un número entero positivo");
                    std::process::exit(1);
                });
                i += 1;
                let fee = if i < args.len() && !args[i].starts_with('-') {
                    let f: u64 = args[i].parse().unwrap_or_else(|_| {
                        eprintln!("Error: fee debe ser un número entero positivo");
                        std::process::exit(1);
                    });
                    i += 1;
                    f
                } else {
                    0
                };
                i -= 1; // bottom i += 1 ya avanza el flag, evitar doble incremento
                pending_send.push((receiver_hex, amount, fee));
            }
            "--mint" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --mint requiere <address_hex> <amount>");
                    std::process::exit(1);
                }
                let addr_hex = args[i].clone();
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --mint requiere <amount> después de la dirección");
                    std::process::exit(1);
                }
                let amount: u64 = args[i].parse().unwrap_or_else(|_| {
                    eprintln!("Error: amount debe ser un número entero positivo");
                    std::process::exit(1);
                });
                pending_mints.push((addr_hex, amount));
            }
            "--faucet" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --faucet requiere <address_hex> [amount]");
                    std::process::exit(1);
                }
                let addr_hex = args[i].clone();
                i += 1;
                let amount = if i < args.len() && !args[i].starts_with('-') {
                    let a: u64 = args[i].parse().unwrap_or_else(|_| {
                        eprintln!("Error: monto debe ser un número entero positivo");
                        std::process::exit(1);
                    });
                    i += 1;
                    a
                } else {
                    faucet_amount
                };
                i -= 1; // bottom i += 1 ya avanza el flag, evitar doble incremento
                pending_faucets.push((addr_hex, amount));
            }
            "--faucet-amount" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --faucet-amount requiere un monto");
                    std::process::exit(1);
                }
                faucet_amount = args[i].parse().unwrap_or_else(|_| {
                    eprintln!("Error: --faucet-amount debe ser un número entero positivo");
                    std::process::exit(1);
                });
            }
            "--faucet-cap" => {
                i += 1;
                if i >= args.len() {
                    eprintln!(
                        "Error: --faucet-cap requiere un monto (tope de por vida por dirección)"
                    );
                    std::process::exit(1);
                }
                faucet_cap = args[i].parse().unwrap_or_else(|_| {
                    eprintln!("Error: --faucet-cap debe ser un número entero positivo");
                    std::process::exit(1);
                });
            }
            "--faucet-key-file" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --faucet-key-file requiere una ruta");
                    std::process::exit(1);
                }
                faucet_key_file = Some(PathBuf::from(&args[i]));
            }
            "--listen-port" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --listen-port requiere un número de puerto");
                    std::process::exit(1);
                }
                listen_port = args[i].parse().unwrap_or_else(|_| {
                    eprintln!("Error: --listen-port debe ser un número entre 0 y 65535");
                    std::process::exit(1);
                });
            }
            "--network" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --network requiere un valor: mainnet | testnet");
                    std::process::exit(1);
                }
                network = NetworkId::from_str(&args[i]).unwrap_or_else(|| {
                    eprintln!(
                        "Error: red desconocida: {}. Usa mainnet o testnet.",
                        args[i]
                    );
                    std::process::exit(1);
                });
            }
            "--no-fallback" => {
                no_fallback = true;
            }
            "--validators-file" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --validators-file requiere una ruta");
                    std::process::exit(1);
                }
                validators_file = Some(PathBuf::from(&args[i]));
            }
            "--validator-id" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --validator-id requiere un entero (0..255)");
                    std::process::exit(1);
                }
                validator_id = Some(args[i].parse().unwrap_or_else(|_| {
                    eprintln!("Error: --validator-id debe ser un entero entre 0 y 255");
                    std::process::exit(1);
                }));
            }
            "--validator-key-file" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --validator-key-file requiere una ruta");
                    std::process::exit(1);
                }
                validator_key_file = Some(PathBuf::from(&args[i]));
            }
            other => {
                eprintln!("Argumento desconocido: {}", other);
                eprintln!(
                    "Uso: {} [--network <mainnet|testnet>] [--connect <multiaddr>]... [--no-fallback] [--data-dir <path>] [--listen-port <port>] [--show-address] [--send <receiver_hex> <amount> [fee]]... [--mint <address_hex> <amount>]... [--faucet <address_hex> [amount]]... [--faucet-amount <n>] [--faucet-cap <n>]",
                    args[0]
                );
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let data_dir = data_dir.unwrap_or_else(|| default_data_dir(network));
    let password = take_password_from_env();

    if show_only_address {
        match crypto::show_addresses_d1_d2(&data_dir, &password) {
            Some((addr_d1, addr_d2)) => {
                drop(password);
                if legacy_d1 {
                    println!("{}", addr_d1.to_hex());
                } else {
                    println!("{}", addr_d2.to_hex());
                    eprintln!(
                        "ℹ️  Dirección D2a (default): {}  |  D1 (legado --legacy-d1): {}",
                        addr_d2.to_hex(),
                        addr_d1.to_hex()
                    );
                }
            }
            None => {
                drop(password);
                eprintln!(
                    "No se encontró wallet en {} o contraseña incorrecta.",
                    data_dir.display()
                );
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    println!("💎 Iniciando Ledgerita Protocol Core...");
    println!("📂 Directorio de datos: {}", data_dir.display());
    println!("🌐 Red: {}", network);

    // Crear el directorio ANTES de load/create (la load de wallet.key falla si el padre no existe).
    std::fs::create_dir_all(&data_dir)?;

    let (signing_key, libp2p_key) = crypto::load_or_create_wallet(&data_dir, &password)?;
    drop(password);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let addr_d1 = Address::from_signing_key(&signing_key);
        let addr_d2 = Address::from_signing_key_d2(&signing_key);
        let local_peer_id = libp2p::PeerId::from(libp2p_key.public());
        println!("🏦 Dirección Wallet (D2a): {}", addr_d2.to_hex());
        println!("🏦 Dirección Legado (D1):  {}", addr_d1.to_hex());

        if init_wallet {
            println!(
                "💎 Wallet inicializada y lista. (No se corre el nodo; se puede desbloquear en la wallet-node o abrir con --show-address.)"
            );
            return Ok(());
        }

        let storage = Arc::new(Storage::open(&data_dir.join("state"))?);
        let state = Arc::new(Mutex::new(State::load(&storage)?));
        {
            let mut s = state.lock().await;
            if genesis::apply_genesis(&mut s, network) {
                println!("🌱 Génesis de {} aplicado (data-dir nuevo).", network);
                if let Err(e) = s.flush(&storage) {
                    eprintln!("⚠️  Error persistiendo génesis: {:?}", e);
                }
            }
        }
        let wallet_address = {
            let s = state.lock().await;
            println!("💾 Estado cargado desde disco. {} cuentas en memoria.", s.balances.len());
            if legacy_d1 {
                addr_d1
            } else {
                s.resolve_account_d1_d2(&addr_d1, &addr_d2).0
            }
        };

    let mempool: Mempool = Arc::new(Mutex::new(std::collections::HashSet::new()));
    let pending_broadcast: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));

    let faucet_key = faucet_key_file.as_ref().map(|path| {
        if network != NetworkId::Testnet {
            eprintln!("Error: --faucet-key-file solo está disponible en TestNet.");
            std::process::exit(1);
        }
        load_signing_key_file(path, "--faucet-key-file")
    });

    // Apply --mint flags (creates LIT from the genesis treasury)
    if !pending_mints.is_empty() {
        // Deuda 3b (diseño aprobado 2026-11-07): en MainNet la emisión es
        // estado-derived — el goteo del tesoro virtual es la ÚNICA entrada
        // de LIT nuevos. --mint queda como bootstrap de TestNet nomás.
        if network != NetworkId::Testnet {
            eprintln!(
                "Error: --mint en MainNet está desactivado (deuda 3b): la emisión de MainNet es estado-derived (goteo del tesoro), el fondo inicial de laboratorio ya se estableció."
            );
            eprintln!("       --mint sigue disponible en TestNet (--network testnet).");
            std::process::exit(1);
        }
        if validators_file.is_none() {
            eprintln!(
                "⚠️  Aviso: --mint modifica saldos locales sin TX en epoch_txs (solo válido en bootstrap/sandbox TestNet)."
            );
        }
        let mut s = state.lock().await;
        for (hex, amount) in &pending_mints {
            let addr = Address::from_hex(hex).unwrap_or_else(|| {
                eprintln!("Error: dirección inválida para --mint: {}", hex);
                std::process::exit(1);
            });
            let new_emitido = s
                .emitido_total
                .checked_add(*amount)
                .filter(|&e| e <= HARD_CAP_SAT)
                .unwrap_or_else(|| {
                    eprintln!("Error en --mint: excedería límite de emisión HARD_CAP_SAT (21M LIT)");
                    std::process::exit(1);
                });
            if s
                .checked_total_supply()
                .and_then(|tot| tot.checked_add(*amount))
                .is_none_or(|tot| tot > HARD_CAP_SAT)
            {
                eprintln!("Error en --mint: excedería suministro máximo HARD_CAP_SAT (21M LIT)");
                std::process::exit(1);
            }
            if let Err(e) = s.checked_mint(addr.clone(), *amount) {
                eprintln!("Error en --mint: {}", e);
                std::process::exit(1);
            }
            s.emitido_total = new_emitido;
            s.tx_count = s.tx_count.saturating_add(1);
            s.last_undo = None;
            if s.is_checkpoint_due() {
                s.finalize_checkpoint();
            }
            println!(
                "🪙 Mint creado: {} → {} LIT (tx_count={})",
                short_hex(&addr.to_hex()),
                format_lit(*amount),
                s.tx_count
            );
        }
        if let Err(e) = s.flush(&storage) {
            eprintln!("⚠️  Error persistiendo mint: {:?}", e);
        }
        println!("💾 Estado persistido después de mint.");
        drop(s);
    }

    // Apply --faucet flags (TestNet only, con TX firmada bajo BFT g7 o sandbox --validators-file)
    if !pending_faucets.is_empty() {
        if network != NetworkId::Testnet {
            eprintln!("Error: --faucet solo está disponible en TestNet. Usa --network testnet.");
            std::process::exit(1);
        }
        if faucet_key.is_none() && validators_file.is_none() {
            eprintln!(
                "❌ Error: TestNet tiene Gate g7 activo por defecto; --faucet sin --faucet-key-file"
            );
            eprintln!(
                "   generaría un estado sin diario firmado y sería rechazado por la red."
            );
            eprintln!(
                "   Usa --faucet-key-file <seed> (TX firmada verificable) o --validators-file para sandbox."
            );
            std::process::exit(1);
        }
        let mut s = state.lock().await;
        for (hex, amount) in &pending_faucets {
            let addr = Address::from_hex(hex).unwrap_or_else(|| {
                eprintln!("Error: dirección inválida para --faucet: {}", hex);
                std::process::exit(1);
            });
            if let Some(ref fsk) = faucet_key {
                match s.apply_faucet_tx(&storage, network, fsk, &addr, *amount, faucet_cap) {
                    Ok((res, tx)) => {
                        mempool.lock().await.insert(hash_tx(&tx));
                        pending_broadcast.lock().await.push(encode_transaction(&tx));
                        println!(
                            "💧 Faucet TX firmada: {} LIT → {} (total dirección: {} LIT, tope: {} LIT)",
                            format_lit(res.minted),
                            short_hex(&addr.to_hex()),
                            format_lit(res.total_given),
                            format_lit(faucet_cap),
                        );
                    }
                    Err(e) => {
                        eprintln!("❌ Faucet TX rechazado: {}", e);
                        std::process::exit(1);
                    }
                }
            } else {
                match s.apply_faucet(&storage, network, &addr, *amount, faucet_cap) {
                    Ok(res) => {
                        println!(
                            "💧 Faucet (sandbox): {} LIT → {} (total dirección: {} LIT, tope: {} LIT)",
                            format_lit(res.minted),
                            short_hex(&addr.to_hex()),
                            format_lit(res.total_given),
                            format_lit(faucet_cap),
                        );
                    }
                    Err(e) => {
                        eprintln!("❌ Faucet rechazado: {}", e);
                        std::process::exit(1);
                    }
                }
            }
        }
        if let Err(e) = s.flush(&storage) {
            eprintln!("⚠️  Error persistiendo faucet: {:?}", e);
        }
        println!("💾 Estado persistido después del faucet.");
        drop(s);
    }

    // Validar y encolar --send como envío diferido tras completar sync P2P inicial
    let deferred_sends: Vec<DeferredSend> = pending_send
        .into_iter()
        .map(|(hex, amount, fee)| {
            if amount == 0 && fee == 0 {
                eprintln!("Error: --send no permite TX nula (amount=0 y fee=0)");
                std::process::exit(1);
            }
            let receiver = Address::from_hex(&hex).unwrap_or_else(|| {
                eprintln!("Error: dirección de receptor inválida: {}", hex);
                std::process::exit(1);
            });
            println!(
                "📝 TX preparada (esperando sync inicial): → {} por {} LIT (fee: {} LIT)",
                short_hex(&receiver.to_hex()),
                format_lit(amount),
                format_lit(fee),
            );
            DeferredSend {
                signing_key: signing_key.clone(),
                receiver,
                amount,
                fee,
                force_legacy_d1: legacy_d1,
            }
        })
        .collect();
    drop(signing_key);

    if connect_peers.is_empty() && !no_fallback {
        connect_peers = fallback_seeds_for_peer(network, Some(&local_peer_id));
        if !connect_peers.is_empty() {
            println!(
                "🌐 Semillas de fallback automáticas ({}): {} peer(s)",
                network,
                connect_peers.len()
            );
        }
    }

    if connect_peers.is_empty() && deferred_sends.is_empty() {
        println!("ℹ️  Modo semilla: esperando conexiones entrantes...");
        println!("   Otros nodos pueden conectarse usando --connect");
    }

    if !deferred_sends.is_empty() {
        println!(
            "📤 {} transacción(es) --send lista(s) para firmar/enviar tras sync inicial",
            deferred_sends.len()
        );
    }

    if listen_port != 0 {
        println!("📡 Puerto de escucha: {}", listen_port);
    }

    let validator_set = Some(match validators_file.as_ref() {
        Some(path) => {
            let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
                eprintln!("Error leyendo --validators-file {}: {}", path.display(), e);
                std::process::exit(1);
            });
            ValidatorSet::parse_config(&text).unwrap_or_else(|e| {
                eprintln!("Error en --validators-file {}: {}", path.display(), e);
                std::process::exit(1);
            })
        }
        None => default_validator_set(network),
    });
    if let Some(ref vset) = validator_set {
        println!(
            "🛡️  Quórum BFT activo: {}-de-{} validadores",
            vset.threshold,
            vset.validators.len()
        );
    }

    let local_validator = match (validator_id, validator_key_file.as_ref()) {
        (Some(id), Some(path)) => {
            let sk = load_signing_key_file(path, "--validator-key-file");
            if let Some(ref vset) = validator_set {
                let expected = vset.get(id).unwrap_or_else(|| {
                    eprintln!("Error: validator_id {} no está en --validators-file", id);
                    std::process::exit(1);
                });
                if expected.pubkey != sk.verifying_key().to_bytes() {
                    eprintln!(
                        "Error: la clave en {} no coincide con la pubkey del validador {}",
                        path.display(),
                        id
                    );
                    std::process::exit(1);
                }
            }
            println!("✍️  Modo validador activo (id={})", id);
            Some((id, sk))
        }
        (None, None) => None,
        _ => {
            eprintln!(
                "Error: --validator-id y --validator-key-file deben especificarse juntos"
            );
            std::process::exit(1);
        }
    };

    if faucet_key.is_some() {
        println!(
            "💧 Modo servidor Faucet TestNet activo (tope {} LIT por dirección)",
            format_lit(faucet_cap)
        );
    }
    let quorum_cfg = QuorumNodeConfig {
        validator_set,
        local_validator,
        testnet_faucet: faucet_key.map(|k| (k, faucet_cap)),
        deferred_sends,
    };

    run_p2p_node_with_quorum(
        libp2p_key,
        connect_peers,
        state.clone(),
        storage.clone(),
        wallet_address,
        mempool,
        Vec::new(),
        pending_broadcast,
        listen_port,
        network,
        quorum_cfg,
    )
    .await?;

    Ok::<(), Box<dyn std::error::Error>>(())
    })
}

#[cfg(test)]
mod ckpt57_main_tests {
    use super::*;

    #[test]
    fn test_ckpt57_default_data_dir_aisla_testnet_de_mainnet_legado() {
        let tmp = std::env::temp_dir().join("ledgerita_test_ckpt57_data_dir");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join(".ledgerita").join("state")).unwrap();

        let mn = default_data_dir_in(&tmp, NetworkId::Mainnet);
        let tn = default_data_dir_in(&tmp, NetworkId::Testnet);
        assert_eq!(mn, tmp.join(".ledgerita"));
        assert_eq!(tn, tmp.join(".ledgerita").join("testnet"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_ckpt57_validate_password_rechaza_vacia() {
        assert!(validate_password_value("".to_string()).is_err());
        assert!(validate_password_value("   \n\t ".to_string()).is_err());
        let ok = validate_password_value("clave-segura".to_string()).unwrap();
        assert_eq!(ok.as_str(), "clave-segura");
    }
}
