//! updater — el CONSUMIDOR de la cadena de custodia (Fase F2 del
//! autoupdate, `DISENO_AUTOUPDATE.md`, leyes U1/U2/U3/U6/U9, Ckpt 33).
//!
//! Pipeline de `apply` (TestNet ONLY):
//!   1. VERIFICACIÓN mecánica del bundle (U2):
//!      firma ed25519 del manifiesto ∧ sha256(binario) == declarado
//!      ∧ campos completos ∧ llave de firma NO revocada (revokey)
//!      ∧ red y target coherentes con este host.
//!   2. TESTS DEL HOST (U3/U8 — doble verde): `cargo test --release
//!      --lib` contra la FUENTE del bundle. Si algo falla ⇒ JAMÁS swap.
//!   3. BACKUP `/proc/<pid>/exe` del binario corriendo (U6).
//!   4. SWAP: copia atómica del binario al path del servicio.
//!      Un swap fallido DEJA EL ANTERIOR corriendo (test bloqueante 5).
//!   5. RESTART solo del servicio de TestNet (`ledgerita-core-testnet`).
//!      **`--red mainnet` SE RECHAZA SIEMPRE**: el swap MainNet exige
//!      ventana humana (U1) — es la Fase F3 y vive fuera de este bin.
//!
//! El "baja" es deliberadamente tonto: el operador (o su script VISIBLE,
//! U9) deja el bundle en un directorio; el updater jamás descarga de la
//! red por su cuenta — menos superficie, cero instruction injection.

use ed25519_dalek::{Signature, VerifyingKey};
use std::path::Path;
use std::process::ExitCode;

use ledgerita_core::release::{hex, parse_manifest, unhex, verify_manifest, Manifest};

/// Distingue el RECHAZO mecánico (verificación falló ⇒ NADA se toca)
/// de un error de uso del operador.
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

/// Target EXACTO que este updater sabe instalar (host x86_64 linux —
/// el binario de otra plataforma no corre acá; el manifiesto declara
/// el suyo y se coteja).
const TARGET_HOST: &str = "x86_64-unknown-linux-gnu";

/// El servicio TestNet (el ÚNICO que este bin puede restartear).
const SERVICIO_TESTNET: &str = "ledgerita-core-testnet";

/// El path del binario que los servicios ejecutan (ExecStart).
const PATH_BINARIO: &str = "/home/user/ledgerita_core/target/release/ledgerita_core";

/// Verifica que la llave usada NO esté en la lista de revocadas
/// (test bloqueante 4: una llave revocada JAMÁS firma válido — aunque
/// la firma sea criptográficamente correcta, el rechazo es de POLÍTICA).
fn llave_revocada(pubkey_hex: &str, ruta_revoked: &str) -> Result<bool, String> {
    let texto =
        std::fs::read_to_string(ruta_revoked).map_err(|e| format!("revokey {ruta_revoked}: {e}"))?;
    Ok(texto
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .any(|l| l == pubkey_hex))
}

/// Verificación mecánica completa de un bundle (paso 1). El bundle:
/// `release.bin` + `RELEASE_MANIFEST.txt` + `RELEASE_MANIFEST.sig`.
/// Devuelve el manifiesto verificado.
fn verificar_bundle(dir: &str, pubkey_hex: &str, ruta_revoked: &str) -> Result<Manifest, String> {
    if llave_revocada(pubkey_hex, ruta_revoked)? {
        return Err("llave de firma REVOCADA — el rechazo es de política aunque la firma sea válida".to_string());
    }
    let bytes_pk = unhex(pubkey_hex)?;
    if bytes_pk.len() != 32 {
        return Err(format!("pubkey debe ser 32 bytes, hay {}", bytes_pk.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes_pk);
    let vk = VerifyingKey::from_bytes(&arr)
        .map_err(|e| format!("pubkey inválida: {e}"))?;

    let manifest_bytes =
        std::fs::read(format!("{dir}/RELEASE_MANIFEST.txt")).map_err(|e| format!("manifiesto: {e}"))?;
    let binario =
        std::fs::read(format!("{dir}/release.bin")).map_err(|e| format!("binario release.bin: {e}"))?;
    let sig_bytes =
        std::fs::read_to_string(format!("{dir}/RELEASE_MANIFEST.sig")).map_err(|e| format!("firma: {e}"))?;
    let sig_bytes = unhex(sig_bytes.trim())?;
    if sig_bytes.len() != 64 {
        return Err(format!("firma debe ser 64 bytes, hay {}", sig_bytes.len()));
    }
    let mut arr64 = [0u8; 64];
    arr64.copy_from_slice(&sig_bytes);
    let sig = Signature::from_bytes(&arr64);

    // Campos completos ANTES del cotejo de red/target (parse ya lo
    // garantiza: los 6 campos son obligatorios).
    let m = verify_manifest(&vk, &manifest_bytes, &binario, &sig)?;

    // Guardias declarativas: la red del manifiesto debe incluir a la
    // que el operador pidió, y el target debe ser el de este host.
    if !m.red.contains("testnet") && m.red != "ambas" {
        return Err(format!(
            "manifiesto es para red '{}' — no cubre TestNet",
            m.red
        ));
    }
    if m.target != TARGET_HOST {
        return Err(format!(
            "manifiesto target '{}' ≠ host '{TARGET_HOST}'",
            m.target
        ));
    }
    Ok(m)
}

/// Doble verde (U3/U8): la suite COMPLETA de lib contra la fuente del
/// bundle, EN ESTE HOST. Devuelve el resumen de la última línea de
/// `test result:` para el reporte. Requiere `cargo` en el host.
fn correr_tests_host(fuente_dir: &str) -> Result<String, String> {
    let salida = std::process::Command::new("cargo")
        .args(["test", "--release", "--lib"])
        .current_dir(fuente_dir)
        .output()
        .map_err(|e| format!("cargo test: {e}"))?;
    let texto = format!(
        "{}{}",
        String::from_utf8_lossy(&salida.stdout),
        String::from_utf8_lossy(&salida.stderr)
    );
    let resumen = texto
        .lines()
        .rev()
        .find(|l| l.contains("test result:"))
        .unwrap_or("(sin línea de test result)")
        .trim()
        .to_string();
    if !salida.status.success() || !resumen.contains("ok") || resumen.contains("failed") {
        return Err(format!("tests del host NO en verde: {resumen}"));
    }
    Ok(resumen)
}

/// Backup U6 del binario que ESTÁ corriendo (via `/proc/<pid>/exe` —
/// el inode vivo aunque el archivo ya fue reemplazado antes).
fn backup_proc_exe(servicio: &str, backup_path: &str) -> Result<u64, String> {
    let out = std::process::Command::new("systemctl")
        .args(["show", "-p", "MainPID", "--value", servicio])
        .output()
        .map_err(|e| format!("systemctl show: {e}"))?;
    let pid = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if pid.is_empty() || pid == "0" {
        return Err(format!("servicio {servicio} no corre (pid {pid}) — sin backup /proc/exe no hay swap"));
    }
    std::process::Command::new("cp")
        .args([format!("/proc/{pid}/exe"), backup_path.to_string()])
        .output()
        .map_err(|e| format!("cp /proc/exe: {e}"))?;
    let meta = std::fs::metadata(backup_path).map_err(|e| format!("backup: {e}"))?;
    Ok(meta.len())
}

/// SWAP (paso 4): copia del binario nuevo DIRECTO al destino. Copiar
/// directo (no rename) porque el inode en ejecución sobrevive: el
/// servicio sigue con el binario viejo hasta su restart. Si la copia
/// falla, el destino queda sin tocar (el anterior sigue corriendo —
/// test bloqueante 5) y el backup ya existe para el rollback manual.
fn instalar_binario(src: &str, dst: &str) -> Result<u64, String> {
    std::fs::copy(src, dst).map_err(|e| format!("instalación del binario: {e}"))
}

/// Restart del servicio (paso 5). SOLO TestNet puede llegar acá:
/// la guardia de red vive en `red_de_servicio` y `main`.
fn red_de_servicio(red: &str) -> Result<&'static str, String> {
    match red {
        "testnet" => Ok(SERVICIO_TESTNET),
        // U1: el swap MainNet SIEMPRE exige ventana humana (F3) — este
        // bin JAMÁS lo ejecuta (test bloqueante 6).
        "mainnet" => Err("U1: el swap a MainNet SIEMPRE exige ventana humana (Fase F3) — este updater solo TestNet".to_string()),
        otra => Err(format!("red desconocida: {otra} (solo testnet)")),
    }
}

fn restart_servicio(servicio: &str) -> Result<(), String> {
    let out = std::process::Command::new("sudo")
        .args(["-n", "systemctl", "restart", servicio])
        .output()
        .map_err(|e| format!("sudo systemctl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "restart falló (rc {:?}): {}{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        ));
    }
    Ok(())
}

fn cmd_verify(args: &[String]) -> Res {
    let (dir, pubkey, revoked) = tres_args(args, "verify requiere <bundle_dir> --pubkey <hex> [--revoked <archivo>]")?;
    let m = verificar_bundle(&dir, &pubkey, &revoked).map_err(Error::Rechazo)?;
    println!("✅ VERIFICADO (mecánica U2):");
    println!("   version {} · red {} · commit {} · fecha {}", m.version, m.red, m.commit_git, m.fecha);
    println!("   sha256 {}", m.sha256);
    Ok(())
}

fn cmd_apply(args: &[String]) -> Res {
    let (dir, pubkey, revoked) =
        tres_args(args, "apply requiere <bundle_dir> --pubkey <hex> [--revoked <archivo>] y --red <red>")?;
    let mut red = String::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--red" {
            i += 1;
            red = args.get(i).cloned().unwrap_or_default();
        }
        i += 1;
    }
    if red.is_empty() {
        return Err(Error::Uso("apply requiere --red <testnet>".into()));
    }

    // GUARDIA DE RED (U1) — antes de tocar NADA.
    let servicio = red_de_servicio(&red).map_err(Error::Rechazo)?;

    // PASO 1: verificación mecánica del bundle.
    let m = verificar_bundle(&dir, &pubkey, &revoked).map_err(Error::Rechazo)?;
    println!("①  bundle verificado: v{} commit {} sha256 {}…", m.version, m.commit_git, &m.sha256[..16]);

    // PASO 2: doble verde (tests del host contra la fuente del bundle).
    let fuente = format!("{dir}/fuente/ledgerita_core");
    if !Path::new(&fuente).exists() {
        return Err(Error::Rechazo(format!(
            "sin fuente del bundle en {fuente} — sin tests del host NO hay swap (U3)"
        )));
    }
    let resumen_tests = correr_tests_host(&fuente).map_err(Error::Rechazo)?;
    println!("②  tests del host: {resumen_tests}");

    // PASO 3: backup /proc/exe del binario corriendo (U6).
    let backup = format!("/home/user/updater_backup_{}_{}", m.version.replace('v', ""), red);
    let n = backup_proc_exe(servicio, &backup).map_err(Error::Rechazo)?;
    println!("③  backup /proc/exe → {backup} ({n} bytes)");

    // PASO 4: swap. Si algo falla ACÁ, el anterior sigue corriendo.
    let n2 = instalar_binario(&format!("{dir}/release.bin"), PATH_BINARIO).map_err(Error::Rechazo)?;
    println!("④  binario instalado en {PATH_BINARIO} ({n2} bytes)");

    // PASO 5: restart del servicio (solo TestNet — guardia ya pasada).
    restart_servicio(servicio).map_err(Error::Rechazo)?;
    println!("⑤  servicio {servicio} restarteado");

    // Verificación post mínima: activo.
    let out = std::process::Command::new("systemctl")
        .args(["is-active", servicio])
        .output()
        .map_err(|e| format!("systemctl is-active: {e}"))?;
    let estado = String::from_utf8_lossy(&out.stdout).trim().to_string();
    println!("POST: {servicio} = {estado}");
    if estado != "active" {
        return Err(Error::Rechazo(format!(
            "servicio {servicio} quedó '{estado}' — rollback: cp {backup} {PATH_BINARIO} + restart"
        )));
    }
    println!("APPLY_OK v{} commit {}", m.version, m.commit_git);
    Ok(())
}

/// Parsea `<dir> --pubkey <hex> [--revoked <archivo>]` de args.
fn tres_args(args: &[String], uso: &str) -> Result<(String, String, String), Error> {
    let dir = args.first().ok_or(Error::Uso(uso.into()))?.clone();
    let mut pubkey = String::new();
    let mut revoked = String::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--pubkey" => {
                i += 1;
                pubkey = args.get(i).cloned().unwrap_or_default();
            }
            "--revoked" => {
                i += 1;
                revoked = args.get(i).cloned().unwrap_or_default();
            }
            "--red" => {
                i += 1; // consumido por cmd_apply
            }
            _ => {}
        }
        i += 1;
    }
    if pubkey.is_empty() {
        return Err(Error::Uso("falta --pubkey <hex>".into()));
    }
    if revoked.is_empty() {
        revoked = "/home/user/updater_revoked.txt".to_string();
        if !Path::new(&revoked).exists() {
            // Sin lista de revocación el operador la crea vacía: el
            // archivo es OBLIGATORIO para forzar la decisión consciente.
            return Err(Error::Uso(format!(
                "falta --revoked <archivo> (o crea la lista vacía en {revoked})"
            )));
        }
    }
    Ok((dir, pubkey, revoked))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "Uso: ledgerita_updater verify <bundle_dir> --pubkey <hex> [--revoked <archivo>] | apply <bundle_dir> --red testnet --pubkey <hex> [--revoked <archivo>]"
        );
        eprintln!(
            "  Bundle: release.bin + RELEASE_MANIFEST.txt + RELEASE_MANIFEST.sig + fuente/ledgerita_core (para tests del host)."
        );
        eprintln!(
            "  SOLO TestNet: el swap MainNet exige ventana humana (U1, Fase F3)."
        );
        return ExitCode::from(2);
    }
    let resultado = match args[1].as_str() {
        "verify" => cmd_verify(&args[2..]),
        "apply" => cmd_apply(&args[2..]),
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

#[cfg(test)]
mod tests {
    use super::*;
    use ledgerita_core::release::{build_manifest_text, firma_de};
    use ed25519_dalek::SigningKey;
    use sha2::{Digest, Sha256};

    const PK_OFICIAL: &str = "a3f4145f8e3bc717ad06127ae8aa49ca3e79438deb3e92adc675908acc9005ad";

    /// La llave REAL de release (misma seed — el test recrea la pubkey
    /// sin leer el archivo gitignored).
    fn seed_real() -> SigningKey {
        // NOTA: solo para tests de forma — el test de revokey usa una
        // llave FIJA distinta para no acoplarse a la seed de producción.
        SigningKey::from_bytes(&[0x11; 32])
    }

    /// La pubkey de la llave fija de tests (hex).
    fn pk_de(seed: &SigningKey) -> String {
        hex(seed.verifying_key().as_bytes())
    }

    fn bundle_valido(dir: &std::path::Path, seed: &SigningKey, binario: &[u8]) -> String {
        std::fs::create_dir_all(dir).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(binario);
        let texto = build_manifest_text(
            "v4.1",
            "testnet",
            "44ecfb6",
            "x86_64-unknown-linux-gnu",
            "2026-11-11",
            &hex(&hasher.finalize()),
        );
        let sig = firma_de(seed, texto.as_bytes());
        std::fs::write(dir.join("RELEASE_MANIFEST.txt"), &texto).unwrap();
        std::fs::write(dir.join("RELEASE_MANIFEST.sig"), hex(&sig.to_bytes())).unwrap();
        std::fs::write(dir.join("release.bin"), binario).unwrap();
        hex(seed.verifying_key().as_bytes())
    }

    #[test]
    fn test_revokey_rechaza() {
        // Test bloqueante 4: llave revocada JAMÁS firma válido — el
        // rechazo es de política aunque la firma sea criptográficamente
        // correcta.
        let seed = SigningKey::from_bytes(&[0x55; 32]);
        let binario = b"binario-firmado-por-llave-revocada";
        let tmp = std::env::temp_dir().join("updater_t_revokey");
        let pk = bundle_valido(&tmp, &seed, binario);
        let revoked_path = tmp.join("revoked.txt");
        std::fs::write(&revoked_path, format!("{pk}\n")).unwrap();
        let err = verificar_bundle(tmp.to_str().unwrap(), &pk, revoked_path.to_str().unwrap())
            .unwrap_err();
        assert!(err.contains("REVOCADA"), "revokey debe rechazar: {err}");
        // La MISMA llave sin revocación PASA la política.
        std::fs::write(&revoked_path, "# otra llave\n").unwrap();
        assert!(
            verificar_bundle(tmp.to_str().unwrap(), &pk, revoked_path.to_str().unwrap()).is_ok()
        );
    }

    #[test]
    fn test_swap_fallido_deja_anterior_corriendo() {
        // Test bloqueante 5: un swap que falla (fuente inexistente)
        // DEJA el binario anterior intacto en su path.
        let tmp = std::env::temp_dir().join("updater_t_swap");
        std::fs::create_dir_all(&tmp).unwrap();
        let dst = tmp.join("ledgerita_core");
        std::fs::write(&dst, b"BINARIO-ANTERIOR-V4.0").unwrap();
        let antes = std::fs::read(&dst).unwrap();

        let err = instalar_binario(tmp.join("NO-EXISTE.bin").to_str().unwrap(), dst.to_str().unwrap())
            .unwrap_err();
        assert!(err.contains("instalación"), "el fallo se reporta: {err}");
        assert_eq!(
            std::fs::read(&dst).unwrap(),
            antes,
            "el binario anterior sobrevive intacto"
        );

        // Y el swap exitoso sí reemplaza.
        let src = tmp.join("nuevo.bin");
        std::fs::write(&src, b"BINARIO-NUEVO-V4.1").unwrap();
        instalar_binario(src.to_str().unwrap(), dst.to_str().unwrap()).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"BINARIO-NUEVO-V4.1");
    }

    #[test]
    fn test_updater_testnet_nunca_toca_mainnet() {
        // Test bloqueante 6 (y ley U1): la guardia de red rechaza
        // mainnet SIEMPRE, antes de tocar cualquier cosa.
        let err = red_de_servicio("mainnet").unwrap_err();
        assert!(err.contains("U1"), "la guardia cita la ley: {err}");
        assert!(err.contains("MainNet"), "la guardia nombra la red prohibida: {err}");
        // Y testnet mapea al servicio correcto.
        assert_eq!(red_de_servicio("testnet").unwrap(), SERVICIO_TESTNET);
        assert_eq!(SERVICIO_TESTNET, "ledgerita-core-testnet");
        // Red desconocida también rechaza.
        assert!(red_de_servicio("staging").is_err());
    }

    #[test]
    fn test_verificar_bundle_campos_y_guardias() {
        // La verificación completa caza: bundle con target equivocado
        // (otra plataforma) y red que no cubre TestNet.
        let seed = seed_real();
        let binario = b"binario-x";
        let tmp = std::env::temp_dir().join("updater_t_guardias");
        std::fs::create_dir_all(&tmp).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(binario);
        let sha = hex(&hasher.finalize());

        // target equivocado.
        let texto = build_manifest_text("v4.1", "testnet", "44ecfb6", "aarch64-linux", "2026-11-11", &sha);
        let sig = firma_de(&seed, texto.as_bytes());
        std::fs::write(tmp.join("RELEASE_MANIFEST.txt"), &texto).unwrap();
        std::fs::write(tmp.join("RELEASE_MANIFEST.sig"), hex(&sig.to_bytes())).unwrap();
        std::fs::write(tmp.join("release.bin"), binario).unwrap();
        let pk = pk_de(&seed);
        let revoked_vacio = tmp.join("vacio.txt");
        std::fs::write(&revoked_vacio, "# vacía\n").unwrap();
        let err = verificar_bundle(tmp.to_str().unwrap(), &pk, revoked_vacio.to_str().unwrap())
            .unwrap_err();
        assert!(err.contains("target"), "target de otra plataforma rechaza: {err}");

        // red equivocada (manifiesto solo-mainnet no cubre TestNet).
        let texto = build_manifest_text("v4.1", "mainnet", "44ecfb6", "x86_64-unknown-linux-gnu", "2026-11-11", &sha);
        let sig = firma_de(&seed, texto.as_bytes());
        std::fs::write(tmp.join("RELEASE_MANIFEST.txt"), &texto).unwrap();
        std::fs::write(tmp.join("RELEASE_MANIFEST.sig"), hex(&sig.to_bytes())).unwrap();
        let err = verificar_bundle(tmp.to_str().unwrap(), &pk, revoked_vacio.to_str().unwrap())
            .unwrap_err();
        assert!(err.contains("red") || err.contains("TestNet"), "red equivocada rechaza: {err}");

        // Bundle válido (red testnet, target host) pasa la mecánica.
        let pk_ok = bundle_valido(&tmp, &seed, binario);
        assert!(verificar_bundle(tmp.to_str().unwrap(), &pk_ok, revoked_vacio.to_str().unwrap()).is_ok());
        // Y la pubkey oficial de producción NO valida este bundle (otro firmante).
        let err2 = verificar_bundle(tmp.to_str().unwrap(), PK_OFICIAL, revoked_vacio.to_str().unwrap())
            .unwrap_err();
        assert!(err2.contains("firma") || err2.contains("inválida"), "otro firmante rechaza: {err2}");
    }

    #[test]
    fn test_backup_proc_exe_sin_servicio_rechaza() {
        // Sin servicio corriendo (pid 0) NO hay backup ⇒ NO hay swap
        // (U6: rollback garantizado antes de tocar nada).
        let tmp = std::env::temp_dir().join("updater_t_backup");
        std::fs::create_dir_all(&tmp).unwrap();
        let backup = tmp.join("backup.bin");
        // Un servicio inexistente en este host de tests ⇒ systemctl da
        // pid 0 o error ⇒ RECHAZO (el swap no ocurre sin backup).
        assert!(backup_proc_exe("servicio-inexistente-test", backup.to_str().unwrap()).is_err());
    }
}
