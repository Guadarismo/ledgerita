# Reporte de Sesión — Ledgerita (Deploy producción + helpers P2P)
**Fecha:** 2026-08-02
**Tests:** 54 passed, 0 failed
**Último commit (previo):** `87c821a` — Respaldo sesion 2026-07-03

---

## Resumen

Sesión enfocada en producción: re-deploy del binario actualizado a ambos servidores y corrección de bugs en `deploy.ps1` que impedían el despliegue. Luego se completaron los pendientes de media prioridad en los helpers de test P2P.

---

## 🛠️ Cambios de Esta Sesión

### 1. Fix: `deploy.ps1` no podía ejecutarse en PowerShell 5.1
**Archivo:** `deploy.ps1`

Tres bugs encadenados que rompían el deploy:

- **String sin terminar** en las líneas de SFTP: `$name":"` estaba escrito como `$name":` (faltaba el `"` de cierre). El parser de PowerShell abortaba con `Unexpected token '}'`.
- **Encoding UTF-8 sin BOM**: el archivo contiene emojis/box-drawing (`─`, `✅`). PowerShell 5.1 lo leía como ANSI corrompiendo caracteres. Fix: añadir BOM UTF-8.
- **Tarball vacío**: `tar cf ... ledgerita_core/` con path relativo + slash final fallaba con `Couldn't visit directory` y producía un tar de 0 entradas → el deploy subía código vacío y los servidores compilaban un no-op. Fix: usar `-C $ProjectRoot` con `--exclude="ledgerita_core/target"` y `--exclude="ledgerita_core/local_deps"`.

**Además:**
- IPs corregidas: kettle03 `192.168.68.59` → `192.168.68.61`, kettle02 `192.168.68.60` → `192.168.68.64` (también en el ejemplo del paso 2).
- El deploy ya **no sobrescribe** un service file existente (`if [ ! -f ... ]`), preservando los flags `--listen-port`/`--connect` de cada servidor.
- Añadida verificación `$LASTEXITCODE` tras crear el tar (aborta si falla).

### 2. Sudo passwordless para deploy en servidores
**Archivo (en servidores):** `/etc/sudoers.d/ledgerita-deploy`

- Regla `NOPASSWD` limitada a los comandos exactos que usa el deploy: `cp` (service file), `mkdir -p /etc/ledgerita`, `tee`/`chmod` (env), `systemctl daemon-reload|enable|start|restart|stop`, `cat /etc/ledgerita/env`.
- Verificado con `visudo -cf` en ambos servidores.

### 3. Deploy a producción ✅
- Binario actualizado compilado en **kettle03** (Ubuntu, `.61:36565`) y **kettle02** (Debian 12, `.64`). Incluye fixes de la sesión 07-03 (mint + retry P2P).
- `/etc/ledgerita/env` corregido en ambos: se eliminó un BOM invisible que hacía que systemd ignorara `LEDGERITA_PASSWORD` (crash loop). Se añadió `RUST_LOG=info`.
- Servicios reiniciados: ambos **active**.

### 4. Verificación de sincronización ✅
- kettle03 recibe conexión de kettle02 (`Conexión establecida peer_id=12D3KooWGrahZ1...`).
- Ping: ~5-6ms.
- Ambos nodos: `Estado ya sincronizado`, `Snapshot de estado enviado count=2`.
- Remote monitor local confirmó: `status: "healthy"`, supply 10.000095 LIT, state_root `c0198d1fcf725b28`, 2 cuentas, tx_count 1, epoch 0.

### 5. Helpers P2P: media prioridad
**Archivo:** `ledgerita_core/tests/helpers/mod.rs`

- **`wait_for_listen_addr`**: ahora acepta loopback IPv4 (`127.0.0.1`) **e IPv6** (`::1`) vía helper `is_loopback()`.
- **Nueva `wait_for_listen_addrs`**: recolecta múltiples multiaddrs loopback con timeout de 150ms de silencio.
- **`TempTestDirs::with_labels(["seed", ...])`**: crea N directorios con nombres personalizados, más método `get(label)`. Mantiene compatibilidad con campos legacy `a`/`b`/`c` (se auto-crean si no se pasan).

### 6. Nuevo test: `tests/helpers_advanced.rs` (4 tests)
- `test_wait_for_listen_addr_ipv6` — escucha en `::1`, verifica loopback IPv6.
- `test_wait_for_listen_addrs_multiple` — IPv4 + IPv6 simultáneos.
- `test_temp_dirs_with_labels` — labels personalizados + legacy.
- `test_temp_dirs_default_legacy_fields` — compatibilidad `a`/`b`/`c`.

---

## 🧪 Tests (54 passed)

```
Unit tests (lib)         29 passed
monitor                  2 passed
remote_monitor          15 passed
helpers_advanced         4 passed  ← NUEVO
P2P integration          1 passed
P2P stress               1 passed
P2P fork                 1 passed
P2P reconnect            1 passed
───────────────────────────────────────
TOTAL                   54 passed, 0 failed
```

---

## 🌐 Red de Producción (estado post-deploy)

| Nodo | IP | OS | Rol | Puerto | PeerId |
|---|---|---|---|---|---|
| kettle03 | 192.168.68.61 | Ubuntu | Seed | 36565 | `12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1` |
| kettle02 | 192.168.68.64 | Debian 12 | Cliente | dinámico | `12D3KooWGrahZ1gW5pFBu9Q1U1QEryPyEUGh8rCGGJJzuuPoHLdu` |

**Estado:** Ambos nodos sincronizados con binario actualizado.
- Supply: 10.000095 LIT, tx_count: 1, epoch: 0, 2 cuentas.
- State Root: `c0198d1fcf725b28...`
- Logs con formato `tracing` (RUST_LOG=info en `/etc/ledgerita/env`).

---

## 🟡 Pendientes para Próxima Sesión

### Baja Prioridad
1. **Reducir ruido de logs** — `ResponseSent` event en `network.rs` cambiar a `debug!`/`trace!`
2. **Documentación de código** — Docstrings en `RawCodec`, `State::apply_transaction`, `network::run_p2p_node`
3. **TestNet formal** — Chain ID / network magic en handshake P2P, génesis alternativo con faucet
4. **Script `local_deps/`** — Script que ejecute `cargo vendor local_deps` automáticamente
5. **Límite de logs systemd** — Configurar `SystemMaxUse=50M` en journald

### Limpieza / Infra
6. **Eliminar `ledgerita_core.tar`** de la raíz del repo (residuo del deploy; ya está en `.gitignore`)
7. **Restaurar SCP vs SFTP**: el deploy de kettle03 usa SCP y kettle02 SFTP (por inestabilidad histórica de SCP >10KB en kettle02) — verificar si sigue siendo necesario

---

## ⚠️ Notas Técnicas Importantes

1. **`deploy.ps1` requiere PowerShell con soporte UTF-8**: el archivo ahora tiene BOM. Usar PowerShell 5.1+ (o pwsh) para ejecutarlo.
2. **El tar debe usar `-C $ProjectRoot`** con excludes prefijados (`ledgerita_core/target`), no path relativo con `/`.
3. **Los service files de producción tienen flags**: no sobrescribirlos (el deploy ya los preserva).
4. **`/etc/ledgerita/env`**: no debe tener BOM. Contiene `LEDGERITA_PASSWORD` y `RUST_LOG=info`.
5. **Sudo passwordless** en `/etc/sudoers.d/ledgerita-deploy` solo permite los comandos del deploy (mínimo privilegio).
6. **1 LIT = 1,000,000 satoshis**: `format_amount()` divide entre 1,000,000 para display.
7. **Sled lock exclusivo**: monitor local requiere nodo DETENIDO.
8. **Para crear un nuevo test P2P**: usar `mod helpers;` e importar `build_test_swarm`, `wait_for_listen_addr`, `wait_for_listen_addrs`, `TempTestDirs`.

---

## 💻 Comandos Útiles

```powershell
# Tests completos
cd ledgerita_core; cargo test

# Test de helpers avanzados
cd ledgerita_core; cargo test --test helpers_advanced -- --nocapture

# Desplegar a servidores
.\deploy.ps1

# SSH
ssh -i ~/.ssh/id_ed25519_ledgerita kettle03
ssh -i ~/.ssh/id_ed25519_ledgerita kettle02

# Logs en vivo (requiere sudo para ver todo)
ssh -i ~/.ssh/id_ed25519_ledgerita kettle03 'journalctl -u ledgerita-core -f'

# Remote monitor
.\ledgerita_core\target\release\remote_monitor.exe --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1
```

---

*Generado el 2026-08-02. Producción actualizada y helpers P2P generalizados.*
