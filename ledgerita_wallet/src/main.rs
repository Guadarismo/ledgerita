use std::path::PathBuf;

use ed25519_dalek::Signer;
use eframe::egui;
use ledgerita_core::crypto::{self, Address};
use ledgerita_core::faucet::HARD_CAP_SAT;
use ledgerita_core::genesis;
use ledgerita_core::network::NetworkId;
use ledgerita_core::state::State;
use ledgerita_core::storage::Storage;
use ledgerita_core::transaction::Transaction;

const LIT_DECIMALS: u64 = 1_000_000;

fn format_lit(amount: u64) -> String {
    let integer = amount / LIT_DECIMALS;
    let fraction = amount % LIT_DECIMALS;
    format!("{}.{:06}", integer, fraction)
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

fn default_data_dir() -> String {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".ledgerita").to_string_lossy().to_string()
}

enum Screen {
    Unlock,
    Main,
}

enum SendResult {
    None,
    Ok(String),
    Err(String),
}

struct WalletApp {
    data_dir: String,
    network: NetworkId,
    password: String,
    unlock_error: Option<String>,
    screen: Screen,

    address: Option<Address>,
    state: Option<State>,
    storage: Option<Storage>,

    send_receiver: String,
    send_amount: String,
    send_fee: String,
    send_result: SendResult,

    copied: Option<String>,
}

impl WalletApp {
    fn new() -> Self {
        WalletApp {
            data_dir: default_data_dir(),
            network: NetworkId::Testnet,
            password: String::new(),
            unlock_error: None,
            screen: Screen::Unlock,
            address: None,
            state: None,
            storage: None,
            send_receiver: String::new(),
            send_amount: String::new(),
            send_fee: String::new(),
            send_result: SendResult::None,
            copied: None,
        }
    }

    fn unlock(&mut self, ctx: &egui::Context) {
        let result = (|| -> Result<(Address, Storage, State), String> {
            let data_dir = PathBuf::from(&self.data_dir);
            let (_sk, _kp) = crypto::load_or_create_wallet(&data_dir, &self.password)
                .map_err(|e| format!("Wallet: {}", e))?;
            let address = crypto::show_address(&data_dir, &self.password)
                .ok_or_else(|| "No se pudo leer la dirección de la wallet".to_string())?;
            std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
            let storage = Storage::open(&data_dir.join("state")).map_err(|e| e.to_string())?;
            let mut state = State::load(&storage).map_err(|e| e.to_string())?;
            if genesis::apply_genesis(&mut state, self.network) {
                state.flush(&storage).map_err(|e| e.to_string())?;
            }
            Ok((address, storage, state))
        })();

        match result {
            Ok((address, storage, state)) => {
                self.address = Some(address);
                self.storage = Some(storage);
                self.state = Some(state);
                self.unlock_error = None;
                self.screen = Screen::Main;
            }
            Err(e) => {
                self.unlock_error = Some(e);
            }
        }
        ctx.request_repaint();
    }

    fn do_send(&mut self, ctx: &egui::Context) {
        let (addr, storage, mut state) = match (&self.address, &self.storage, &self.state) {
            (Some(a), Some(s), Some(st)) => (a.clone(), s, st.clone()),
            _ => return,
        };
        let result = (|| -> Result<String, String> {
            let receiver = Address::from_hex(&self.send_receiver.trim())
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
            let nonce = state.nonces.get(&addr).copied().unwrap_or(0) + 1;
            let mut tx = Transaction::new(addr.clone(), receiver.clone(), amount, fee, nonce);
            let msg = tx.to_bytes();
            let data_dir = PathBuf::from(&self.data_dir);
            let sk = crypto::load_or_create_wallet(&data_dir, &self.password)
                .map_err(|e| format!("Wallet: {}", e))?
                .0;
            tx.sign(sk.sign(&msg));
            state.apply_transaction(&tx).map_err(|e| e.to_string())?;
            state.flush(&storage).map_err(|e| e.to_string())?;
            Ok(format!(
                "TX nonce {} aplicada localmente: {} → {} por {} LIT (fee {})",
                nonce,
                &addr.to_hex()[..12],
                &receiver.to_hex()[..12],
                format_lit(amount),
                format_lit(fee),
            ))
        })();

        match result {
            Ok(msg) => {
                self.send_result = SendResult::Ok(msg);
                self.state = Some(state);
            }
            Err(e) => {
                self.send_result = SendResult::Err(e);
            }
        }
        ctx.request_repaint();
    }

    fn copy_address(&mut self, ctx: &egui::Context) {
        if let Some(addr) = &self.address {
            ctx.copy_text(addr.to_hex());
            self.copied = Some("✓ Dirección copiada".to_string());
        }
    }

    fn draw_unlock(&mut self, ui: &mut egui::Ui) {
        ui.add_space(16.0);
        ui.vertical_centered(|ui| {
            ui.heading("💎 Ledgerita Wallet");
            ui.label("Protocolo de Prueba de Fórmula");
        });
        ui.add_space(24.0);

        egui::Grid::new("unlock_grid")
            .num_columns(2)
            .spacing([8.0, 8.0])
            .show(ui, |ui| {
                ui.label("Red:");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.network, NetworkId::Mainnet, "MainNet");
                    ui.radio_value(&mut self.network, NetworkId::Testnet, "TestNet");
                });
                ui.end_row();

                ui.label("Directorio:");
                ui.add(egui::TextEdit::singleline(&mut self.data_dir).desired_width(280.0));
                ui.end_row();

                ui.label("Contraseña:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.password)
                        .password(true)
                        .desired_width(280.0),
                );
                ui.end_row();
            });

        ui.add_space(16.0);
        if ui.button("🔓 Desbloquear / Crear").clicked() {
            self.unlock(ui.ctx());
        }
        if let Some(err) = &self.unlock_error {
            ui.colored_label(egui::Color32::RED, format!("❌ {}", err));
        }
        ui.add_space(12.0);
        ui.separator();
        ui.small("⚠️  Modo demostración: esto NO es dinero real. La wallet se guarda encriptada en el directorio indicado.");
    }

    fn draw_main(&mut self, ui: &mut egui::Ui) {
        let addr = match &self.address {
            Some(a) => a.clone(),
            None => return,
        };
        let state = match &self.state {
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

        ui.add_space(12.0);
        if ui.button("📋 Copiar dirección").clicked() {
            self.copy_address(ui.ctx());
        }
        if let Some(c) = &self.copied {
            ui.colored_label(egui::Color32::LIGHT_GREEN, c);
        }

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);

        ui.heading("📤 Enviar LIT");
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
        if ui.button("💸 Firmar y enviar").clicked() {
            self.do_send(ui.ctx());
        }
        match &self.send_result {
            SendResult::None => {}
            SendResult::Ok(msg) => {
                ui.colored_label(egui::Color32::LIGHT_GREEN, format!("✅ {}", msg));
            }
            SendResult::Err(e) => {
                ui.colored_label(egui::Color32::RED, format!("❌ {}", e));
            }
        }

        ui.add_space(8.0);
        ui.small("Nota: la TX se aplica y persiste en el estado local. La difusión P2P se habilitará en la Fase 2.");
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
            .with_inner_size([520.0, 620.0])
            .with_min_inner_size([420.0, 480.0]),
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
}
