use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use std::path::Path;
use zeroize::{Zeroize, Zeroizing};

/// Una cuenta en Ledgerita identificada por 32 bytes:
/// - Era D1 (legado): la clave pública ed25519 (32 bytes).
/// - Era D2 (`DISENO_DIRECCION_HASH.md`): `SHA-256(pubkey)` (32 bytes opacos).
#[derive(Debug, Clone, Eq)]
pub struct Address {
    bytes: [u8; 32],
}

impl PartialEq for Address {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

impl std::cmp::PartialOrd for Address {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::cmp::Ord for Address {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.bytes.cmp(&other.bytes)
    }
}

impl std::hash::Hash for Address {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.bytes.hash(state);
    }
}

impl Address {
    /// Creates a new Address from an existing public key (D1 legacy).
    pub fn new(key: VerifyingKey) -> Self {
        Address {
            bytes: key.to_bytes(),
        }
    }

    /// Creates a new Address from raw 32 bytes (D2 opaque hash or D1 pubkey).
    pub fn from_hash_bytes(bytes: [u8; 32]) -> Self {
        Address { bytes }
    }

    /// Derives a D2 Address = `SHA-256(pubkey)` from a `VerifyingKey`.
    pub fn from_pubkey_hash(key: &VerifyingKey) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(key.as_bytes());
        Address {
            bytes: hasher.finalize().into(),
        }
    }

    /// Creates a D1 Address from a SigningKey (convenience, backward-compatible).
    pub fn from_signing_key(key: &SigningKey) -> Self {
        Address {
            bytes: key.verifying_key().to_bytes(),
        }
    }

    /// Creates a D2 Address (`SHA-256(pubkey)`) from a SigningKey.
    pub fn from_signing_key_d2(key: &SigningKey) -> Self {
        Self::from_pubkey_hash(&key.verifying_key())
    }

    /// Returns the 32-byte representation of this Address.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    /// Parses an address from its 64-char hex representation (D1 or D2).
    pub fn from_hex(hex: &str) -> Option<Self> {
        let bytes = hex_decode(hex)?;
        if bytes.len() != 32 {
            return None;
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Some(Address { bytes: arr })
    }

    /// Encodes the address as a hex string for display / transport
    pub fn to_hex(&self) -> String {
        self.bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }

    /// Verifies a cryptographically signed message for D1 addresses (address == pubkey).
    pub fn verify_signature(&self, message: &[u8], signature: &Signature) -> bool {
        let Ok(key) = VerifyingKey::from_bytes(&self.bytes) else {
            return false;
        };
        key.verify_strict(message, signature).is_ok()
    }

    /// Verifies a D2 commitment (`SHA-256(pubkey_bytes) == self.bytes`) and the
    /// ed25519 signature under `pubkey_bytes` (`DISENO_DIRECCION_HASH.md`).
    pub fn verify_signature_with_pubkey(
        &self,
        pubkey_bytes: &[u8; 32],
        message: &[u8],
        signature: &Signature,
    ) -> bool {
        let mut hasher = Sha256::new();
        hasher.update(pubkey_bytes);
        let commitment: [u8; 32] = hasher.finalize().into();
        if commitment != self.bytes {
            return false;
        }
        let Ok(key) = VerifyingKey::from_bytes(pubkey_bytes) else {
            return false;
        };
        key.verify_strict(message, signature).is_ok()
    }
}

// ---------------------------------------------------------------------------
// Wallet persistence: 32-byte seed stored encrypted in ~/.ledgerita/wallet.key
// ---------------------------------------------------------------------------

fn encrypt_seed(seed: &[u8; 32], password: &str) -> std::io::Result<String> {
    use orion::aead;
    use orion::kdf;

    let pwd_bytes = Zeroizing::new(password.as_bytes().to_vec());
    let pwd = kdf::Password::from_slice(&pwd_bytes)
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

fn decrypt_seed(encoded: &str, password: &str) -> std::io::Result<Zeroizing<[u8; 32]>> {
    use orion::aead;
    use orion::kdf;

    let parts: Vec<&str> = encoded.splitn(2, ':').collect();
    if parts.len() != 2 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Invalid encrypted wallet format",
        ));
    }

    let salt_bytes = Zeroizing::new(hex_decode(parts[0])
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid salt hex"))?);
    let ciphertext = Zeroizing::new(hex_decode(parts[1]).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid ciphertext hex")
    })?);

    let salt = kdf::Salt::from_slice(&salt_bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let pwd_bytes = Zeroizing::new(password.as_bytes().to_vec());
    let pwd = kdf::Password::from_slice(&pwd_bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let key = kdf::derive_key(&pwd, &salt, 3, 65536, 32)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let plaintext = aead::open(&key, &ciphertext).map_err(|_e| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Wrong password or corrupted wallet",
        )
    })?;

    let mut seed = Zeroizing::new([0u8; 32]);
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
            let seed_bytes = Zeroizing::new(hex_decode(&content).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "wallet.key: invalid hex")
            })?);
            if seed_bytes.len() != 32 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "wallet.key must be 32 bytes (64 hex chars)",
                ));
            }
            let mut arr = Zeroizing::new([0u8; 32]);
            arr.copy_from_slice(&seed_bytes);
            // Re-encrypt and save in new format
            let encrypted = encrypt_seed(&arr, password)?;
            write_wallet_key(&key_path, &encrypted)?;
            println!("🔒 Wallet existente migrada a formato encriptado.");
            arr
        };

        derive_from_seed(&seed)
    } else {
        let mut seed = Zeroizing::new([0u8; 32]);
        getrandom::getrandom(&mut *seed)
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
        let seed_bytes = Zeroizing::new(hex_decode(&content)?);
        if seed_bytes.len() != 32 {
            return None;
        }
        let mut arr = Zeroizing::new([0u8; 32]);
        arr.copy_from_slice(&seed_bytes);
        arr
    };

    let signing_key = SigningKey::from_bytes(&seed);
    Some(Address::from_signing_key(&signing_key))
}

fn derive_from_seed(seed: &[u8; 32]) -> std::io::Result<(SigningKey, libp2p::identity::Keypair)> {
    let signing_key = SigningKey::from_bytes(seed);
    let mut seed_bytes = *seed;
    let libp2p_key = libp2p::identity::Keypair::ed25519_from_bytes(seed_bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    seed_bytes.zeroize();
    Ok((signing_key, libp2p_key))
}

// ---------------------------------------------------------------------------
// Minimal hex helpers (no extra dependencies)
// ---------------------------------------------------------------------------
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.trim().as_bytes();
    if bytes.len() % 2 != 0 {
        return None;
    }
    bytes
        .chunks_exact(2)
        .map(|pair| {
            let hi = (pair[0] as char).to_digit(16)? as u8;
            let lo = (pair[1] as char).to_digit(16)? as u8;
            Some((hi << 4) | lo)
        })
        .collect()
}

// ─── Respaldo / Restauración de wallet (Checkpoint 24 — Fase 3 pulido) ───
//
// El propio arranque advierte "GUARDA ESTE ARCHIVO Y TU CONTRASEÑA": estas
// dos funciones lo convierten en FEATURE de primer nivel. El blob viaja
// SIEMPRE en bruto (tal como vive en wallet.key, cifrado con orion aead —
// la seed en claro JAMÁS vuelve al disco): exportar valida el roundtrip
// con la contraseña (respaldo probado, no basura) y restaurar valida el
// descifre ANTES de tocar la wallet vigente (contraseña equivocada o
// archivo corrupto no pueden vaciar la carpeta). El restaurar respeta el
// formato legacy en claro (hex 32B) para soportar copias viejas.

/// Valida que `blob` sea una wallet legible con `password`:
/// formato cifrado (`salt:ciphertext`) → descifra; formato legacy en
/// claro → hex de exactamente 32 bytes. `Ok(())` = blob sano.
fn validar_blob_wallet(blob: &str, password: &str) -> Result<(), String> {
    if blob.contains(':') {
        decrypt_seed(blob, password)
            .map(|_| ())
            .map_err(|e| format!("Contraseña incorrecta o wallet corrupta: {e}"))
    } else {
        let raw = Zeroizing::new(hex_decode(blob).ok_or_else(|| {
            "Formato de wallet irreconocible (ni cifrada ni hex legacy)".to_string()
        })?);
        if raw.len() != 32 {
            return Err(format!(
                "wallet legacy de tamaño inválido: {} bytes (deben ser 32)",
                raw.len()
            ));
        }
        Ok(())
    }
}

/// Exporta un RESPALDO de la wallet de `data_dir` hacia `dest`:
/// valida que `wallet.key` descifre con la contraseña y copia el blob
/// CIFRADO byte a byte. Jamás sobreescribe (`dest` existente = error) —
/// un respaldo no pisa otro respaldo.
pub fn export_wallet_backup(
    data_dir: &Path,
    password: &str,
    dest: &Path,
) -> Result<(), String> {
    if dest.exists() {
        return Err(format!(
            "{} ya existe — el respaldo NO pisa archivos; elegí otro destino",
            dest.display()
        ));
    }
    let key_path = data_dir.join("wallet.key");
    let contenido = std::fs::read_to_string(&key_path)
        .map_err(|e| format!("Leyendo {} : {e}", key_path.display()))?;
    let blob = contenido.trim().to_string();
    validar_blob_wallet(&blob, password)?;
    if let Some(padre) = dest.parent() {
        std::fs::create_dir_all(padre)
            .map_err(|e| format!("Creando carpeta destino: {e}"))?;
    }
    std::fs::write(dest, &blob).map_err(|e| format!("Escribiendo respaldo: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            dest,
            std::fs::Permissions::from_mode(0o600),
        );
    }
    Ok(())
}

/// Restaura el respaldo `backup` dentro de `data_dir` (convierte el
/// archivo en el `wallet.key` del directorio). Valida el descifre ANTES
/// de tocar nada; si la carpeta ya tenía wallet, la actual se guarda como
/// `wallet.key.bak-<unix_ms>` y se reporta el nombre. OJO: la identidad
/// P2P nace de la misma seed (crypto derive_from_seed) — restaurar otra
/// wallet cambia también la identidad del nodo de la carpeta.
pub fn import_wallet_backup(
    backup: &Path,
    data_dir: &Path,
    password: &str,
) -> Result<Option<std::path::PathBuf>, String> {
    let contenido = std::fs::read_to_string(backup)
        .map_err(|e| format!("Leyendo {} : {e}", backup.display()))?;
    let blob = contenido.trim().to_string();
    validar_blob_wallet(&blob, password)?;

    std::fs::create_dir_all(data_dir)
        .map_err(|e| format!("Creando directorio de wallet: {e}"))?;
    let key_path = data_dir.join("wallet.key");
    let mut reemplazada: Option<std::path::PathBuf> = None;
    if key_path.exists() {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let bak = data_dir.join(format!("wallet.key.bak-{millis}"));
        std::fs::rename(&key_path, &bak)
            .map_err(|e| format!("Resguardando la wallet actual: {e}"))?;
        reemplazada = Some(bak);
    }
    std::fs::write(&key_path, &blob)
        .map_err(|e| format!("Escribiendo {} : {e}", key_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            &key_path,
            std::fs::Permissions::from_mode(0o600),
        );
    }
    Ok(reemplazada)
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
        assert_eq!(*decrypted, seed);
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
    fn test_backup_roundtrip_restaura_la_misma_wallet() {
        // Checkpoint 24: exportar → restaurar en un dir FRESCO → la wallet
        // derivada es idéntica (misma seed ⇒ misma dirección e identidad).
        let dir1 =
            std::env::temp_dir().join(format!("ledgerita_backup_{}", std::process::id()));
        let dir2 =
            std::env::temp_dir().join(format!("ledgerita_restaurada_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir1);
        let _ = std::fs::remove_dir_all(&dir2);
        let password = "respaldo-2026";
        let (sk, _) = load_or_create_wallet(&dir1, password).unwrap();
        let destino = dir1.join("respaldo-test.key");

        export_wallet_backup(&dir1, password, &destino).unwrap();

        // La seed en claro JAMÁS viaja: el respaldo es el blob cifrado.
        let blob = std::fs::read_to_string(&destino).unwrap();
        assert!(blob.contains(':'), "el respaldo viaja cifrado");

        let reemplazo = import_wallet_backup(&destino, &dir2, password).unwrap();
        assert!(reemplazo.is_none(), "dir fresco: nada que reemplazar");
        let (sk2, _) = load_or_create_wallet(&dir2, password).unwrap();
        assert_eq!(sk.to_bytes(), sk2.to_bytes(), "misma seed restaurada");

        std::fs::remove_dir_all(&dir1).ok();
        std::fs::remove_dir_all(&dir2).ok();
    }

    #[test]
    fn test_backup_contrasena_mal_rechazada_en_ambos_lados() {
        // Contraseña incorrecta: ni exportar (el roundtrip ya no valida) ni
        // restaurar (el descifre falla ANTES de tocar los archivos).
        let dir =
            std::env::temp_dir().join(format!("ledgerita_bkw_{}", std::process::id()));
        let dir2 =
            std::env::temp_dir().join(format!("ledgerita_bkr_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
        let password = "clave-buena";
        load_or_create_wallet(&dir, password).unwrap();
        let destino = dir.join("mal-respaldo.key");

        let err_export = export_wallet_backup(&dir, "otra-clave", &destino).unwrap_err();
        assert!(
            err_export.contains("Contraseña incorrecta"),
            "mensaje legible en el export: {err_export}"
        );
        assert!(!destino.exists(), "export fracasado no deja basura");

        // Restaurar con mala contraseña sobre un dir con wallet VIGENTE:
        // la wallet actual SOBREVIVE (nada se toca).
        import_wallet_backup(&dir.join("wallet.key"), &dir2, "otra-clave")
            .expect_err("contraseña incorrecta al restaurar");
        let (sk, _) = load_or_create_wallet(&dir, password).unwrap();
        // La original sigue siendo la misma (el dir2 quedó vacío).
        let (sk_orig, _) = load_or_create_wallet(&dir, password).unwrap();
        assert_eq!(sk.to_bytes(), sk_orig.to_bytes());

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&dir2).ok();
    }

    #[test]
    fn test_backup_import_reemplaza_con_bak_y_nuevo_destino_no_pisa() {
        // Restaurar SOBRE una wallet existente: la actual se resguarda como
        // wallet.key.bak-<ms> y la nueva ocupa su lugar. Y exportar sobre un
        // destino existente NO pisa.
        let dir1 =
            std::env::temp_dir().join(format!("ledgerita_pi1_{}", std::process::id()));
        let dir2 =
            std::env::temp_dir().join(format!("ledgerita_pi2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir1);
        let _ = std::fs::remove_dir_all(&dir2);
        let password = "clavito";
        let (sk1, _) = load_or_create_wallet(&dir1, password).unwrap();
        let _ = load_or_create_wallet(&dir2, password).unwrap(); // sk2 nace
        let backup = dir1.join("respaldo-de-una.key");
        export_wallet_backup(&dir1, password, &backup).unwrap();

        // Restaurar el respaldo de dir1 (seed sk1) SOBRE la wallet sk2 de
        // dir2: la vigente se resguarda como wallet.key.bak-<ms> y sk1 la
        // reemplaza. (sk2 queda VIVO dentro del .bak.)
        let bak = import_wallet_backup(&backup, &dir2, password).unwrap()
            .expect("wallet existente ⇒ crea .bak");
        assert!(bak.exists(), "el .bak de la wallet reemplazada vive");
        let (sk2r, _) = load_or_create_wallet(&dir2, password).unwrap();
        assert_eq!(
            sk2r.to_bytes(), sk1.to_bytes(),
            "tras restaurar, dir2 vive con la seed del respaldo (la de dir1)"
        );

        // Export a destino EXISTENTE: rechazo sin pisa (el backup de machaco
        // mismo no renace).
        assert!(
            export_wallet_backup(&dir1, password, &backup).is_err(),
            "destino existente ⇒ NO pisa"
        );

        std::fs::remove_dir_all(&dir1).ok();
        std::fs::remove_dir_all(&dir2).ok();
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

    #[test]
    fn test_zeroize_limpieza_memoria_activa() {
        let mut buffer = Zeroizing::new([0xAAu8; 32]);
        assert_eq!(buffer[0], 0xAA);
        buffer.zeroize();
        assert_eq!(*buffer, [0u8; 32]);
    }

    #[test]
    fn test_hex_decode_rechaza_utf8_multibyte_sin_panic() {
        assert!(hex_decode("áá").is_none());
        assert!(hex_decode("🦀a").is_none());
        assert!(hex_decode("abc").is_none());
        assert!(Address::from_hex("á".repeat(64).as_str()).is_none());
    }

    #[test]
    fn test_verify_strict_rechaza_firma_orden_pequeno_y_maleable() {
        use ed25519_dalek::{Signature, Signer, SigningKey};
        let sk = SigningKey::from_bytes(&[0x55; 32]);
        let addr = Address::from_signing_key(&sk);
        let msg = b"ledgerita-strict-check";
        let valid = sk.sign(msg);
        assert!(addr.verify_signature(msg, &valid));

        // Firma degenerada de orden pequeño (R = punto identidad comprimido 0x01 || 0..0, S = 0)
        let mut small_order = [0u8; 64];
        small_order[0] = 1;
        let bad_sig = Signature::from_bytes(&small_order);
        assert!(!addr.verify_signature(msg, &bad_sig));

        // Firma maleable alterando S >= L (bit alto en byte 63)
        let mut raw = valid.to_bytes();
        raw[63] ^= 0x80;
        let malleable = Signature::from_bytes(&raw);
        assert!(!addr.verify_signature(msg, &malleable));
    }
}
