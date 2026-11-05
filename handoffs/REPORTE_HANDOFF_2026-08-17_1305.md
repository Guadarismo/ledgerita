# ✅ Reporte de Handoff — Ledgerita (2026-08-17 13:05)
**Fecha y hora:** 2026-08-17 13:05 (hora local, Windows)
**Último commit:** `6606ec9` — GUI wallet (egui) + monitor TUI (ratatui) + healthcheck desplegado + fix reconexión P2P
**Branch:** `main` — working tree limpio
**Tests:** 79 passed, 0 failed (core 70 release + wallet 4 + monitor TUI 5)

> **PARA LA NUEVA SESIÓN:** este reporte es autocontenido. Léelo completo y luego
> `REPORTE_SESION_2026-08-17.md` (detalle del día) y `REPORTE_FINAL_CONTEXTO.md`
> (historia/arquitectura). No asumas que recuerdas nada de la sesión anterior.

---

## 🎯 Resumen de lo hecho hoy (2026-08-17)

1. **Healthcheck de nodos** (pendiente baja prioridad del roadmap) — IMPLEMENTADO e instalado en producción (kettle03 + kettle02): `ledgerita-healthcheck.sh` + wrapper journal de mínimo privilegio + sudoers NOPASSWD + cron `*/15`.
2. **🆕 Bug crítico P2P ARREGLADO** (`ledgerita_core/src/network.rs`): los peers se desconectaban a los ~60s (`KeepAliveTimeout`) y **nunca reconectaban** (el peer nunca se re-agregaba a `pending_peers` al desconectar). Fix: re-insertar en `pending_peers` en `ConnectionClosed` + `idle_connection_timeout` 60s → **300s**. Verificado en vivo.
3. **🖥️ GUI de la wallet IMPLEMENTADA (Fase 1 MVP)** en DOS frameworks:
   - `ledgerita_wallet/` — **egui/eframe 0.35** (ventana, ratón, portapapeles).
   - `ledgerita_monitor_tui/` — **ratatui 0.30** (TUI para SSH/sin X11).
   - Ambos verificados contra producción en vivo.

---

## 🧱 Estructura del proyecto (IMPORTANTE)

```
C:\Users\user\Documents\job\Ledgerita\
├── ledgerita_core/                  ← NÚCLEO: librería + daemon P2P
│   ├── src/lib.rs                   ← crypto, transaction, state, network, error, serialize, storage, genesis, faucet
│   ├── src/main.rs                  ← CLI: --send, --mint, --faucet, --connect, --network, --data-dir
│   ├── src/bin/monitor.rs           ← monitor local (requiere nodo DETENIDO por lock de sled)
│   ├── src/bin/remote_monitor.rs    ← monitor remoto por P2P (1 shot, exit code 1 si anomalías)
│   ├── scripts/ledgerita-healthcheck.sh       ← healthcheck producción (is-active + sync P2P)
│   ├── scripts/ledgerita-journal.sh           ← wrapper journal read-only
│   ├── scripts/ledgerita-healthcheck.sudoers  ← regla NOPASSWD
│   ├── scripts/vendor-local-deps.ps1          ← regenera local_deps/
│   └── .cargo/config.toml           ← VENDORING: reemplaza crates-io por local_deps/ (solo aplica al build DENTRO de ledgerita_core)
├── ledgerita_wallet/                ← NUEVO: wallet egui/eframe (Fase 1 MVP)
│   └── src/main.rs
├── ledgerita_monitor_tui/           ← NUEVO: monitor ratatui por P2P
│   └── src/main.rs
├── deploy.ps1                       ← deploy a servidores (Send-File + SHA-256), parametrizado mainnet/testnet
├── REPORTE_*.md                     ← historial de sesiones (leer para contexto)
└── .gitignore                       ← target de los 3 crates, local_deps/, *.log, *.tar, wallet_recovery.env
```

### Relación entre crates (decisión de arquitectura)
- `ledgerita_wallet` y `ledgerita_monitor_tui` son **crates independientes** que dependen de `ledgerita_core` por `path`.
- NO son miembros de un workspace con el core → el `deploy.ps1` (que empaqueta solo `ledgerita_core/`) y el vendoring de `local_deps` quedan intactos.
- Los crates GUI compilan contra **crates.io directo** (necesitan internet). El core, al compilar desde su propio directorio, usa `local_deps` vendoreados (offline).
- Dependencias GUI: `eframe 0.35.0` (la 0.36 exige Rust 1.95; el entorno tiene 1.94), `ratatui 0.30.2`, `crossterm 0.29.0`, `libp2p 0.56`, `tokio 1.50`, `futures 0.3`, `ed25519-dalek 2.2.0`.

### ⚠️ API nueva de egui 0.35 (TRAMPA)
- `App::ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame)` (ANTES era `update(&mut self, ctx: &Context, frame)`).
- `CentralPanel::default().show(ui, |ui| ...)` (ANTES tomaba `&Context`).
- Copiar al portapapeles: `ctx.copy_text(String)`.
- Si una futura versión de egui rompe el build, revisar estos 3 puntos.

---

## 🖥️ GUI Wallet (`ledgerita_wallet/`) — Fase 1 MVP

### Cómo ejecutarla
```powershell
cd C:\Users\user\Documents\job\Ledgerita\ledgerita_wallet
.\target\release\ledgerita_wallet.exe        # binario ya compilado (14.4 MB)
# o: cargo run --release
```

### Qué hace
- **Desbloqueo**: password → `crypto::load_or_create_wallet` (crea si no existe; error claro si password incorrecta). Selector MainNet/TestNet y campo de data-dir editable.
- **Dashboard**: dirección (mono, botón "📋 Copiar dirección" usa portapapeles), balance LIT, supply vs hard cap 21M, restante al cap, fee pool, época, TX confirmadas, cuentas, red.
- **Enviar LIT**: receptor (64 hex), monto LIT, fee LIT → valida (formato, auto-envío, fondos vía `State::apply_transaction` que valida firma+nonce+saldo), firma con la wallet, aplica al estado local y persiste (`State::flush`).
- **Recibir**: el dashboard muestra la dirección completa + copiar.
- **NOTA (importante)**: la TX se aplica y persiste **localmente**; la difusión P2P NO está implementada (es Fase 2). Para broadcast real hay que conectar nodo.

### Detalles técnicos
- Conversión LIT↔sats propia: `lit_to_sats` / `format_lit` (1 LIT = 1_000_000 unidades, 6 decimales; mínimo `0.000001`). Sin dep extra.
- Defaults: `data_dir = ~/.ledgerita`, `network = Testnet`.
- Tests: 4 (conversión + inválidos + roundtrip). `cargo test` en `ledgerita_wallet/`.

---

## 🖥️ Monitor TUI (`ledgerita_monitor_tui/`) — verificado en vivo

### Cómo ejecutarla
```powershell
cd C:\Users\user\Documents\job\Ledgerita\ledgerita_monitor_tui
.\target\release\ledgerita_monitor_tui.exe --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1
# TestNet:
.\target\release\ledgerita_monitor_tui.exe --network testnet --connect /ip4/192.168.68.61/tcp/46565/p2p/12D3KooWNjCgE866sbqjr2626KEC3MGYKvYuVM6MvkuG5VhHvaaS
```
Flags: `--connect/-c` (repetible), `--refresh/-r` segs (default 5), `--network` (mainnet|testnet). `q`/`Esc` salir.

### Qué hace
- Conecta por libp2p a N peers, pide snapshot (`MSG_SYNC` = 0x02), muestra en vivo: Peer ID, dirección, supply (LIT) vs hard cap, state root (8 bytes), fee pool, TX, época, cuentas, estado (OK ✓ / SUPPLY EXCEDIDO 🚨 / desconectado).
- Re-sondea cada `--refresh` y en cada `ConnectionEstablished`. Colores: verde OK, amarillo conectando, rojo anomalía.
- Actor de red en hilo con runtime tokio, estado compartido `Arc<Mutex<SharedState>>`; UI principal con `ratatui::init()`/`restore()`.
- Tests: 5 (format_amount, root_prefix, extract_peer_id, supply limit).

### Verificación en vivo de hoy
- **MainNet**: supply 10.000095 LIT, state root `c0198d1f…`, fee pool 0.000005, 1 TX, época 0, 2 cuentas, OK ✓.
- **TestNet**: supply 1000000.005000 LIT, root `f084ffce…`, 1 TX, época 0, 2 cuentas, OK ✓.

---

## 🔗 Producción (servidores)

| Host | IP | Rol | Servicios |
|---|---|---|---|
| kettle03 | 192.168.68.61 | seed (escucha) | `ledgerita-core` (36565) + `ledgerita-core-testnet` (46565) |
| kettle02 | 192.168.68.64 | cliente | `ledgerita-core` (conecta a kettle03) + `ledgerita-core-testnet` |

- **SSH**: `ssh -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" user@<IP>`
- **Sudo (servidores)**: `echo '<SUDO_PASSWORD>' | sudo -S -p '' <cmd>`
- **Multiaddrs**:
  - MainNet seed: `/ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1`
  - TestNet seed: `/ip4/192.168.68.61/tcp/46565/p2p/12D3KooWNjCgE866sbqjr2626KEC3MGYKvYuVM6MvkuG5VhHvaaS`
- **Healthcheck instalado**: `/home/user/ledgerita-healthcheck.sh` (cron `*/15`), log `/home/user/backups/ledgerita/healthcheck.log`, webhook opcional `/home/user/ledgerita-webhook.url` (POST JSON `{"text": "..."}`).
- **Estado servicios**: `systemctl is-active ledgerita-core ledgerita-core-testnet` (ambos active en ambos hosts, verificado hoy).
- **Backups**: cron en servidores → `/home/user/backups/ledgerita/` (wallet.key + state). Restore test verificado en sesiones previas.
- **Credenciales de red (env de nodos, NO wallet)**: en `/etc/ledgerita/env` de cada servidor y en `REPORTE_FINAL_CONTEXTO.md` / `REPORTE_HANDOFF_2026-08-02.md`. Password de servidor: `<SUDO_PASSWORD>` (ambiente local de laboratorio).

### Comandos útiles de diagnóstico
```powershell
# Ver reconexión P2P en vivo (servidor)
ssh -i $env:USERPROFILE\.ssh\id_ed25519_ledgerita user@192.168.68.61 "echo '<SUDO_PASSWORD>' | sudo -S journalctl -u ledgerita-core -f | grep -E 're-agregado|Conexión establecida|Desconectado'"

# Healthcheck manual (servidor)
ssh -i $env:USERPROFILE\.ssh\id_ed25519_ledgerita user@192.168.68.61 "/home/user/ledgerita-healthcheck.sh"

# Estado en vivo con remote_monitor (1 shot, exit code 1 si anomalías)
cd ledgerita_core; cargo run --release --bin remote_monitor -- --json --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1
```

---

## 🧪 Tests (79 passed)

```
ledgerita_core (release):   70 passed  ← cargo test --release DENTRO de ledgerita_core (usa local_deps)
  - Unit lib 43, monitor 2, remote_monitor 15, helpers_advanced 4,
    network_isolation 2, P2P integration 1, P2P stress 1, P2P fork 1, P2P reconnect 1
ledgerita_wallet:           4 passed
ledgerita_monitor_tui:      5 passed
```

---

## 🗂️ Git

```powershell
git status                    # working tree limpio
git log --oneline             # 6606ec9 = último (GUI + healthcheck + fix P2P)
```
`git add` / `git commit` cuando haya cambios nuevos. NUNCA commitear `wallet_recovery.env`, `local_deps/`, `*.tar`, `target/` (ya en .gitignore).

---

## 🚀 Pendientes (próximas sesiones)

### Fase 2 — Utilidad real de la wallet (la próxima natural)
1. **Historial de TX** — mostrar últimas TX aplicadas (el core no guarda historial; habría que extender storage o mantener log local).
2. **Conexión a nodo remoto (sync en vivo) en la wallet** — reusar el patrón del monitor TUI (`ledgerita_core::network` + `MSG_SYNC`) para que la wallet traiga el estado real de la red, no solo el local.
3. **Difusión P2P de TX firmadas** — para que "Enviar" propague de verdad (hoy solo aplica local).
4. **Botón faucet TestNet** — wrapper del `--faucet` del CLI (solo TestNet, cap por dirección).

### Fase 3 — Pulido
- QR de dirección (`qrcode` crate), dark mode, selector MainNet/TestNet en dashboard (el radio ya está en desbloqueo), binario portable, (opcional) WASM.

### Otros
- Configurar webhook real en `/home/user/ledgerita-webhook.url` (opcional).
- Observar ciclos de desconexión/reconexión P2P para confirmar estabilidad a largo plazo (fix de hoy debería bastar, pero validar).
- El monitor TUI podría aceptar un `--data-dir` para modo local (hoy solo remoto).

---

## ⚠️ Trampas conocidas (NO repetir el debugging)

1. **Vendoring**: el core compila SOLO desde su directorio (usa `local_deps/`). Los crates GUI compilan desde su propio directorio (crates.io). NO mover el `.cargo/config.toml` ni `local_deps/`.
2. **egui 0.35 API**: `App::ui(ui, frame)` + `CentralPanel::show(ui, ...)` (ver arriba).
3. **Healthcheck + pipefail**: con `set -o pipefail`, `grep -q` corta el pipe (SIGPIPE) y sudo reporta error → falso "sin sync". Fix: capturar salida en variable antes de grep.
4. **Sled lock exclusivo**: `monitor.rs` (local) requiere el nodo DETENIDO. Para ver estado de un nodo corriendo, usar `remote_monitor` o el TUI (P2P).
5. **Nombre de archivo sudoers**: en `/etc/sudoers.d/` el archivo NO puede llevar punto (`ledgerita-healthcheck`, no `.sudoers`).
6. **Quoting PowerShell→SSH**: usar archivos `.sh` subidos con scp en lugar de cadenas largas con comillas.
7. **2 saldos distintos entre MainNet/TestNet** es NORMAL (cadenas aisladas por `chain_id` y `protocol_id`).

---

*Fin del handoff 2026-08-17 13:05. Estado: todo commiteado, working tree limpio, 79 tests OK, producción activa y verificada. ¡Gracias por la sesión! 💎*