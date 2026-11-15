# ✅ HANDOFF — Ledgerita (vigente)

> **CONVENCIÓN:** este archivo tiene **nombre fijo** y siempre contiene el handoff
> **actual** (el de la sesión más reciente). Cualquier sesión nueva debe empezar
> leyendo ESTE archivo completo. Las versiones anteriores viven en `handoffs/`
> (ver `handoffs/INDEX.md` para buscar una sesión del pasado).
>
> Última actualización: **2026-11-09** (Checkpoint 21/21b: GOTE en core + redeploy atómico TestNet + mudanza de IP de la seed .61→.51)

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
| kettle03 | **192.168.68.61** (⚠️ IP VARIABLE por DHCP: .61 → .51 el 2026-11-09 → .61 otra vez el 2026-11-10, Ckpt 21d; rutina de mudanza rodada, ver SECRETS_LOCAL.md) | seed (escucha) | `ledgerita-core` (36565) + `ledgerita-core-testnet` (46565) |
| kettle02 | 192.168.68.64 | cliente | `ledgerita-core` (conecta a kettle03) + `ledgerita-core-testnet` |
| kettle01 | 192.168.68.58 | **esta PC** (dev + wallet-node GUI) | `ledgerita_wallet.exe` (:efímero, se diala al seed) |

(Nota 2026-11-06: kettle01 es la computadora de desarrollo de Windows — no corre demonios; la wallet-node GUI y sus tests viven aquí. SSH a .58 es "refusado" porque la conexión es local.)

- **🔴 Credenciales, claves SSH, passwords y multiaddrs: ver `SECRETS_LOCAL.md`** (archivo NO versionado en la raíz).
- **Multiaddrs** (IP del 2026-11-10, Ckpt 21d; los defaults `SEED_*` de la GUI (net_actor.rs) apuntan a .61 de fábrica — quedaron alineados con la reversión):
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
- **E. ~~Tesoro TestNet nace de seed cero~~ → ✅ RESUELTO 2026-11-07 (Checkpoint 19, diseño deuda 3b)** — el patrón seed-cero queda ACOOTADO a TestNet para siempre: en MainNet `--mint` ahora DA ERROR (la emisión es estado-derived, goteo del tesoro virtual — diseño completo en `handoffs/DISENO_TESORO_MAINNET.md` aprobado), y el burn de fees hace que el fee_pool NO herede su destino en MainNet jamás.
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
- [x] **3b. Deuda A — reparto definitivo de fees** ✔ RESUELTA DE FACTO 2026-11-09 (auditoría de cierre, noche del corte — en la era de goteo): el "pool de validadores" de la visión ES el pool de elegibles del diseño GOTE (D6): los remitentes de al menos 1 TX en la época SON los participantes de la red. `finalize_checkpoint` (state.rs L418-482): **era de goteo** (tesoro disponible > 0) = emisión 100% derivada del estado + burn 100% del fee_pool (el 3b placeholder "reparto 100% a wallet_address local" ya no existe desde el park del Ckpt 12); **era post-goteo** (disponible == 0) = split 50/50 del pool con los ELEGIBLES del epoch (`participacion_epoch`) como receptores del 50% distribuido + dust → carry (poza muerta prohibida, D9). Evidencia en vivo del Ckpt 21c: fee_pool 41+4,200 = 4,241 → 4,110 quemados (era goteo) + 131 en la pool, supply exacta al sat.
- [x] **4. Deuda B+C+G — defensa de red** ✔ COMPLETA (subitems 4a–4e todos ✔ y deployada a los 4 demonios desde el Ckpt 14; flip del padre pendía de un resumen viejo — cerrado en la auditoría de la noche del corte 2026-11-09)
  - [x] **4a. Checkpoint 10 — `DISENO_DEFENSA_RED.md` v1 APROBADO** (2026-11-06): blueprint completo de G (5 invariantes g1–g5 + 6 tests bloqueantes), B (`defense.rs` PeerGuard, strikes/ban/backoff) y C (Prueba de Tiempo node-local: antigüedad, uptime_ratio, contribuciones; tree `peers`). Orden G → B → C → redeploy core con deploy.ps1.
  - [x] Decisiones cerradas con el usuario (§5): ① ban inmediato 60 min al primer snapshot rechazado por gate; ② gate en ambos lados (core + wallet-node); ③ umbrales como constantes.
  - [x] **4b. Implementar G** ✔ HECHO 2026-11-06 (Checkpoint 11) — `validate_incoming_snapshot(&self, remote) -> Result<(), GateReason>` en `state.rs` (g1 hard cap con suma checked, g2 rango epoch/TX con cota superior INCLUSIVA, g3 nonces en la intersección, g4 fee_pool monotónico bajo park, g5 cuentas implausibles — forma RELATIVA: `nuevas <= remote.tx_count + |local| + 1`). Nueva variante `SyncResult::RejectedPorGate(GateReason)`; `merge_if_fresher` corre el gate SOLO en la rama de adopción (refinamiento: snapshots atrasados honestos no se castigan, Divergent queda fuera). Wiring: `network.rs` WARN con `?gate` (strike grave cuando dé PeerGuard §2/4c) y `net_actor.rs` `PeerView.status = "🚨 Snapshot rechazado (gate: …)"` + `synced=false` (gate en ambos lados, decisión §5-②; evento visible en la GUI). 9 tests nuevos en `state.rs`; core 82 tests en verde, wallet 16 + E2E en verde (regresión limpia).
  - [x] **4c. Implementar B** ✔ HECHO 2026-11-06 (Checkpoint 12) — `ledgerita_core/src/defense.rs` NUEVO: `PeerGuard` (scores por PeerId en HashMap, bans SOLO en RAM del proceso, jamás en consenso). `StrikeKind::{TxInvalid, SnapshotInvalido, ChainIdSpam, RequestSpam}` + `BanOutcome`; reglas: 3 leves/10min ⇒ ban 30min; snapshot de gate ⇒ ban inmediato 60min (decisión §5-①); flood ⇒ 10min; chain_id rate-based >20 en 5min (version-skew tolerado, lección H); backoff `base*2^n` tope 24h; ventana se limpia al banear; `now: Instant` SIEMPRE parámetro (tests deterministas). Wiring en ambos lados: core `network.rs` (skip dial baneado, respuesta mínima `NO`, strikes en decode/firma/chain_id/flood sin tocar estado, gate→strike grave, filtro OUTBOUND en gossip/pending/sync) y wallet `net_actor.rs` (idéntico + helper `peers_sin_baneados` para broadcast/sync/forward). 10 tests del guard en `defense.rs`; core 92 tests en verde (65 unit + integración), wallet 17 en verde (regresión limpia, incluido E2E).
  - [x] **4d. Implementar C** ✔ HECHO 2026-11-06 (Checkpoint 13) — Prueba de Tiempo node-local completa: métricas por peer en `PeerScore` (PRIMERA conexión como `primera_conexion` — re-conexiones no retroceden; `pings_ok/fallo` → `uptime_ratio`; `tx_aceptadas`; `snapshots_servidos`; `strikes_totales`) + struct persistente `PeerLocal` en el nuevo tree `peers` de `Storage` (7×u64 BE con TAG de versión; roundtrip + formato desconocido negado → 2 tests nuevos en `storage.rs`). Margen de veterano (`MARGEN_ANTIGUEDAD_MIN` 24h + uptime ≥0.99 + ≥10 pings): tolera 1 `TxInvalid` leve más — SOLO leves, gate/flood banean igual. `prioridad_dial` veteranos primero (desconocidos al final). Export/Import `Instant ↔ wall clock` via `defense::unix_millis_now()`: la antigüedad sobrevive el reinicio. Wiring en ambos lados: core `network.rs` (import al arranque, ConnectionEstablished, ping events, TX aceptada, snapshot servido, tick 30s = dial priorizado + prune + export + flush) y wallet `net_actor.rs` (idéntico con guard del actor + resumen por eprintln). 7 tests C en `defense.rs`; core 101 tests en verde (74 unit + integración), wallet 16 + E2E en verde.
  - [x] **4e. Redeploy core** ✔ HECHO 2026-11-07 (Checkpoint 14) — deploy.ps1 mainnet+testnet a kettle03/kettle02 con los binarios nuevos (park de fees + gate + PeerGuard + Prueba de Tiempo), misma identidad P2P de siempre (identidad derivada del password en `/etc/ledgerita/env`); PeerIds del seed preservados (mainnet `12D3KooWMkKuW…`, testnet `12D3KooWNjCgE8…`). Units regenerados en testnet con `--connect` correcto; mainnet units intocados. Los 4 demonios activos, sync/snapshot inmediato, cero rechazos de gate ante tráfico honesto. De paso: `apt upgrade` completo (kettle03 sin reboot; kettle02 reinicio de apt tras pausa + conexión trancada, log `/tmp/k02_apt_upgrade.log`). TX de prueba post-deploy TestNet A→B 0.1 LIT confirmada punta a punta (broadcast-first GUI → seed aplicó 08:05:56, gossip el cliente aplicó 08:05:56.9).
- [x] **5. Deuda E — regla del seed para MainNet** ✔ HECHA 2026-11-07 (Checkpoint 19, diseño `DISENO_TESORO_MAINNET.md` APROBADO): burn de fees en cada Cierre (`fees_quemados` al state root), g4→g4' (par pool+quemados monotónico), g5 formalizado (`genesis_accounts(red)`: MainNet 0 / TestNet 1), `--mint` guardia MainNet=error, snapshot v2 en el cable (4×u64) — **redeploy atómico requerido**. El goteo (500 LIT/época) + reparto 50/50 va en su unidad futura.
  - [x] **5a. Redeploy atómico** ✔ HECHO 2026-11-07 (Checkpoint 19b) — Fase A (build en ambos hosts sin downtime, servicios viejos con inode vivo) + Fase B (restart de los 4 demonios en ventana de segundos, service files intactos). Trampa capturada: tar nuevo en `/tmp/` vs tar viejo en `/home/user/` (la verificación de contenido del BINARIO — grep del string nuevo + mtime — lo atrapó antes del restart). Evidencia: `Estado ya sincronizado` con snapshot v2 en MainNet (count=2) y TestNet (count=4), mismos PeerIds, cero gates/bans. Wallet GUI local recompilada para el actor nuevo (snapshot v2) — TX de prueba TestNet post-deploy corre el usuario en la GUI.
  - [x] **5b. TX post-deploy + fee mínimo precargado + directorio de contactos** ✔ HECHO 2026-11-08 (Checkpoint 19c/20) — TX 0.1 LIT A→B **confirmada punta a punta en la red del burn** (seed aplicó 06:51:22, gossip al cliente el mismo segundo, fee 0.000001 → primera vez que el fee_pool de TestNet nace vivo). UX: fee mínimo con sentido (0.000001 = 1 sat) precargado en el formulario (`DEFAULT_FEE_LIT`, editable). Directorio node-local (tree `contactos` en Storage del core, CERO consenso, sin redeploy): panel **📖 Directorio** con [usar]/[✖]/alta/formulario, contactos de fábrica (TestNet: B + nodos kettle02/03; MainNet: nodos) precargados solo si el book nace vacío. Core 81 + wallet 20 + E2E en verde. Sin redeploy necesario (los demonios no usan el tree).
  - [x] **5c. GOTE — goteo del tesoro + reparto post-goteo** ✔ CODIFICADO 2026-11-08 (Checkpoint 21, commit `f07522d`; diseño `DISENO_GOTE.md` APROBADO con D6–D10 firmadas): **D6/R1** elegible = envió ≥1 TX en la época (`participacion_epoch: BTreeMap<Address,u64>`, sender +1 en `apply_transaction`, entra al ROOT — función pura, el test del hallazgo A sigue en verde) y se vacía en cada Cierre; **D7/B** techo EVER-MINTEADO (`emitido_total ≤ HARD_CAP`; seed de migración = supply congelada y PERSISTIDA en el mismo `State::load` — reload jamás re-seedea con una supply ya movida); **D9** el dust del goteo queda EN el tesoro (emitido solo suma `parte×len`), y en la era post-goteo el dust de la mitad distribuida queda de carry en el pool (poza muerta prohibida: quemado+distribuido+carry == pool, exacto); Cierre: goteo `min(500 LIT, disponible)` en partes iguales + MINT directo (tesoro virtual sin clave, nadie lo firma); **sin elegibles ⇒ cero emisión y fee_pool ESTACIONADO** (jamás se quema sin destino legal); era post-goteo (disponible == 0): split 50/50 de fees. **g6** nuevo (`GoteoIncoherente`): emitido ≤ 21M + monotonía (fresco con emitido menor = mentira). Snapshot **v3** en el cable: 5×u64 + entradas (addr,count) 40B → **REDEPLOY ATÓMICO requerido** (Fase A/B del 19b a repetir). Storage: tree `particip` + 5ª meta clave. GUI: filas "Goteo emitido" / "Tesoro restante" en el dashboard; monitor TUI y `remote_monitor` con columnas/JSON nuevos. Core **93** (+12 tests GOTE) + wallet + E2E + TUI en verde. → **REDEPLOY TESTNET ✔ HECHO 2026-11-09 (Checkpoint 21b)**: Fase A (backup del binario v2 vía `/proc/<pid>/exe` + ExecStart MainNet al backup — guardia D10 más dura que el 19b; extract + build online en ambos hosts, delete del vendoring config como `deploy.ps1`; binarios nuevos verificados por marcador v3 + mtime) + Fase B (restart `ledgerita-core-testnet` en ambos; "Estado ya sincronizado" con root v3 compartido adf3ba95…, snapshot v3 con `goteo_emitido` = 1,000,003.004979 (supply congelada; el génesis de TestNet ES una cuenta de tesoro con 1M LIT) y `tesoro_restante` = 19,999,996.995021, tx_count 7, epoch 0, 4 cuentas; cero RECHAZADO/strikes/bans). MainNet: ambos servicios ACTIVE sin tokens tocados, correrían el backup v2 ante crash (pre-window intacto). **Mudanza de IP: kettle03 .61 → .51** (mismo PeerId, identity nace del password; service files parcheados y handoff actualizado; herramienta `escanear_lan.ps1` nueva para cazar hosts de LAN). Pendiente: TX de carga TestNet (falta ~4,089 TX para el primer Cierre con goteo REAL en vivo) y el redeploys MainNet en SU propia ventana. → **TX DE CARGA + PRIMER CIERRE DE LIBRO CON GOTEO REAL ✔ HECHO EN VIVO 2026-11-09 (Checkpoint 21c)**: burst desde la MISMA wallet A (CLI core con el data-dir de la GUI, GUI cerrada). Dos trampas cerradas de raíz: (1) faucet-fresh-dir YA NO es legal — g6 rechaza snapshots frescos con `emitido_total` menor (el seed D7 de un dir virgen nace con supply 0), y (2) la identidad libp2p nace de la MISMA seed que la signkey (`derive_from_seed`) ⇒ copiar la wallet de otro nodo = self-dial imposible. Camino legal: A ya vive en el estado de red (nonce real 13) ⇒ TXs válidas instantáneas sin faucet ni adopción. Piloto (10 TX) detectó archivo de wallet VIEJO (nonce almacenado 3) + el CLI aplica TXs inválidas LOCALMENTE con flush ANTES de conectar ⇒ archivo "más fresco" con estado inventado ⇒ recovery: borrar SOLO `state/` (wallet.key NUNCA) + CLI sync-only re-mergea (gates pasan). Burst 12 chunks × 350 TX (1 sat amount + 1 sat fee, self-transfer A→A; límite argv Windows 32 KB) → **Cierre en tx_count 4096 EXACTO en TODOS los nodos** (determinista): epoch 1, `goteo_emitido` 1,000,503.004979 (= seed + 500.000000 LIT completos a A, única elegible), `tesoro_restante` 19,999,496.995021, fee_pool = 41 + 4,200 − 4,110 quemados (100% burn era goteo) = 131 sat, supply exacta al sat, root compartido `b1718aa4…`; A ≈ **500.495770 LIT** (visible al reabrir la GUI). kettle02 convergió por RELAY (la seed drena y re-envía su buffer de TX en cada MSG_SYNC, network.rs L560-583) con su propio Cierre local idéntico. Rechazos del journal = SOLO duplicados harmless. **DEUDA nueva**: sync solo al conectar (`ConnectionEstablished`) — nodo vivo con conexión estable NO re-sincroniza (tick de re-sync pendiente); dir de wallet sin correr hace rato queda VIEJO ⇒ `Invalid nonce: replay` ⇒ curar borrando `state/` y sync nuevo. → **DEUDA CORTADA 2026-11-9 noche (Checkpoint 22)**: tick de re-sync de 30 s en el core (`resync_interval` en `run_p2p_node`, mismo flujo de sync que el handshake: request con root propio → snapshot del peer → `merge_if_fresher` con gates, y el seed drena/re-envía su buffer de TX en cada MSG_SYNC recibido, que es el RELY vivo) — logs de estado QUIETO/INFO ya-sync desprometidos a `debug!` (el intervalo es el steady state — info flooding); wallet GUI ya lo tenía (`request_sync_all` en su propio tick). Deployado en kettle02 TestNet (tick en vivo, build online + solo restart de testnet; MainNet queda en su ventana D10 con la seed). **Queda: redeploy MainNet en SU ventana (D10, patrón 21b).** → **g6 COTA RELATIVA POR ÉPOCA ✔ codificada 2026-11-9 noche (Checkpoint 23)**: el gate g6 sumó el inciso c) — `remote.emitido ≤ local.emitido + GOTEO_POR_EPOCA_SAT×(remote.epoch − local.epoch)` en la rama de adopción (forma RELATIVA sin "seed por red firmado" que el diseño pedía y nunca existió: el anclaje es el punto del nodo local, ya gateado por sí mismo). Consecuencia de ley que endurece: con la MISMA época la emisión debe ser IDÉNTICA (el Cierre AVANZA la época — nadie mintea dentro de una época; monotonía + cota delta-0 ⇒ igualdad exacta). Tests nuevos: inflación de +1 sat tras 3 épocas REJECT, crecimiento exacto 500×2 ADOPTA, misma época con +1 suelto REJECT; el test viejo "igual o mayor" renovado al lado bueno (delta 1 con el máximo legal exacto). Core 96 lib (+3) + integración (4+2+1+1+1) + wallet 19 + E2E en verde. **Deployed en kettle02 TestNet** (tar 485,376 B, build online 13.1 s, tests DEL HOST 96 en verde, restart SOLO testnet — goteo con 4 cuentas intacto, MainNet sin tocar). → **21d COMPLETO 2026-11-10 con la luz — VENTANA MAINNET D10 + kettle03 DESPIERTO**: ① caza LAN: kettle03 despertó de vuelta en su IP histórica **.61** (el DHCP la devolvió — rutina 21b ejecutada en reversión: `escanear_lan.ps1` + `sed .51→.61` en las dos unidades de kettle02 + daemon-reload; los defaults `SEED_*` de la GUI ya eran `.61` de fábrica ⇒ cero cambios de código). ② kettle03 Fase A: tar v3.1 (tick+g6, 485,376 B) + build online 30.9 s + 96 tests del host + backup binario pre-tick vía `/proc/exe` + restart SOLO testnet. ③ **PAR TestNet verificado con evidencia dura**: `remote_monitor --json` contra ambas puntas → MISMA raíz `b1718aa40b5d085b`, epoch 1, tx 4,227, `all_match: true`, healthy/anomalies 0 (el silencio INFO es el steady_state del debug! del 22). ④ **Ventana MainNet (08:35:29 seed / 08:35:44 cliente)**: swap del binario `c19_backup` v2 → v3 en las unidades de AMBOS hosts (flags EXACTAS preservadas, identidad P2P idéntica `12D3KooWMkKuW…` preservada — nace del wallet.key, backups pre-21d de ambos MainNet via `/proc/exe` → `ledgerita_mainnet_v2_pre21d_backup`) + daemon-reload + restart de `ledgerita-core`. **Verificación histórica**: Conexión establecida entre .61 y .64 + `remote_monitor --network mainnet --json` → MISMA raíz MainNet `bb15a206625e5e42` en ambos, **seed D7 de migración verificada EN VIVO: `goteo_emitido` = 10.000095 LIT = supply congelada EXACTA** (el patrón D7 clavado en ambas redes), tesoro armado 20,999,989.999905 LIT, epoch 0/tx 1/2 cuentas, healthy/anomalies 0. → **CEREMONIA MAINNET ✔ HECHA 2026-11-11 (Checkpoint 29)**: ver abajo la línea completa — primer Cierre de Libro de MainNet CON GOTEO REAL (500 LIT), par verificado `all_match` con raíz `3517f336cc6f7ca1`.
  - [x] **5d. CEREMONIA MAINNET — PRIMER CIERRE DE LIBRO con goteo real** ✔ HECHA 2026-11-11 (Checkpoint 29): piloto 10 TX (seed `b1435c81…` → wallet MainNet nueva del usuario `5736ec25…` — GUI k01, password = password MainNet documentada en SECRETS_LOCAL) verificado on-chain con monitor (tx 11, contabilidad exacta al satoshi) → bulk 4.085 TX en 12 chunks (11×350 + 235) con el CLI core en kettle03 sobre el data-dir real de la seed (ventana: demonio parado ~20 min, kettle02 sosteniendo; `--connect` a kettle02 para red viva; `timeout 90` por chunk — hallazgo: el CLI NUNCA sale, corre como nodo; el flush es TX-a-TX en el arranque ANTES de la espera ⇒ rc=124 inocuo y nonce secuencial intacto) → **CIERRE en tx_count 4.096 EXACTO** (la ÚLTIMA TX del bulk fue el borde — epoch 0→1, goteo REAL de 500 LIT → seed única emisora). Contabilidad EXACTA al satoshi: seed 5.000095 − 0.008190 + 500.000000 = **504.991905 LIT**; wallet usuario **0.004095 LIT** (4.095 sats); kettle02 5.000000 intacta; fee_pool 0 (fees quemados, dust cero, D9 legal); circulante 509.996000 = suma de balances ✓; `goteo_emitido` 510.000095 (D7 avanzó con el goteo); tesoro 20.999.489.999905. Restart del demonio → kettle02 re-dialó y "Estado sincronizado count=3 epoch=1 tx_count=4096" → **par verificado con remote_monitor: MISMA raíz `3517f336cc6f7ca1`, all_match, healthy 2/2, anomalies 0**. Nota: el puerto efímero de kettle02 MainNet cambió a 38955 (su demonio se reinició tras el corte — pid 625; el dial de cliente a seed es inmune a esto). Backups nuevos: `ledgerita_mainnet_v3_preceremonia_backup` (k03). El usuario ve su saldo en la GUI MainNet.
- [x] **6. Resto de Fase 2** ✔ COMPLETA 2026-11-07 (Checkpoint 15) — sync en vivo ya venía de la wallet-node (checkpoints 6–9); quedaban historial TX + botón faucet, HECHOS abajo:
  - [x] **6a. Historial de TX en la GUI** ✔ HECHO 2026-11-07 (Checkpoint 15) — tree `historia` node-local nuevo en la Storage de la wallet-node (JAMÁS consenso, no viaja en MSG_SYNC): `TxHistoryRecord {tx_hash, from, to, amount, fee, nonce, ts_ms, kind}` — kind: 0=TX, 1=faucet — codificado en 112 bytes (TAG + 6×u64 BE + 32B remitente + 32B receptor); `save_tx_record` con hash como CLAVE (idempotente: la misma TX no se duplica) + `load_tx_history(limit)` (iter `.rev()` → más nueva primero; registros corruptos se saltan sin panic). Wiring en el actor: hook al aplicar TX de red entrante (junto a `note_tx_aceptada`) y al confirmar la propia (`apply_confirmed`); RAM `NodeShared.historial` ≤200 (`HISTORIAL_MEM_MAX`); carga al arranque con eprintln de resumen. GUI: panel "📜 Historial de TX" (Grid: hora UTC HH:MM:SS desde ts_ms — sin chrono —, marca 🪚/↑/↓/• comparando from/to con la dirección propia, monto +/- coloreado por familia, fee, hash corto de 8 hex; ScrollArea de 200px). Tests: roundtrip + límite/basura; core 103 en verde, wallet 16 + E2E.
  - [x] **6b. Botón faucet TestNet** ✔ HECHO 2026-11-07 (Checkpoint 15) — `NodeCmd::Faucet{recipient, amount, cap}` → el actor llama `State::apply_faucet` IN-PROCESS contra su PROPIO estado/storage (mismo proceso: cero conflicto del lock exclusivo de sled; la wallet usa `~/.ledgerita-wallet/<red>`, el daemon su carpeta). Orden: flush + checkpoint due. El mint nace en la wallet-node y la red lo copia con el pull de los peers (~30s). GUI: panel "🪚 Faucet TestNet" (visible SOLO en TestNet; receptor vacío = esta wallet; monto default 1.000000 LIT; cap `DEFAULT_FAUCET_CAP` 10,000 LIT de por vida por dirección) con status desde `NodeShared.faucet_status`. El mint queda registrado en el historial con kind=1 y hash sintético (24B del hex del recipient + 8B del ts BE). **Nota (mejor esfuerzo): si una TX concurrente avanza la red mientras mintea, el mint se pierde al ser el snapshot menos fresco.**
- [x] **7. Fase 3** ✔ COMPLETA 2026-11-09 (7a–7d ✔ desde el Ckpt 18 + 7f esta noche; el "WASM" de la visión era EXPLÍCITAMENTE "(opcional)" en el inicio.md y queda para un futuro con gateway HTTP — libp2p/TCP no viaja en browsers; cierre del padre en el Ckpt 24)
  - [x] **7a. Dark mode** ✔ HECHO 2026-11-07 (Checkpoint 16) — dark de fábrica en `run_native` (`cc.egui_ctx.set_visuals(egui::Visuals::dark())`) + botón toggle ☀️/🌙 en la cabecera del dashboard, en vivo (`ui.style().visuals.dark_mode` → `set_visuals`); theme no se persiste (la sesión siempre arranca en dark).
  - [x] **7b. Selector de red en el dashboard** ✔ HECHO 2026-11-07 (Checkpoint 16) — radio MainNet/TestNet junto al toggle de tema: cambiar de red `cambiar_red()` CIERRA la sesión (soltar `NodeRuntime` mata el channel del actor → shutdown limpio), limpia caches/status/pendiente del faucet y vuelve a la pantalla de desbloqueo; `on_network_changed()` reposiciona dir/seed solo si eran defaults (custom siempre respetado).
  - [x] **7c. QR de dirección** ✔ HECHO 2026-11-07 (Checkpoint 17) — botón "📷 Ver QR" junto al de copiar; crate `qrcode` 0.14 SIN feature de imagen: la matriz la pinta el painter de egui módulo a módulo (cuadraditos 4px, quiet zone de 4 módulos por lado sobre fondo blanco — convención de lectores). Muestra la dirección hex extendida. Toggle `show_qr` en `WalletApp`; `draw_qr()` helper sin texturas ni crate `image`. PROBADO EN VIVO por el usuario (lector del celular).
  - [x] **7d. Binario portable + verificador anti-contaminación** ✔ HECHO 2026-11-07 (Checkpoint 18) — modo portable SIN flag ambiental: si existe `portable.flag` junto al EXE, los datos van a `<exe_dir>/ledgerita-data/<red>` (cero contaminación del HOME; una copia SIN flag jamás cambia su comportamiento — `default_wallet_dir_in(network, portable)` con input explícito para poder testear, test nuevo). Empaquetado: `empaquetar_portable.ps1` (compila → `dist\LedgeritaWalletPortatil\` con exe + flag + `ledgerita-data/` + verificador + `SHA256SUMS.txt` de los tres archivos → auto-verificación final). Verificador: `scripts\verificar_portable.ps1` (hash SHA-256 REAL vs manifest; ✅ INTACTO exit 0 / 🚨 CONTAMINADO exit 1 — probado con tamper a propósito: detección correcta). Evolución anotada (no bloqueante): SHA-256 detecta corrupción y tampering casual; la defensa fuerte es firmar el manifest con ed25519 propia.
  - [x] **7f. Respaldo/Restauración de wallet** ✔ HECHO 2026-11-09 (Checkpoint 24, esta noche) — FEATURE de primer nivel que convierte el aviso de fábrica ("GUARDA ESTE ARCHIVO Y TU CONTRASEÑA") en función real: `export_wallet_backup(data_dir, password, dest)` en core/crypto.rs valida el DESCIFRE roundtrip con la contraseña y copia el blob CIFRADO (`salt:ciphertext` orion aead) byte a byte al destino elegido — la seed en claro JAMÁS viaja ni toca disco, y el export JAMÁS pisa un archivo existente; `import_wallet_backup(backup, data_dir, password)` valida el descifre ANTES de tocar nada (mala contraseña o blob corrupto no pueden vaciar la carpeta) y si ya había wallet la salva como `wallet.key.bak-<unix_ms>`. Formato legacy en claro (hex 32B) soportado en ambos lados. GUI: panel **💾 Respaldo de wallet** en el dashboard (collapse + campo Destino + [💾 Exportar respaldo] + status) y sección **📥 Restaurar desde un respaldo** en la pantalla de desbloqueo (campo Ruta + [📥 Restaurar en el directorio] + aviso: restaura OTRA wallet cambia también la identidad P2P de la carpeta — la identidad nace de la misma seed). `cambiar_red` limpia los statuses (respaldo es por red). 3 tests nuevos en crypto.rs (roundtrip restaura idéntica seed; mala contraseña rechaza EN ambos lados SIN dejar basura ni tocar la wallet vigente; reemplazo con .bak + no-pisa en destino existente) — core 99 lib en verde, wallet build sin warnings, wallet 19+ E2E verde.
  - [x] **7g. Explorador de archivos nativo para el respaldo** ✔ HECHO 2026-11-10 (Checkpoint 25, pedido del usuario: "en el respaldo pon un botón que diga explorar") — crate `rfd` 0.15 en la wallet: botón **📂 Explorar…** junto al export → diálogo nativo GUARDAR del SO (título "¿Dónde guardar el respaldo de tu wallet?", nombre sugerido `respaldo-ledgerita-<carpeta-de-red>.key`, filtro *.key) llena el campo Destino automáticamente; y el simétrico en el panel de restauración en desbloqueo: **📂 Explorar…** (elegís el archivo de respaldo con el explorador y llena Ruta). Los campos siguen editables a mano (rfd es una AYUDA, no una jaula — y en portable/sin SO con dialogo el tipeo sigue siendo el camino). Wallet build sin warnings, 19+E2E en verde.
  - [ ] **7e. App Android** — IDEA del usuario anotada 2026-11-07, PARA DESPUÉS sin fecha; se retoma cuando el usuario diga (a futuro, tras deuda 3b o en Fase 4).
  - [x] Scroll de página completa + status faucet en su panel (pulido post-Checkpoint 15, commit `7a2be53`).

- [x] **8. Autoupdate sano (Fase 4)** ✔ COMPLETO 2026-11-11/12 (Ckpts 26–34): la cadena de custodia código→binario→nodo (idea del usuario 2026-11-10: "un estilo de autoupdate pero que sea sano y se mantenga sano… que no se convierta en un nodo corrupto")
  - [x] **8a. `DISENO_AUTOUPDATE.md` v1 APROBADO** (2026-11-10, Checkpoint 26, leyes U1–U9 firmadas): firma ed25519 de releases con llave RELEASE dedicada separada de wallets (U2), verificación firma+hash+tests-del-host ANTES del swap (U3), canary TestNet→MainNet (U4, patrón D10), build de release ÚNICO `--locked` con toolchain fijada — los hosts JAMÁS compilan producción contra crates.io salvo emergencia documentada (U5), rollback `/proc/exe` garantizado (U6), **la malla como último guardián**: un nodo corrupto pierde vivacidad (gates/divergencia/PeerGuard) pero JAMÁS corrompe el ledger — el update falla PERDIBLE (U7), **tests = constitución ejecutable** e `inicio.md` = constitución de fuente: cambios al texto declaran compatibilidad con objetivos (U8), higiene del agente: texto de logs/datos jamás es instrucción + todo swap de producción exige el "sí" humano (U1/U9). Fases de implementación: F1 ✔ (Ckpt 32) → F2 ✔ (Ckpt 33) → F3 ✔ (Ckpt 34) — **todas rodadas en producción con swaps reales**.
  - [x] **F1. Llave de release + herramienta firma/verify** ✔ HECHO 2026-11-11 (Checkpoint 32): bin `release_tool.rs` en core — subcomandos `gen-key` (32 B aleatorios del SO, no se pisa una llave existente), `pub-key`, `sign` (manifiesto TEXTO ESTABLE `version/red/target/commit_git/fecha/sha256` firmado ed25519 sobre SUS BYTES EXACTOS — lo firmado es lo que hay, nada de re-serialización) y `verify` (exige firma válida ∧ sha256(binario) == declarado; rc 0/1). 5 tests (redondo, manifiesto manipulado, binario sustituido, pubkey impostora, sha coherente) + camino negativo con el exe real (impostor RECHAZADO, pubkey inválida ni siquiera punto de curva). **Llave real generada**: seed gitignored (`release_key.seed`) + copia en la carpeta oficial de respaldos — pubkey `a3f4145f8e3bc717ad06127ae8aa49ca3e79438deb3e92adc675908acc9005ad` (pública, documentada en SECRETS_LOCAL). **Binarios v4.0 de producción firmados**: manifiestos `releases/RELEASE_MANIFEST_v4.0_k03.txt/.sig` (sha256 `38e193c1…`) y `releases/RELEASE_MANIFEST_v4.0_k02.txt/.sig` (sha256 `c55085b3…`), commiteados (datos públicos), verify roundtrip VERIFICADO. Cadena de custodia código→binario→nodo: eslabón U2 rodado; falta el consumidor (F2: updater que verifica antes del swap).
  - [x] **F2. Updater TestNet** ✔ HECHO 2026-11-11/12 (Checkpoint 33): módulo lib `release.rs` nuevo (esquema del manifiesto U2 compartido — `Manifest` con 6 campos OBLIGATORIOS, parse rechaza campos faltantes [test bloqueante 3], formato estable byte a byte) + bin `updater.rs` (el CONSUMIDOR de la cadena): `verify` (mecánica U2 sobre bundle `release.bin`+`RELEASE_MANIFEST.txt/.sig`) y `apply` (pipeline: ① verificación firma∧sha∧campos∧**revokey** [lista de pubkeys revocadas OBLIGATORIA — llave revocada JAMÁS firma válido aunque la firma sea correcta, test 4] ∧red∧target → ② tests del host contra la fuente del bundle [doble verde U3] → ③ backup `/proc/exe` [U6] → ④ swap copia+**rename** al path del servicio [un exe vivo no se pisa: ETXTBSY; el rename reemplaza la entrada de directorio y los procesos conservan su inode; temp con **chmod 755** — un tar de Windows llega sin bit de exec y systemd lo rechazaría con 203/EXEC — lecciones AMBAS vividas en el ensayo] → ⑤ restart SOLO `ledgerita-core-testnet` → POST profundo [is-active **∧ ExecMainStatus==0** — is-active puede ganarle la carrera al auto-restart; lección vivida]). **GUARDIA U1**: `--red mainnet` SE RECHAZA SIEMPRE antes de tocar nada (test 6). El "baja" deliberadamente tonto: el operador deja el bundle, el updater jamás descarga de la red (U9). Suite: lib **109** (+4 release.rs) + release_tool 5 + updater 5. **ENSAYO END-TO-END EN PRODUCCIÓN (TestNet)**: build en k02 como máquina de release (9.8 s, 109 tests de la fuente) → firma v4.1 (sha256 `a84e67b4…e17fb`, commit `7eef8b3`, `releases/RELEASE_MANIFEST_v4.1_k02build.txt/.sig` commiteados) → bundle a k03 → 3 rechazos EN VIVO (manifiesto manipulado, `--red mainnet`, llave "revocada") → apply en k03 y k02: **APPLY_OK** en ambos (verify+tests+backup+swap+restart+POST) → **par TestNet verificado: raíz `b1718aa40b5d085b` intacta byte a byte**, all_match, healthy 2/2, ambos binarios sha `a84e67b4` idéntico. Entre medio: kettle03 se APAGÓ (el apply quedó intacto — el swap nunca parcial; recuperación limpia al volver). Faltan: F3 (MainNet — SIEMPRE ventana humana, U1).
  - [x] **F3. Updater MainNet** ✔ HECHO 2026-11-11/12 (Checkpoint 34) — **ÍTEM 8 COMPLETO: la cadena de custodia código→binario→nodo vive MECÁNICAMENTE de punta a punta**: mismas leyes con las dos humanas en forma verificable — **U1 como frase de consentimiento**: el swap MainNet exige la frase EXACTA `sí, mainnet <versión> commit <commit>` que el propio updater IMPRIME al rechazar (sin frase / frase equivocada ⇒ nada tocado; la frase es determinística de versión+commit ⇒ el humano vio EXACTAMENTE lo que MainNet va a adoptar — un flag booleano sería demasiado barato); **canario U4/D10 mecánico**: TestNet debe estar activa ∧ su binario con el sha256 del manifiesto ANTES de que MainNet adopte (MainNet solo adopta lo que TestNet YA vive). Guardias probadas EN VIVO: sin consentimiento rechaza imprimiendo la frase, frase equivocada rechaza. **SWAP MAINNET REAL con el "sí" del usuario (2026-11-12)**: `APPLY_OK` en k03 (seed) y k02 — ① bundle verificado → ①b consentimiento humano → ①c canario TestNet → ② 109 tests del host → ③ backup `/proc/exe` (`updater_backup_4.1_mainnet` ×2) → ④ swap → ⑤ restart → POST `active (ExecMainStatus 0)` → **par MainNet verificado: raíz `3517f336cc6f7ca1` INTACTA byte a byte**, all_match, healthy 2/2, 3 cuentas post-ceremonia, goteo 510.000095. Commit del updater F3: `d0264c3`.
- [x] **9. Cripto-agilidad: preparación (NO migración) del hash y de las firmas (Fase 4)** ✔ COMPLETO 2026-11-11/12 (9a ejecutada+deployada Ckpts 30/31; 9b/9c diseños ✔ Ckpt 35 — ejecución de eras por señales, mucho después) — idea del usuario 2026-11-10 ("¿estar preparados para un update rápido del algoritmo de hash? o ¿de una vez mudarnos?"; opinión entregada: **preparar sí, migrar ya no**):
  - [x] **9a. Funnel único de hash + etiqueta de algoritmo** ✔ HECHO 2026-11-11 (Checkpoint 30): módulo `hash.rs` nuevo en core — `HASH_ALGO_SHA256=1`/`HASH_ALGO_ACTUAL` (constantes de era), `new_state_hasher()` (única puerta del hasher de estado; `generate_state_root` lo usa) y `hash_tx()` (movido acá; `serialize::hash_tx` delega por compatibilidad de API). **Snapshot v4**: byte de etiqueta de algoritmo como ÚLTIMO byte del cable — diseño clave: el decode v3 no exigía bytes sobrantes ⇒ los nodos v3 viejos IGNORAN la etiqueta (aceptan snapshots v4) y los v4 aceptan snapshots v3 como legado era 1 (asumen SHA-256, el único algoritmo que existió) ⇒ **transición SIN ventana de incompatibilidad** (deploy gradual posible), y un algoritmo DESCONOCIDO se rechaza (root no verificable acá — U7). Tests nuevos: +3 en hash.rs (era/determinismo/distinción) y +3 en state.rs (etiqueta al final, legado v3 se acepta, algoritmo desconocido rechazado) — core **105 lib** en verde (los tests de raíces existentes pasaron byte a byte idénticos: el funnel no movió el consenso), bins + wallet 20+E2E verdes. → **DEPLOY v4.0 A PRODUCCIÓN ✔ HECHO 2026-11-11 (Checkpoint 31)**: tar v4.0 (502.272 B) a ambos hosts → build online (k03 31.1 s / k02 15.2 s) + marcadores (g6/tick/HASH_ALGO) + **105 tests DEL HOST en AMBOS** + backups `/proc/exe` ×4 (`ledgerita_core_v31_pre9a_backup` testnet k03+k02, `ledgerita_mainnet_v31_pre9a_backup` mainnet k03+k02) → canary TestNet primero (par verificado: **raíz `b1718aa40b5d085b` INTACTA byte a byte**, all_match) → ventana MainNet (k03 01:08:12 / k02 01:08:27, 3 cuentas post-ceremonia intactas) → par MainNet verificado: **raíz `3517f336cc6f7ca1` intacta**, `goteo_emitido` 510.000095, all_match, healthy 2/2, anomalies 0. Los CUATRO demonios (TestNet k03/k02 + MainNet k03/k02) corren el binario era v4. Nota de puertos efímeros de kettle02 (se renuevan en cada restart): TestNet ahora 37257, MainNet ahora 41867.
  - [x] **9b. Firma post-cuántica** — DISEÑO ✔ HECHO 2026-11-11/12 (Checkpoint 35): `handoffs/DISENO_FIRMA_PQ.md` v1 — modelo de amenaza (Shor rompe ed25519, Grover solo deja SHA-256 en ~128 bits; la pieza frágil es la firma), candidatas NIST (recomendación: híbrida **ed25519 + ML-DSA-65** con SLH-DSA como repliegue), **9c como PREREQUISITO** (una pubkey PQ no entra en 32 bytes — con dirección=hash(pubkey) la dirección es PQ-agóstica para siempre), eras E1–E5 (funnel `sig.rs` → dual opcional en TX v2 → dual obligatoria anclada a época N → PQ-only en M) — CADA era con el pipeline U1–U9 (¡ya mecánico en producción!), inventario completo de call sites, 6 tests bloqueantes planificados, **puertas de decisión por SEÑALES** (librerías auditas, FIPS 206 cerrada, QC con miles de qubits lógicos, deprecación NIST — no calendario), y lo que JAMÁS se hace. Ejecución: mucho después, empezando por E2 (una tarde, patrón 9a) cuando 9c esté asentada.
  - [x] **9c. Dirección = hash(pubkey)** — DISEÑO ✔ HECHO 2026-11-11/12 (Checkpoint 35): `handoffs/DISENO_DIRECCION_HASH.md` v1 — el hallazgo (Ledgerita publica la pubkey de TODAS las cuentas vs Bitcoin que la revela solo al gastar ⇒ superficie quantum mayor), formato **D2: address = SHA-256(pubkey)** (32 bytes SIEMPRE — sin cambio de tamaño; el contenido pasa a opaco y la pubkey viaja solo en la TX que gasta, verificando sha256(pubkey)==sender), eras D2a (acepta ambas, patrón snapshot v4) → D2b (época N: D1 no gasta, migrar primero) → D2c (D1 solo lectura), **el vehículo de migración ya existe** (la herramienta de ceremonia del Ckpt 29 — una TX de traspaso por cuenta, saldo preservado al satoshi), inventario de call sites, 6 tests bloqueantes, el estado se transforma por CONSENSO (TXs de traspaso), JAMÁS rehash en disco ni patch caliente. Ejecución: D2a una tarde; D2b+c una sesión con ceremonia; ORDEN NATURAL: D2a → E2/E3 (9b preparada) → D2b → E4.

### Fase 2 — Utilidad real de la wallet ✔ COMPLETA 2026-11-12 (Checkpoint 36)

1. ✔ **Difusión P2P de TX firmadas** — HALLAZGO: **ya vivía** (el HANDOFF estaba viejo): `net_actor.rs` es broadcast-first desde el Ckpt 8 — la GUI firma → `NodeCmd::BroadcastTx` → el actor envía `MSG_TX` a TODOS los peers → apply solo tras `OK`/`DUPLICATE` de la red (jamás aplica sin confirmación), cola de reintento en memoria, y reenvío de TXs recibidas (nodo full). Probada en vivo desde el Ckpt 9 (kettle01→kettle03→kettle02) y en E2E siempre.
2. ✔ **Sync en vivo en la wallet** — HALLAZGO: **también ya vivía** end-to-end: la wallet-node pide `MSG_SYNC` al conectar y cada 30s, adopta el snapshot con `merge_if_fresher` (gates B+C+G, strikes PeerGuard) y la GUI lee **el mismo `Arc<AsyncMutex<State>>`** que el actor (el estado local ES el estado de la red tras el merge) → el dashboard ya muestra el estado real. Lo añadido en este Ckpt: **indicador de sincronía** en el dashboard — fila "Estado de red" + marca "(confirmado por la red)" junto al Balance, declarando confirmado SOLO si la raíz local coincide con la raíz de un peer con snapshot válido (`net_actor::root_prefix` ahora `pub`); sin sync, la fila dice "saldo local SIN confirmar por la red" (la wallet jamás miente el saldo). Test nuevo `test_root_prefix_distingue_estados`. Wallet: 21 tests OK (10 lib + 10 bin + 1 E2E).

*(Los ítems 3 y 4 de esta fase se cerraron como 6a/6b — historial de TX y faucet — en el Ckpt 15.)*

### Fase 3 — Pulido ✔ COMPLETA (Ckpts 16–18, 24, 25: dark mode, selector de red, QR, portable+verificador, respaldo/restauración, explorador nativo)

### 🛡️ Plan de Corrección de Vulnerabilidades (Ckpts 37–45) — 2026-11-13
> Plan maestro detallado en `handoffs/PLAN_CORRECCION_VULNERABILIDADES_2026-11-13.md`.
> Se avanza checkpoint por checkpoint, commiteando al finalizar cada uno.

- [x] **37. Zeroize en Core y Wallet** ✔ HECHO 2026-11-13 (Checkpoint 37) — `zeroize` 1.8/1.9 integrado en core (vendored) y wallet. `Zeroizing` envolviendo seeds, contraseñas y buffers en `crypto.rs`; `impl Drop for WalletApp` con borrado de memoria; tests `test_zeroize_limpieza_memoria_activa` y `test_wallet_password_zeroize`. Fix concurrencia de test IDs (`TEST_COUNTER`). Core 147 passed · Wallet 22 passed.
- [x] **38. Diseño BFT Ligero (`DISENO_QUORUM_BFT.md`)** ✔ HECHO 2026-11-15 (Checkpoint 38) — Especificación formal de `ValidatorSet`, `QuorumCertificate` ed25519 $M$-de-$N$, separación de dominio `LEDGERITA_QC_V1`, intercambio P2P `MSG_VOTE (0x03)`, Gate `g7` en `state.rs`, formato Snapshot v5, integración con `PeerGuard` y checklist de pruebas bloqueantes para Ckpts 39–41 en `handoffs/DISENO_QUORUM_BFT.md`.
- [x] **39. Módulo `quorum.rs`** ✔ HECHO 2026-11-15 (Checkpoint 39) — `ValidatorSet`, `ValidatorInfo`, `QuorumCertificate`, `VotePayload`, firma canónica `LEDGERITA_QC_V1`, verificación estricta ed25519 (`verify_qc`) y serialización binaria determinista con 9 tests unitarios. Core 156 passed.
- [x] **40. Gate `g7` en `state.rs` y wiring en red** ✔ HECHO 2026-11-15 (Checkpoint 40) — Gate `g7` (`FaltaCertificadoQuorum`, `CertificadoQuorumInvalido`), Snapshot v5 (`0x01 || qc.encode()`), `VoteCollector`, `MSG_VOTE (0x03)` + `process_incoming_vote` y `PeerGuard` en `network.rs`. Core 163 passed.
- [ ] **41. Despliegue TestNet y ensayo de quórum** — verificación de resistencia ante inyección de snapshots falsos.
- [x] **42. Fork Choice Rule (desempate determinista)** ✔ HECHO 2026-11-15 (Checkpoint 42) — `TxUndo`, `rollback_last_tx()`, `ForkResolution` y `resolve_fork_if_divergent()` en `state.rs` + wiring en `network.rs` para desempate por menor `state_root` (validando `g1`–`g7`), rollback y reintento automático con 3 tests de convergencia. Core 166 passed.
- [ ] **43. Dirección = SHA-256(pubkey) (Fase D2a)** — implementación del diseño 9c (`DISENO_DIRECCION_HASH.md`).
- [ ] **44. Resiliencia P2P (DNS / multi-seeds)** — soporte de nombres de dominio y mitigación de IP variable DHCP.
- [ ] **45. Releases Multifirma (2 de 3)** — mitigación de SPOF en `release_key.seed`.

### Otros
- Configurar webhook real en `/home/user/ledgerita-webhook.url` (opcional).
- Observar ciclos de desconexión/reconexión P2P para confirmar estabilidad a largo plazo.
- El monitor TUI podría aceptar un `--data-dir` para modo local (hoy solo remoto).
- Faucet como servicio HTTP o mensaje P2P (`faucet.rs` ya está aislado para eso).
- (Solo si el repo va a ser público) purgar secretos del historial con `git filter-repo`/BFG.
- Respaldo extra de `release_key.seed` (tercer lugar, criterio del usuario — es la llave que firma todos los releases).

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
