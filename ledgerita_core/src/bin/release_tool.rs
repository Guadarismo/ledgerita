//! release_tool — cadena de custodia código → binario → nodo (Fase F1
//! de `DISENO_AUTOUPDATE.md`, leyes U2/U3/U6, Checkpoint 32).
//!
//! La llave de RELEASE es dedicada y está SEPARADA de las wallets: si
//! la llave de release se compromete, puede meter código pero JAMÁS
//! fondos (los fondos solo mueven TXs firmadas con wallet keys) —
//! separación de dominios.
//!
//! El manifiesto viaja como TEXTO ESTABLE y se firma ed25519 sobre SUS
//! BYTES EXACTOS (nada de re-serialización: lo firmado es lo que hay).
//! El verificador exige firma válida ∧ sha256(binario) == manifiesto.
//!
//! Subcomandos:
//!   gen-key  <seed.hex>          — crea una llave de release (32 B aleatorios)
//!   pub-key  <seed.hex>          — imprime la pubkey ed25519 (64 hex)
//!   sign     <seed.hex> <binario> --version V --red R --commit C --target T [--fecha F] [salida_prefix]
//!   verify   <pubkey_hex> <manifest> <binario> <sig_hex>
//!
//! Códigos de salida de verify: 0 = VERIFICADO, 1 = RECHAZADO.

use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::process::ExitCode;

// El ESQUEMA del manifiesto vive en la lib (compartido con el updater
// de F2 — una sola fuente de verdad del formato firmado).
use ledgerita_core::release::{firma_de, hex, unhex, verify_manifest};

fn build_manifest(
    version: &str,
    red: &str,
    commit_git: &str,
    target: &str,
    fecha: &str,
    sha256_binario: &str,
) -> String {
    ledgerita_core::release::build_manifest_text(version, red, commit_git, target, fecha, sha256_binario)
}

fn cargar_seed(ruta: &str) -> Result<SigningKey, String> {
    let texto = std::fs::read_to_string(ruta).map_err(|e| format!("no se pudo leer {ruta}: {e}"))?;
    let texto = texto.trim();
    let bytes = unhex(texto).map_err(|e| format!("seed corrupta: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!("la seed debe ser 32 bytes, hay {}", bytes.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(SigningKey::from_bytes(&arr))
}

fn sha256_de_archivo(ruta: &str) -> Result<String, String> {
    let data = std::fs::read(ruta).map_err(|e| format!("no se pudo leer {ruta}: {e}"))?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    Ok(hex(&hasher.finalize()))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "Uso: release_tool gen-key <seed.hex> | pub-key <seed.hex> | sign <seed.hex> <binario> --version V --red R --commit C --target T [--fecha F] [prefix] | verify <pubkey_hex> <manifest> <binario> <sig_hex>"
        );
        return ExitCode::from(2);
    }
    let resultado = match args[1].as_str() {
        "gen-key" => cmd_gen_key(&args[2..]),
        "pub-key" => cmd_pub_key(&args[2..]),
        "sign" => cmd_sign(&args[2..]),
        "verify" => cmd_verify(&args[2..]),
        _ => Err(Error::Uso(format!("subcomando desconocido: {}", args[1]))),
    };
    match resultado {
        Ok(()) => ExitCode::SUCCESS,
        Err(Error::Rechazo(m)) => {
            eprintln!("❌ RECHAZADO: {m}");
            ExitCode::FAILURE
        }
        Err(Error::Uso(m)) => {
            eprintln!("Error: {m}");
            ExitCode::FAILURE
        }
    }
}

/// Distingue el RECHAZO mecánico (verificación falló) de un error de uso.
enum Error {
    Rechazo(String),
    Uso(String),
}
impl From<String> for Error {
    fn from(m: String) -> Self {
        Error::Uso(m)
    }
}
impl From<&str> for Error {
    fn from(m: &str) -> Self {
        Error::Uso(m.to_string())
    }
}
type Res = Result<(), Error>;

fn cmd_gen_key(args: &[String]) -> Res {
    let ruta = args.first().ok_or("gen-key requiere la ruta de la seed")?;
    if Path::new(ruta).exists() {
        return Err(Error::Uso(format!("{ruta} YA existe — no se pisa una llave de release")));
    }
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| Error::Uso(format!("getrandom: {e}")))?;
    std::fs::write(ruta, hex(&seed)).map_err(|e| Error::Uso(format!("escritura: {e}")))?;
    let sk = SigningKey::from_bytes(&seed);
    println!("🔑 Llave de release creada en {ruta} (gitignored, JAMÁS commitear)");
    println!("🔑 Pubkey (documentable): {}", hex(sk.verifying_key().as_bytes()));
    Ok(())
}

fn cmd_pub_key(args: &[String]) -> Res {
    let ruta = args.first().ok_or("pub-key requiere la ruta de la seed")?;
    let sk = cargar_seed(ruta)?;
    println!("{}", hex(sk.verifying_key().as_bytes()));
    Ok(())
}

fn cmd_sign(args: &[String]) -> Res {
    if args.len() < 2 {
        return Err(Error::Uso("sign requiere <seed.hex> <binario>".into()));
    }
    let sk = cargar_seed(&args[0])?;
    let binario = &args[1];
    let mut version = String::new();
    let mut red = String::new();
    let mut commit = String::new();
    let mut target = String::new();
    let mut fecha = String::new();
    let mut prefix = String::new();
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--version" => {
                i += 1;
                version = args.get(i).cloned().unwrap_or_default();
            }
            "--red" => {
                i += 1;
                red = args.get(i).cloned().unwrap_or_default();
            }
            "--commit" => {
                i += 1;
                commit = args.get(i).cloned().unwrap_or_default();
            }
            "--target" => {
                i += 1;
                target = args.get(i).cloned().unwrap_or_default();
            }
            "--fecha" => {
                i += 1;
                fecha = args.get(i).cloned().unwrap_or_default();
            }
            otro if prefix.is_empty() && !otro.starts_with("--") => prefix = otro.to_string(),
            _ => {}
        }
        i += 1;
    }
    if fecha.is_empty() {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        fecha = format!("unix-{secs}");
    }
    if version.is_empty() || red.is_empty() || commit.is_empty() || target.is_empty() {
        return Err(Error::Uso(
            "sign requiere --version, --red, --commit y --target".into(),
        ));
    }
    let sha = sha256_de_archivo(binario)?;
    let manifest = build_manifest(&version, &red, &commit, &target, &fecha, &sha);
    let firma = firma_de(&sk, manifest.as_bytes());

    let (ruta_manifest, ruta_sig) = if prefix.is_empty() {
        ("RELEASE_MANIFEST.txt".to_string(), "RELEASE_MANIFEST.sig".to_string())
    } else {
        (format!("{prefix}.txt"), format!("{prefix}.sig"))
    };
    std::fs::write(&ruta_manifest, &manifest).map_err(|e| Error::Uso(format!("escritura: {e}")))?;
    std::fs::write(&ruta_sig, hex(&firma.to_bytes()))
        .map_err(|e| Error::Uso(format!("escritura: {e}")))?;
    println!("📜 Manifiesto: {ruta_manifest}");
    println!("🖋️  Firma (hex): {ruta_sig}");
    println!("   sha256 del binario: {sha}");
    Ok(())
}

fn cmd_verify(args: &[String]) -> Res {
    if args.len() < 4 {
        return Err(Error::Uso(
            "verify requiere <pubkey_hex> <manifest> <binario> <sig_hex>".into(),
        ));
    }
    let pubkey_bytes = unhex(args[0].trim())?;
    if pubkey_bytes.len() != 32 {
        return Err(Error::Uso(format!("pubkey debe ser 32 bytes, hay {}", pubkey_bytes.len())));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&pubkey_bytes);
    let pubkey = VerifyingKey::from_bytes(&arr).map_err(|e| Error::Uso(format!("pubkey inválida: {e}")))?;

    let manifest = std::fs::read(&args[1]).map_err(|e| Error::Uso(format!("manifiesto: {e}")))?;
    let binario = std::fs::read(&args[2]).map_err(|e| Error::Uso(format!("binario: {e}")))?;
    let sig_bytes = unhex(args[3].trim())?;
    if sig_bytes.len() != 64 {
        return Err(Error::Uso(format!("firma debe ser 64 bytes, hay {}", sig_bytes.len())));
    }
    let mut arr64 = [0u8; 64];
    arr64.copy_from_slice(&sig_bytes);
    let sig = Signature::from_bytes(&arr64);

    match verify_manifest(&pubkey, &manifest, &binario, &sig) {
        Ok(m) => {
            println!("✅ VERIFICADO: firma ed25519 válida ∧ sha256 del binario coherente");
            println!("   version {} · red {} · commit {} · fecha {}", m.version, m.red, m.commit_git, m.fecha);
            Ok(())
        }
        Err(m) => Err(Error::Rechazo(m)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BINARIO: &[u8] = b"binario-falso-para-tests\x00\x01\x02";

    fn seed_fija() -> SigningKey {
        SigningKey::from_bytes(&[0x42; 32])
    }

    fn manifest_de(binario: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(binario);
        build_manifest(
            "v4.0",
            "ambas",
            "407a22e",
            "x86_64-unknown-linux-gnu",
            "2026-11-11",
            &hex(&hasher.finalize()),
        )
    }

    #[test]
    fn test_sign_y_verify_redondo() {
        let sk = seed_fija();
        let manifest = manifest_de(BINARIO);
        let sig = firma_de(&sk, manifest.as_bytes());
        let vk = sk.verifying_key();
        assert!(verify_manifest(&vk, manifest.as_bytes(), BINARIO, &sig).is_ok());
    }

    #[test]
    fn test_manifest_manipulado_rechaza() {
        let sk = seed_fija();
        let manifest = manifest_de(BINARIO);
        let sig = firma_de(&sk, manifest.as_bytes());
        let vk = sk.verifying_key();
        // Un byte cambiado del manifiesto (tampering) invalida la firma.
        let manipulado = manifest.replace("v4.0", "v9.9");
        assert!(verify_manifest(&vk, manipulado.as_bytes(), BINARIO, &sig).is_err());
    }

    #[test]
    fn test_binario_incoherente_rechaza() {
        // La firma es válida pero el binario NO es el que declara el
        // manifiesto (substitución de binario) — el eslabón lo caza.
        let sk = seed_fija();
        let manifest = manifest_de(BINARIO);
        let sig = firma_de(&sk, manifest.as_bytes());
        let vk = sk.verifying_key();
        let otro_binario = b"otro binario distinto";
        assert!(verify_manifest(&vk, manifest.as_bytes(), otro_binario, &sig).is_err());
    }

    #[test]
    fn test_pubkey_distinta_rechaza() {
        // Otra llave de release no puede validar el manifiesto de esta.
        let sk = seed_fija();
        let manifest = manifest_de(BINARIO);
        let sig = firma_de(&sk, manifest.as_bytes());
        let impostora = SigningKey::from_bytes(&[0x99; 32]);
        assert!(verify_manifest(&impostora.verifying_key(), manifest.as_bytes(), BINARIO, &sig).is_err());
    }

    #[test]
    fn test_sha256_declarado_y_real_coinciden() {
        let manifest = manifest_de(BINARIO);
        let mut hasher = Sha256::new();
        hasher.update(BINARIO);
        let linea = format!("sha256: {}\n", hex(&hasher.finalize()));
        assert!(
            manifest.contains(&linea),
            "el manifiesto declara el sha256 real del binario"
        );
    }
}
