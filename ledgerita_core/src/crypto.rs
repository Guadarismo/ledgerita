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
        Address {
            key: key.verifying_key(),
        }
    }

    /// Parses an address from its hex representation
    pub fn from_hex(hex: &str) -> Option<Self> {
        let bytes = hex_decode(hex)?;
        if bytes.len() != 32 {
            return None;
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        let key = VerifyingKey::from_bytes(&arr).ok()?;
        Some(Address { key })
    }

    /// Encodes the address as a hex string for display / transport
    pub fn to_hex(&self) -> String {
        self.key
            .as_bytes()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect()
    }

    /// Verifies a cryptographically signed message ("Prueba de Fórmula" fundamental).
    /// If the signature doesn't match the message and the public key, it returns false.
    pub fn verify_signature(&self, message: &[u8], signature: &Signature) -> bool {
        self.key.verify(message, signature).is_ok()
    }
}

// ---------------------------------------------------------------------------
// Wallet persistence: 32-byte seed stored encrypted in ~/.ledgerita/wallet.key
// ---------------------------------------------------------------------------

fn encrypt_seed(seed: &[u8; 32], password: &str) -> std::io::Result<String> {
    use orion::aead;
    use orion::kdf;

    let pwd = kdf::Password::from_slice(password.as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let salt =
        kdf::Salt::generate(16).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let key = kdf::derive_key(&pwd, &salt, 3, 65536, 32)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let ciphertext =
        aead::seal(&key, seed).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    Ok(format!(
        "{}:{}",
        hex_encode(salt.as_ref()),
        hex_encode(ciphertext.as_ref())
    ))
}

fn decrypt_seed(encoded: &str, password: &str) -> std::io::Result<[u8; 32]> {
    use orion::aead;
    use orion::kdf;

    let parts: Vec<&str> = encoded.splitn(2, ':').collect();
    if parts.len() != 2 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Invalid encrypted wallet format",
        ));
    }

    let salt_bytes = hex_decode(parts[0])
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid salt hex"))?;
    let ciphertext = hex_decode(parts[1]).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid ciphertext hex")
    })?;

    let salt = kdf::Salt::from_slice(&salt_bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let pwd = kdf::Password::from_slice(password.as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let key = kdf::derive_key(&pwd, &salt, 3, 65536, 32)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let plaintext = aead::open(&key, &ciphertext).map_err(|_e| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Wrong password or corrupted wallet",
        )
    })?;

    let mut seed = [0u8; 32];
    seed.copy_from_slice(&plaintext);
    Ok(seed)
}

/// Loads a wallet seed from disk, or creates a new one if none exists.
/// The seed is encrypted with the given password when stored.
/// Returns `(SigningKey, libp2p::identity::Keypair)` derived from the same seed,
/// so that the wallet address and the P2P node identity are the same key.
pub fn load_or_create_wallet(
    data_dir: &Path,
    password: &str,
) -> std::io::Result<(SigningKey, libp2p::identity::Keypair)> {
    let key_path = data_dir.join("wallet.key");

    if key_path.exists() {
        let content = std::fs::read_to_string(&key_path)?;
        let content = content.trim().to_string();

        // Detect format: encrypted (has ':') vs legacy plain hex
        let seed = if content.contains(':') {
            decrypt_seed(&content, password)?
        } else {
            // Legacy plaintext format — upgrade to encrypted
            let seed_bytes = hex_decode(&content).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "wallet.key: invalid hex")
            })?;
            if seed_bytes.len() != 32 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "wallet.key must be 32 bytes (64 hex chars)",
                ));
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&seed_bytes);
            // Re-encrypt and save in new format
            let encrypted = encrypt_seed(&arr, password)?;
            write_wallet_key(&key_path, &encrypted)?;
            println!("🔒 Wallet existente migrada a formato encriptado.");
            arr
        };

        derive_from_seed(&seed)
    } else {
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        std::fs::create_dir_all(data_dir)?;
        let encrypted = encrypt_seed(&seed, password)?;
        write_wallet_key(&key_path, &encrypted)?;

        println!("🔑 Wallet encriptada creada en: {}", key_path.display());
        println!(
            "⚠️  GUARDA ESTE ARCHIVO Y TU CONTRASEÑA. Sin ellos perderás acceso a tus fondos."
        );

        derive_from_seed(&seed)
    }
}

/// Writes the wallet file and restricts permissions to the owner only (0600).
fn write_wallet_key(key_path: &Path, content: &str) -> std::io::Result<()> {
    std::fs::write(key_path, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Returns only the Address derived from an existing wallet (without exposing the private key).
/// Returns `None` if the wallet file does not exist or the password is wrong.
pub fn show_address(data_dir: &Path, password: &str) -> Option<Address> {
    let key_path = data_dir.join("wallet.key");
    if !key_path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(key_path).ok()?;
    let content = content.trim().to_string();

    let seed = if content.contains(':') {
        decrypt_seed(&content, password).ok()?
    } else {
        // Legacy plaintext
        let seed_bytes = hex_decode(&content)?;
        if seed_bytes.len() != 32 {
            return None;
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&seed_bytes);
        arr
    };

    let signing_key = SigningKey::from_bytes(&seed);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let seed = [0xab; 32];
        let password = "test-password-123";

        let encrypted = encrypt_seed(&seed, password).unwrap();
        assert!(encrypted.contains(':'));

        let decrypted = decrypt_seed(&encrypted, password).unwrap();
        assert_eq!(decrypted, seed);
    }

    #[test]
    fn test_wrong_password_fails() {
        let seed = [0x42; 32];
        let password = "correct-password";

        let encrypted = encrypt_seed(&seed, password).unwrap();

        let result = decrypt_seed(&encrypted, "wrong-password");
        assert!(result.is_err());
    }

    #[test]
    fn test_different_seeds_different_output() {
        let seed1 = [0xaa; 32];
        let seed2 = [0xbb; 32];
        let password = "pwd";

        let enc1 = encrypt_seed(&seed1, password).unwrap();
        let enc2 = encrypt_seed(&seed2, password).unwrap();

        assert_ne!(enc1, enc2);
    }

    #[test]
    fn test_same_seed_different_output_different_salt() {
        let seed = [0xcc; 32];
        let password = "pwd";

        // Same seed + same password should produce different ciphertexts
        // due to random salt each time
        let enc1 = encrypt_seed(&seed, password).unwrap();
        let enc2 = encrypt_seed(&seed, password).unwrap();

        assert_ne!(enc1, enc2);
    }

    #[test]
    fn test_wallet_creation_and_loading() {
        let dir = std::env::temp_dir().join(format!("ledgerita_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let password = "wallet-test-456";

        // Create wallet
        let (sk1, _kp1) = load_or_create_wallet(&dir, password).unwrap();
        let addr1 = Address::from_signing_key(&sk1);

        // Load existing wallet
        let (sk2, _kp2) = load_or_create_wallet(&dir, password).unwrap();
        let addr2 = Address::from_signing_key(&sk2);

        assert_eq!(addr1.to_hex(), addr2.to_hex());
        assert_eq!(sk1.to_bytes(), sk2.to_bytes());

        // Show address should work
        let shown = show_address(&dir, password).unwrap();
        assert_eq!(shown.to_hex(), addr1.to_hex());

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_legacy_plaintext_upgrade() {
        let dir = std::env::temp_dir().join(format!("ledgerita_legacy_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let password = "migrate-pwd";

        // Write legacy plaintext wallet
        let legacy_hex = "0101010101010101010101010101010101010101010101010101010101010101";
        std::fs::write(dir.join("wallet.key"), legacy_hex).unwrap();

        // Load should work (migrates to encrypted format)
        let (sk, _kp) = load_or_create_wallet(&dir, password).unwrap();
        let addr = Address::from_signing_key(&sk);

        // File should now be in encrypted format
        let content = std::fs::read_to_string(dir.join("wallet.key")).unwrap();
        assert!(
            content.contains(':'),
            "Wallet should be migrated to encrypted format"
        );

        // Loading again should work
        let (sk2, _kp2) = load_or_create_wallet(&dir, password).unwrap();
        assert_eq!(sk.to_bytes(), sk2.to_bytes());

        // Show address should work
        let shown = show_address(&dir, password).unwrap();
        assert_eq!(shown.to_hex(), addr.to_hex());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_wallet_wrong_password_fails() {
        let dir = std::env::temp_dir().join(format!("ledgerita_wrong_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // Create wallet with first password
        let _ = load_or_create_wallet(&dir, "correct-password").unwrap();

        // Try to load with wrong password
        let result = load_or_create_wallet(&dir, "wrong-password");
        assert!(result.is_err());

        // Show address with wrong password
        let shown = show_address(&dir, "wrong-password");
        assert!(shown.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
