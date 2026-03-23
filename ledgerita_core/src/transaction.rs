use crate::crypto::Address;
use ed25519_dalek::Signature;

/// Represents a single transfer of Ledgerita (LIT) from one account to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    pub sender: Address,
    pub receiver: Address,
    pub amount: u64, // In Satoshis of Ledgerita
    pub fee: u64,
    pub nonce: u64, // Prevents replay attacks
    pub signature: Option<Signature>, // None if unsigned
}

impl Transaction {
    /// Creates a new, unsigned transaction
    pub fn new(sender: Address, receiver: Address, amount: u64, fee: u64, nonce: u64) -> Self {
        Transaction {
            sender,
            receiver,
            amount,
            fee,
            nonce,
            signature: None,
        }
    }

    /// Serializes the core transaction data to bytes for signing/verification
    pub fn to_bytes(&self) -> Vec<u8> {
        // A simple binary representation for the "Prueba de Fórmula"
        let mut bytes = Vec::new();
        bytes.extend_from_slice(self.sender.key.as_bytes());
        bytes.extend_from_slice(self.receiver.key.as_bytes());
        bytes.extend_from_slice(&self.amount.to_be_bytes());
        bytes.extend_from_slice(&self.fee.to_be_bytes());
        bytes.extend_from_slice(&self.nonce.to_be_bytes());
        bytes
    }

    /// Attaches a signature to the transaction
    pub fn sign(&mut self, signature: Signature) {
        self.signature = Some(signature);
    }

    /// Verifies that the transaction's signature is valid and matches the sender.
    /// This is the heart of the Zero-Trust algorithmic validation.
    pub fn is_signature_valid(&self) -> bool {
        if let Some(sig) = &self.signature {
            let message = self.to_bytes();
            self.sender.verify_signature(&message, sig)
        } else {
            false // Unsigned transactions are mathematically invalid
        }
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

        assert!(tx.is_signature_valid(), "Valid signature should pass Proof of Formula");
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

        assert!(!tx.is_signature_valid(), "Altered amount should fail Proof of Formula");
    }
}
