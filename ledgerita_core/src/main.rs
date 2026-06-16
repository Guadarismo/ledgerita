use std::path::PathBuf;
use ledgerita_core::crypto::{self, Address};
use ledgerita_core::network::run_p2p_node;

fn default_data_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".ledgerita")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    let mut data_dir = default_data_dir();
    let mut connect_peers = Vec::new();
    let mut show_only_address = false;

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
            other => {
                eprintln!("Argumento desconocido: {}", other);
                eprintln!("Uso: {} [--connect <multiaddr>]... [--data-dir <path>] [--show-address]", args[0]);
                std::process::exit(1);
            }
        }
        i += 1;
    }

    // --show-address: solo imprime la dirección pública, sin iniciar el nodo
    if show_only_address {
        match crypto::show_address(&data_dir) {
            Some(addr) => {
                println!("{}", addr.to_hex());
            }
            None => {
                eprintln!("No se encontró wallet en {}. Crea una ejecutando el nodo sin --show-address.", data_dir.display());
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    println!("💎 Iniciando Ledgerita Protocol Core...");
    println!("📂 Directorio de datos: {}", data_dir.display());

    // Cargar o crear wallet persistente
    let (_signing_key, libp2p_key) = crypto::load_or_create_wallet(&data_dir)?;
    let wallet_address = Address::from_signing_key(&_signing_key);
    println!("🏦 Dirección de Wallet: {}", wallet_address.to_hex());

    if connect_peers.is_empty() {
        println!("ℹ️  Modo semilla: esperando conexiones entrantes...");
        println!("   Otros nodos pueden conectarse usando --connect");
    }

    // Iniciar el nodo P2P con identidad persistente
    run_p2p_node(libp2p_key, connect_peers).await?;

    Ok(())
}
