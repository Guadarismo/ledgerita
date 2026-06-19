use std::path::PathBuf;
use ledgerita_core::state::State;
use ledgerita_core::storage::Storage;

const HARD_CAP: u64 = 21_000_000;

fn default_data_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".ledgerita")
}

fn format_amount(amount: u64) -> String {
    let integer = amount / 1_000_000;
    let fraction = amount % 1_000_000;
    format!("{}.{:06}", integer, fraction)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut data_dir = default_data_dir();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--data-dir" | "-d" => {
                i += 1;
                if i < args.len() {
                    data_dir = PathBuf::from(&args[i]);
                }
            }
            "--help" | "-h" => {
                println!("Uso: monitor [--data-dir <path>]");
                println!("  --data-dir, -d  Directorio de datos del nodo (default: ~/.ledgerita)");
                return;
            }
            other => {
                eprintln!("Argumento desconocido: {}", other);
                eprintln!("Uso: monitor [--data-dir <path>]");
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let state_dir = data_dir.join("state");

    let storage = match Storage::open(&state_dir) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("{:?}", e);
            if msg.contains("could not acquire lock") || msg.contains("Resource temporarily unavailable") {
                eprintln!("Error: el nodo está ejecutándose. Detén el nodo antes de ejecutar el monitor.");
                eprintln!("       Base de datos: {}", state_dir.display());
            } else {
                eprintln!(
                    "Error al abrir la base de datos en {}: {:?}",
                    state_dir.display(),
                    e
                );
            }
            std::process::exit(1);
        }
    };

    let state = match State::load(&storage) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error al cargar el estado: {:?}", e);
            std::process::exit(1);
        }
    };

    let supply = state.total_supply();
    let remaining = HARD_CAP.saturating_sub(supply);
    let account_count = state.balances.len();

    println!("===========================================");
    println!(" 🔍  Ledgerita State Monitor");
    println!("===========================================");
    println!();
    println!("📊  Supply");
    println!(
        "     Circulante  : {:>15} LIT",
        format_amount(supply)
    );
    println!(
        "     Hard Cap    : {:>15} LIT",
        format_amount(HARD_CAP)
    );
    println!(
        "     Restante    : {:>15} LIT",
        format_amount(remaining)
    );

    if supply > HARD_CAP {
        println!("     🚨  ALERTA: INFLACIÓN DETECTADA — ¡Regla de 21M rota!");
    } else {
        println!("     ✅  Saludable — Reglas económicas intactas");
    }
    println!();
    println!("📦  Estado");
    println!("     Época      : {}", state.epoch);
    println!("     TX Count   : {}", state.tx_count);
    println!("     Fee Pool   : {} LIT", state.fee_pool);
    println!("     Cuentas    : {}", account_count);
    println!();

    if !state.balances.is_empty() {
        println!("🏆  Top 10 cuentas por balance");
        let mut entries: Vec<_> = state.balances.iter().collect();
        entries.sort_by(|a, b| b.1.cmp(a.1));
        let top_n = entries.len().min(10);
        for (i, (addr, bal)) in entries.iter().take(top_n).enumerate() {
            println!(
                "     {:2}.  {}  {:>15} LIT",
                i + 1,
                &addr.to_hex()[..16],
                format_amount(**bal)
            );
        }
        println!();
    }

    println!("===========================================");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_amount() {
        assert_eq!(format_amount(0), "0.000000");
        assert_eq!(format_amount(1_000_000), "1.000000");
        assert_eq!(format_amount(21_000_000), "21.000000");
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
}
