use ed25519_dalek::{Signature, Verifier, VerifyingKey};

/// An Account in Ledgerita is identified by its ed25519 public key.
#[derive(Debug, Clone, Eq)]
pub struct Address {
    pub key: VerifyingKey,
}

impl PartialEq for Address {
    fn eq(&self, other: &Self) -> bool {
        self.key.as_bytes() == other.key.as_bytes()
    }
}

impl std::cmp::PartialOrd for Address {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::cmp::Ord for Address {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key.as_bytes().cmp(other.key.as_bytes())
    }
}

impl std::hash::Hash for Address {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.key.as_bytes().hash(state);
    }
}

impl Address {
    /// Creates a new Address from an existing public key
    pub fn new(key: VerifyingKey) -> Self {
        Address { key }
    }

    /// Verifies a cryptographically signed message ("Prueba de Fórmula" fundamental).
    /// If the signature doesn't match the message and the public key, it returns false.
    pub fn verify_signature(&self, message: &[u8], signature: &Signature) -> bool {
        self.key.verify(message, signature).is_ok()
    }
}
