# ✅ HANDOFF — Ledgerita (vigente)

> **CONVENCIÓN:** este archivo tiene **nombre fijo** y siempre contiene el handoff
> **actual** (el de la sesión más reciente). Cualquier sesión nueva debe empezar
> leyendo ESTE archivo completo. Las versiones anteriores viven en `handoffs/`
> (ver `handoffs/INDEX.md` para buscar una sesión del pasado).
>
> Última actualización: **2026-11-05** (sesión /review: mejoras de handoff + auditoría informal del código)

---

## 🎯 Última sesión (2026-08-17)

1. **Healthcheck de nodos** — implementado e instalado en producción (kettle03 + kettle02): `ledgerita-healthcheck.sh` + wrapper journal de mínimo privilegio + sudoers NOPASSWD + cron `*/15`.
2. **Bug crítico P2P ARREGLADO** (`ledgerita_core/src/network.rs`): los peers se desconectaban a los ~60s (`KeepAliveTimeout`) y nunca reconectaban (el peer no volvía a `pending_peers` al desconectar). Fix: re-insertar en `pending_peers` en `ConnectionClosed` + `idle_connection_timeout` 60s → **300s**. Verificado en vivo (reconexión automática a los 30s).
3. **GUI de la wallet (Fase 1 MVP)** en DOS frameworks:
   - `ledgerita_wallet/` — **egui/eframe 0.35** (ventana, ratón, portapapeles).
   - `ledgerita_monitor_tui/` — **ratatui 0.30** (TUI para SSH/sin X11).
   - Ambos verificados contra producción en vivo.

**Estado:** 79 tests OK · working tree limpio · producción activa y verificada.

---

## 🧱 Estructura del proyecto (IMPORTANTE)

```
C:\Users\user\Documents\job\Ledgerita\
├── HANDOFF.md                         ← ESTE ARCHIVO (siempre el vigente)
├── inicio.md                          ← Visión del protocolo (LIT, cierre de libro, Prueba de Fórmula)
├── PLANTILLA_SESION.md                ← Plantilla estándar de reportes
├── nueva-sesion.ps1                   ← Generador de reportes (crea/archiva handoffs)
├── SECRETS_LOCAL.md                   ← Credenciales (NO versionado, en .gitignore)
├── deploy.ps1                         ← Deploy a servidores (Send-File + SHA-256), mainnet/testnet
├── handoffs/                          ← Historial de sesiones + INDEX.md
├── ledgerita_core/                    ← NÚCLEO: librería + daemon P2P
│   ├── src/lib.rs                     ← crypto, transaction, state, network, error, serialize, storage, genesis, faucet
│   ├── src/main.rs                    ← CLI: --send, --mint, --faucet, --connect, --network, --data-dir
│   ├── src/bin/monitor.rs             ← monitor local (requiere nodo DETENIDO por lock de sled)
│   ├── src/bin/remote_monitor.rs      ← monitor remoto por P2P (1 shot, exit code 1 si anomalías)
│   ├── scripts/                       ← healthcheck, journal wrapper, sudoers, backup, systemd, vendor
│   └── .cargo/config.toml             ← VENDORING: reemplaza crates-io por local_deps/ (solo aplica dentro de ledgerita_core)
├── ledgerita_wallet/                  ← wallet egui/eframe (Fase 1 MVP)
└── ledgerita_monitor_tui/             ← monitor ratatui por P2P
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
- **Dashboard**: dirección (mono, botón "📋 Copiar dirección"), balance LIT, supply vs hard cap 21M, restante al cap, fee pool, época, TX confirmadas, cuentas, red.
- **Enviar LIT**: receptor (64 hex), monto LIT, fee LIT → valida (formato, auto-envío, fondos vía `State::apply_transaction`), firma con la wallet, aplica al estado local y persiste (`State::flush`).
- **NOTA (importante)**: la TX se aplica y persiste **localmente**; la difusión P2P NO está implementada (es Fase 2). Para broadcast real hay que conectar nodo.

### Detalles técnicos
- Conversión LIT↔sats propia: `lit_to_sats` / `format_lit` (1 LIT = 1_000_000 unidades, 6 decimales). Sin dep extra.
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
- Re-sondea cada `--refresh` y en cada `ConnectionEstablished`.
- Actor de red en hilo con runtime tokio, estado compartido `Arc<Mutex<SharedState>>`; UI con `ratatui::init()`/`restore()`.
- Tests: 5 (format_amount, root_prefix, extract_peer_id, supply limit).

### Verificación en vivo (2026-08-17)
- **MainNet**: supply 10.000095 LIT, state root `c0198d1f…`, fee pool 0.000005, 1 TX, época 0, 2 cuentas, OK ✓.
- **TestNet**: supply 1000000.005000 LIT, root `f084ffce…`, 1 TX, época 0, 2 cuentas, OK ✓.

---

## 🔗 Producción (servidores)

| Host | IP | Rol | Servicios |
|---|---|---|---|
| kettle03 | 192.168.68.61 | seed (escucha) | `ledgerita-core` (36565) + `ledgerita-core-testnet` (46565) |
| kettle02 | 192.168.68.64 | cliente | `ledgerita-core` (conecta a kettle03) + `ledgerita-core-testnet` |
| kettle01 | 192.168.68.58 | **esta PC** (dev + wallet-node GUI) | `ledgerita_wallet.exe` (:efímero, se diala al seed) |

(Nota 2026-11-06: kettle01 es la computadora de desarrollo de Windows — no corre demonios; la wallet-node GUI y sus tests viven aquí. SSH a .58 es "refusado" porque la conexión es local.)

- **🔴 Credenciales, claves SSH, passwords y multiaddrs: ver `SECRETS_LOCAL.md`** (archivo NO versionado en la raíz).
- **Multiaddrs** (no secretos):
  - MainNet seed: `/ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1`
  - TestNet seed: `/ip4/192.168.68.61/tcp/46565/p2p/12D3KooWNjCgE866sbqjr2626KEC3MGYKvYuVM6MvkuG5VhHvaaS`
- **Healthcheck instalado**: `/home/user/ledgerita-healthcheck.sh` (cron `*/15`), log `/home/user/backups/ledgerita/healthcheck.log`, webhook opcional `/home/user/ledgerita-webhook.url` (POST JSON `{"text": "..."}`).
- **Estado servicios**: `systemctl is-active ledgerita-core ledgerita-core-testnet` (ambos active en ambos hosts, verificado 2026-08-17).
- **Backups**: cron en servidores → `/home/user/backups/ledgerita/` (wallet.key + state). Restore test verificado en sesiones previas.
- **Credenciales de red de los nodos (env, NO wallet)**: en `/etc/ledgerita/env` de cada servidor (sin BOM; ver `handoffs/REPORTE_HANDOFF_2026-08-02.md` línea 127).

### Comandos útiles de diagnóstico
```powershell
# Ver reconexión P2P en vivo (servidor)
ssh -i $env:USERPROFILE\.ssh\id_ed25519_ledgerita user@192.168.68.61 "echo 'clavefacil' | sudo -S journalctl -u ledgerita-core -f | grep -E 're-agregado|Conexión establecida|Desconectado'"

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
git status                    # working tree limpio al cierre de 2026-08-17
git log --oneline             # a446074 = último (reporte 2026-08-17 13:05)
```
`git add` / `git commit` cuando haya cambios nuevos. NUNCA commitear `SECRETS_LOCAL.md`, `wallet_recovery.env`, `local_deps/`, `*.tar`, `target/` ni logs `*.log`.

---

## 🚀 Pendientes (próximas sesiones)

> Sesión más reciente: **2026-11-05** → detalle completo en `handoffs/REPORTE_SESION_2026-11-05_1714.md`
> (mejoras del sistema de handoff + **auditoría del código con hallazgos nuevos**).

### Inmediato (esta semana)
1. ✅ **Commit** del 2026-11-05 hecho: `3bda935` (sistema de handoff) + `89ac264` (typo deploy).
2. ✅ **Fix typo en `deploy.ps1`** línea 203: `p2d` → `p2p` (commit `89ac264`).

### 🚨 Deuda técnica del núcleo (hallazgos NUEVOS 2026-11-05 — no están en reportes antiguos)

- **A. Reparto de fees** — El reparto a self (bug de divergencia de los 4096 TX) quedó RESUELTO con el park 2026-11-05 (Checkpoint 5): `finalize_checkpoint` ya no toca el fee_pool ni acepta identidades node-local; el fee_pool queda estacionado acumulando entre épocas. **Pendiente (3b):** el reparto definitivo a validadores, con regla 100% derivada del estado pre-checkpoint (diseño a junto de B/C — Prueba de Tiempo). La variante "burn parcial" queda anotada para ese documento de diseño, NO decidida.
- **B. No hay baneo de nodos engañosos** — `network.rs` solo rechaza mensajes inválidos (`warn!` + respuesta `INVALID…`); el "baneo o ignorado inmediatamente" de la visión no existe.
- **C. Prueba de Tiempo / reputación anti-Sybil: cero implementación.** B y C comparten diseño de fondo (reputación de nodos) → abordar juntos.
- **D. ~~Faucet: `cap` mal pasado~~ → ✅ RESUELTO 2026-11-05** — `main.rs` L260 usaba `faucet_amount` como monto Y como cap → faucet de un solo uso por dirección. Ahora: flag `--faucet-cap` separado + `DEFAULT_FAUCET_CAP = 10_000 LIT` + test de goteo repetible. (La lógica base en `faucet.rs` ya estaba bien.)
- **G. Adopción de snapshots sin validar invariantes** — `state.rs` `merge_if_fresher` adopta cualquier snapshot "más fresco" (epoch, tx_count mayores) **sin validar nada del contenido**: ni que el supply no se infló (≤ hard cap), ni que no retrocedieron nonces/saldos, ni coherencia de fee_pool. Un peer malicioso puede empujar un snapshot mentiroso y el nodo ingenuo lo traga (fue hallazgo 2026-11-05 al analizar la visión "la red se defiende sola"). Fix: **gate de invariantes antes de adoptar** → snapshot inválido = rechazar + registrar/banear al peer. Cae en la misma unidad de diseño que B+C (defensa de red). Afecta a TODOS los nodos (core y futura wallet-node).
- **H. Respuestas TX del core SIN framing** — el daemon responde a `MSG_TX` con status ASCII plano (`OK`, `DUPLICATE`, `REJECT:x`, ...) que NO parsea con `split_msg` (el payload de una `Response` TX en `network.rs` se loguea raro y nadie lo procesa). El actor de la wallet-node (Fase 2.1a) ya clasifica ambos caminos (`classify_tx_response`: framing primero, si no plano) y responde plano para compat. Si algún día se arregla el core, la wallet ya está lista (test `test_classify_framed_status_compat_futuro`). Pendiente corto (bajo): decidir si se arregla en el core (rompe compat hacia atrás con monitores viejos) o queda como está con el clasificador dual.
- **E. Tesoro TestNet = seed cero (`genesis.rs` L21-23)** — aceptable en TestNet; JAMÁS reutilizar el patrón en MainNet (la seed determinista es derivable por cualquier que lea el código).
- **F. Toolchain** — eframe 0.36 exige Rust 1.95 (entorno: 1.94). Solo al actualizar deps.

### 🎯 Plan de trabajo — CHECKLIST DE AVANCE (actualizar tras cada checkpoint y commitear)

> Convención de checkpoint adoptada 2026-11-05: al terminar cada unidad del plan,
> marcar aquí el estado + detalle mínimo, actualizar el reporte de sesión, y
> commitear. Así el avance sobrevive a cualquier corte de sesión/tokens.

- [x] **0. Inmediato** — ✔ commit `3bda935` + typo deploy `89ac264`
- [x] **1. Deuda D — faucet `--faucet-cap`** — ✔ HECHO 2026-11-05
  - Nuevo flag `--faucet-cap <n>` en `main.rs` (cap separado del monto por entrega).
  - `DEFAULT_FAUCET_CAP = 10_000 LIT` en `faucet.rs` (antes cap = monto → un solo uso por dirección; ahora goteo repetible).
  - Test nuevo: `test_default_cap_permitting_repeated_drips`. Core: 71 tests OK.
- [x] **2. Fase 2.1a — wallet-node: actor de red + broadcast-first** ✔ HECHO 2026-11-05 (Checkpoint 6)
  - `ledgerita_wallet/src/net_actor.rs` NUEVO: `WalletActor` en hilo tokio (patrón del monitor TUI), behaviour ping+rr con `RawCodec` y protocolo por red.
  - API GUI↔actor: `spawn_wallet_node(network, identity, state: Arc<tokio::Mutex<State>>, storage: Arc<Storage>, peers, listen_port) -> NodeHandles { shared: Arc<Mutex<NodeShared>>, send_cmd: UnboundedSender<NodeCmd> }` (canal unbounded → `send` sincrónico desde egui).
  - `NodeCmd`: `BroadcastTx(tx firmado)` y `ChangePeers(Vec<Multiaddr>)`. `NodeShared`: peers (`PeerView` con supply/root/epoch/tx_count/cuentas), `last_broadcast`, `retry_pending`.
  - Broadcast-first: `BroadcastTx` → `MSG_TX` a TODOS los conectados → job de recolección de acks (~10s de ventana, dirigida por eventos) → algún `OK` o `DUPLICATE` ⇒ `Accepted` ⇒ apply + flush + checkpoint-due. Sin conexión ⇒ `Offline` + cola de reintento EN MEMORIA (reenvía sola al reconectar). REJECT de todos ⇒ `Rejected`, no se aplica.
  - Incoming: nodo full (valida firma/mempool/apply/forward + checkpoint); responde ASCII PLANO (compat: `OK`, `DUPLICATE`, `INVALID`, `INVALID_SIG`, `REJECT:x`, `BADNET`, `EMPTY`, `UNKNOWN`, `INVALID_SYNC`). `MSG_SYNC` entrante ⇒ snapshot enmarcado.
  - Sync en vivo: `MSG_SYNC` al conectar cada 30s; snapshot adoptado con `merge_if_fresher` (ojo: aquí sigue sin gate de invariantes — es el hallazgo G pendiente); PeerView se llena con supply/root/epoch de la red.
  - Seeds por red como constantes (`SEED_MAINNET`/`SEED_TESTNET`, kettle03) + `default_seed(net)`.
  - 9 tests nuevos del actor (clasificación compat, reportes, seeds, PeerId). Wallet: 13 tests OK.
- [x] **2b. Fase 2.1b — rewire de la GUI de la wallet** ✔ HECHO 2026-11-05 (Checkpoint 7)
  - Data-dir default wallet-node: `~/.ledgerita-wallet/<red>/` (`default_wallet_dir`); al cambiar de red se reposiciona SIEMPRE que siga siendo un default de otra red; rutas custom del usuario se respetan. Migración: UI detecta `~/.ledgerita` (default del daemon) → advertencia amarilla de sled-lock (`is_core_default_dir`, canonical tolerante con prefijo verbatim `\\?\` de Windows).
  - `unlock()`: pasa a `Arc<tokio::Mutex<State>>` + `Arc<Storage>`, y llama `spawn_wallet_node` con el Keypair unificado (misma seed ⇒ mismo PeerId), listen port efímero 0.
  - La GUI NUNCA toca el estado directamente: el actor es dueño; la GUI lee con `try_lock` + cache de frame (`state_cache`) — nunca bloquea al actor.
  - Dashboard: panel "🌐 Nodo (red en vivo)" con PeerViews (epoca, tx_count, supply, status, 🚨 supply excedido) + resultado del último broadcast (Accepted/Rejected/Offline) + editor "🔌 Conexión" con `parse_peers` (coma/`;`/newline) → `NodeCmd::ChangePeers` aplicable en caliente.
  - "💸 Firmar y enviar a la red": valida → firma (sk recargado del disco con el password, como antes) → `NodeCmd::BroadcastTx`; estado en `send_status`; resultado llega por `shared.last_broadcast`. `BroadcastTx` conserva la TX firmada en `last_tx` (memoria) + botón "🔁 Reintentar la última TX ahora" cuando `retry_pending` (el actor también reenvía sola al reconectar).
  - Widht/height de la ventana: 540×720.
  - Tests GUI nuevos: `default_wallet_dir` por red, `is_core_default_dir` (advertencia de migración), `on_network_changed` no pisa custom. Wallet total: 16 tests OK.
  - Ver conversación 2026-11-05 en el reporte de sesión (decisión A incremental, descartada la one-shot C).
- [x] **2c. Fase 2.1c — test E2E local del wallet-node** ✔ HECHO 2026-11-05 (Checkpoint 8)
  - `ledgerita_wallet/src/lib.rs` NUEVO (los `tests/` solo pueden importar la lib, no el binario) + `main.rs` ahora consume `ledgerita_wallet::net_actor` (sin doble compilación).
  - `tests/wallet_node_e2e.rs`: demonio core real in-process (`run_p2p_node`, TestNet, temp dirs) + actor de la wallet. Escenario: ① TX sin conexión → `Offline{tried:0}` y estado local intacto; ② `ChangePeers` → sync adopta el snapshot (frescura); ③ reintento automático al conectar → `OK` → apply exacto (5.000.000 − 1.000 − 10 fee) + receptor 1.000; ④ supply/tx_count de demonio y wallet coinciden; ⑤ reporte final `Accepted{acks:1, duplicates:0}`.
  - Gotchas capturados: `std::thread::sleep` en async bloquea el runtime single-threaded del test (usar `tokio::time::sleep`); PeerId del demonio en la multiaddr se genera UNA vez.
- [x] **2d. Fase 2.1d — PRUEBA EN VIVO ⭐** ✔ HECHO 2026-11-06 (Checkpoint 9)
  - Topología real: kettle01 (esta PC, wallet-node GUI) → kettle03 (seed TestNet 46565) → kettle02 (cliente).
  - Fondeo: parada breve de `ledgerita-core-testnet` → CLI faucet 1 LIT a cada wallet de prueba → re-arranque (el CLI faucet queda en "modo semilla" tras aplicar: matarlo manual, no hay flag de salir — ver deuda).
  - **Flujo verificado punta a punta:** wallet levanta su nodo (PeerId propio `12D3KooWCzhb3…`), sync del seed (snapshot, 4 cuentas), `Firmar y enviar` 0.5 LIT A→B → seed aplicó la TX 13:11:51 (`Transacción aplicada`, acks=1), reenvío al cliente kettle02 aplicado +68ms. GUI mostró balance 0.500000 y "✅ La red confirmó la TX".
  - TestNet tras la prueba: 3 precisamente TX más el génesis — supply 1.000.002.005000 LIT, 4 cuentas, época 0.
  - **Hallazgo H presenciado en vivo:** kettle03 loguea `WARN Respuesta de otra red (chain_id inválido) peer=12D3KooWCvE6…` — es la respuesta `OK` plana de kettle02 que el core viejo intenta enmarcar. Cosmético; la wallet ya clasifica dual.
  - Wallets de prueba (A=GUI `2f38d370…e6e2` `/ B=`9ce91363…bdf`, password `viva2026`):detalle en `SECRETS_LOCAL.md`.
- [x] **3a. Deuda A — park provisional de fees** — ✔ HECHO 2026-11-05 (Checkpoint 5)
  - `finalize_checkpoint` ya NO recibe `validators`: no distribuye ni pisa el fee_pool; solo avanza `epoch` → fee_pool estacionado (consenso + determinista).
  - Los dos llamados del loop en `network.rs` ya no pasan `&wallet_address` (el dato node-local que divergía los roots).
  - Parámetro `wallet_address` de `run_p2p_node` ficou como `_wallet_address` reservado para la reputación de la unidad B+C.
  - Tests nuevos: `test_checkpoint_parks_fee_pool`, `test_checkpoint_root_identico_entre_nodos_con_wallets_distintas` (el que habría atrapado el bug original), `test_fee_pool_acumula_entre_epocas`.
- [ ] **3b. Deuda A — reparto definitivo de fees** (diseño primero: regla 100% derivada del estado pre-checkpoint, a decidir junto al diseño de validadores de B/C)
- [ ] **4. Deuda B+C+G — defensa de red** (baneo + Prueba de Tiempo + gate de invariantes en snapshots; documento de diseño ANTES de código)
  - [x] **4a. Checkpoint 10 — `DISENO_DEFENSA_RED.md` v1 APROBADO** (2026-11-06): blueprint completo de G (5 invariantes g1–g5 + 6 tests bloqueantes), B (`defense.rs` PeerGuard, strikes/ban/backoff) y C (Prueba de Tiempo node-local: antigüedad, uptime_ratio, contribuciones; tree `peers`). Orden G → B → C → redeploy core con deploy.ps1.
  - [x] Decisiones cerradas con el usuario (§5): ① ban inmediato 60 min al primer snapshot rechazado por gate; ② gate en ambos lados (core + wallet-node); ③ umbrales como constantes.
  - [x] **4b. Implementar G** ✔ HECHO 2026-11-06 (Checkpoint 11) — `validate_incoming_snapshot(&self, remote) -> Result<(), GateReason>` en `state.rs` (g1 hard cap con suma checked, g2 rango epoch/TX con cota superior INCLUSIVA, g3 nonces en la intersección, g4 fee_pool monotónico bajo park, g5 cuentas implausibles — forma RELATIVA: `nuevas <= remote.tx_count + |local| + 1`). Nueva variante `SyncResult::RejectedPorGate(GateReason)`; `merge_if_fresher` corre el gate SOLO en la rama de adopción (refinamiento: snapshots atrasados honestos no se castigan, Divergent queda fuera). Wiring: `network.rs` WARN con `?gate` (strike grave cuando dé PeerGuard §2/4c) y `net_actor.rs` `PeerView.status = "🚨 Snapshot rechazado (gate: …)"` + `synced=false` (gate en ambos lados, decisión §5-②; evento visible en la GUI). 9 tests nuevos en `state.rs`; core 82 tests en verde, wallet 16 + E2E en verde (regresión limpia).
  - [x] **4c. Implementar B** ✔ HECHO 2026-11-06 (Checkpoint 12) — `ledgerita_core/src/defense.rs` NUEVO: `PeerGuard` (scores por PeerId en HashMap, bans SOLO en RAM del proceso, jamás en consenso). `StrikeKind::{TxInvalid, SnapshotInvalido, ChainIdSpam, RequestSpam}` + `BanOutcome`; reglas: 3 leves/10min ⇒ ban 30min; snapshot de gate ⇒ ban inmediato 60min (decisión §5-①); flood ⇒ 10min; chain_id rate-based >20 en 5min (version-skew tolerado, lección H); backoff `base*2^n` tope 24h; ventana se limpia al banear; `now: Instant` SIEMPRE parámetro (tests deterministas). Wiring en ambos lados: core `network.rs` (skip dial baneado, respuesta mínima `NO`, strikes en decode/firma/chain_id/flood sin tocar estado, gate→strike grave, filtro OUTBOUND en gossip/pending/sync) y wallet `net_actor.rs` (idéntico + helper `peers_sin_baneados` para broadcast/sync/forward). 10 tests del guard en `defense.rs`; core 92 tests en verde (65 unit + integración), wallet 17 en verde (regresión limpia, incluido E2E).
  - [x] **4d. Implementar C** ✔ HECHO 2026-11-06 (Checkpoint 13) — Prueba de Tiempo node-local completa: métricas por peer en `PeerScore` (PRIMERA conexión como `primera_conexion` — re-conexiones no retroceden; `pings_ok/fallo` → `uptime_ratio`; `tx_aceptadas`; `snapshots_servidos`; `strikes_totales`) + struct persistente `PeerLocal` en el nuevo tree `peers` de `Storage` (7×u64 BE con TAG de versión; roundtrip + formato desconocido negado → 2 tests nuevos en `storage.rs`). Margen de veterano (`MARGEN_ANTIGUEDAD_MIN` 24h + uptime ≥0.99 + ≥10 pings): tolera 1 `TxInvalid` leve más — SOLO leves, gate/flood banean igual. `prioridad_dial` veteranos primero (desconocidos al final). Export/Import `Instant ↔ wall clock` via `defense::unix_millis_now()`: la antigüedad sobrevive el reinicio. Wiring en ambos lados: core `network.rs` (import al arranque, ConnectionEstablished, ping events, TX aceptada, snapshot servido, tick 30s = dial priorizado + prune + export + flush) y wallet `net_actor.rs` (idéntico con guard del actor + resumen por eprintln). 7 tests C en `defense.rs`; core 101 tests en verde (74 unit + integración), wallet 16 + E2E en verde.
  - [x] **4e. Redeploy core** ✔ HECHO 2026-11-07 (Checkpoint 14) — deploy.ps1 mainnet+testnet a kettle03/kettle02 con los binarios nuevos (park de fees + gate + PeerGuard + Prueba de Tiempo), misma identidad P2P de siempre (identidad derivada del password en `/etc/ledgerita/env`); PeerIds del seed preservados (mainnet `12D3KooWMkKuW…`, testnet `12D3KooWNjCgE8…`). Units regenerados en testnet con `--connect` correcto; mainnet units intocados. Los 4 demonios activos, sync/snapshot inmediato, cero rechazos de gate ante tráfico honesto. De paso: `apt upgrade` completo (kettle03 sin reboot; kettle02 reinicio de apt tras pausa + conexión trancada, log `/tmp/k02_apt_upgrade.log`). TX de prueba post-deploy TestNet A→B 0.1 LIT confirmada punta a punta (broadcast-first GUI → seed aplicó 08:05:56, gossip el cliente aplicó 08:05:56.9).
- [ ] **5. Deuda E — documentar regla del seed no-determinista para MainNet** (10 min)
- [x] **6. Resto de Fase 2** ✔ COMPLETA 2026-11-07 (Checkpoint 15) — sync en vivo ya venía de la wallet-node (checkpoints 6–9); quedaban historial TX + botón faucet, HECHOS abajo:
  - [x] **6a. Historial de TX en la GUI** ✔ HECHO 2026-11-07 (Checkpoint 15) — tree `historia` node-local nuevo en la Storage de la wallet-node (JAMÁS consenso, no viaja en MSG_SYNC): `TxHistoryRecord {tx_hash, from, to, amount, fee, nonce, ts_ms, kind}` — kind: 0=TX, 1=faucet — codificado en 112 bytes (TAG + 6×u64 BE + 32B remitente + 32B receptor); `save_tx_record` con hash como CLAVE (idempotente: la misma TX no se duplica) + `load_tx_history(limit)` (iter `.rev()` → más nueva primero; registros corruptos se saltan sin panic). Wiring en el actor: hook al aplicar TX de red entrante (junto a `note_tx_aceptada`) y al confirmar la propia (`apply_confirmed`); RAM `NodeShared.historial` ≤200 (`HISTORIAL_MEM_MAX`); carga al arranque con eprintln de resumen. GUI: panel "📜 Historial de TX" (Grid: hora UTC HH:MM:SS desde ts_ms — sin chrono —, marca 🪚/↑/↓/• comparando from/to con la dirección propia, monto +/- coloreado por familia, fee, hash corto de 8 hex; ScrollArea de 200px). Tests: roundtrip + límite/basura; core 103 en verde, wallet 16 + E2E.
  - [x] **6b. Botón faucet TestNet** ✔ HECHO 2026-11-07 (Checkpoint 15) — `NodeCmd::Faucet{recipient, amount, cap}` → el actor llama `State::apply_faucet` IN-PROCESS contra su PROPIO estado/storage (mismo proceso: cero conflicto del lock exclusivo de sled; la wallet usa `~/.ledgerita-wallet/<red>`, el daemon su carpeta). Orden: flush + checkpoint due. El mint nace en la wallet-node y la red lo copia con el pull de los peers (~30s). GUI: panel "🪚 Faucet TestNet" (visible SOLO en TestNet; receptor vacío = esta wallet; monto default 1.000000 LIT; cap `DEFAULT_FAUCET_CAP` 10,000 LIT de por vida por dirección) con status desde `NodeShared.faucet_status`. El mint queda registrado en el historial con kind=1 y hash sintético (24B del hex del recipient + 8B del ts BE). **Nota (mejor esfuerzo): si una TX concurrente avanza la red mientras mintea, el mint se pierde al ser el snapshot menos fresco.**
- [ ] **7. Fase 3** (QR, dark mode, selector red dashboard, portable, WASM)

### Fase 2 — Utilidad real de la wallet
1. **Difusión P2P de TX firmadas** — que "Enviar" propague de verdad (hoy solo aplica local). Reusar `ledgerita_core::network` desde la wallet.
2. **Conexión a nodo remoto (sync en vivo) en la wallet** — reusar el patrón del monitor TUI (`MSG_SYNC`) para que la wallet traiga el estado real de la red.
3. **Historial de TX** — el core no guarda historial; extender storage o mantener log local.
4. **Botón faucet TestNet** — wrapper del `--faucet` del CLI (solo TestNet).

### Fase 3 — Pulido
- QR de dirección (`qrcode` crate), dark mode, selector MainNet/TestNet en dashboard (el radio ya está en desbloqueo), binario portable, (opcional) WASM.

### Otros
- Configurar webhook real en `/home/user/ledgerita-webhook.url` (opcional).
- Observar ciclos de desconexión/reconexión P2P para confirmar estabilidad a largo plazo.
- El monitor TUI podría aceptar un `--data-dir` para modo local (hoy solo remoto).
- Faucet como servicio HTTP o mensaje P2P (`faucet.rs` ya está aislado para eso).
- Limpieza de raíz: `ledgerita_core.tar` y `add_error.log` desechables.
- (Solo si el repo va a ser público) purgar secretos del historial con `git filter-repo`/BFG.

---

## ⚠️ Trampas conocidas (NO repetir el debugging)

1. **Vendoring**: el core compila SOLO desde su directorio (usa `local_deps/`). Los crates GUI compilan desde su propio directorio (crates.io). NO mover el `.cargo/config.toml` ni `local_deps/`.
2. **egui 0.35 API**: `App::ui(ui, frame)` + `CentralPanel::show(ui, ...)` (ver arriba).
3. **Healthcheck + pipefail**: con `set -o pipefail`, `grep -q` corta el pipe (SIGPIPE) y sudo reporta error → falso "sin sync". Fix: capturar salida en variable antes de grep.
4. **Sled lock exclusivo**: `monitor.rs` (local) requiere el nodo DETENIDO. Para estado de un nodo corriendo, usar `remote_monitor` o el TUI (P2P).
5. **Nombre de archivo sudoers**: en `/etc/sudoers.d/` el archivo NO puede llevar punto.
6. **Quoting PowerShell→SSH**: usar archivos `.sh` subidos con scp en lugar de cadenas largas con comillas.
7. **2 saldos distintos entre MainNet/TestNet** es NORMAL (cadenas aisladas por `chain_id` y `protocol_id`).

---

## 📜 Convención de cierre de sesión

1. Generar `handoffs/REPORTE_SESION_<fecha>.md` (usar `PLANTILLA_SESION.md` o `./nueva-sesion.ps1`).
2. **Regenerar ESTE archivo** con lo hecho en la sesión (o usar `./nueva-sesion.ps1 -Archivar`, que archiva el handoff viejo en `handoffs/` y crea el esqueleto nuevo).
3. Actualizar `handoffs/INDEX.md` con una línea por sesión.
4. Commitear todo menos lo del `.gitignore`.

*Fin del handoff vigente. Estado: 79 tests OK, producción activa y verificada. 💎*
