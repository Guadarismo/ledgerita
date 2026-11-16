use crate::crypto::Address;
use ed25519_dalek::{Signature, VerifyingKey};

/// Represents a single transfer of Ledgerita (LIT) from one account to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    pub sender: Address,
    pub receiver: Address,
    pub amount: u64, // In Satoshis of Ledgerita
    pub fee: u64,
    pub nonce: u64,                   // Prevents replay attacks
    /// Identificador de red (`chain_id`) incluido en el dominio firmado (Ckpt 48).
    pub chain_id: u8,
    pub signature: Option<Signature>, // None if unsigned
    /// Era D2 (`DISENO_DIRECCION_HASH.md`): clave pública gastadora (32 B) cuando
    /// `sender == SHA-256(pubkey)`. `None` en transacciones D1 legado.
    pub spender_pubkey: Option<[u8; 32]>,
}

impl Transaction {
    /// Creates a new, unsigned D1 transaction (default `Mainnet` chain_id = 1).
    pub fn new(sender: Address, receiver: Address, amount: u64, fee: u64, nonce: u64) -> Self {
        Transaction {
            sender,
            receiver,
            amount,
            fee,
            nonce,
            chain_id: crate::network::NetworkId::Mainnet.chain_id(),
            signature: None,
            spender_pubkey: None,
        }
    }

    /// Creates a new, unsigned D2 transaction where `sender = SHA-256(sender_pubkey)`
    /// and `spender_pubkey` travels with the spending transaction.
    pub fn new_d2(
        sender_pubkey: &VerifyingKey,
        receiver: Address,
        amount: u64,
        fee: u64,
        nonce: u64,
    ) -> Self {
        Transaction {
            sender: Address::from_pubkey_hash(sender_pubkey),
            receiver,
            amount,
            fee,
            nonce,
            chain_id: crate::network::NetworkId::Mainnet.chain_id(),
            signature: None,
            spender_pubkey: Some(sender_pubkey.to_bytes()),
        }
    }

    /// Fija el `chain_id` de red antes de firmar la transacción.
    pub fn with_chain_id(mut self, chain_id: u8) -> Self {
        self.chain_id = chain_id;
        self
    }

    /// Serializes the core transaction data to bytes for signing/verification
    pub fn to_bytes(&self) -> Vec<u8> {
        // Binary representation for "Prueba de Fórmula" including network domain separation.
        let mut bytes = Vec::with_capacity(32 + 32 + 8 + 8 + 8 + 1);
        bytes.extend_from_slice(self.sender.as_bytes());
        bytes.extend_from_slice(self.receiver.as_bytes());
        bytes.extend_from_slice(&self.amount.to_be_bytes());
        bytes.extend_from_slice(&self.fee.to_be_bytes());
        bytes.extend_from_slice(&self.nonce.to_be_bytes());
        bytes.push(self.chain_id);
        bytes
    }

    /// Attaches a signature to the transaction
    pub fn sign(&mut self, signature: Signature) {
        self.signature = Some(signature);
    }

    /// Verifies that the transaction's signature is valid and matches the sender.
    /// Fase D2a: si `spender_pubkey` está presente, exige `SHA-256(pubkey) == sender`
    /// y firma válida bajo `pubkey`; si es `None`, acepta D1 legado (`sender == pubkey`).
    pub fn is_signature_valid(&self) -> bool {
        let Some(sig) = &self.signature else {
            return false; // Unsigned transactions are mathematically invalid
        };
        let message = self.to_bytes();
        match &self.spender_pubkey {
            Some(pk) => self.sender.verify_signature_with_pubkey(pk, &message, sig),
            None => self.sender.verify_signature(&message, sig),
        }
    }

    /// Verifica firma válida Y que el `chain_id` de la TX coincida con `network`.
    pub fn is_signature_valid_for_network(&self, network: crate::network::NetworkId) -> bool {
        self.chain_id == network.chain_id() && self.is_signature_valid()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn test_valid_transaction_signature() {
        let sender_keypair = SigningKey::from_bytes(&[1u8; 32]);
        let receiver_keypair = SigningKey::from_bytes(&[2u8; 32]);

        let sender_address = Address::new(sender_keypair.verifying_key());
        let receiver_address = Address::new(receiver_keypair.verifying_key());

        let mut tx = Transaction::new(sender_address, receiver_address, 1000, 10, 1);

        // Sign the transaction
        let message = tx.to_bytes();
        let signature = sender_keypair.sign(&message);
        tx.sign(signature);

        assert!(
            tx.is_signature_valid(),
            "Valid signature should pass Proof of Formula"
        );
    }

    #[test]
    fn test_invalid_transaction_signature_altered_amount() {
        let sender_keypair = SigningKey::from_bytes(&[1u8; 32]);
        let receiver_keypair = SigningKey::from_bytes(&[2u8; 32]);

        let sender_address = Address::new(sender_keypair.verifying_key());
        let receiver_address = Address::new(receiver_keypair.verifying_key());

        let mut tx = Transaction::new(sender_address, receiver_address, 1000, 10, 1);

        let message = tx.to_bytes();
        let signature = sender_keypair.sign(&message);
        tx.sign(signature);

        // Hacker tries to alter the amount after it was signed
        tx.amount = 9999;

        assert!(
            !tx.is_signature_valid(),
            "Altered amount should fail Proof of Formula"
        );
    }

    #[test]
    fn test_d2_compromiso_verifica() {
        let sender_kp = SigningKey::from_bytes(&[10u8; 32]);
        let wrong_kp = SigningKey::from_bytes(&[11u8; 32]);
        let receiver_kp = SigningKey::from_bytes(&[12u8; 32]);
        let receiver_d2 = Address::from_signing_key_d2(&receiver_kp);

        let mut tx = Transaction::new_d2(&sender_kp.verifying_key(), receiver_d2, 500, 5, 1);
        let msg = tx.to_bytes();
        tx.sign(sender_kp.sign(&msg));
        assert!(tx.is_signature_valid(), "TX v3 con compromiso SHA-256 válido debe pasar");

        // Atacar cambiando spender_pubkey a otra clave que no corresponde a sender = SHA-256(sender_kp)
        let mut forged = tx.clone();
        forged.spender_pubkey = Some(wrong_kp.verifying_key().to_bytes());
        let forged_msg = forged.to_bytes();
        forged.sign(wrong_kp.sign(&forged_msg));
        assert!(
            !forged.is_signature_valid(),
            "pubkey que no compromete al hash sender debe rechazarse"
        );
    }

    #[test]
    fn test_tx_chain_id_rechaza_replay_entre_redes() {
        use crate::network::NetworkId;
        let sender_kp = SigningKey::from_bytes(&[15u8; 32]);
        let receiver = Address::from_signing_key(&SigningKey::from_bytes(&[16u8; 32]));
        let mut tx = Transaction::new(
            Address::from_signing_key(&sender_kp),
            receiver,
            500,
            1,
            1,
        )
        .with_chain_id(NetworkId::Testnet.chain_id());
        let msg = tx.to_bytes();
        tx.sign(sender_kp.sign(&msg));

        assert!(tx.is_signature_valid_for_network(NetworkId::Testnet));
        assert!(
            !tx.is_signature_valid_for_network(NetworkId::Mainnet),
            "TX firmada para Testnet no debe aceptarse en Mainnet"
        );

        let mut replayed = tx.clone();
        replayed.chain_id = NetworkId::Mainnet.chain_id();
        assert!(
            !replayed.is_signature_valid(),
            "cambiar chain_id sin re-firmar debe invalidar la firma"
        );
    }
}
