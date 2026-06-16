use ed25519_dalek::{Signature, SigningKey, Verifier, VerifyingKey};
use std::path::Path;

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

    /// Creates an Address from a SigningKey (convenience)
    pub fn from_signing_key(key: &SigningKey) -> Self {
        Address { key: key.verifying_key() }
    }

    /// Encodes the address as a hex string for display / transport
    pub fn to_hex(&self) -> String {
        self.key.as_bytes().iter().map(|b| format!("{:02x}", b)).collect()
    }

    /// Verifies a cryptographically signed message ("Prueba de Fórmula" fundamental).
    /// If the signature doesn't match the message and the public key, it returns false.
    pub fn verify_signature(&self, message: &[u8], signature: &Signature) -> bool {
        self.key.verify(message, signature).is_ok()
    }
}

// ---------------------------------------------------------------------------
// Wallet persistence: 32-byte seed stored as hex in ~/.ledgerita/wallet.key
// ---------------------------------------------------------------------------

/// Loads a wallet seed from disk, or creates a new one if none exists.
/// Returns `(SigningKey, libp2p::identity::Keypair)` derived from the same seed,
/// so that the wallet address and the P2P node identity are the same key.
pub fn load_or_create_wallet(data_dir: &Path) -> std::io::Result<(SigningKey, libp2p::identity::Keypair)> {
    let key_path = data_dir.join("wallet.key");

    if key_path.exists() {
        let hex_str = std::fs::read_to_string(&key_path)?;
        let hex_str = hex_str.trim();
        let seed = hex_decode(hex_str)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "wallet.key: invalid hex"))?;
        if seed.len() != 32 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "wallet.key must be 32 bytes (64 hex chars)"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&seed);
        derive_from_seed(&arr)
    } else {
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        std::fs::create_dir_all(data_dir)?;
        let hex_str = hex_encode(&seed);
        std::fs::write(&key_path, &hex_str)?;

        println!("🔑 Wallet creada en: {}", key_path.display());
        println!("⚠️  GUARDA ESTE ARCHIVO. Sin él perderás acceso a tus fondos.");

        derive_from_seed(&seed)
    }
}

/// Returns only the Address derived from an existing wallet (without exposing the private key).
/// Returns `None` if the wallet file does not exist.
pub fn show_address(data_dir: &Path) -> Option<Address> {
    let key_path = data_dir.join("wallet.key");
    if !key_path.exists() {
        return None;
    }
    let hex_str = std::fs::read_to_string(key_path).ok()?;
    let hex_str = hex_str.trim();
    let seed = hex_decode(hex_str)?;
    if seed.len() != 32 {
        return None;
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&seed);
    let signing_key = SigningKey::from_bytes(&arr);
    Some(Address::from_signing_key(&signing_key))
}

fn derive_from_seed(seed: &[u8; 32]) -> std::io::Result<(SigningKey, libp2p::identity::Keypair)> {
    let signing_key = SigningKey::from_bytes(seed);
    let libp2p_key = libp2p::identity::Keypair::ed25519_from_bytes(*seed)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    Ok((signing_key, libp2p_key))
}

// ---------------------------------------------------------------------------
// Minimal hex helpers (no extra dependencies)
// ---------------------------------------------------------------------------
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}
