# ✅ Reporte de Sesión — Ledgerita (2026-08-17)
**Fecha y hora:** 2026-08-17 ~10:00 (hora local)
**Tests:** 70 passed (core, release) + 4 wallet + 5 monitor TUI = **79 tests OK, 0 failed**
**Estado git:** pendiente de commit

---

## 🎯 Resumen de la sesión

1. **Healthcheck de nodos** (pendiente baja prioridad del roadmap) — IMPLEMENTADO, instalado y verificado en vivo en ambos servidores.
2. **🆕 Bug de producción encontrado y ARREGLADO**: los nodos se desconectaban a los ~60s (`KeepAliveTimeout`) y **nunca reconectaban**. Esto rompía la propagación de TX y era el motivo por el que el healthcheck de sync daba falsas alarmas.
3. **🖥️ Interfaz gráfica de la wallet IMPLEMENTADA (ambos frameworks)**: `ledgerita_wallet` (egui/eframe) + `ledgerita_monitor_tui` (ratatui). Ver sección al final.

---

## 🐛 Bug crítico: reconexión P2P rota (encontrado durante el healthcheck)

**Síntoma (verificado en journal de ambos hosts):**
- Tras un deploy/reinicio, los nodos conectaban, sincronizaban y ~60s después se desconectaban con `cause=Some(KeepAliveTimeout)`.
- Después de eso, **20+ minutos de silencio total** (0 pings, 0 reconexiones) hasta el siguiente reinicio manual.
- Consecuencia: una TX pendiente NO se propagaba (el broadcast solo ocurre tras un sync, y el sync solo ocurre en `ConnectionEstablished`).

**Causa raíz en `ledgerita_core/src/network.rs`:**
1. Al conectar un peer, se ejecutaba `pending_peers.remove(&peer_id)` (line ~321) → el peer salía de la lista de reintentos.
2. En `ConnectionClosed` **nunca se re-agregaba** a `pending_peers`, así que el dial automático de 30s (line ~309) ya no lo volvía a intentar.
3. Combinado con `with_idle_connection_timeout(60s)` (line ~241), la conexión se caía y jamás volvía.

**Fix aplicado (`network.rs`):**
- En `ConnectionClosed`, si el peer pertenece a `connect_peers`, se re-inserta en `pending_peers` → el retry de 30s lo reconecta. Nuevo log: `Peer re-agregado a lista de reintentos`.
- `idle_connection_timeout` subido de 60s → **300s** (los pings verifican liveness; menos churn).

**Verificación en vivo (tras deploy):**
- `09:51:49` conexión establecida → pings cada ~15s durante 4+ min (antes se cortaba a los 60s).
- `09:56:49` desconexión (`KeepAliveTimeout`) → `Peer re-agregado a lista de reintentos` → **`09:57:19` reconexión** (30s después, exactamente el intervalo de retry).
- 18 pings en 5 min en ambos servicios y ambos hosts. Red auto-recuperable.

---

## 🩺 Healthcheck de nodos (NUEVO)

### Archivos (commiteados en `ledgerita_core/scripts/`)
- **`ledgerita-healthcheck.sh`**: comprueba que `ledgerita-core` y `ledgerita-core-testnet` estén `active` y que cada nodo tenga actividad P2P reciente (pings/sync) en el journal (ventana 15 min).
- **`ledgerita-journal.sh`**: wrapper read-only (mínimo privilegio) que permite a `user` leer SOLO los journals de los servicios Ledgerita vía sudo NOPASSWD. Instalado en `/usr/local/bin/ledgerita-journal`.
- **`ledgerita-healthcheck.sudoers`**: regla `user ALL=(root) NOPASSWD: /usr/local/bin/ledgerita-journal`. Instalado en `/etc/sudoers.d/ledgerita-healthcheck` (nombre sin punto, requerido por sudoers.d).

### Alertas (dedupe por transición de estado)
- **Down → alarma**: `ALERTA [svc] DOWN → ...` + webhook opcional.
- **Recuperación**: `RECUPERADO [svc] ...` + webhook.
- **Persistencia**: `PERSISTE [svc] ...` (solo log, sin re-alerta por cada run).
- Log persistente: `/home/user/backups/ledgerita/healthcheck.log`.
- **Webhook opcional** (Telegram/Discord/Slack): crear `/home/user/ledgerita-webhook.url` con la URL. POST JSON `{"text": "..."}` vía curl.

### Degradación elegante
Si el wrapper no está instalado (sin sudo NOPASSWD), degrada a solo-check de `is-active` y lo anota en el log (`WARN journal no accesible`), reportando `ok` con "sync no verificada".

### Instalación (verificada en kettle03 y kettle02)
- Script: `/home/user/ledgerita-healthcheck.sh` (chmod 700).
- Wrapper: `/usr/local/bin/ledgerita-journal` (chmod 755).
- Sudoers: `/etc/sudoers.d/ledgerita-healthcheck` (chmod 440, `visudo -cf` OK).
- Cron: `*/15 * * * * /home/user/ledgerita-healthcheck.sh`.
- Resultado primer run: `RECUPERADO [ledgerita-core] ... con actividad P2P` + idem testnet (ambos hosts). Segunda ejecución silenciosa (dedupe OK).

### Trampa resuelta durante el desarrollo
Con `set -o pipefail`, `grep -q` corta el pipe con SIGPIPE y `sudo` reporta error → falso "sin sync". Fix: capturar la salida del wrapper en una variable y luego grep.

---

## 🧪 Tests (70 passed, release)

```
Unit tests (lib)         43 passed
monitor                  2 passed
remote_monitor          15 passed
helpers_advanced         4 passed
network_isolation        2 passed
P2P integration          1 passed
P2P stress               1 passed
P2P fork                 1 passed
P2P reconnect            1 passed
```
(Sin tests nuevos; el cambio en `network.rs` se validó con el deploy + verificación en vivo. El test `p2p_reconnect` cubre el resync tras reconexión.)

---

## 📋 Pendientes / notas

- **Healthcheck: COMPLETO** (era el último de la prioridad 2). Opcional futuro: configurar webhook real en `/home/user/ledgerita-webhook.url`.
- **GUI de la wallet: Fase 1 IMPLEMENTADA** (ver sección abajo). Fases 2 y 3 pendientes.
- La red ahora es auto-recuperable; conviene observar un par de ciclos de desconexión/reconexión para confirmar estabilidad a largo plazo.

---

## 🖥️ PLAN: Interfaz Gráfica de la Wallet (HidroLedger)

### Decisión de framework
- **Wallet (HidroLedger): `egui` + `eframe`** (immediate mode, puro Rust).
  - Binario único ~5MB, multiplataforma (Win/Linux/macOS/WASM), corre en hardware viejo. Respeta la filosofía "ultra-ligero".
  - Immediate mode = UI como funciones Rust, iteración rápida, sin runtime pesado.
  - Accesible al usuario final (ventanas + ratón + QR), no requiere terminal.
- **Monitor de servidores: `ratatui` (TUI)** como complemento opcional para SSH/sin X11.
- **No usar** Tauri (trae Chromium, pesado) ni web (contradice la filosofía).

### Arquitectura (IMPLEMENTADA)
```
ledgerita_core/             (ya existía, como librería)
└── src/lib.rs              ← los 2 crates GUI lo reusan (crypto, state, storage, serialize, network)
ledgerita_wallet/           (NUEVO crate — egui/eframe)
└── src/main.rs             ← desbloqueo, dashboard, enviar, recibir
ledgerita_monitor_tui/      (NUEVO crate — ratatui)
└── src/main.rs             ← monitor P2P en vivo de nodos
```
- Los 2 crates son **independientes** (no miembros de un workspace del core): dependen de `ledgerita_core` por `path`. Así el deploy de servidores (`deploy.ps1`, que empaqueta solo `ledgerita_core/`) y el vendoring de `local_deps` del core quedan intactos. La GUI compila contra crates.io directamente.
- La wallet no toca P2P en el MVP: opera sobre el `data-dir` local (wallet encriptada) y aplica/persiste TX localmente.
- El monitor TUI SÍ usa P2P: reutiliza `RawCodec`/`build_msg`/`split_msg`/`MSG_SYNC` de `ledgerita_core::network` con la misma lógica que `remote_monitor.rs`, pero en loop continuo con refresh.

### Fases
- **Fase 1 — MVP ✅ IMPLEMENTADA**: desbloqueo (password), dashboard (dirección/balance/supply/epoch/tx_count), enviar LIT, recibir (dirección + copiar).
- **Fase 2 — Utilidad real (pendiente)**: historial de TX, conexión a nodo remoto (sync en vivo) en la wallet, botón faucet TestNet.
- **Fase 3 — Pulido (pendiente)**: QR de dirección (`qrcode`), dark mode, selector MainNet/TestNet (la wallet ya tiene el radio en la pantalla de desbloqueo), binario portable, (opcional) WASM.

### Consideraciones de seguridad
1. Password pedida en la GUI (no en env var visible) — hoy el CLI exige `LEDGERITA_PASSWORD`.
2. `wallet.key` con permisos 0600 (ya verificado).
3. Avisar que NO es dinero real aún (proyecto de aprendizaje) — ambos crates lo muestran en su UI.
4. Mantener intactos hard cap 21M y anti-replay — la GUI solo consume la lib (`State::apply_transaction` valida firma + nonce + saldo).

---

## 🖥️ IMPLEMENTADO: Wallet egui/eframe (`ledgerita_wallet/`)

### Qué hace (Fase 1)
- **Desbloqueo**: pide password y usa `crypto::load_or_create_wallet` (crea si no existe; error claro si la password es incorrecta). Selector MainNet/TestNet y campo de data-dir editable.
- **Dashboard**: dirección (mono), balance en LIT, supply total vs hard cap (21M), restante al cap, fee pool, época, TX confirmadas, cuentas, red. Botón "Copiar dirección" (usa el portapapeles de egui).
- **Enviar**: receptor (64 hex), monto en LIT, fee en LIT → valida (formato, auto-envío, fondos vía `apply_transaction`), firma con la clave de la wallet, aplica al estado local y persiste (`State::flush`). Muestra resultado OK/error.
- **Recibir**: el dashboard muestra la dirección completa + copiar.

### Detalles técnicos
- **egui/eframe 0.35** (la 0.36 exige Rust 1.95; el entorno tiene 1.94). API nueva: `App::ui(&mut self, ui, frame)` y `CentralPanel::show(ui, ...)` (cambió en 0.35, ya adaptado).
- Conversión LIT↔sats propia (`lit_to_sats` / `format_lit`), sin dep extra, con roundtrip testado.
- **Tests: 4 passed** (conversión de unidades + casos inválidos + roundtrip).
- Binario release ~48MB en debug; release optimizado. Compila limpio en Windows.

### Cómo ejecutar
```powershell
cd ledgerita_wallet
cargo run --release
```

---

## 🖥️ IMPLEMENTADO: Monitor TUI ratatui (`ledgerita_monitor_tui/`)

### Qué hace
- Conecta por **libp2p** a N nodos remotos (flag `--connect` repetible), les pide el snapshot de estado (`MSG_SYNC`) y muestra **en vivo**:
  - Tabla por peer: Peer ID, dirección, supply (LIT) vs hard cap, state root, fee pool, TX, época, cuentas, estado (OK ✓ / SUPPLY EXCEDIDO 🚨 / desconectado).
  - Banners: red, refresh, uptime, nº de peers.
- Re-sondea cada `--refresh` segundos (default 5s) y en cada `ConnectionEstablished`.
- Colores por estado (verde OK, amarillo conectando, rojo anomalía). `q`/`Esc` para salir.

### Detalles técnicos
- **ratatui 0.30** + crossterm 0.29; actor de red en un hilo con runtime tokio y estado compartido vía `Arc<Mutex<SharedState>>`; UI en el hilo principal con `ratatui::init()`/`restore()`.
- Misma versión libp2p 0.56 y protocolo que el core → compatible con MainNet y TestNet.
- **Tests: 5 passed** (format_amount, root_prefix, extract_peer_id, supply limit).
- **Verificado en vivo contra producción**: MainNet (supply 10.000095 LIT, root c0198d1f..., 2 cuentas, OK ✓) y TestNet (supply 1000000.005000 LIT, root f084ffce..., OK ✓).

### Cómo ejecutar
```powershell
cd ledgerita_monitor_tui
cargo run --release -- --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1
cargo run --release -- --network testnet --connect /ip4/192.168.68.61/tcp/46565/p2p/12D3KooWNjCgE866sbqjr2626KEC3MGYKvYuVM6MvkuG5VhHvaaS
```

---

## 💻 Comandos útiles (esta sesión)

```powershell
# Healthcheck manual (en el servidor)
/home/user/ledgerita-healthcheck.sh

# Log de healthcheck
cat /home/user/backups/ledgerita/healthcheck.log

# Configurar webhook opcional (en el servidor)
echo "https://api.telegram.org/bot<TOKEN>/sendMessage?chat_id=<ID>" > /home/user/ledgerita-webhook.url

# Ver reconexión P2P en vivo
echo 'clavefacil' | sudo -S -p '' journalctl -u ledgerita-core -f | grep -E 're-agregado|Conexión establecida|Desconectado'

# Estado de servicios
systemctl is-active ledgerita-core ledgerita-core-testnet

# Monitor TUI (desde Windows)
cd ledgerita_monitor_tui; cargo run --release -- --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1

# Wallet (desde Windows)
cd ledgerita_wallet; cargo run --release
```

---

*Reporte de sesión 2026-08-17. Healthcheck implementado + bug de reconexión P2P arreglado y desplegado en producción. Plan de GUI de la wallet incluido (framework decidido: egui/eframe para la wallet, ratatui como monitor opcional). Fase 1 de la GUI implementada y verificada en vivo (wallet egui + monitor ratatui).*