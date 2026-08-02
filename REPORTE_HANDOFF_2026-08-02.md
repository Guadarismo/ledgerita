# ✅ Reporte Final de Sesión — Ledgerita
**Fecha y hora:** 2026-08-02 10:10 (hora local)
**Tests:** 54 passed, 0 failed
**Estado git:** limpio (working tree sin cambios pendientes)
**Último commit:** `c93134d` — Unificar SCP/SFTP en deploy.ps1

---

## 🎯 Resumen de lo logrado esta sesión (2026-08-02)

Sesión completa de producción + limpieza técnica. **Todos los pendientes del roadmap quedaron cerrados** excepto el TestNet formal.

1. **Fix `deploy.ps1`** (3 bugs que impedían el deploy).
2. **Deploy a producción** con binario actualizado (2 nodos sincronizados).
3. **Helpers P2P generalizados** (IPv6, multiaddr múltiples, TempTestDirs con labels).
4. **Baja prioridad completada**: logs (`trace!`), docstrings, journald 50M, script vendor.
5. **Unificación SCP/SFTP** en deploy.ps1 + **segundo deploy real** con verificación SHA-256.

---

## 🛠️ Cambios de Código (commitear)

### `deploy.ps1` (refactorizado esta sesión)
- **IPs corregidas**: kettle03 `192.168.68.59→.61`, kettle02 `192.168.68.60→.64`.
- **BOM UTF-8**: requerido para PowerShell 5.1 (el archivo tiene emojis; sin BOM los lee como ANSI corruptos).
- **Tar**: `tar cf $TarFile -C $ProjectRoot --exclude="ledgerita_core/target" --exclude="ledgerita_core/local_deps" ledgerita_core` (¡obligatorio `-C` con path relativo, sin trailing slash; path absoluto extrae en `/home/user/Users/...`).
- **`Send-File` function**: SCP con fallback a SFTP (`sftp -b batch`) + **verificación SHA-256 post-transferencia** (compara `Get-FileHash` local vs `sha256sum` remoto, aborta en mismatch).
- **Ya NO sobrescribe service files existentes** (`if [ ! -f ... ]`) — preserva flags `--listen-port`/`--connect` de cada servidor.
- Los 3 bugs originales: string sin terminar en `$name":"` (faltaba comilla), encoding sin BOM, tar vacío por path relativo con `/`.

### `ledgerita_core/tests/helpers/mod.rs`
- `is_loopback(addr)`: detecta loopback IPv4 (`127.0.0.1`) e IPv6 (`::1`).
- `wait_for_listen_addr`: ahora usa `is_loopback()` (soporta IPv6).
- `wait_for_listen_addrs` (NUEVA): recolecta múltiples multiaddrs loopback con timeout de 150ms de silencio.
- `TempTestDirs`: `with_labels(["seed", ...])` + `get(label)`. Compatible con campos legacy `a`/`b`/`c` (se auto-crean).
- Imports añadidos: `use std::collections::HashMap;`, `use std::time::Duration;`.

### `ledgerita_core/tests/helpers_advanced.rs` (NUEVO — 4 tests)
`test_wait_for_listen_addr_ipv6`, `test_wait_for_listen_addrs_multiple`, `test_temp_dirs_with_labels`, `test_temp_dirs_default_legacy_fields`.

### `ledgerita_core/src/network.rs`
- `ResponseSent` event: `info!` → `trace!` (reduce ruido en producción). Import `trace` añadido.
- Docstring en `RawCodec` (formato length-prefix: 4 bytes big-endian + payload) y `run_p2p_node` (bucle de eventos, no retorna salvo error fatal).

### `ledgerita_core/src/state.rs`
- Docstring en `apply_transaction` (validaciones: firma, nonce anti-replay, overflow, saldo; estado intacto si falla).

### `ledgerita_core/scripts/vendor-local-deps.ps1` (NUEVO)
- Regenera `local_deps/` vía `cargo vendor` (build offline Windows).
- Verifica `Cargo.toml`/`Cargo.lock`, limpia previo, borra `local_deps/config.toml` generado por cargo, verifica con `cargo check --offline`.
- Fallback documentado para rutas largas en Windows (comentar `.cargo/config.toml`).
- Uso: `powershell -ExecutionPolicy Bypass -File ledgerita_core/scripts/vendor-local-deps.ps1`

### Infra (en servidores, NO en git)
- `/etc/systemd/journald.conf`: `SystemMaxUse=50M` en ambos servidores + journald reiniciado.
- `/etc/sudoers.d/ledgerita-deploy`: reglas NOPASSWD específicas del deploy (local en `C:\Users\user\AppData\Local\Temp\opencode\ledgerita-deploy-sudoers`).

---

## 🌐 Producción (ESTADO ACTUAL — verificado 2026-08-02 10:05)

| Nodo | IP | OS | Rol | Puerto | PeerId |
|---|---|---|---|---|---|
| kettle03 | 192.168.68.61 | Ubuntu | Seed | 36565 | `12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1` |
| kettle02 | 192.168.68.64 | Debian 12 | Cliente | dinámico | `12D3KooWGrahZ1gW5pFBu9Q1U1QEryPyEUGh8rCGGJJzuuPoHLdu` |

- **Binario**: el actual (con `ResponseSent`→trace, docstrings, fixes de sesión 07-03).
- **Estado del ledger**: supply 10.000095 LIT, tx_count 1, epoch 0, 2 cuentas, state_root `c0198d1fcf725b28`, fee_pool 0.000005 LIT.
- **Sync**: ambos nodos conectados, "Estado ya sincronizado", snapshot count=2, ping ~13-15ms.
- **Logs**: formato tracing, `RUST_LOG=info`, 0 ocurrencias de "Respuesta enviada".
- Service files: kettle03 `--listen-port 36565`; kettle02 `--connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1`.

---

## 🔑 Credenciales y Accesos (CRÍTICO — no se recuerda entre sesiones)

- **SSH key**: `$env:USERPROFILE\.ssh\id_ed25519_ledgerita` (hosts aliases en `~/.ssh/config`: `kettle02`, `kettle03`, `celular`).
- **Sudo en servidores**: password `clavefacil` (usar `echo 'clavefacil' | sudo -S -p '' <cmd>`).
  - ⚠️ En kettle03 el `sudo -n` para restart rechazó pese a estar en sudoers → usar sudo con password.
- **Wallet password**: `obegtBl4U5CPqzSMJ3mOhEvDWGIHfyL6` (en `wallet_recovery.env`, gitignored).
- **Wallets**: kettle03=`b1435c81522ac1e2b0f503986ca93bde92542b5d4cab4b9a9fd21253667e4cba`, kettle02=`6893239f5850555f5701f24342bfbef5afdab94835f08d6e5f352899d4a17f28`.
- **Env files**: `/etc/ledgerita/env` en cada servidor contiene `LEDGERITA_PASSWORD=obegtBl4U5CPqzSMJ3mOhEvDWGIHfyL6` + `RUST_LOG=info`. **NO debe tener BOM** (rompe el parsing de systemd).

---

## 📋 Pendientes para PRÓXIMA SESIÓN

### 🟡 ÚNICO pendiente restante: **TestNet formal** (baja prioridad)
- **Chain ID / network magic en handshake P2P**: distinguir redes para que nodos de distintas redes no se mezclen.
- **Génesis alternativo con faucet**: red de prueba separada de mainnet.
- Toca: `network.rs` (handshake/protocolo), `state.rs` (merge/sync), genesis.
- ⚠️ **Implica romper compatibilidad de estado producido** → requiere decisión de migración o reset.
- Es la feature más grande del roadmap: merece sesión dedicada con plan previo.

### Tareas futuras exploratorias (sin prioridad asignada, del backlog histórico)
- Docstrings restantes en otros módulos (solo se documentaron 3 este día).
- Mejoras UX del CLI / monitor.
- Cualquier ampliación del protocolo.

---

## 🧪 Tests (54 passed)

```
Unit tests (lib)         29 passed
monitor                  2 passed
remote_monitor          15 passed
helpers_advanced         4 passed  ← NUEVO esta sesión
P2P integration          1 passed
P2P stress               1 passed
P2P fork                 1 passed
P2P reconnect            1 passed
```

---

## ⚠️ Notas Técnicas CRÍTICAS (trapañas del proyecto)

1. **`deploy.ps1`**: requiere BOM UTF-8 (PowerShell 5.1). No sobrescribe service files existentes.
2. **Subida unificada**: SCP con fallback SFTP + SHA-256. El SCP a kettle02 YA es fiable (3/3 PASS con 0.29MB).
3. **`.cargo/config.toml` en servidores**: eliminar tras extraer tar (`rm -f ledgerita_core/.cargo/config.toml`) — referencia `local_deps` que no existe ahí (el deploy ya lo hace).
4. **Sled lock exclusivo**: monitor local (`monitor.exe`) requiere nodo DETENIDO.
5. **`/etc/ledgerita/env`**: sin BOM. 600 permisos.
6. **1 LIT = 1,000,000 satoshis**; hard cap 21,000,000 LIT.
7. **Checkpoint interval**: 4096 transacciones. Supply real = `total_supply() + fee_pool`.
8. **Sin serde**: serialización manual en `serialize.rs`.
9. **Sudoers del deploy**: limitado a comandos específicos (cp service, mkdir, tee env, chmod, systemctl daemon-reload/enable/start/restart/stop, cat env). Los demás comandos sudo requieren password `clavefacil`.
10. **journalctl en kettle02 requiere sudo**.
11. **Para crear un nuevo test P2P**: `mod helpers;` + `build_test_swarm`, `wait_for_listen_addr`, `wait_for_listen_addrs`, `TempTestDirs`, y `State::merge_if_fresher`.
12. **Prune antes que insert**: `State::flush()` prunea antes de insertar (fix 06-19-v2). No revertir.
13. **Orden broadcast P2P**: TXs pendientes se envían DESPUÉS del sync (fix 06-18).
14. **Retry dial automático**: peers pendientes se reintentan cada 30s.
15. **Igualdad de PeerId para reconexión**: clonar `identity::Keypair` para reconstruir swarm con mismo PeerId.

---

## 💻 Comandos Útiles (PowerShell)

```powershell
# Tests completos
cd ledgerita_core; cargo test

# Tests de helpers avanzados
cd ledgerita_core; cargo test --test helpers_advanced -- --nocapture

# Deploy a producción (compila+tests local, sube, compila en servidores)
.\deploy.ps1

# Regenerar vendor (build offline local)
powershell -ExecutionPolicy Bypass -File ledgerita_core/scripts/vendor-local-deps.ps1

# SSH
ssh -i ~/.ssh/id_ed25519_ledgerita kettle03
ssh -i ~/.ssh/id_ed25519_ledgerita kettle02

# Sudo con password en servidores
echo 'clavefacil' | sudo -S -p '' systemctl restart ledgerita-core

# Logs en vivo
ssh -i ~/.ssh/id_ed25519_ledgerita kettle03 "echo 'clavefacil' | sudo -S -p '' journalctl -u ledgerita-core -f"

# Remote monitor (requiere servicio RUNNING)
.\ledgerita_core\target\release\remote_monitor.exe --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1
```

---

## 📁 Estructura del Repo (relevante)

```
Ledgerita/
├── deploy.ps1                          # Deploy script (refactorizado esta sesión)
├── wallet_recovery.env                 # Credenciales wallet (GITIGNORED)
├── REPORTE_SESION_2026-08-02.md        # Reporte de esta sesión (detallado)
├── REPORTE_FINAL_SESION_2026-07-03.md  # Reporte anterior (contexto histórico)
└── ledgerita_core/
    ├── .cargo/config.toml              # Source-replacement → local_deps (SOLO local)
    ├── local_deps/                     # Vendored crates (gitignored, regenerable)
    ├── scripts/
    │   ├── ledgerita-core.service      # systemd unit (se preserva en deploy)
    │   ├── setup-systemd.sh            # Setup manual de systemd en servidor
    │   └── vendor-local-deps.ps1       # NUEVO: regenerar vendor
    ├── src/                            # network.rs, state.rs, serialize.rs, crypto.rs, storage.rs, transaction.rs, main.rs
    ├── src/bin/                        # monitor.rs, remote_monitor.rs
    └── tests/                          # helpers/mod.rs, helpers_advanced.rs (nuevo), p2p_*.rs
```

---

*Reporte de handoff completo generado el 2026-08-02 10:10. Para retomar en nueva sesión sin pérdida de contexto.*
