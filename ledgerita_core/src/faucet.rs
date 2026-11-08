//! Faucet del TestNet (red de prueba).
//!
//! Reparte LIT de prueba desde el tesoro a una dirección, limitado por:
//! * red: solo disponible en TestNet (nunca en MainNet);
//! * un tope de por vida por dirección (`cap`), persistido en un tree local
//!   (`faucet`) que NO forma parte del snapshot de consenso. El `cap` se pasa
//!   por llamada desde la CLI (`--faucet-cap`); ver [`DEFAULT_FAUCET_CAP`];
//! * el hard cap de 21,000,000 LIT de suministro total.
//!
//! La lógica está aislada en este módulo para poder exponerla después como un
//! servicio (HTTP o mensaje P2P) sin tocar el núcleo del consenso.

use crate::crypto::Address;
use crate::network::NetworkId;
use crate::state::State;
use crate::storage::Storage;

/// Suministro máximo de Ledgerita (en satoshis): 21,000,000 LIT.
pub const HARD_CAP_SAT: u64 = 21_000_000_000_000;

/// Monto por defecto entregado por `--faucet` (en satoshis): 1,000 LIT.
pub const DEFAULT_FAUCET_AMOUNT: u64 = 1_000 * 1_000_000;

/// Tope de por vida por dirección por defecto (en satoshis): 10,000 LIT.
///
/// Es independiente del `--faucet-amount` (monto por entrega). El operador
/// puede ajustarlo con `--faucet-cap <n>`. Semántica nueva (2026-11-05):
/// antes el cap se tomaba igual al monto de la llamada → faucet de un solo
/// uso por dirección; ahora el monto y el tope son parámetros separados.
pub const DEFAULT_FAUCET_CAP: u64 = 10_000 * 1_000_000;

/// Resultado de una entrega del faucet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaucetResult {
    /// Monto entregado en esta llamada (satoshis).
    pub minted: u64,
    /// Total entregado a esa dirección, de por vida, tras esta llamada.
    pub total_given: u64,
}

impl State {
    /// Entrega LIT de prueba a `recipient` (solo TestNet).
    ///
    /// Valida que la red sea TestNet, que el monto sea positivo, que la
    /// dirección no supere su tope de por vida (`cap`) y que el suministro
    /// total no exceda [`HARD_CAP_SAT`]. En éxito, mintea el monto, registra
    /// la entrega en `storage` y devuelve [`FaucetResult`].
    pub fn apply_faucet(
        &mut self,
        storage: &Storage,
        network: NetworkId,
        recipient: &Address,
        amount: u64,
        cap: u64,
    ) -> Result<FaucetResult, &'static str> {
        if network != NetworkId::Testnet {
            return Err("Faucet solo disponible en TestNet");
        }
        if amount == 0 {
            return Err("Monto de faucet debe ser mayor que cero");
        }
        if cap == 0 {
            return Err("Tope de faucet debe ser mayor que cero");
        }

        let given = storage
            .faucet_given(recipient)
            .map_err(|_| "Error de almacenamiento del faucet")?;
        if given.saturating_add(amount) > cap {
            return Err("La dirección alcanzó el límite de por vida del faucet");
        }

        if self.total_supply().saturating_add(amount) > HARD_CAP_SAT {
            return Err("El faucet excedería el suministro máximo de 21,000,000 LIT");
        }

        self.mint(recipient.clone(), amount);
        self.tx_count += 1;
        let total_given = storage
            .add_faucet_given(recipient, amount)
            .map_err(|_| "Error de almacenamiento del faucet")?;

        Ok(FaucetResult {
            minted: amount,
            total_given,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Address;
    use ed25519_dalek::SigningKey;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_CTR: AtomicU64 = AtomicU64::new(0);

    fn test_dir() -> std::path::PathBuf {
        let id = TEST_CTR.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "ledgerita_faucet_test_{}_{}",
            std::process::id(),
            id
        ))
    }

    fn recipient() -> Address {
        Address::from_signing_key(&SigningKey::from_bytes(&[0xfa; 32]))
    }

    fn setup() -> (State, Storage, std::path::PathBuf) {
        let dir = test_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let storage = Storage::open(&dir).unwrap();
        (State::new(), storage, dir)
    }

    #[test]
    fn test_faucet_credits_recipient() {
        let (mut state, storage, dir) = setup();
        let addr = recipient();

        let res = state
            .apply_faucet(
                &storage,
                NetworkId::Testnet,
                &addr,
                DEFAULT_FAUCET_AMOUNT,
                DEFAULT_FAUCET_AMOUNT,
            )
            .unwrap();

        assert_eq!(res.minted, DEFAULT_FAUCET_AMOUNT);
        assert_eq!(res.total_given, DEFAULT_FAUCET_AMOUNT);
        assert_eq!(*state.balances.get(&addr).unwrap(), DEFAULT_FAUCET_AMOUNT);
        assert_eq!(state.tx_count, 1, "el faucet cuenta como transacción");
        assert_eq!(storage.faucet_given(&addr).unwrap(), DEFAULT_FAUCET_AMOUNT);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_faucet_rejects_mainnet() {
        let (mut state, storage, dir) = setup();
        let addr = recipient();

        let err = state
            .apply_faucet(
                &storage,
                NetworkId::Mainnet,
                &addr,
                DEFAULT_FAUCET_AMOUNT,
                DEFAULT_FAUCET_AMOUNT,
            )
            .unwrap_err();
        assert_eq!(err, "Faucet solo disponible en TestNet");
        assert!(state.balances.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_faucet_zero_amount() {
        let (mut state, storage, dir) = setup();
        let addr = recipient();

        assert!(
            state
                .apply_faucet(
                    &storage,
                    NetworkId::Testnet,
                    &addr,
                    0,
                    DEFAULT_FAUCET_AMOUNT
                )
                .is_err()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_faucet_respects_per_address_cap() {
        let (mut state, storage, dir) = setup();
        let addr = recipient();

        state
            .apply_faucet(
                &storage,
                NetworkId::Testnet,
                &addr,
                500_000_000,
                1_000_000_000,
            )
            .unwrap();
        let second = state
            .apply_faucet(
                &storage,
                NetworkId::Testnet,
                &addr,
                500_000_000,
                1_000_000_000,
            )
            .unwrap();
        assert_eq!(second.total_given, 1_000_000_000);

        let third = state
            .apply_faucet(&storage, NetworkId::Testnet, &addr, 1, 1_000_000_000)
            .unwrap_err();
        assert_eq!(
            third,
            "La dirección alcanzó el límite de por vida del faucet"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_faucet_respects_hard_cap() {
        let (mut state, storage, dir) = setup();
        let addr = recipient();
        let treasury = Address::from_signing_key(&SigningKey::from_bytes(&[0xbb; 32]));

        state.mint(treasury, HARD_CAP_SAT - 500_000_000);
        let err = state
            .apply_faucet(
                &storage,
                NetworkId::Testnet,
                &addr,
                1_000_000_000,
                DEFAULT_FAUCET_AMOUNT,
            )
            .unwrap_err();
        assert_eq!(
            err,
            "El faucet excedería el suministro máximo de 21,000,000 LIT"
        );
        assert!(state.balances.get(&addr).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_faucet_persists_across_open() {
        let dir = test_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let addr = recipient();

        {
            let storage = Storage::open(&dir).unwrap();
            let mut state = State::new();
            state
                .apply_faucet(
                    &storage,
                    NetworkId::Testnet,
                    &addr,
                    300_000_000,
                    1_000_000_000,
                )
                .unwrap();
            storage.flush().unwrap();
        }

        {
            let storage = Storage::open(&dir).unwrap();
            assert_eq!(storage.faucet_given(&addr).unwrap(), 300_000_000);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_default_cap_permitting_repeated_drips() {
        // Semántica nueva (2026-11-05): con `DEFAULT_FAUCET_CAP` separado del
        // monto, la misma dirección puede recibir varios goteos hasta el final.
        assert!(
            DEFAULT_FAUCET_CAP >= DEFAULT_FAUCET_AMOUNT,
            "el cap por defecto debe permitir al menos una entrega del monto por defecto"
        );

        let (mut state, storage, dir) = setup();
        let addr = recipient();

        for n in 1..=9 {
            let res = state
                .apply_faucet(
                    &storage,
                    NetworkId::Testnet,
                    &addr,
                    DEFAULT_FAUCET_AMOUNT,
                    DEFAULT_FAUCET_CAP,
                )
                .unwrap();
            assert_eq!(res.total_given, DEFAULT_FAUCET_AMOUNT * n);
        }

        // La 10ª entrega (9,000 + 1,000 = 10,000 LIT) alcanza el tope exacto
        // y por eso SÍ es aceptada (la regla es `> cap`, no `>= cap`).
        let tenth = state
            .apply_faucet(
                &storage,
                NetworkId::Testnet,
                &addr,
                DEFAULT_FAUCET_AMOUNT,
                DEFAULT_FAUCET_CAP,
            )
            .unwrap();
        assert_eq!(tenth.total_given, DEFAULT_FAUCET_CAP);
        // ...pero una 11ª ya lo excedería.
        let err = state
            .apply_faucet(
                &storage,
                NetworkId::Testnet,
                &addr,
                DEFAULT_FAUCET_AMOUNT,
                DEFAULT_FAUCET_CAP,
            )
            .unwrap_err();
        assert_eq!(err, "La dirección alcanzó el límite de por vida del faucet");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
