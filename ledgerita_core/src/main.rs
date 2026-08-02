use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use ed25519_dalek::Signer;
use ledgerita_core::crypto::{self, Address};
use ledgerita_core::faucet::DEFAULT_FAUCET_AMOUNT;
use ledgerita_core::genesis;
use ledgerita_core::network::{run_p2p_node, Mempool, NetworkId};
use ledgerita_core::state::State;
use ledgerita_core::storage::Storage;
use ledgerita_core::transaction::Transaction;
use tracing_subscriber::EnvFilter;

fn format_lit(amount: u64) -> String {
    let integer = amount / 1_000_000;
    let fraction = amount % 1_000_000;
    format!("{}.{:06}", integer, fraction)
}

fn default_data_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".ledgerita")
}

fn get_password() -> String {
    std::env::var("LEDGERITA_PASSWORD").unwrap_or_else(|_| {
        eprintln!("Error: LEDGERITA_PASSWORD no está configurada.");
        eprintln!("Establece la variable de entorno con la contraseña de tu wallet.");
        eprintln!("Ejemplo (PowerShell):  $env:LEDGERITA_PASSWORD=\"tu-clave-segura\"");
        eprintln!("Ejemplo (Linux):       export LEDGERITA_PASSWORD=\"tu-clave-segura\"");
        std::process::exit(1);
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let args: Vec<String> = std::env::args().collect();

    let mut data_dir = default_data_dir();
    let mut connect_peers = Vec::new();
    let mut show_only_address = false;
    let mut pending_send: Vec<(String, u64, u64)> = Vec::new();
    let mut pending_mints: Vec<(String, u64)> = Vec::new();
    let mut pending_faucets: Vec<(String, u64)> = Vec::new();
    let mut faucet_amount: u64 = DEFAULT_FAUCET_AMOUNT;
    let mut listen_port: u16 = 0;
    let mut network = NetworkId::Mainnet;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--data-dir" | "-d" => {
                i += 1;
                if i < args.len() {
                    data_dir = PathBuf::from(&args[i]);
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
                    eprintln!("Error: red desconocida: {}. Usa mainnet o testnet.", args[i]);
                    std::process::exit(1);
                });
            }
            other => {
                eprintln!("Argumento desconocido: {}", other);
                eprintln!("Uso: {} [--network <mainnet|testnet>] [--connect <multiaddr>]... [--data-dir <path>] [--listen-port <port>] [--show-address] [--send <receiver_hex> <amount> [fee]]... [--mint <address_hex> <amount>]... [--faucet <address_hex> [amount]]... [--faucet-amount <n>]", args[0]);
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let password = get_password();

    if show_only_address {
        match crypto::show_address(&data_dir, &password) {
            Some(addr) => {
                println!("{}", addr.to_hex());
            }
            None => {
                eprintln!("No se encontró wallet en {} o contraseña incorrecta.", data_dir.display());
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    println!("💎 Iniciando Ledgerita Protocol Core...");
    println!("📂 Directorio de datos: {}", data_dir.display());
    println!("🌐 Red: {}", network);

    let (signing_key, libp2p_key) = crypto::load_or_create_wallet(&data_dir, &password)?;
    let wallet_address = Address::from_signing_key(&signing_key);
    println!("🏦 Dirección de Wallet: {}", wallet_address.to_hex());

    std::fs::create_dir_all(&data_dir)?;

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
    println!("💾 Estado cargado desde disco. {} cuentas en memoria.", {
        let s = state.lock().await;
        s.balances.len()
    });

    let mempool: Mempool = Arc::new(Mutex::new(std::collections::HashSet::new()));
    let pending_broadcast: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));

    // Apply --mint flags (creates LIT from the genesis treasury)
    if !pending_mints.is_empty() {
        let mut s = state.lock().await;
        for (hex, amount) in &pending_mints {
            let addr = Address::from_hex(hex).unwrap_or_else(|| {
                eprintln!("Error: dirección inválida para --mint: {}", hex);
                std::process::exit(1);
            });
            s.mint(addr, *amount);
            s.tx_count += 1;
            println!("🪙 Mint creado: {} → {} LIT (tx_count={})", &hex[..16], amount, s.tx_count);
        }
        if let Err(e) = s.flush(&storage) {
            eprintln!("⚠️  Error persistiendo mint: {:?}", e);
        }
        println!("💾 Estado persistido después de mint.");
        drop(s);
    }

    // Apply --faucet flags (TestNet only, mint with per-address cap)
    if !pending_faucets.is_empty() {
        if network != NetworkId::Testnet {
            eprintln!("Error: --faucet solo está disponible en TestNet. Usa --network testnet.");
            std::process::exit(1);
        }
        let mut s = state.lock().await;
        for (hex, amount) in &pending_faucets {
            let addr = Address::from_hex(hex).unwrap_or_else(|| {
                eprintln!("Error: dirección inválida para --faucet: {}", hex);
                std::process::exit(1);
            });
            match s.apply_faucet(&storage, network, &addr, *amount, faucet_amount) {
                Ok(res) => {
                    println!(
                        "💧 Faucet: {} LIT → {} (total por dirección: {} LIT)",
                        format_lit(res.minted),
                        &addr.to_hex()[..16],
                        format_lit(res.total_given),
                    );
                }
                Err(e) => {
                    eprintln!("❌ Faucet rechazado: {}", e);
                    std::process::exit(1);
                }
            }
        }
        if let Err(e) = s.flush(&storage) {
            eprintln!("⚠️  Error persistiendo faucet: {:?}", e);
        }
        println!("💾 Estado persistido después del faucet.");
        drop(s);
    }

    // Build pending transactions from --send flags
    let pending_txs = if !pending_send.is_empty() {
        let s = state.lock().await;
        let base_nonce = s.nonces.get(&wallet_address).copied().unwrap_or(0);
        drop(s);
        pending_send
            .into_iter()
            .enumerate()
            .map(|(i, (hex, amount, fee))| {
                let receiver = Address::from_hex(&hex).unwrap_or_else(|| {
                    eprintln!("Error: dirección de receptor inválida: {}", hex);
                    std::process::exit(1);
                });
                let nonce = base_nonce + i as u64 + 1;
                let mut tx = Transaction::new(wallet_address.clone(), receiver, amount, fee, nonce);
                let msg = tx.to_bytes();
                tx.sign(signing_key.sign(&msg));
                println!("📝 TX preparada: {} → {} por {} LIT (fee: {}, nonce: {})",
                    wallet_address.to_hex()[..16].to_string(),
                    tx.receiver.to_hex()[..16].to_string(),
                    amount,
                    fee,
                    nonce,
                );
                tx
            })
            .collect()
    } else {
        Vec::new()
    };

    if connect_peers.is_empty() && pending_txs.is_empty() {
        println!("ℹ️  Modo semilla: esperando conexiones entrantes...");
        println!("   Otros nodos pueden conectarse usando --connect");
    }

    if !pending_txs.is_empty() {
        println!("📤 {} transacción(es) pendiente(s) por enviar a la red", pending_txs.len());
    }

    if listen_port != 0 {
        println!("📡 Puerto de escucha: {}", listen_port);
    }

    run_p2p_node(
        libp2p_key,
        connect_peers,
        state.clone(),
        storage.clone(),
        wallet_address,
        mempool,
        pending_txs,
        pending_broadcast,
        listen_port,
        network,
    ).await?;

    Ok(())
}
