//! release — el ESQUEMA del manifiesto de release (Fases F1/F2 del
//! autoupdate, `DISENO_AUTOUPDATE.md`, ley U2).
//!
//! El manifiesto es TEXTO ESTABLE y se firma ed25519 sobre SUS BYTES
//! EXACTOS (lo firmado es lo que hay — nada de re-serialización):
//!
//! ```text
//! # Ledgerita Release Manifest (U2 — DISENO_AUTOUPDATE)
//! version: v4.0
//! red: ambas
//! target: x86_64-unknown-linux-gnu
//! commit_git: 407a22e
//! fecha: 2026-11-11
//! sha256: 38e193c1…
//! ```
//!
//! Verificación mecánica (U2/U3): firma válida ∧ sha256(binario) ==
//! el declarado ∧ TODOS los campos presentes. Cualquier fallo ⇒
//! RECHAZO (el release jamás toca el nodo).

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

/// El manifiesto parseado. Todos los campos son OBLIGATORIOS
/// (test bloqueante 3 del diseño: campos faltantes ⇒ rechaza).
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub version: String,
    pub red: String,
    pub target: String,
    pub commit_git: String,
    pub fecha: String,
    pub sha256: String,
}

/// El texto EXACTO del manifiesto — el formato que la llave de release
/// firma y el nodo verifica. Cambiar ESTA función = cambiar la era de
/// firmas (los manifiestos viejos dejan de verificarse).
pub fn build_manifest_text(
    version: &str,
    red: &str,
    commit_git: &str,
    target: &str,
    fecha: &str,
    sha256_binario: &str,
) -> String {
    build_manifest_text_with_source(
        version,
        red,
        commit_git,
        target,
        fecha,
        sha256_binario,
        None,
    )
}

pub fn build_manifest_text_with_source(
    version: &str,
    red: &str,
    commit_git: &str,
    target: &str,
    fecha: &str,
    sha256_binario: &str,
    source_sha256: Option<&str>,
) -> String {
    let mut base = format!(
        "# Ledgerita Release Manifest (U2 — DISENO_AUTOUPDATE)\n\
         version: {version}\n\
         red: {red}\n\
         target: {target}\n\
         commit_git: {commit_git}\n\
         fecha: {fecha}\n\
         sha256: {sha256_binario}\n"
    );
    if let Some(src_sha) = source_sha256 {
        base.push_str(&format!("source_sha256: {src_sha}\n"));
    }
    base
}

/// Extrae y valida el campo opcional `source_sha256:` (64 hex minúsculos/mayúsculos normalizados)
/// desde los bytes firmados del manifiesto.
pub fn parse_manifest_source_sha256(manifest_bytes: &[u8]) -> Result<Option<String>, String> {
    let texto = std::str::from_utf8(manifest_bytes)
        .map_err(|_| "manifiesto no es UTF-8 válido".to_string())?;
    let mut found: Option<String> = None;
    for linea in texto.lines() {
        if let Some(v) = linea.strip_prefix("source_sha256: ") {
            if found.is_some() {
                return Err("campo source_sha256 duplicado en manifiesto".to_string());
            }
            let norm = v.trim().to_ascii_lowercase();
            let raw = unhex(&norm)?;
            if raw.len() != 32 {
                return Err(format!(
                    "source_sha256 debe tener 32 bytes (64 hex), hay {}",
                    raw.len()
                ));
            }
            found = Some(norm);
        }
    }
    Ok(found)
}

fn collect_source_files(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<(String, std::path::PathBuf)>,
) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("dir_entry {}: {e}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_source_files(root, &path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            let rel = path
                .strip_prefix(root)
                .map_err(|e| format!("strip_prefix: {e}"))?
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, path));
        }
    }
    Ok(())
}

/// Calcula un SHA-256 canónico sobre el árbol fuente relevante (`Cargo.toml`, `Cargo.lock`,
/// `.cargo/config.toml` si existe, y todos los `.rs` bajo `src/`) ordenados lexicográficamente.
pub fn hash_fuente_dir(dir: &std::path::Path) -> Result<String, String> {
    let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
    for rel in ["Cargo.toml", "Cargo.lock"] {
        let p = dir.join(rel);
        if !p.exists() {
            return Err(format!("fuente incompleta: falta {}", p.display()));
        }
        files.push((rel.to_string(), p));
    }
    let cargo_cfg = dir.join(".cargo/config.toml");
    if cargo_cfg.exists() {
        files.push((".cargo/config.toml".to_string(), cargo_cfg));
    }
    let src_dir = dir.join("src");
    if !src_dir.is_dir() {
        return Err(format!("fuente incompleta: falta {}", src_dir.display()));
    }
    collect_source_files(dir, &src_dir, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    for (rel, path) in files {
        let bytes = std::fs::read(&path).map_err(|e| format!("leer {}: {e}", path.display()))?;
        hasher.update((rel.len() as u64).to_be_bytes());
        hasher.update(rel.as_bytes());
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(&bytes);
    }
    Ok(hex(&hasher.finalize()))
}

/// Parseo del manifiesto: cada línea `clave: valor`; los 6 campos son
/// obligatorios. Un manifiesto sin campos (o con basura en las líneas
/// de campo) se RECHAZA — el verificador no adivina.
pub fn parse_manifest(manifest_bytes: &[u8]) -> Result<Manifest, String> {
    let texto = std::str::from_utf8(manifest_bytes)
        .map_err(|_| "manifiesto no es UTF-8 válido".to_string())?;
    let mut m = Manifest {
        version: String::new(),
        red: String::new(),
        target: String::new(),
        commit_git: String::new(),
        fecha: String::new(),
        sha256: String::new(),
    };
    for linea in texto.lines() {
        let campo = |prefijo: &str, destino: &mut String| {
            if let Some(v) = linea.strip_prefix(prefijo) {
                if destino.is_empty() {
                    *destino = v.trim().to_string();
                }
            }
        };
        campo("version: ", &mut m.version);
        campo("red: ", &mut m.red);
        campo("target: ", &mut m.target);
        campo("commit_git: ", &mut m.commit_git);
        campo("fecha: ", &mut m.fecha);
        campo("sha256: ", &mut m.sha256);
    }
    if m.version.is_empty()
        || m.red.is_empty()
        || m.target.is_empty()
        || m.commit_git.is_empty()
        || m.fecha.is_empty()
        || m.sha256.is_empty()
    {
        return Err("manifiesto con campos faltantes (version/red/target/commit_git/fecha/sha256 son obligatorios)".to_string());
    }
    Ok(m)
}

/// La verificación mecánica del eslabón (U2): firma del manifiesto
/// válida con `pubkey` ∧ manifiesto parseable (campos completos) ∧
/// sha256 del binario == el declarado. Devuelve el manifiesto.
fn verify_manifest_body(manifest_bytes: &[u8], binario: &[u8]) -> Result<Manifest, String> {
    let m = parse_manifest(manifest_bytes)?;

    let mut hasher = Sha256::new();
    hasher.update(binario);
    let real = hex(&hasher.finalize());
    if real != m.sha256 {
        return Err(format!(
            "sha256 incoherente: manifiesto declara {}, binario real {real}",
            m.sha256
        ));
    }
    Ok(m)
}

pub fn verify_manifest(
    pubkey: &VerifyingKey,
    manifest_bytes: &[u8],
    binario: &[u8],
    sig: &Signature,
) -> Result<Manifest, String> {
    pubkey
        .verify_strict(manifest_bytes, sig)
        .map_err(|e| format!("firma del manifiesto inválida: {e}"))?;

    verify_manifest_body(manifest_bytes, binario)
}

/// Decodifica una llave pública Ed25519 desde 64 caracteres hex.
pub fn parse_pubkey_hex(pubkey_hex: &str) -> Result<VerifyingKey, String> {
    let bytes_pk = unhex(pubkey_hex.trim())?;
    if bytes_pk.len() != 32 {
        return Err(format!("pubkey debe ser 32 bytes, hay {}", bytes_pk.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes_pk);
    VerifyingKey::from_bytes(&arr).map_err(|e| format!("pubkey inválida: {e}"))
}

/// Parsea un archivo `.sig` con una o más firmas Ed25519 (128 hex por línea).
/// Ignora líneas vacías y comentarios (`#`).
pub fn parse_signatures(sig_text: &str) -> Result<Vec<Signature>, String> {
    let mut sigs = Vec::new();
    for (idx, line) in sig_text.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let bytes = unhex(t).map_err(|e| format!("línea {} de firma: {e}", idx + 1))?;
        if bytes.len() != 64 {
            return Err(format!(
                "línea {} de firma debe ser 64 bytes, hay {}",
                idx + 1,
                bytes.len()
            ));
        }
        let mut arr64 = [0u8; 64];
        arr64.copy_from_slice(&bytes);
        sigs.push(Signature::from_bytes(&arr64));
    }
    if sigs.is_empty() {
        return Err("archivo de firmas vacío".to_string());
    }
    Ok(sigs)
}

/// Verifica un manifiesto con umbral M-de-N (`threshold` firmas válidas de
/// llaves distintas en `pubkeys`), más la coherencia sha256 del binario.
pub fn verify_manifest_multisig(
    pubkeys: &[VerifyingKey],
    threshold: usize,
    manifest_bytes: &[u8],
    binario: &[u8],
    sigs: &[Signature],
) -> Result<Manifest, String> {
    if threshold == 0 {
        return Err("threshold multifirma debe ser >= 1".to_string());
    }
    if pubkeys.len() < threshold {
        return Err(format!(
            "llaves autorizadas insuficientes: {} < threshold {threshold}",
            pubkeys.len()
        ));
    }
    if sigs.len() < threshold {
        return Err(format!(
            "firmas insuficientes: {} < threshold {threshold}",
            sigs.len()
        ));
    }

    let mut used = vec![false; pubkeys.len()];
    let mut valid_count = 0usize;

    for sig in sigs {
        let matched = pubkeys
            .iter()
            .position(|pk| pk.verify_strict(manifest_bytes, sig).is_ok());
        match matched {
            Some(idx) => {
                if used[idx] {
                    return Err(format!(
                        "firma duplicada de la misma llave autorizada (índice {idx})"
                    ));
                }
                used[idx] = true;
                valid_count += 1;
            }
            None => {
                return Err("firma del manifiesto inválida o de llave no autorizada".to_string());
            }
        }
    }

    if valid_count < threshold {
        return Err(format!(
            "firmas válidas insuficientes: {valid_count} < threshold {threshold}"
        ));
    }

    verify_manifest_body(manifest_bytes, binario)
}

/// Firma un manifiesto (texto exacto) con la llave de release.
pub fn firma_de(seed: &SigningKey, manifest_bytes: &[u8]) -> Signature {
    seed.sign(manifest_bytes)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &str) -> Result<Vec<u8>, String> {
    let bytes = s.trim().as_bytes();
    if bytes.len() % 2 != 0 {
        return Err("hex inválido: longitud impar".to_string());
    }
    bytes
        .chunks_exact(2)
        .map(|pair| {
            let hi = (pair[0] as char)
                .to_digit(16)
                .ok_or_else(|| "hex inválido".to_string())? as u8;
            let lo = (pair[1] as char)
                .to_digit(16)
                .ok_or_else(|| "hex inválido".to_string())? as u8;
            Ok((hi << 4) | lo)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BINARIO: &[u8] = b"binario-de-release-para-tests\x00\x42";

    fn seed_fija() -> SigningKey {
        SigningKey::from_bytes(&[0x77; 32])
    }

    fn manifest_de(binario: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(binario);
        build_manifest_text(
            "v4.1", "testnet", "44ecfb6", "x86_64-unknown-linux-gnu", "2026-11-11",
            &hex(&hasher.finalize()),
        )
    }

    #[test]
    fn test_parse_manifest_completo_ok() {
        let m = parse_manifest(manifest_de(BINARIO).as_bytes()).unwrap();
        assert_eq!(m.version, "v4.1");
        assert_eq!(m.red, "testnet");
        assert_eq!(m.commit_git, "44ecfb6");
        assert_eq!(m.target, "x86_64-unknown-linux-gnu");
        assert_eq!(m.fecha, "2026-11-11");
        let mut hasher = Sha256::new();
        hasher.update(BINARIO);
        assert_eq!(m.sha256, hex(&hasher.finalize()));
    }

    #[test]
    fn test_manifest_campos_faltantes_rechaza() {
        // Test bloqueante 3 del diseño: sin campos el verificador no
        // adivina — RECHAZO.
        for vaciar in [
            "version: v4.1\ntarget: t\ncommit_git: c\nfecha: f\nsha256: 00\n",   // sin red
            "version: v4.1\nred: testnet\ncommit_git: c\nfecha: f\nsha256: 00\n", // sin target
            "version: v4.1\nred: testnet\ntarget: t\ncommit_git: c\nfecha: f\n",  // sin sha256
        ] {
            assert!(
                parse_manifest(vaciar.as_bytes()).is_err(),
                "manifiesto incompleto debe rechazar: {vaciar}"
            );
        }
    }

    #[test]
    fn test_verify_manifest_firma_valida_acepta_y_alterada_rechaza() {
        // Test bloqueante 1 del diseño.
        let sk = seed_fija();
        let texto = manifest_de(BINARIO);
        let sig = firma_de(&sk, texto.as_bytes());
        let vk = sk.verifying_key();
        assert!(verify_manifest(&vk, texto.as_bytes(), BINARIO, &sig).is_ok());
        // Un byte alterado del manifiesto invalida la firma.
        let alterado = texto.replace("v4.1", "v9.9");
        assert!(verify_manifest(&vk, alterado.as_bytes(), BINARIO, &sig).is_err());
    }

    #[test]
    fn test_sha256_binario_incoherente_rechaza() {
        // Test bloqueante 2 del diseño: firma válida ∧ binario
        // sustituido ⇒ el eslabón caza el tampering.
        let sk = seed_fija();
        let texto = manifest_de(BINARIO);
        let sig = firma_de(&sk, texto.as_bytes());
        let otro_binario = b"binario distinto del firmado";
        assert!(
            verify_manifest(&sk.verifying_key(), texto.as_bytes(), otro_binario, &sig).is_err()
        );
    }

    #[test]
    fn test_multisig_2_of_3_acepta_y_casos_invalidos_rechaza() {
        let sk1 = SigningKey::from_bytes(&[0x11; 32]);
        let sk2 = SigningKey::from_bytes(&[0x22; 32]);
        let sk3 = SigningKey::from_bytes(&[0x33; 32]);
        let sk_intruder = SigningKey::from_bytes(&[0x99; 32]);

        let pubkeys = [sk1.verifying_key(), sk2.verifying_key(), sk3.verifying_key()];
        let texto = manifest_de(BINARIO);

        let sig1 = firma_de(&sk1, texto.as_bytes());
        let sig2 = firma_de(&sk2, texto.as_bytes());
        let sig3 = firma_de(&sk3, texto.as_bytes());
        let sig_intruder = firma_de(&sk_intruder, texto.as_bytes());

        // 2 de 3 válidas (1 y 3) -> acepta
        let sigs_text = format!("{}\n{}\n", hex(&sig1.to_bytes()), hex(&sig3.to_bytes()));
        let parsed = parse_signatures(&sigs_text).unwrap();
        assert_eq!(parsed.len(), 2);
        let m = verify_manifest_multisig(&pubkeys, 2, texto.as_bytes(), BINARIO, &parsed).unwrap();
        assert_eq!(m.version, "v4.1");

        // 3 de 3 válidas -> acepta
        assert!(verify_manifest_multisig(&pubkeys, 2, texto.as_bytes(), BINARIO, &[sig1, sig2, sig3]).is_ok());

        // 1 de 3 -> rechaza
        assert!(verify_manifest_multisig(&pubkeys, 2, texto.as_bytes(), BINARIO, &[sig1]).is_err());

        // Firma duplicada del mismo firmante -> rechaza
        assert!(verify_manifest_multisig(&pubkeys, 2, texto.as_bytes(), BINARIO, &[sig1, sig1]).is_err());

        // Firma de llave no autorizada -> rechaza
        assert!(verify_manifest_multisig(&pubkeys, 2, texto.as_bytes(), BINARIO, &[sig1, sig_intruder]).is_err());

        // Binario alterado -> rechaza
        assert!(verify_manifest_multisig(&pubkeys, 2, texto.as_bytes(), b"alterado", &[sig1, sig2]).is_err());
    }

    #[test]
    fn test_unhex_rechaza_utf8_multibyte_e_impar() {
        assert!(unhex("áá").is_err());
        assert!(unhex("🦀a").is_err());
        assert!(unhex("abc").is_err());
        assert_eq!(unhex("0a1b").unwrap(), vec![0x0a, 0x1b]);
    }

    #[test]
    fn test_ckpt57_source_sha256_en_manifiesto_y_hash_fuente_dir() {
        let tmp = std::env::temp_dir().join("ledgerita_test_ckpt57_release_src");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::write(tmp.join("Cargo.toml"), "[package]\nname = \"ledgerita_core\"\n").unwrap();
        std::fs::write(tmp.join("Cargo.lock"), "# lock\n").unwrap();
        std::fs::write(tmp.join("src/lib.rs"), "pub fn ok() {}\n").unwrap();

        let h1 = hash_fuente_dir(&tmp).unwrap();
        let h2 = hash_fuente_dir(&tmp).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);

        let texto = build_manifest_text_with_source(
            "v4.3",
            "mainnet",
            "abc1234",
            "x86_64-unknown-linux-gnu",
            "2026-11-16",
            "00".repeat(32).as_str(),
            Some(&h1),
        );
        assert_eq!(
            parse_manifest_source_sha256(texto.as_bytes()).unwrap().as_deref(),
            Some(h1.as_str())
        );

        std::fs::write(tmp.join("src/lib.rs"), "pub fn altered() {}\n").unwrap();
        let h3 = hash_fuente_dir(&tmp).unwrap();
        assert_ne!(h1, h3);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
