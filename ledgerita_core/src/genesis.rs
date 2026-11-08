//! Génesis de Ledgerita por red.
//!
//! El MainNet nace con un estado vacío: las asignaciones del tesoro las hace
//! el operador con `--mint`. El TestNet nace con un tesoro/faucet
//! pre-financiado con una dirección bien conocida (seed determinista), de modo
//! que la red de prueba se distingue claramente del MainNet y dispone de fondos
//! desde el primer bloque.

use ed25519_dalek::SigningKey;

use crate::crypto::Address;
use crate::network::NetworkId;

/// Monto del tesoro/faucet del TestNet (en satoshis): 1,000,000 LIT.
pub const GENESIS_TESTNET_FAUCET_AMOUNT: u64 = 1_000_000 * 1_000_000;

/// Dirección bien conocida del tesoro/faucet del TestNet.
///
/// Seed de solo ceros (determinista, sin clave privada "conocida"): todos los
/// nodos TestNet derivan la misma dirección de tesoro.
pub fn testnet_faucet_address() -> Address {
    Address::from_signing_key(&SigningKey::from_bytes(&[0u8; 32]))
}

/// Indica si un estado recién cargado debe recibir el génesis de su red.
pub fn should_apply_genesis(state: &crate::state::State) -> bool {
    state.tx_count == 0 && state.epoch == 0 && state.balances.is_empty()
}

/// Deuda 3b — número de cuentas que el GÉNESIS de la red puede crear sin TX.
/// MainNet: 0 (sin génesis). TestNet: 1 (el tesoro del faucet de seed cero).
/// Formaliza la vieja holgura "+1" mágica del gate g5 (cuentas plausibles).
pub fn genesis_accounts(network: NetworkId) -> u64 {
    match network {
        NetworkId::Mainnet => 0,
        NetworkId::Testnet => 1,
    }
}

/// Aplica el génesis de la red al estado, si corresponde.
///
/// Devuelve `true` si el génesis fue aplicado (data-dir nuevo), `false` si el
/// estado ya existía o el MainNet no tiene génesis que aplicar.
pub fn apply_genesis(state: &mut crate::state::State, network: NetworkId) -> bool {
    if !should_apply_genesis(state) {
        return false;
    }
    match network {
        NetworkId::Mainnet => false,
        NetworkId::Testnet => {
            state.mint(testnet_faucet_address(), GENESIS_TESTNET_FAUCET_AMOUNT);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;

    #[test]
    fn test_genesis_testnet_prefunds_faucet() {
        let mut state = State::new();
        assert!(apply_genesis(&mut state, NetworkId::Testnet));
        assert_eq!(
            *state.balances.get(&testnet_faucet_address()).unwrap_or(&0),
            GENESIS_TESTNET_FAUCET_AMOUNT
        );
        assert_eq!(
            state.tx_count, 0,
            "el génesis no debe contar como transacción"
        );
        assert_eq!(state.epoch, 0);
        assert_eq!(state.total_supply(), GENESIS_TESTNET_FAUCET_AMOUNT);
    }

    #[test]
    fn test_genesis_mainnet_stays_empty() {
        let mut state = State::new();
        assert!(!apply_genesis(&mut state, NetworkId::Mainnet));
        assert!(state.balances.is_empty());
    }

    #[test]
    fn test_genesis_not_applied_twice() {
        let mut state = State::new();
        assert!(apply_genesis(&mut state, NetworkId::Testnet));
        assert!(
            !apply_genesis(&mut state, NetworkId::Testnet),
            "un estado con génesis no debe re-aplicarse"
        );
    }

    #[test]
    fn test_genesis_not_applied_to_existing_state() {
        let mut state = State::new();
        state.mint(testnet_faucet_address(), 5);
        assert!(!apply_genesis(&mut state, NetworkId::Testnet));
        assert_eq!(state.total_supply(), 5);
    }

    #[test]
    fn test_genesis_not_applied_when_tx_count_nonzero() {
        let mut state = State::new();
        state.tx_count = 3;
        assert!(!apply_genesis(&mut state, NetworkId::Testnet));
    }

    #[test]
    fn test_genesis_accounts_allowance_g5() {
        // Deuda 3b: formalización de la holgura del gate g5 — MainNet no
        // crea cuentas génesis; TestNet solo la del tesoro del faucet.
        assert_eq!(genesis_accounts(NetworkId::Mainnet), 0);
        assert_eq!(genesis_accounts(NetworkId::Testnet), 1);
    }
}
