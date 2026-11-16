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
//!   gen-key      <seed.hex>          — crea una llave de release (32 B aleatorios)
//!   pub-key      <seed.hex>          — imprime la pubkey ed25519 (64 hex)
//!   sign         <seed.hex> <binario> --version V --red R --commit C --target T [--fecha F] [salida_prefix]
//!   co-sign      <seed.hex> <manifest> <sig_file> — agrega firma a un .sig existente (multifirma)
//!   verify       <pubkey_hex> <manifest> <binario> <sig_hex>
//!   verify-multi <pubkeys_csv> <threshold> <manifest> <binario> <sig_file>
//!
//! Códigos de salida de verify / verify-multi: 0 = VERIFICADO, 1 = RECHAZADO.

use ed25519_dalek::{Signature, SigningKey};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::process::ExitCode;

// El ESQUEMA del manifiesto vive en la lib (compartido con el updater
// de F2 — una sola fuente de verdad del formato firmado).
use ledgerita_core::release::{
    build_manifest_text_with_source, firma_de, hash_fuente_dir, hex,
    parse_pubkey_hex, parse_signatures, unhex, verify_manifest, verify_manifest_multisig,
};
use zeroize::Zeroizing;

fn build_manifest(
    version: &str,
    red: &str,
    commit_git: &str,
    target: &str,
    fecha: &str,
    sha256_binario: &str,
    source_sha256: Option<&str>,
) -> String {
    build_manifest_text_with_source(
        version,
        red,
        commit_git,
        target,
        fecha,
        sha256_binario,
        source_sha256,
    )
}

fn cargar_seed(ruta: &str) -> Result<SigningKey, String> {
    let texto =
        Zeroizing::new(std::fs::read_to_string(ruta).map_err(|e| format!("no se pudo leer {ruta}: {e}"))?);
    let bytes = Zeroizing::new(unhex(texto.trim()).map_err(|e| format!("seed corrupta: {e}"))?);
    if bytes.len() != 32 {
        return Err(format!("la seed debe ser 32 bytes, hay {}", bytes.len()));
    }
    let mut arr = Zeroizing::new([0u8; 32]);
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
            "Uso: release_tool gen-key <seed.hex> | pub-key <seed.hex> | sign <seed.hex> <binario> --version V --red R --commit C --target T [--fecha F] [prefix] | co-sign <seed.hex> <manifest> <sig_file> | verify <pubkey_hex> <manifest> <binario> <sig_hex> | verify-multi <pubkeys_csv> <threshold> <manifest> <binario> <sig_file>"
        );
        return ExitCode::from(2);
    }
    let resultado = match args[1].as_str() {
        "gen-key" => cmd_gen_key(&args[2..]),
        "pub-key" => cmd_pub_key(&args[2..]),
        "sign" => cmd_sign(&args[2..]),
        "co-sign" => cmd_co_sign(&args[2..]),
        "verify" => cmd_verify(&args[2..]),
        "verify-multi" => cmd_verify_multi(&args[2..]),
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
    let mut source_dir = String::new();
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
            "--source-dir" => {
                i += 1;
                source_dir = args.get(i).cloned().unwrap_or_default();
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
    let source_sha = if source_dir.is_empty() {
        None
    } else {
        Some(hash_fuente_dir(Path::new(&source_dir))?)
    };
    let manifest = build_manifest(
        &version,
        &red,
        &commit,
        &target,
        &fecha,
        &sha,
        source_sha.as_deref(),
    );
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
    if let Some(src_sha) = source_sha {
        println!("   source_sha256: {src_sha}");
    }
    Ok(())
}

fn append_signature_text(existing: &str, new_sig_hex: &str) -> String {
    let mut lines: Vec<String> = existing
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if !lines.iter().any(|l| l == new_sig_hex) {
        lines.push(new_sig_hex.to_string());
    }
    format!("{}\n", lines.join("\n"))
}

fn cmd_co_sign(args: &[String]) -> Res {
    if args.len() < 3 {
        return Err(Error::Uso(
            "co-sign requiere <seed.hex> <manifest> <sig_file>".into(),
        ));
    }
    let sk = cargar_seed(&args[0])?;
    let manifest = std::fs::read(&args[1]).map_err(|e| Error::Uso(format!("manifiesto: {e}")))?;
    let sig_file = &args[2];
    let firma = firma_de(&sk, &manifest);
    let firma_hex = hex(&firma.to_bytes());
    let existing = std::fs::read_to_string(sig_file).unwrap_or_default();
    let updated = append_signature_text(&existing, &firma_hex);
    std::fs::write(sig_file, updated).map_err(|e| Error::Uso(format!("escritura: {e}")))?;
    println!("🖋️  Co-firma agregada en {sig_file}");
    println!("   pubkey firmante: {}", hex(sk.verifying_key().as_bytes()));
    Ok(())
}

fn cmd_verify(args: &[String]) -> Res {
    if args.len() < 4 {
        return Err(Error::Uso(
            "verify requiere <pubkey_hex> <manifest> <binario> <sig_hex>".into(),
        ));
    }
    let pubkey = parse_pubkey_hex(&args[0])?;

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

fn cmd_verify_multi(args: &[String]) -> Res {
    if args.len() < 5 {
        return Err(Error::Uso(
            "verify-multi requiere <pubkeys_csv> <threshold> <manifest> <binario> <sig_file>".into(),
        ));
    }
    let pubkeys: Vec<_> = args[0]
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(parse_pubkey_hex)
        .collect::<Result<Vec<_>, _>>()?;
    let threshold: usize = args[1]
        .parse()
        .map_err(|_| Error::Uso(format!("threshold inválido: {}", args[1])))?;
    let manifest = std::fs::read(&args[2]).map_err(|e| Error::Uso(format!("manifiesto: {e}")))?;
    let binario = std::fs::read(&args[3]).map_err(|e| Error::Uso(format!("binario: {e}")))?;
    let sig_text = std::fs::read_to_string(&args[4]).map_err(|e| Error::Uso(format!("sig_file: {e}")))?;
    let sigs = parse_signatures(&sig_text)?;

    match verify_manifest_multisig(&pubkeys, threshold, &manifest, &binario, &sigs) {
        Ok(m) => {
            println!(
                "✅ VERIFICADO MULTIFIRMA ({threshold}-de-{}): firmas ed25519 válidas ∧ sha256 coherente",
                pubkeys.len()
            );
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
            None,
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

    #[test]
    fn test_co_sign_y_verify_multi_2_de_3() {
        let sk1 = SigningKey::from_bytes(&[0x11; 32]);
        let sk2 = SigningKey::from_bytes(&[0x22; 32]);
        let sk3 = SigningKey::from_bytes(&[0x33; 32]);
        let manifest = manifest_de(BINARIO);

        let sig1 = hex(&firma_de(&sk1, manifest.as_bytes()).to_bytes());
        let sig3 = hex(&firma_de(&sk3, manifest.as_bytes()).to_bytes());

        let sig_text = append_signature_text("", &sig1);
        // Re-firmar con la misma llave no duplica línea
        let sig_text = append_signature_text(&sig_text, &sig1);
        assert_eq!(parse_signatures(&sig_text).unwrap().len(), 1);

        // Co-firma de sk3 agrega la segunda firma
        let sig_text = append_signature_text(&sig_text, &sig3);
        let sigs = parse_signatures(&sig_text).unwrap();
        assert_eq!(sigs.len(), 2);

        let pks = [sk1.verifying_key(), sk2.verifying_key(), sk3.verifying_key()];
        assert!(verify_manifest_multisig(&pks, 2, manifest.as_bytes(), BINARIO, &sigs).is_ok());
    }
}
