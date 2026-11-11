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

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
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
    format!(
        "# Ledgerita Release Manifest (U2 — DISENO_AUTOUPDATE)\n\
         version: {version}\n\
         red: {red}\n\
         target: {target}\n\
         commit_git: {commit_git}\n\
         fecha: {fecha}\n\
         sha256: {sha256_binario}\n"
    )
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
pub fn verify_manifest(
    pubkey: &VerifyingKey,
    manifest_bytes: &[u8],
    binario: &[u8],
    sig: &Signature,
) -> Result<Manifest, String> {
    pubkey
        .verify(manifest_bytes, sig)
        .map_err(|e| format!("firma del manifiesto inválida: {e}"))?;

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

/// Firma un manifiesto (texto exacto) con la llave de release.
pub fn firma_de(seed: &SigningKey, manifest_bytes: &[u8]) -> Signature {
    seed.sign(manifest_bytes)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &str) -> Result<Vec<u8>, String> {
    (0..s.len() / 2)
        .map(|i| {
            u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|_| "hex inválido".to_string())
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
}
