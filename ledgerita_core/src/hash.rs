//! hash — LA ÚNICA PUERTA del algoritmo de hash (deuda 9a, Checkpoint 30).
//!
//! Idea del usuario (Checkpoint 27): estar PREPARADOS para un swap
//! rápido del algoritmo de hash — no migrar hoy. Todo el hashing de
//! consenso pasa por acá: si algún día el algoritmo cambia (SHA-3,
//! BLAKE3…), el cambio vive en ESTE módulo y en la etiqueta
//! `HASH_ALGO_*` que viaja al final del snapshot (v4) — los nodos
//! rechazan roots producidos por un algoritmo que no conocen (U7:
//! pierden vivacidad, jamás corrompen el ledger).
//!
//! La transición futura será una ERA anclada al Cierre ("desde la
//! época N, roots con algoritmo X"), ejecutada con el pipeline
//! DISENO_AUTOUPDATE U1–U9 — misma ley que la emisión: misma época ⇒
//! emisión idéntica Y mismo algoritmo.

use sha2::{Digest, Sha256};

use crate::serialize::encode_transaction;

/// Algoritmo de hash ERA 1: SHA-256 (el que vivió toda la historia).
pub const HASH_ALGO_SHA256: u8 = 1;

/// El algoritmo ACTUAL del nodo (lo que produce roots y validamos).
pub const HASH_ALGO_ACTUAL: u8 = HASH_ALGO_SHA256;

/// La única puerta del hasher de estado: cambiar el algoritmo = cambiar
/// ESTA función (y firmar una nueva era de `HASH_ALGO_*`).
pub fn new_state_hasher() -> Sha256 {
    Sha256::new()
}

/// Hash de una TX (identidad de la transacción). Misma puerta.
pub fn hash_tx(tx: &crate::transaction::Transaction) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(encode_transaction(tx));
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::Transaction;
    use ed25519_dalek::SigningKey;

    #[test]
    fn test_algo_actual_es_sha256_era1() {
        // La era 1 es SHA-256: la etiqueta que viaja en el snapshot v4
        // debe coincidir con lo que produce new_state_hasher.
        assert_eq!(HASH_ALGO_ACTUAL, HASH_ALGO_SHA256);
        assert_eq!(HASH_ALGO_ACTUAL, 1);
    }

    #[test]
    fn test_hash_tx_determinista() {
        let tx = Transaction::new(
            crate::crypto::Address::from_signing_key(&SigningKey::from_bytes(&[0xab; 32])),
            crate::crypto::Address::from_signing_key(&SigningKey::from_bytes(&[0xcd; 32])),
            1000,
            10,
            7,
        );
        let h1 = hash_tx(&tx);
        let h2 = hash_tx(&tx);
        assert_eq!(h1, h2, "hash_tx determinista");
        assert_eq!(h1.len(), 32);
    }

    #[test]
    fn test_hash_tx_distinto_contenido_distinto_hash() {
        let sender = crate::crypto::Address::from_signing_key(&SigningKey::from_bytes(&[0xab; 32]));
        let receiver =
            crate::crypto::Address::from_signing_key(&SigningKey::from_bytes(&[0xcd; 32]));
        let tx1 = Transaction::new(sender.clone(), receiver.clone(), 1000, 10, 7);
        let tx2 = Transaction::new(sender, receiver, 1001, 10, 7);
        assert_ne!(hash_tx(&tx1), hash_tx(&tx2), "contenido distinto ⇒ hash distinto");
    }
}
