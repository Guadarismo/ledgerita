# ✅ Reporte de Sesión — Ledgerita (2026-11-05)
**Fecha y hora:** 17:14 → ampliación ~17:45 (hora local)
**Branch:** main
**Último commit al inicio:** a446074 Reporte de handoff 2026-08-17 13:05: GUI wallet + monitor TUI + healthcheck + fix P2P, autocontenido para nueva sesion
**Estado de git al cierre:** pendiente de commit (21 renames + 5 archivos nuevos + 1 edit)
**Tipo de sesión:** `/review` — revisión integral del proyecto (sin tocar código de producción) + mejora del sistema de handoff + auditoría informal que descubrió deuda técnica nueva (sección ⚠️ de este reporte)

> ⚠️ SOBRE ESTE REPORTE: la **deuda técnica descubierta leyendo el código** (sección
> "Deuda técnica — hallazgos NUEVOS") NO está documentada en ningún otro reporte
> del proyecto. Un análisis de `inicio.md` (la visión) contra el código real.
> La próxima sesión puede arrancar con este reporte + `HANDOFF.md` solamente.

---

## 🎯 Resumen de lo hecho

1. **Revisión integral del proyecto**: leídos el núcleo completo (`ledgerita_core/src/*`, bins), los 3 crates (wallet egui, TUI ratatui), `deploy.ps1`, los scripts de producción (healthcheck, journal wrapper, sudoers, backup) y todo el historial de reportes.
2. **Mejora 1 — `HANDOFF.md` de nombre fijo**: creado en la raíz como única puerta de entrada para una sesión nueva (siempre contiene el handoff vigente).
3. **Mejora 2 — historial ordenado**: 21 reportes movidos con `git mv` (renames preservados en git) a `handoffs/`, con `handoffs/INDEX.md` (una línea por sesión). La raíz quedó con lo operativo: `HANDOFF.md`, `inicio.md`, `PLANTILLA_SESION.md`, `nueva-sesion.ps1`, `SECRETS_LOCAL.md`, `deploy.ps1` y el código.
4. **Mejora 3 — plantilla + generador**: `PLANTILLA_SESION.md` (secciones estándar) y `nueva-sesion.ps1` (crea el reporte pre-rellenado con branch/commit/estado de git reales; `-Archivar` mueve el HANDOFF.md viejo a `handoffs/` y crea el esqueleto del nuevo). Probado en vivo: generó este mismo archivo.
5. **Mejora 4 — credenciales fuera del repo**: creado `SECRETS_LOCAL.md` (NO versionado, añadido a `.gitignore`) como única fuente local de passwords (sudo, wallet, env de nodos, SSH). Scrub en bloque de los 15 reportes históricos: `obegt…` → `<VER_SECRETS_LOCAL.md>` y `clavefacil` → `<SUDO_PASSWORD>`.
6. **Ignore global**: `%USERPROFILE%\.gitignore_global` con `SECRETS_LOCAL.md` + `wallet_recovery.env`, registrado en `core.excludesFile` → aplica a todos los repos del PC (blindaje contra `git add -f`).

---

## 🔧 Cambios técnicos (detalle)

- **Creados:** `HANDOFF.md`, `PLANTILLA_SESION.md`, `nueva-sesion.ps1`, `SECRETS_LOCAL.md`, `handoffs/INDEX.md` y este reporte.
- **Movidos:** todos los `REPORTE_*.md`, `sesion-*.md` y la `opinion de deepseek…` a `handoffs/` vía `git mv`.
- **Editados:** `.gitignore` (+`SECRETS_LOCAL.md`); los reportes históricos de `handoffs/` (solo sustitución de secretos, sin cambio de contenido); `%USERPROFILE%\.gitignore_global` (fuera del repo).
- **Sin tocar:** todo el código (`ledgerita_core`, `ledgerita_wallet`, `ledgerita_monitor_tui`), `deploy.ps1` y scripts de producción.

## 🖥️ Producción (si aplica)

Ningún cambio de producción; la sesión no tocó servidores (kettle03/kettle02 quedan como el handoff 2026-08-17).

---

## 🧪 Tests

```
Sin cambios de código → estado previo vigente: 79 passed
(70 core release + 4 wallet + 5 monitor TUI)
```

## 🗂️ Git

Cambios pendientes de commit. Sugerencia de mensaje:
`Mejor sistema de handoff: HANDOFF.md fijo + historial en handoffs/ con INDEX + plantilla/generador de reportes + credenciales a SECRETS_LOCAL.md (repo limpio de secretos)`

---

## 🚨 (⚠️ NUEVO — no está en ningún reporte anterior) Deuda técnica del núcleo

> Detectada el 2026-11-05 contrastando `inicio.md` (la visión del protocolo) contra el código real.

### A. El reparto de fees a "validadores" NO existe (placeholder) — el gap más serio vs la visión
- **Dónde:** `ledgerita_core/src/state.rs` líneas 144-158 (`finalize_checkpoint`) y sus dos llamadas en `network.rs` (líneas ~279 y ~428).
- **Realidad:** SIEMPRE se llama con `std::slice::from_ref(&wallet_address)` — el `fee_pool` se reparte 100% al nodo local. Nunca se reparte entre un pool de validadores reales.
- **Riesgo si se arregla mal:** tocar `finalize_checkpoint` cambia saldos → cambia el `generate_state_root()` → los peers se verían **Divergent** hasta que todos migren. Es un cambio de consenso: requiere versión o desfase coordinado.

### B. No hay "baneo/expulsión" de nodos engañosos (prometido en inicio.md, faltante)
- **Dónde:** `network.rs` — la visión dice "Cualquier nodo que emita un resultado inválido es inmediatamente ignorado y expulsado (baneado)". El código solo hace `warn!` + `send_response(b"INVALID…")`. No hay ban list, ni reputación, ni kick.

### C. "Prueba de Tiempo" (reputación / anti-Sybil): cero implementación
- La visión promete "los nodos ganan peso por estabilidad y antigüedad". Hoy: nada. B y C comparten el mismo diseño de fondo (reputación de nodos), conviene abordarlos juntos.

### D. Semántica del `cap` del faucet es rara
- **Dónde:** `main.rs` línea 260: `s.apply_faucet(&storage, network, &addr, *amount, faucet_amount)` — el 4º argumento es **amount**, el 5º es **cap**, y a ambos se les pasa la misma variable `faucet_amount`.
- **Efecto:** el tope por dirección = el monto dado en cada llamada → faucet de un solo uso por dirección, y el cap no es un valor global configurable. Sugerencia: flag `--faucet-cap <n>` separado del monto por entrega.
- La lógica base en `faucet.rs` es correcta (`given + amount > cap` → rechaza); el problema es sólo quién le pasa el cap.

### E. Tesoro TestNet nace de seed cero — JAMÁS reutilizar el patrón en MainNet
- **Dónde:** `genesis.rs` líneas 21-23: `SigningKey::from_bytes(&[0u8; 32])`.
- **Por qué importa:** "sin clave privada conocida" es una ilusión: cualquiera que lea el código puede derivar la privada (la seed es pública). Aceptable en TestNet; prohibido futuras semillas de tesoro para MainNet por este estilo determinista a no ser que la seed secreta quede fuera del código (recovery/hardware), NUNCA hardcoded en `genesis.rs`.

### F. Toolchain Rust vetado para el upgrade de eframe (solo nota)
- Actual: `eframe 0.35.0` (la 0.36 exige Rust 1.95; el entorno tiene 1.94). Si alguna versión futura rompe el build, revisar los 3 puntos de la API: `App::ui`, `CentralPanel::show(ui, ...)`, `ctx.copy_text`.

---

## 🚦 Plan sugerido (pendiente de validar con el usuario en la próxima sesión)

1. **Ya (rápido):** commit de hoy + fix typo `deploy.ps1` L203 (`p2d` → `p2p`).
2. **Fase 2 punto 1 — broadcast P2P desde la wallet** (siguiente sesión propuesta). Ver justificación en `HANDOFF.md` → "Pendientes".
3. **Deuda técnica (sección anterior):** sesiones dedicadas DESPUÉS de Fase 2 punto 1, porque A/B/C son cambios de semántica de consenso/red (romper el trabajo de la wallet en plena migración sería caro).
   - Orden interno sugerido: **D** (faucet cap, trivial) → **A** (fee pool / validators, requiere diseño propio) → **B + C** (baneo + Prueba de Tiempo, juntos por diseño compartido de reputación) → **E** (solo documentar/advertir ahora) → **F** (opcional, cuando toque upgrade de deps).
4. **Resto de Fase 2** (sync en vivo en la wallet, historial de TX, botón faucet) → Fase 3 (pulido GUI).

## 🚀 Pendientes para la próxima (catálogo completo)

### Inmediato (esta semana)
1. **Commit** de los cambios de hoy.
2. **Fix typo `deploy.ps1`** L203: `p2d` → `p2p`.
3. (Opcional) Purgar secretos del historial git (solo si el repo va a ser público).

### Fase 2 — wallet útil (próxima natural)
1. Broadcast P2P de TX firmadas (desde GUI).
2. Sync en vivo desde nodo remoto (patrón del TUI `MSG_SYNC`).
3. Historial de TX.
4. Botón faucet TestNet.

### Deuda técnica → ver sección 🚨 de arriba (A-F con orden interno sugerido).

### Fase 3 — pulido GUI
QR dirección, dark mode, selector red en dashboard, binario portable, WASM (opcional).

### Infra/opcional
- Webhook real (`/home/user/ledgerita-webhook.url`).
- Observar estabilidad reconexión P2P a largo plazo (journal, semanas).
- `--data-dir` en el monitor TUI (modo local).
- Faucet como servicio HTTP o mensaje P2P (`faucet.rs` ya está aislado para eso).
- Limpieza de raíz: `ledgerita_core.tar`, `add_error.log` desechables.
- (Solo si repo va a ser público) `git filter-repo`/BFG para secretos en commits viejos.

## ⚠️ Trampas descubiertas en esta sesión

1. **`git mv` de varios archivos a un destino aún inexistente falla** ("destination is not a directory"): crear la carpeta antes (`New-Item -ItemType Directory`) y después mover todo.
2. El scrub por PowerShell debe escribir con `UTF8Encoding($false)` (sin BOM) y `WriteAllText` para no alterar la codificación de los `.md` históricos.
3. `git check-ignore -v` siempre reporta la regla local primero; para verificar el ignore global inspeccionar `%USERPROFILE%\.gitignore_global` + `git config --global --get core.excludesFile`.

---

# ✅ CHECKPOINTS DE AVANCE (2026-11-05, sesión continua)

> Convención acordada con el usuario: cada unidad terminada del plan genera un
> checkpoint aquí + actualización del checklist en `HANDOFF.md` + commit.

## Checkpoint 1 — ecosistema de handoff (commit `3bda935`)
- `HANDOFF.md` fijo, `handoffs/` con 21 archivos + `INDEX.md`, `PLANTILLA_SESION.md`, `nueva-sesion.ps1` (probado), `SECRETS_LOCAL.md` + `.gitignore` + ignore global del PC.

## Checkpoint 2 — typo deploy (commit `89ac264`)
- `deploy.ps1` L203: `/p2d/` → `/p2p/` en el hint mainnet.

## Checkpoint 3 — Deuda técnica D resuelta: faucet con cap separado

**Problema original:** `main.rs` L260 pasaba `faucet_amount` como monto Y como cap
de `apply_faucet()` → el tope por dirección = el monto de esa llamada → faucet
de un solo uso por dirección y cap no configurable.

**Solución (3 piezas):**
1. **`faucet.rs`**: nueva constante `pub const DEFAULT_FAUCET_CAP: u64 = 10_000 * 1_000_000;` (10,000 LIT de por vida por dirección) + doc del módulo actualizado explicando la semántica nueva.
2. **`main.rs`**: nuevo flag `--faucet-cap <n>` (parseo idéntico al de `--faucet-amount`); `apply_faucet(&storage, network, &addr, *amount, faucet_cap)`; la salida del comando ahora imprime el tope; usage string actualizado.
3. **`deploy.ps1`**: hint del paso 5 de TestNet ahora muestra `[--faucet-cap 10000]`.

**Nota de diseño:** el `cap` NO se persiste (solo se persiste el total entregado
en el tree `faucet`, como siempre). El cap es una regla de ejecución del nodo;
si el operador baja el cap por debajo de lo ya entregado, la dirección queda
bloqueada para nuevos goteos — comportamiento deseado (cap de por vida).

**Tests:** nuevo `faucet::tests::test_default_cap_permitting_repeated_drips`
(9 goteos OK + 10ª entrega alcanza el tope exacto — la regla es `> cap`, no
`>= cap` — + 11ª rechazada). Primera pasada del test FALLÓ por error de conteo
mío (la 6ª entrega no excede el tope de 10,000 LIT); corregido a la secuencia arriba.

**Estado de tests del core tras el checkpoint: 71 passed, 0 failed**
(44 unit lib + 2 monitor + 15 remote_monitor + 4 helpers_advanced
+ 2 network_isolation + 4 P2P). Total proyecto: 71 + 4 (wallet) + 5 (TUI) = 80.

**Siguiente:** Fase 2.1 — broadcast P2P de TX firmadas desde la wallet GUI.

---

## Checkpoint 4 — decisión de diseño Fase 2.1 + hallazgo G (solo documentación)

**Decisión de la ronda:** abandonar la función one-shot de la "Alternativa C" y
adoptar la **Alternativa A incremental** (wallet-node con data-dir propio):
- El conflicto del lock de sled solo existe cuando **dos procesos** abren la
  misma carpeta de datos (daemon + wallet GUI). El mini-nodo de la wallet corre
  **dentro del proceso de la wallet** (hilo tokio, actor como el monitor TUI) →
  `Arc<Storage>` / `Arc<Mutex<State>>` compartidos en memoria, sin locks de disco.
- **Data-dir propio:** `~/.ledgerita-wallet/<red>/` (configurable) — la
  wallet-node nunca abre la carpeta del daemon de producción. Consecuencia
  (deseada por el usuario): la wallet es **un nodo real más** que valida todo
  por sí misma, alineado con "muchos nodos vigilando que la red sea sana".
- **Broadcast-first se mantiene:** firma → actor envía `MSG_TX` a los peers →
  con OK de la red se aplica localmente y persiste; sin conexión NO se aplica
  (la wallet jamás miente el saldo; evita choques de nonce con la red).
- Bonus barato: el actor ya trae sync → el dashboard puede empezar a mostrar
  estado sincronizado de red (paga parte de la Fase 2.2 de antemano).
- **Gotcha de migración a vigilar:** cambiar el data-dir default crea una
  wallet NUEVA (vacía) para quien tenga `wallet.key` en `~/.ledgerita`. La UI
  debe permitir apuntar al directorio existente (con la advertencia de no
  correr el daemon sobre esa misma carpeta a la vez).

### 🚨 Hallazgo G — la "confianza cero" se rompe al ADOPTAR snapshots (NUEVO)
Al analizar la visión del usuario ("nodos que defienden la red de nodos
maliciosos") se detectó en `state.rs` → `merge_if_fresher`:
- Adopta cualquier snapshot "más fresco" ((epoch, tx_count) mayores) **sin
  validar nada del contenido**: no verifica supply ≤ hard cap, no verifica que
  saldos/nonces/fee_pool sean coherentes, no detecta supply inflado por un peer.
- **Ataque viable hoy:** un peer malicioso fabrica un snapshot con saldos
  gigantes y tx_count mayor → cualquier nodo ingenuo lo adopta y lo replica en
  el siguiente `MSG_SYNC`. El hard cap solo lo chequea el monitor
  (`check_supply_limit`), no el conector.
- **Fix propuesto (para la unidad B+C+G de defensa):** gate de invariantes
  ANTES de adoptar — supply no puede crecer sin mint "explicada" (comparar con
  supply previo: solo puede crecer por fees no goteados o nada), nonces no
  retroceden, coherencia de fee_pool. Fallar ⇒ `REJECT` + registrar al peer
  (base del baneo). Test obligatorio: "snapshot mentiroso es rechazado".

**Impacto:** afecta TODOS los receptores de sync (daemon core y futura
wallet-node). Prioridad de implementación: junto con B (baneo) y C (Prueba de
Tiempo), en una unidad única de defensa de red con documento de diseño previo.

---

## Checkpoint 5 — Deuda A (park provisional de fees) RESUELTA

**Decisión con el usuario 2026-11-05:** el fee_pool en cada checkpoint va a
**park** — no se distribuye ni se pisa hasta que existan validadores reales
(Prueba de Tiempo, deuda B/C). Descartadas por ahora: reparto igualitario y
reparto proporcional al saldo (el segundo además concentra riqueza y contradice
la "Distribución Equitativa" de `inicio.md`; el reparto de la visión es a
*validadores*, cuentas equitativas fueron una idea nuestra, no de la visión).
Queda anotada la variante "burn parcial" (estilo EIP-1559) para el documento
de diseño de la deuda A+B/C — el usuario la ama 😄, pero NO está decidida.

**El insight clave que habilitó el parche simple:** la divergencia no la
causaba `finalize_checkpoint` en sí, sino que sus llamados le entregaban
`&wallet_address` — un dato del nodo (fuera del consenso) mezclado con saldos
(consenso). Muerto el parámetro, la función solo puede producir resultados
función-pura del estado → roots idénticos entre nodos por construcción.

**Cambios (`ledgerita_core`):**
- `state.rs` — firma nueva: `finalize_checkpoint(&mut self) -> [u8; 32]`; solo
  avanza epoch; el fee_pool queda stacionado (acumula entre épocas). Doc
  completa con la decisión y la regla de oro ("NUNCA recibir identidades
  node-local").
- `network.rs` — dos llamados actualizados (TX local y TX recibida);
  parámetro `wallet_address` de `run_p2p_node` renombrado `_wallet_address`
  (reservado para la reputación de B+C).
- `state.rs` tests — muerto `test_checkpoint_distributes` (probaba el
  comportamiento viejo); nuevos: `test_checkpoint_parks_fee_pool`,
  `test_checkpoint_root_identico_entre_nodos_con_wallets_distintas` (el que
  habría atrapado el bug original) y `test_fee_pool_acumula_entre_epocas`.

**Tests del core: 73 passed, 0 failed** (46 unit lib + 2 monitor +
15 remote_monitor + 10 de integración). Total proyecto: 73 + 4 + 5 = 82.
⚠️ Observación: `helpers_advanced::test_temp_dirs_with_labels` falló UNA vez
en una corrida completa y pasó aislado y en la corrida siguiente — **flaky** por
colisión de temp dirs en paralelo (preexistente, no del park). Si vuelve a
aparecer, considerar nombre de dir con contador global en el helper.

**Nota de seguridad (respuesta a la preocupación del usuario):** el fondo
estacionado NO es honeypot — el fee_pool no es cuenta ni tiene clave privada
personalizable; es una celda del estado de consenso replicada en todos los
nodos, solo movible por regla de protocolo determinista. La defensa
contra snapshots mentirosos (fee_pool inflado) es el gate del hallazgo G;
con park, el gate gana la invariante barata "fee_pool nunca retrocede entre
épocas (hasta que exista distribución)".

**Siguiente:** Fase 2.1a — wallet-node (actor de red en `ledgerita_wallet`,
data-dir propio) + broadcast-first.

---

## Checkpoint 6 — Fase 2.1a: actor de red de la wallet-node TERMINADO

**Nuevo archivo:** `ledgerita_wallet/src/net_actor.rs` (~850 líneas con tests)
+ `mod net_actor;` wired en `ledgerita_wallet/src/main.rs`
(`#[allow(dead_code)]` temporal hasta el rewiring de la GUI — checkpoint 7).

**Cargo.toml de la wallet:** agregadas `futures = "0.3"`,
`libp2p = 0.56.0` (tcp/tokio/noise/yamux/ping/macros/request-response),
`tokio = 1.50.0` (rt-multi-thread/macros/time/sync). Espejo del core.

### API GUI ↔ actor

- `spawn_wallet_node(network, identity: Keypair, state: Arc<tokio::sync::Mutex<State>>,
  storage: Arc<Storage>, peers: Vec<Multiaddr>, listen_port: u16) ->
  Result<NodeHandles, String>` — hilo propio `"wallet-net-actor"` con runtime
  tokio de 2 workers. Canal `mpsc::unbounded_channel` (el `send` es
  sincrónico → usable directo desde egui).
- `NodeHandles { shared: Arc<Mutex<NodeShared>>, send_cmd: UnboundedSender<NodeCmd> }`.
- `NodeCmd`: `BroadcastTx(Transaction)` (ya firmada por la GUI) y
  `ChangePeers(Vec<Multiaddr>)`.
- `NodeShared` (std Mutex, lock corto): `peers: HashMap<PeerId, PeerView>`
  (addr, connected, synced, supply_ok, supply_lit, root, fee_pool_lit,
  tx_count, epoch, accounts, status), `last_broadcast: Option<BroadcastReport>`,
  `retry_pending: bool`, `network`, `started`.

### Broadcast-first (dirigido por eventos, sin bloquear el actor)

- `start_broadcast`: envía `MSG_TX` con `encode_transaction` a TODOS los
  conectados y arma un `BroadcastJob` (deadline ahora+10s, window
  `BROADCAST_WINDOW`). **Sin peers conectados NO se aplica**: reporte
  `Offline { tried: 0 }` y la TX firmada queda en `retry_tx` (memoria) — se
  reenvía sola en el próximo `ConnectionEstablished`.
- Los acks llegan como `Message::Response { request_id, response }` y se
  enrutan por `request_id` al job del actor (`handle_response`).
- `service_jobs` (fuera del select, con awaits): cierra jobs con
  `done()` = aceptado ‖ todos respondieron ‖ venció la ventana.
  `report_for_job`: `Accepted{acks,duplicates}` si algún `OK`/`DUPLICATE`
  (regla broadcast-first: DUPLICATE = la red ya lo tiene ⇒ consenso de que
  existe); `Rejected{reason}` si todos respondieron rechazo; `Offline{tried}`
  si nadie respondió. SOLO `Accepted` aplica localmente (`apply_confirmed`:
  apply + mempool insert + flush + checkpoint-due, espiando el patrón core).
  - Edge manejado: la red puede confirmar la TX y ELLA MISMA reenviársela a
    la wallet (forward del peer) → la aplica el handler entrante y el
    `apply_confirmed` posterior falla por nonce/duplicado → se loguea INFO y
    el reporte permanece `Accepted` (la red es la fuente de verdad).

### La wallet-node como nodo full (incoming requests)

`handle_incoming_request` replica la lógica del daemon: valida framing
(chain_id), mempool, firma y apply con checkpoint; responde **ASCII plano
(compat, hallazgo H)**: `EMPTY`, `BADNET`, `INVALID`, `INVALID_SIG`,
`DUPLICATE`, `OK`, `REJECT:{e}`, `INVALID_SYNC`, `UNKNOWN`; reenvía la TX
válida a los demás conectados. `MSG_SYNC` entrante ⇒ responde snapshot
framed (`encode_snapshot`).

### Sync en vivo (zero-trust de la wallet)

`MSG_SYNC` al conectar y cada 30s (patrón del core: `[0u8;32]` como payload
request — el daemon exige ≥32). Respuesta framed → `decode_snapshot` →
`merge_if_fresher` (ojo: sigue SIN gate de invariantes — hallazgo G, cae en
la unidad B+C+G) → `PeerView` se llena con supply/root/epoch/tx_count/cuentas
de la red. `supply_ok` chequeado contra `HARD_CAP_SAT` (patrón monitor TUI).

### Re conexión (trampa clásica del core, respetada)

`targets` NUNCA se retira; el tick de 30s re-diala solo los no conectados
(`!swarm.is_connected`) y re-pide sync. `ChangePeers` reemplaza `targets`,
registra PeerViews "Conectando…" y diala los nuevos.

### Clasificación de respuestas TX (hallazgo H)

- `TxStatus { Accepted, Duplicate, Rejected(String), Other(String) }` +
  `tx_status_is_accepted` (regla: OK ‖ DUPLICATE).
- `classify_tx_response(net, payload)`: prueba framing primero (para cuando
  se arregle el core — test `test_classify_framed_status_compat_futuro`) y si
  no parsea, lee el payload completo como ASCII plano. Descubrimiento de
  test: un status enmarcado de OTRA red cae como `Other` (el header ensucia
  el texto) → jamás se acepta dudoso. Correcto así.

### Seeds

`SEED_MAINNET = /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKu...`,
`SEED_TESTNET = /ip4/192.168.68.61/tcp/46565/p2p/12D3KooWNjCgE866sbqjr...`
(kettle03) + `default_seed(NetworkId)`.

### Deuda técnica nueva anotada: hallazgo H

Respuestas TX del core sin framing (detalle en `HANDOFF.md` — deuda técnica).
La wallet ya clasifica dual; decisión de arreglar el core pendiente (bajo).

### Tests

- 9 nuevos en `net_actor::tests`: clasificación compat (framed/plano/otra
  red), `report_for_job` (3 estados), contabilidad del job, seeds por red,
  `extract_peer_id`, `format_amount`.
- **Wallet: 13 tests OK** (9 actor + 4 GUI). Core sigue en 73 (intacto).
- Total proyecto: 73 + 13 + (monitor TUI 5) = 91 tests.

**Gotcha de API capturado en compilación:** en libp2p 0.56 `Message::Request`
trae `channel: ResponseChannel<Vec<u8>>` (NO `InboundRequestId`);
`Message::Response` trae `request_id: OutboundRequestId`. `BroadcastJob`
guarda `HashSet<OutboundRequestId>` para enrutar acks.

**Siguiente:** Fase 2.1b — rewiring de la GUI (data-dir `~/.ledgerita-wallet/<red>/`,
spawn en unlock, dashboard = NodeShared + estado del actor, "Firmar y enviar"
→ `BroadcastTx`, botón Reintentar en `retry_pending`).

---

## Checkpoint 7 — Fase 2.1b: GUI de la wallet rewired (Fase 2.1 COMPLETA)

**`ledgerita_wallet/src/main.rs` reescrito** con la wallet-node integrada.
La Fase 2.1 quedó completa: la wallet ahora ES un nodo real (zero-trust,
Prueba de Fórmula validada por sí misma).

### Data-dirs y migración

- `default_wallet_dir(network)` = `~/.ledgerita-wallet/<red>/` (MainNet y
  TestNet separados). `on_network_changed`: reposiciona a defaults SOLO si
  sigue siendo un default de otra red (o vacío) — una ruta custom jamás se
  pisa (test `test_on_network_changed_no_pisa_custom`).
- `is_core_default_dir`: si la ruta apunta a `~/.ledgerita` (default del
  daemon) se muestra ADVERTENCIA en la pantalla de unlock ("NO corras el
  daemon sobre esa carpeta a la vez"). Detalle desafiado en Windows:
  `canonicalize()` devuelve prefijo verbatim `\\?\` — se strippea en
  `canonical()` (el test lo atrapó). Rutas inexistentes comparan por texto
  normalizado (separador `\`) — el usuario puede escribir la ruta antes de
  crearla.

### Unlock

- Carga la wallet (SigningKey + Keypair unificado: la misma seed ⇒ el mismo
  PeerId que el core — la wallet ES su nodo), Storage en `Arc`, `State` en
  `Arc<tokio::sync::Mutex<State>>`, génesis si data-dir nuevo, y
  `spawn_wallet_node(..., listen_port = 0)` (efímero: sin conflicto de
  puertos con el daemon).

### Lectura del estado en la GUI (regla de oro)

- El ACTOR es dueño del estado; la GUI NUNCA lo toca con exclusión: cada
  frame `try_lock()` → refresca `state_cache`; si el actor está ocupado se
  usa el cache del frame anterior. La UI jamás bloquea al actor.
- En `draw_main` se extraen clones baratos ANTES de tocar `&mut self`
  (address, state_arc, shared_arc, send_cmd) — patrón anti-borrow-checker de
  egui.

### Envío (broadcast-first end to end)

1. Validar receptor/monto/fee → `build_signed_tx` (nonce desde el estado del
   actor vía `blocking_lock` — hilo GUI sin runtime tokio, seguro — y firma
   con el sk recargado del disco con el password, igual que antes).
2. `send_cmd.send(NodeCmd::BroadcastTx(tx))` → mensaje "📤 esperando
   confirmación…". La GUI JAMÁS aplica: el actor decide con los acks.
3. Resultado por `shared.last_broadcast`: `Accepted` (✅ verde, acks+dups),
   `Rejected` (❌ con motivo), `Offline` (⏳ "NO aplicada; reintento
   automático al reconectar").
4. `last_tx` (TX firmada en MEMORIA) + botón "🔁 Reintentar la última TX
   ahora" visible mientras `retry_pending`. Los reintentos viven en memoria:
   la TX firmada jamás se persiste sin confirmación de la red.

### Panel de red en el dashboard

- "🌐 Nodo (red en vivo)": por peer — dirección corta, época/TX, supply
  sincronizado, estado con color (conectado verde, caído amarillo) y línea
  🚨 roja si algún peer reportó supply excedido (`supply_ok` vs HARD_CAP).
- "🔌 Conexión" (colapsable): seed editable + botón "🔄 Aplicar cambios de
  conexión" → `NodeCmd::ChangePeers` (wirea la variante: el actor re-diala
  solito cada 30s).

### Tests

- 3 nuevos en GUI: `test_default_wallet_dir_por_red` (defaults diferenciados,
  jamás el del daemon), `test_is_core_default_dir_dispara_advertencia`,
  `test_on_network_changed_no_pisa_custom`.
- **Wallet: 16 tests OK** (9 actor + 7 GUI). Core intacto: 73.
- Total proyecto: 73 (core) + 16 (wallet) + 5 (TUI) = **94 tests**.

**Cero warnings en `cargo build` de la wallet.**

**Pendiente para probar en producción (no code):** correr la wallet contra
kettle03 (TestNet seed `/ip4/192.168.68.61/tcp/46565/...` o vía el botón de
Conexión), mandar una TX TestNet y ver el flujo completo: firma → MSG_TX →
OK → apply + balance refrescado por sync.

**Siguiente de la lista:** termina la 2.1 con esto; lo que sigue es **B+C+G
(defensa de red, documento de diseño primero)** o pulir Fase 2 (historial TX,
botón faucet).

---

## Checkpoint 8 — Fase 2.1c: test E2E del wallet-node (y corrida previa de la prueba en vivo)

**Motivación:** el usuario pidió "prueba en vivo" con los tres kettles —
HENNY01 (.58 = ESTA PC con la GUI), kettle02 (.64 cliente), kettle03 (.61
seed). Antes de tocar producción, cambio de seguridad un test E2E headless
local que reproduce exactamente el guion del broadcast-first.

**Reestructuración:** `ledgerita_wallet/src/lib.rs` NUEVO (los tests de
integración solo pueden importar la lib, no el binario) con
`pub mod net_actor;`. `main.rs` del binario ahora consume
`ledgerita_wallet::net_actor` (una sola compilación del actor).
Deuda de nombres: `daemon_addr` NO va al consenso en ninguno de los dos lados
(`_wallet_address` reservado siempre).

**`ledgerita_wallet/tests/wallet_node_e2e.rs` — `test_wallet_node_broadcast_first_e2e`:**
- Demonio core real in-process: `run_p2p_node(KeyPair rand, peers=[], state
  TestNet temp dir con mint 5.000.000 sats + tx_count=1, mempool, port efimero
  local recén escogido vía bind:0)`. Espera de apertura de puerto por TCP.
- Wallet real en temp dir propio, wallet.key via `crypto::load_or_create_wallet`
  (la MISMA llamada que hace la GUI — identidad unificada). Actor SIN peers.
- **① TX sin conexión:** firmada y encolada → reporte `Offline { tried: 0 }`
  y el tx_count del estado local SIN CAMBIOS (la promesa "jamás miente").
- **② Conexión:** `ChangePeers([seed kette03-style local])` → dial →
  `MSG_SYNC` → snapshot adoptado (RemoteAdopted) → `PeerView` synced=true.
- **③ Reintento automático de la TX en memoria** al conectar → aceptación por
  red (`OK`) → `Accepted{acks:1, duplicates:0}` → `apply_confirmed`: el
  balance local cambia exactamente (4.998.990 emisor / 1.000 receptor) y el
  demonio también tiene la TX (tx_count=2 ambos; supply igual).
- Cleanup best-effort de temp dirs (sled puede seguir abierto al terminar).

**Gotchas captados con el compilador/test:**
1. `wait_for_port` con `std::thread::sleep` dentro del runtime tokio del test
   (single-threaded) BLOQUEA el executor y el demonio jamás corre → usar
   `tokio::time::sleep` (el timeout de 10s lo avisó con el puerto vacío).
2. La multiaddr del seed debe llevar el PeerId del keypair EXACTO que se le
   pasó al daemon (generarlo 2 veces ⇒ PeerIds distintos ⇒ dial Califa).
3. La sonda `wait_until` de un probe async no puede `await` el mutex del
   estado → `try_lock` (nos la deja leer o adivina "ocupado").

**Estado:** test pasa en ~5s (`cargo test --test wallet_node_e2e`).

**Siguiente:** prueba en VIVO — wallet GUI en esta PC (kettle01) contra
kettle03 (TestNet 46565): fondear via faucet con parada breve del demonio y
ejecutar una TX de verdad desde la GUI.

---

## Checkpoint 9 — ⭐ PRUEBA EN VIVO: la wallet-node habló con la red real (2026-11-06)

**Topología (aclaración del usuario):** kettle01 = ESTA PC de dev (Windows,
192.168.68.58) — corre la wallet GUI; kettle03 (192.168.68.61) seed; kettle02
(192.168.68.64) cliente. Actualizada la tabla de producción en `HANDOFF.md`.

### Preparación

- **Flag `--init` nuevo en el core CLI**: crea la wallet (wallet.key) y
  descuelga la dirección sin levantar nodo (antes solo existía
  `--show-address` que exige wallet existente). Crea `create_dir_all` antes
  de `load_or_create_wallet` (antes fallaba con dir inexistente).
- Wallets de prueba creadas con `--init` (password `viva2026`, en
  `SECRETS_LOCAL.md`):
  - A (GUI): `2f38d37045499848943141ccfd52d448f48e0e53f0c2bc3c3502fdca76e3e6e2`
  - B (receptor): `9ce91363fc369caf6a302d091566a1cc8b11e1d118d8ec416a0196210477fbdf`
- **Fondeo via faucet en kettle03** (stop `ledgerita-core-testnet` →
  `--faucet <addr> 1000000` × 2 → start): **1.000000 LIT a cada cuenta**,
  "Estado persistido después del faucet".
  - ⚠️ Operación nueva en la choreografía: el CLI faucet despues de aplicar
    SE QUEDA en modo semilla (levanta run_p2p_node para broadcast) — no hay
    flag para salir; hay que matarlo manual (`pkill -f -- '--network testnet'`)
    because colgó el SSH y dejó el demonio de servicio parado. Deuda: flag
    `--exit-after` o subcommand `faucet-only` (baja).
- Los faucets bump también tx_count: génesis(1) + faucet A + faucet B = 3 TX,
  supply 1.000.002.005000 LIT, 4 cuentas, época 0 — exactamente lo que la GUI
  mostró tras el sync (la wallet-node lee el ESTADO DE LA RED, no su disco).

### El flujo completo (timestamps del servidor)

1. 13:08 — wallet peek conecta: `Snapshot de estado enviado peer=12D3KooWCzhb3…`
   cada 30s (ping 3–16ms) — la wallet-node conversando de tú a tú.
2. `Firmar y enviar a la red`: 0.5 LIT de A (`2f38d370…`) a B
   (`9ce91363…`), fee 0.000010 → GUI "📤 esperando confirmación…" →
   **"✅ La red confirmó la TX (acks=1, duplicates=0) — aplicada localmente"**.
3. 13:11:51.440 kettle03: `Transacción aplicada peer=12D3KooWCzhb3…` (el seed
   aplicó la TX de la wallet).
4. 13:11:51.508 kettle02: `Transacción aplicada peer=12D3KooWNjCgE866…` — el
   seed REENVIO la TX al cliente y kettle02 también la aplicó (+68ms).
   **Tres nodos con el mismo estado, TX nacida de una GUI.**
5. Balance en la GUI: 0.500000 LIT.

### Hallazgo H presenciado en producción

kettle03 loguea tras la TX: `WARN Respuesta de otra red (chain_id inválido)
peer=12D3KooWCvE6…` — es kettle02 respondiendo `OK` en ASCII plano al
request TX del seed, y el core viejo llamando `split_msg` sobre un payload
que no es framed. Cosmético (la TX fluyó), pero ya documentado (hallazgo H)
y la wallet-node clasifica dual para no caer en lo mismo.

### Estado tras la sesión

- Fase 2.1 (wallet-node + broadcast P2P + sync en vivo + GUI): **COMPLETA y
  validada en producción real** (E2E local + red de 3 kettles).
- Core en kettle03/kettle02 sigue en el binario de agosto (protocolo
  idéntico: la wallet conversó sin problema). Pendiente de hygiene ANTES de
  la unidad B+C+G: redeploy del core con park+faucet-cap (deploy.ps1), para
  que los nodos de servicio también cumplan el park de fees del 2026-11-05.

**Siguiente:** unidad B+C+G (defensa de red) — documento de diseño primero
(`DISENO_DEFENSA_RED.md`), como acordado con el usuario.

---

## Checkpoint 10 — Diseño B+C+G aprobado: `DISENO_DEFENSA_RED.md` v1 (2026-11-06)

**Forma de trabajar:** documento de diseño ANTES de código, como pide la
unidad 4 del `HANDOFF.md`. El doc completo vive en `DISENO_DEFENSA_RED.md`;
aquí el resumen de lo que fija:

### G — Gate de invariantes antes de ADOPTAR snapshots (Prioridad 1)

- Ataque viable hoy: `merge_if_fresher` adopta cualquier snapshot más fresco
  SIN validar contenido → snapshot mentiroso (saldos gigantes, supply
  inflado) se traga y se replica viral en el próximo `MSG_SYNC`.
- Solución: `validate_incoming_snapshot(&self, remote) -> Result<(), GateReason>`
  en `state.rs`, llamada por `merge_if_fresher` ANTES de comparar frescura.
- 5 invariantes v1: **g1** supply ≤ `HARD_CAP_SAT` (21M LIT constitucional);
  **g2** `epoch*4096 <= tx_count < (epoch+1)*4096` (el epoch solo avanza con
  checkpoint); **g3** nonces no retroceden (en la intersección de
  direcciones); **g4** fee_pool monotónico si `local.tx_count > 0`
  (invariante barata que ganamos gratis con el park del Checkpoint 5);
  **g5** `balances.len() <= tx_count + 1` (débil, revisar en 3b).
- Invariante descartada con razonamiento: "supply no crece por sync" —
  los faucets legítimos agrandan el supply y se propagan legítimamente.
- Al rechazar: NO adoptar, NO reenviar, WARN con PeerId + GateReason, strike
  grave. El rechazo jamás toca el estado local.
- 6 tests bloqueantes nombrados en el doc (`test_snapshot_mentiroso_*`,
  `test_snapshot_legitimo_aceptado`, `test_gate_no_toca_estado_local_al_rechazar`).

### B — PeerGuard, baneo node-local (Prioridad 2)

- Nueva unidad `ledgerita_core/src/defense.rs`, común a core y wallet.
  `PeerGuard { scores: HashMap<PeerId, PeerScore> }`; strikes con ventana
  rodante, `banned_until`, `ban_count` (backoff `min(30min*2^(n-1), 24h)`).
- Tabla de strikes: TX inválida leve; chain_id spam rate-based; **snapshot
  rechazado por gate = struck grave → ban inmediato 60 min**; flood grave
  10 min. Bans SOLO en memoria del proceso: nunca en estado/root.

### C — Prueba de Tiempo node-local (Prioridad 3)

- Métricas por peer: `antiguedad` (primera conexión, persistida en tree
  NUEVO `peers` de Storage — node-local, jamás en el snapshot de consenso),
  `uptime_ratio` (ventana 1h de pings), `contribuciones`.
- Sirve para prioridad de dial y margen de strikes. Base de datos del
  selector de fees de 3b. **El score local JAMÁS decide quien cobra** —
  lección del hallazgo A es ley; el reparto de fees de 3b será función
  pura del estado público (variante "burn parcial" a decidir EN el diseño
  de 3b, no aquí).

### Decisiones cerradas con el usuario (§5 del doc)

1. **Ban inmediato** (60 min) al primer snapshot rechazado por el gate —
   no 3-strikes: es adulterio, no ruido de conexión.
2. **El gate G corre en ambos lados** (demonios core Y wallet-node) — la
   wallet ya tiene PeerViews para mostrar el evento 🚨 en la GUI.
3. **Umbrales como constantes** con los defaults de la tabla del doc;
   flags del CLI solo si un caso operativo real los exige.

### Próximo paso

Implementar **G** (función + 6 tests + wiring en `network.rs` y
`net_actor.rs`), regresión: `cargo test --release` en core +
E2E wallet. Después B (Checkpoint 12), C (13), y redeploy core con
`deploy.ps1`.

---

## Checkpoint 11 — Unidad G: gate de invariantes implementado y en verde (2026-11-06)

**Qué deja de poder pasar:** un peer que mande un snapshot "más fresco"
mentiroso (supply inflado, epoch incoherente, nonces retrocedidos, fee_pool
devorado o cuentas gratis) ya NO traga nadie — ni los demonios del core ni
la wallet-node. El hallazgo G crítico de la adopción de snapshots queda
cerrado a nivel de función + wiring log.

### `ledgerita_core/src/state.rs`

- `GateReason { SupplyExcedeHardCap, EpochIncoherente, NonceRetrocedido,
  FeePoolRetrocedido, CuentasImplausibles }` — diagnosticable en el WARN y
  contable como strike grave cuando el PeerGuard dé a vivir (B, §2/4c).
- `SyncResult` nueva variante `RejectedPorGate(GateReason)` (el enum sigue
  siendo Copy; `StorageError` sin PartialEq — los tests desenvuelven con
  helper `sync_result`).
- `validate_incoming_snapshot(&self, remote) -> Result<(), GateReason>`:
  - **g1**: suma CHECKEADA (un mentiroso con balances ~u64::MAX no colga el
    nodo) ≤ `HARD_CAP_SAT`.
  - **g2**: `epoch*4096 <= tx_count <= (epoch+1)*4096` con cota superior
    INCLUSIVA (window transitoria del checkpoint pendiente; el borrador la
    tenía exclusiva y castigaría la ventana real).
  - **g3**: en la intersección `remote.nonces ∩ local.nonces`, todo nonce
    del remote ≥ local (cuentas nuevas no violan — no tienen pasado).
  - **g4**: `local.tx_count > 0 → remote.fee_pool >= local.fee_pool`
    (invariante barata del park; solo v1, a relajar en 3b si el reparto
    llega antes).
  - **g5** (débil): `|remote.balances − local.balances| <= remote.tx_count +
    |local.balances| + 1` — REFRESH del borrador (`+1` absoluto castigaría
    génesis con varios minteos tipo helpers P2P; la forma relativa caza
    igual al mentiroso y tolera el génesis compartido).
- `merge_if_fresher`: gate SOLO en la rama `remote_fresh > local_fresh` —
  refina el borrador ("antes de comparar frescura"): un snapshot atrasado
  pero honesto no es una mentira (sus nonces son menores de nuestra
  historia) y castigarlo banearía peers legítimos rezagados. La rama
  `Divergent` tampoco corre gate (problema de consenso aparte, WARN propio
  y los tests de fork esperan ese resultado exacto).

### Wiring

- `network.rs` (core, handler MSG_SYNC): arm nuevo → `warn!(peer, ?gate,
  "🚨 Snapshot RECHAZADO por el gate de invariantes: no se adopta ni
  reenvía")`. El estado local queda intacto por construcción (el gate
  devuelve ANTES de la mutación).
- `net_actor.rs` (wallet): `PeerView.status = "🚨 Snapshot rechazado
  (gate: …)"` con `synced=false` + eprintln — el ataque es visible en la
  GUI (decisión §5-②: gate en ambos lados).

### Tests (9 nuevos en `state.rs`, todos en verde)

g1 mentiroso, g1 overflow, g2 mentiroso, g2 ventana inclusiva aceptada, g3
retrocedido, g4 pool reducido, g5 cuentas gratis, `legitimo_aceptado`
(génesis + faucet + TX con fee adoptados) y `gate_no_toca_estado_local_
al_rechazar` (root/balances/nonces/meta invariantes tras el rechazo).

### Regresión

- Core: `cargo test --release` → **82 tests, 0 failed** (antes 73).
- Wallet: `cargo test` → **17 tests + E2E `wallet_node_e2e` en verde**
  (~4.7 s; el gate no banda el sync legítimo del broadcast-first).

**Siguiente:** unidad **B** — `ledgerita_core/src/defense.rs` con `PeerGuard`
común (strikes/ban/backoff) + wiring en `network.rs` y `net_actor.rs`
(los strikes que hoy quedan como TODO de los WARN del gate). Checkpoint 12.

---

## Checkpoint 12 — Unidad B: PeerGuard (strikes + bans node-local) en verde (2026-11-06)

### `ledgerita_core/src/defense.rs` NUEVO (común a core y wallet)

- `PeerGuard { scores: HashMap<PeerId, PeerScore> }` — bans vive EN MEMORIA
  DEL PROCESO, JAMÁS en el estado/root (lección del hallazgo A). Al reiniciar
  el nodo, las prisiones expiran (la memoria eterna es la unidad C).
- `StrikeKind::{TxInvalid, SnapshotInvalido, ChainIdSpam, RequestSpam}` +
  `BanOutcome::{SinBan, BanCommitido(Duration)}`.
- Reglas (constantes al tope del archivo, decisión §5-③):
  | Evento | Régimen |
  |---|---|
  | 3× `TxInvalid` en 10 min | ban 30 min |
  | `SnapshotInvalido` (gate G) | ban INMEDIATO 60 min (decisión §5-①) |
  | `RequestSpam` (flood <2 bytes) | ban inmediato 10 min |
  | `ChainIdSpam` × > 20 en 5 min | ban 30 min (leves NO banean por cantidad) |
- `duracion_ban(base, ban_count) = min(base * 2^n, 24h)` — backoff exponencial.
- Diseño de implementación digno de nota:
  - El tiempo entra SIEMPRE como parámetro `now: Instant` — funciones
    deterministas, tests sin sleeps reales.
  - Al BANLEAR se limpia la ventana de strikes: la prisión es el ban, no
    reputación eterna (la cicatriz eterna es asunto de la unidad C).
  - `TxInvalid` SOLO por basura estructural (decode/firma): los rechazos de
    política (`REJECT:nonce…`) NO cuenten — no castigar al peer honesto
    rezagado que reenvía TX legítimas que aplicamos primero.
  - `ChainIdSpam` en AMBAS direcciones y tolerante al version-skew de los
    core de agosto (lección del hallazgo H presenciado en vivo: el `OK`
    plano de kettle02 jamás debe banear un vecino por versión).

### Wiring (mismo juego en ambos lados)

- **core `network.rs`**: PeerGuard por nodo (`run_p2p_node`); skip dial de
  reintento para baneados; requests entrantes de baneados → respuesta
  mínima `b"NO"` SIN decode/apply/estado; strikes en las 4 colas (empty
  request, chain_id, decode TX, firma inválida); gate → strike grave con
  ban; outbound (forward TX, pendientes tras sync) filtrando prisioneros
  (ni requests ni respuesta servida a quien ignoramos); `prune()` en el
  tick de 30 s.
- **wallet `net_actor.rs`**: campo `guard: std::sync::Mutex<PeerGuard>`
  (métodos `&self`, lock breve sin awaits); idéntico en
  `handle_incoming_request`; skips de dial en arranque/retry/ChangePeers;
  `peers_sin_baneados()` aplicado a start_broadcast / request_sync_all /
  forward de TX admitidas; strike grave en `adopt_sync_snapshot` cuando el
  gate rechaza (evento 🚨 visible en el panel de red de la GUI).

### Tests (10 nuevos en `defense.rs`, todos en verde)

leves no banean antes del límite; 3 leves banean 30min; ventana rodante
evita el ban (strikes vencidos no cuentan); snapshot inválido ban inmediato
60min; flood 10min; expiración; backoff exponencial con tope 24h; ban limpia
strikes previos; chain_id rate-based (>20 en 5min ⇒ ban, 20 ⇒ nada);
chain_id leve no contamina la canasta de `TxInvalid`.

### Regresión

- Core: `cargo test --release` → **92 tests, 0 failed** (65 unitarios + p2p
  fork/integration/reconnect/stress + network_isolation — además PROBARON el
  guard en marcha: ningún falso ban de peers honestos en los forros de P2P).
- Wallet: `cargo test` → **17 en verde** (actor 16 + E2E ~4.7s).

**Siguiente:** unidad **C** — métricas node-local (antigüedad/uptime/
contribuciones) + tree `peers` en Storage (Checkpoint 13), y redeploy del
core con `deploy.ps1` para cerrar la unidad con higiene de producción.

---

## Checkpoint 13 — Unidad C: Prueba de Tiempo node-local en verde (2026-11-06)

### `ledgerita_core/src/defense.rs`

- Campos C nuevos en `PeerScore` (RAM): `primera_conexion: Option<Instant>`,
  `pings_ok`, `pings_fallo`, `tx_aceptadas`, `snapshots_servidos`,
  `strikes_totales` (el total histórico de strikes, débil, node-local).
- `PeerLocal` persistente: los mismos contadores con `primera_conexion_ms`
  en wall clock (UNIX millis).
- Métodos: `note_conexion` (la PRIMERA marca manda — re-conexiones no
  retroceden), `note_ping(peer, ok)`, `note_tx_aceptada`,
  `note_snapshot_servido`, `antiguedad`, `uptime_ratio` (None sin datos),
  `margen_strikes`, `prioridad_dial` (antigüedad descendente, desconocidos
  al final), `antiq_count`, `export_records(now, now_wall_ms)`,
  `import_records(datos, now, now_wall_ms)`, `unix_millis_now()`.
- **Margen de veterano** (constantes `MARGEN_ANTIGUEDAD_MIN` 24h /
  `UPTIME_MARGEN_MIN` 0.99 / `PINGS_MARGEN_MIN` 10): 1 strike leve extra
  antes del ban; SOLO enmascara `TxInvalid` leves — gate/flood/saturation
  banean igual al veterano (ser viejo no te hace adulterador).
  Detalle de borrow: el margen se calcula ANTES del `.entry()` mut en
  `note_strike`.
- Export/Import `Instant ↔ wall clock`: la edad en millis se resta del
  wall-clock al exportar y se reconstruye el Instant al importar → la
  antigüedad SIEMPRE sobrevive al reinicio (histórico max(), nunca
  hacia atrás).

### `ledgerita_core/src/storage.rs`

- Tree NUEVO `peers` (node-local, JAMÁS en consenso/snapshot/MSG_SYNC).
- `load_peers() -> HashMap<String, PeerLocal>` + `save_peer(&str, &PeerLocal)`.
- Codificación estable: 7×u64 big-endian, slot 0 = TAG de versión (0);
  longitud desconocida → `decode_peer_local None` (se pierde el registro,
  no se pisa con líos ni panica).

### Wiring (mismo juego core y wallet)

- Core `network.rs`: import al arranque (`load_peers` → `import_records`);
  ConnectionEstablished → `note_conexion`; ping events → `note_ping` (el
  patrón ahora captura el `peer`); TX aceptada → `note_tx_aceptada`;
  snapshot servido → `note_snapshot_servido`; tick de 30 s → redial en
  orden `prioridad_dial` + `prune` + `export_peers_node_local()` (helper
  nuevo: upserts `save_peer` + `flush()` si guardó algo).
- Wallet `net_actor.rs`: idéntico con el guard del actor (`Mutex`);
  import al arranque con resumen por eprintln; tick 30 s = prioridad de
  dial + prune + export + `storage.flush()` + request_sync_all;
  ConnectionEstablished/ping/aceptadas/servidos igual que el core.

### Tests

- `defense.rs` (7 nuevos): primera conexión; re-conexiones no retroceden;
  matemática de `uptime_ratio`; margen SOLO veterano (con margen, el ban
  llega al 4º leve y no al 3º); veterano no escapa lo grave; export/import
  sobrevive el reinicio
  (edad ~1h reconstruida); prioridad_dial ordenada.
- `storage.rs` (2 nuevos): roundtrip save→reopen→load; formatos cortos/
  largos/extraños negados sin panic.
- **Regresión: core 101 tests en verde** (74 unitarios + 27 de
  integración P2P), **wallet 16 + E2E (4.86 s) en verde**. Builds sin
  warnings en ambos lados.

### Estado de la unidad B+C+G

✅ **COMPLETA** (diseño + G + B + C). Saldo de tests desde Checkpoint 9:
73 → 101 en core (+28), con cero regresiones en la wallet y la red de
producción tocada todavía NO (falta 4e).

**Siguiente: 4e. Redeploy core** con `deploy.ps1` (mainnet+testnet →
kettle03/kettle02) — reemplazar los demonios de agosto por binarios con
park de fees + gate + PeerGuard + Prueba de Tiempo (protocolo idéntico,
conversación sin quiebre); pausa breve de servicio para no pisar el lock
de sled; TX de prueba TestNet post-deploy. Luego el resto de Fase 2
(historial TX, botón faucet) → Fase 3.

---

## Checkpoint 14 — Redeploy de producción + TX TestNet en la red nueva (2026-11-07)

### ① Higiene de sistema (apt)

- kettle03: `apt-get update` + `apt-get upgrade` (124 paquetes) SIN reboot
  requerido; los demonios no se tocaron durante la instalación.
- kettle02: `apt-get upgrade` (245 paquetes), con dos lecciones aprendidas:
  - La primera sesión SSH cayó a los 30 min y el apt quedó vivo, pero
    descargando lento (~28 kB/s) hasta congelar la conexión en curso.
  - Se pausó el apt con `SIGSTOP` para compilar el deploy sin líos, y al
    reanudar el download quedó trancado (0 bytes/25 s) → `kill -9` +
    relanzar `upgrade -y` DETACHED (`setsid nohup`, log
    `/tmp/k02_apt_upgrade.log`): los ~278 .deb ya descargados quedaron en
    caché y la sesión nueva va a ~280 kB/s, termina sola en el fondo.

### ② Deploy (deploy.ps1)

- Local: 101 tests en verde + `cargo build --release` cacheado antes de
  subir nada.
- MainNet: tar subido (SHA-256 verificado) + build remoto en ambas
  máquinas (32.9 s en kettle03 / 14.9 s en kettle02 con apt pausado).
  Los units mainnet NO se reescriben si ya existen → el `--connect` de
  kettle02 hacia el seed quedó intacto.
- TestNet: deploy con `-SeedPeerId 12D3KooWNjCgE866sbqjr2626…` (PeerId
  extraído ANTES del unit en kettle02, porque el deploy de testnet SÍ
  reescribe ambos units: seed `--listen-port 46565`, cliente
  `--connect <PeerId del seed>`).
- Reinicio en orden: testnet seed → testnet cliente → mainnet ambos
  (`systemctl restart`) — sin pisar el lock de sled.

### ③ Evidencia post-deploy

- Identidad P2P estable: los PeerIds de TODOS los nodos idénticos a la
  era agosto (la identidad nace del password de `/etc/ledgerita/env`,
  no del binario) — cero fricción de re-peering.
- Log nuevo de la unidad C en los 4 arranques: "Reputación node-local
  de peers cargada (unidad C) peers=0".
- TestNet: conexión del cliente al seed en <1 s, ping 4.7 ms, snapshot
  servido (count=4, igual que antes), "Estado ya sincronizado".
- MainNet: sync cruzado inmediato (count=2), ping 4–6 ms.
- Ni un "🚨 Snapshot RECHAZADO por el gate" ni strike/ban en toda la red
  honesta: gate + PeerGuard de quietos ante el tráfico legítimo.

### ④ TX de prueba TestNet post-deploy (GUI, kettle01)

- La wallet GUI (A) envió **0.1 LIT A→B** con broadcast-first:
  - `Transacción aplicada` en el seed a 08:05:56.9 (peer = PeerId de la
    wallet-node de esta PC).
  - El mismo segundo el gossip llegó a kettle02 (peer = PeerId del seed)
    y aplicó → los 3 lados convergieron por el snapshot de consenso.
- La red de producción ya corre íntegramente el código del repo (park de
  fees Checkpoint 5 + gate G + PeerGuard B + Prueba de Tiempo C).

**Siguiente:** historial de TX + botón faucet en la GUI (resto de Fase 2),
y deuda 3b con diseño primero (regla 100% estado-derived + variante
"burn parcial" a decidir ahí).

---

## Checkpoint 15 — Historial de TX + botón faucet TestNet en la GUI (2026-11-07)

Cierra el resto de Fase 2 (HANDOFF sub-ítems 6a/6b). Cero cambios al
protocolo de consenso: todo lo nuevo es node-local de la wallet-node.

### ① Historial de TX (6a)

- **Core** (`storage.rs`): tree `historia` nuevo en la Storage (JAMÁS
  consenso, no viaja en MSG_SYNC). `TxHistoryRecord {tx_hash, from, to,
  amount, fee, nonce, ts_ms, kind}` con kind: 0=TX, 1=faucet. Codificación
  estable de 112 bytes: TAG (versión) + 6×u64 BE + 32B remitente + 32B
  receptor. `save_tx_record` usa el hash de la TX como CLAVE (idempotente);
  `load_tx_history(limit)` itera `.rev()` → más nueva primero y rellena el
  hash desde la clave; registros corruptos (longitud desconocida) se saltan
  sin panic. 2 tests nuevos (roundtrip con reopen + idempotencia; límite y
  basura).
- **Actor** (`net_actor.rs`): hooks en los DOS caminos de aplicación de una
  TX: la de red entrante (junto a `note_tx_aceptada`, después del apply)
  y la propia confirmada por acks (`apply_confirmed`, al final del Ok).
  RAM: `NodeShared.historial` ≤200 (`HISTORIAL_MEM_MAX`), más nueva primero
  (truncate + insert(0)); el tree persiste TODO. Carga al arranque con
  eprintln de resumen ("historial node-local cargado: N TXs"). La
  persistencia de historial NUNCA se hace con el lock de `shared`
  sostenido (std Mutex no reentrante — evitar el deadlock del tick).
- **GUI** (`main.rs`): panel "📜 Historial de TX" — Grid de 6 columnas
  (hora UTC HH:MM:SS desde ts_ms — sin chrono en la GUI —, marca, par,
  monto, fee, hash corto de 8 hex). Marca por si la contraparte es la
  dirección propia: 🪚 (kind=1) / ↑ enviada (naranja) / ↓ recibida (verde
  claro) / • TX de la red (gris); monto con signo +/- siguiendo la familia
  del color. ScrollArea de 200px, lector puro de `NodeShared`.

### ② Faucet TestNet (6b)

- `NodeCmd::Faucet{recipient, amount, cap}` → el actor llama
  `State::apply_faucet` **in-process** contra su PROPIO estado/storage:
  mismo proceso ⇒ cero conflicto del lock exclusivo de sled (la wallet usa
  `~/.ledgerita-wallet/<red>`, el daemon su carpeta). Tras el mint:
  flush + checkpoint due. El mint nace en la wallet-node y la red lo copia
  con el pull de los peers (~30s).
- GUI: panel "🪚 Faucet TestNet" visible SOLO en TestNet. Receptor vacío =
  esta wallet; monto default 1.000000 LIT; cap `DEFAULT_FAUCET_CAP`
  (10,000 LIT de por vida por dirección), status desde
  `NodeShared.faucet_status` (verde OK / rojo rechazo). El mint queda en el
  historial con kind=1 y hash sintético (24B del hex del recipient + 8B del
  ts BE) y remitente = clave de 32 ceros (convención del génesis TestNet).
- **Nota de semántica (mejor esfuerzo):** si una TX concurrente avanza la
  red mientras mintea, el mint se pierde (nuestro snapshot pierde frescura
  ante el resto). Aceptado por diseño — TestNet.

### ③ Fix de compilación y regresión

- E0063 del cierre anterior: inicializador de `NodeShared` en
  `spawn_wallet_node` sin los 2 campos nuevos → explicitados
  (`historial: Vec::new(), faucet_status: None`). Build sin warnings.
- Core: `cargo test --release` 103 en verde (76 unit lib, +2 de storage).
- Wallet: `cargo test` 16 + E2E (~4.8s) en verde.

### ④ Sin probar en vivo todavía

El botón del faucet y el panel de historial no se han corrido contra la
red real de TestNet (la wallet GUI quedó abierta en kettle01 con código
previo): el mano a mano lo hace el usuario desde la GUI — pedido de 1 LIT,
ver el mint en el balance de la dirección y en el historial 🪚. Lo que
SÍ está probado por unit tests: roundtrip del tree, idempotencia, límites,
y el parseo/hook de registro.

**Siguiente:** Fase 3 (QR, dark mode, selector de red en dashboard,
portable) o deuda 3b con diseño primero — a elección del usuario.

---

## Checkpoint 16 — Fase 3 primera tanda: dark mode + selector de red en dashboard (2026-11-07)

Elección del usuario: Fase 3 ANTES que deuda 3b (3b pide sesión de diseño
propia; Fase 3 son victorias GUI de consenso-inocuo). Pulido intermedio con
commit propio `7a2be53`: ScrollArea de página completa (la barra derecha
aparece sola) + el status del faucet vive en SU panel, no en el de Enviar.

### ① 7a — Dark mode

- Dark de fábrica: `cc.egui_ctx.set_visuals(egui::Visuals::dark())` en el
  closure de `run_native`.
- Toggle en vivo en la cabecera del dashboard: botón ☀️/🌙 que consulta
  `ui.style().visuals.dark_mode` y alterna `Visuals::dark()/light()` en
  `set_visuals`. No se persiste el tema: la sesión siempre arranca en dark.

### ② 7b — Selector de red en el dashboard

- Radio MainNet/TestNet junto al toggle de tema. Cambiar de red invoca
  `cambiar_red()`: suelta `NodeRuntime` (el drop del channel `cmd_rx`
  detiene al actor limpio), limpia `state_cache`/`last_tx`/
  `send_status`/`faucet_pending`, vuelve a `Screen::Unlock` y corre
  `on_network_changed()` (reposiciona dir/seed SOLO si eran defaults —
  custom respetado).
- Semántica: cambiar de red = cerrar sesión y re-abrir con la wallet de la
  red elegida (cada red tiene data-dir y node state propios; jamás se
  reusa un estado ya abierto).

### ③ Tests y regresión

- Wallet 16 + E2E (4.9s) en verde; build release sin warnings.
- Verificado en vivo por el usuario: mint del faucet TestNet propagado de
  punta a punta (seed `0 / 6 TX`, supply `1000003.004980`, "En sincronía ✓"
  — cierre limpio del ciclo botón → mint local → pull P2P ~30s).

**Siguiente:** 7c QR de dirección (crate `qrcode` + textura egui) y 7d
portable; luego deuda 3b con su sesión de diseño.

---

## Checkpoint 17 — QR de dirección + acceso directo de escritorio (2026-11-07)

### ① 7c — QR de la dirección

- Crate `qrcode` 0.14 con `default-features = false` (sin `image`): la
  matriz de módulos se dibuja DIRECTO con el painter de egui
  (`rect_filled`, cuadraditos de 4 px, quiet zone de 4 módulos por lado
  sobre fondo blanco). Cero texturas, cero crate `image`, cero fuentes
  externas.
- UI: botón "📷 Ver QR / Esconder QR" junto a "📋 Copiar dirección" en el
  dashboard; el QR muestra la dirección hex (64 chars) y debajo un caption
  para el usuario. Codificación por bytes con ECC media (default del crate).
- Toggle `show_qr` en `WalletApp`; helper `draw_qr()` junto al impl de App.

### ② QoL del usuario

- **Acceso directo** `Ledgerita Wallet.lnk` en la raíz del repo (kettle01):
  doble click → la GUI corre directo; WorkingDirectory la raíz. Lo añadido
  a `.gitignore`: el `.lnk` (path de usuario) y `buenaIdeaDeLaIA.txt`
  (protección anti-commit-accidental).

### ③ Tests y regresión

- Wallet 16 + E2E (4.8s) en verde; build release sin warnings.

**Siguiente:** deuda 3b (sesión de diseño de la regla estado-derived del
seed MainNet, variante burn parcial a decidir ahí) o 7d portable, a
elección del usuario.

---

## Checkpoint 18 — Wallet portable con verificador anti-contaminación (2026-11-07)

Elección del usuario: 7d portable AHORA y deuda 3b después; la app Android
queda ANOTADA como idea futura (HANDOFF sub-ítem 7e, sin fecha).

### ① Modo portable en el código (ledgerita_wallet/src/main.rs)

- Regla: si existe `portable.flag` junto al EXE → los datos viven en
  `<exe_dir>/ledgerita-data/<red>`; sin flag → `~/.ledgerita-wallet/<red>`
  igual que siempre. NADA ambiental (ni variables ni cwd): un flag raro no
  cambia el comportamiento de una copia sin flag.
- `default_wallet_dir_in(network, portable)` con el modo como INPUT
  explícito (testable sin depender de dónde venga el exe), `portable_mode()`
  mira solo el marcador, `exe_dir()` tolerante a fallos del SO.
- Test nuevo: el modo portable reposiciona a `ledgerita-data/<red>`,
  distinto del home, distinto por red, y JAMÁS pisa la carpeta del daemon.

### ② Anti-contaminación: SHA-256 sidecar + verificador

- `empaquetar_portable.ps1` (raíz): compila → arma
  `dist\LedgeritaWalletPortatil\` (exe + `portable.flag` + `ledgerita-data/`
  + verificador) → genera `SHA256SUMS.txt` (3 hashes) → auto-verificación
  final como control de calidad del propio empaquetado.
- `scripts\verificar_portable.ps1` (viaja DENTRO del paquete): compara el
  hash SHA-256 REAL de cada archivo contra el manifest.
  - Estado sano: `PAQUETE INTACTO` exit 0.
  - Probado con tamper a propósito (1 byte añadido al flag): `MAL ... el
    hash difiere` → `PAQUETE CONTAMINADO O CORRUPTO — NO distribuir, NO
    ejecutar.` exit 1.
- Protocolo de uso: verificar ANTES de distribuir y DESPUÉS de copiar a
  USB/otra máquina.
- **Evolución anotada (no bloqueante):** SHA-256 detecta corrupción y
  tampering casual; un atacante con acceso de escritura podría regenerar
  el manifest. La defensa fuerte es firmar el manifest con una clave
  ed25519 propia (infra ya existe en el core) — para una Fase 4.

### ③ QoL

- `.gitignore`: `dist/` (binarios empaquetados no van al repo), junto con
  el `.lnk` y `buenaIdeaDeLaIA.txt` del checkpoint anterior.
- `dist\LedgeritaWalletPortatil\` quedó armado y verificado (INTACTO),
  listo para USB o copiar y correr.

### ④ Tests y regresión

- Wallet: 17 unitarios (incluido el nuevo del modo portable) + E2E (4.9s)
  en verde; build release sin warnings.

**Siguiente:** deuda 3b — sesión de diseño (regla estado-derived del seed
MainNet + variante burn parcial), que se ejecuta CON el usuario.

---

## Diseño aprobado — Tesoro MainNet: goteo estado-derived + burn de fees (2026-11-07)

Antes de implementar (rutina: diseño primero), sesión de diseño completa
con el usuario sobre Deuda 3b / Deuda E. Documento:
`handoffs/DISENO_TESORO_MAINNET.md` (borrador v1 → APROBADO con decisiones
firmadas).

### Lo esencial

- **Brújula = inicio.md**: el "Tesoro sin clave privada, in-hackeable" solo
  existe como REGLA DE ESTADO (tesoro virtual, emisión determinista);
  "micro-goteo por época repartido proporcionalmente entre nodos estables"
  = goteo del tesoro ponderado por Prueba de Tiempo (unidad C ya en
  producción la provee). Opción C (tesoro del operador con `--mint`) es lo
  que inicio.md rechaza → queda como bootstrap que muere.
- **Dos tuberías separadas** (explicado y aplaudido por el usuario):
  el GOTEO del tesoro virtual (única ENTRADA de monedas nuevas; sin
  validadores elegibles esa época no se emite y no se pierde el turno) y el
  BURN del fee_pool (única salida; fees destruidos en cada Cierre de 4096
  TX, fuera del ledger para siempre, auditable vía `fees_quemados`).
- **Desviación oficial aceptada**: el "2000 bloques" de inicio.md queda
  REEMPLAZADO por `CHECKPOINT_INTERVAL = 4096` TX (decisión expresa del
  usuario). Unidad mínima 10⁻⁶ se mantiene (Q8).

### Decisiones firmadas (D1–D5)

1. Opción A (estado-derived) como esencia + B (burn) transitoria + C
   (operador-mint) funeraria.
2. **Deflación SÍ** — pedido explícito del usuario.
3. 4096 TX queda como ley del Cierre.
4. Mecánica de dos tuberías explicada y aprobada ("me encanta la idea").
5. Recomendaciones adoptadas: goteo PLANO de **500 LIT/época** (plan b,
   42k épocas ≈ 172M TX), post-reparto **50% burn / 50% validadores**
   (poza muerta prohibida: la mitad sin destinatarios se estaciona),
   unidad mínima **10⁻⁶**.

### Aterrizaje

- **Esta unidad (Checkpoint 19)**: plumas core de burn (`fees_quemados`
  al state root, g4→g4' par monotónico pool+quemados, g5 formalizado con
  `GENESIS_ACCOUNTS(red)`, `--mint` guardia en MainNet) + redeploy
  ATÓMICO de los 4 nodos (el campo nuevo cambia el state root).
- **Unidad con checkpoints futuros**: el goteo+reparto (emisión a
  validadores ponderados por PeerLocal) y el split 50/50.

**Siguiente:** implementación del burn (Checkpoint 19), luego el goteo.

---

## Checkpoint 19 — Burn de fees implementado (core) + guardia de mintea en MainNet (2026-11-07)

Implementación del diseño aprobado (commit `477c306`: tesoro virtual,
deflación SI, 4096 for life). Todo el cambio es core + un remache en el
actor de la wallet. La parte de GOTE (emisión a validadores) queda para su
unidad futura — aquí nace la plomería.

### ① Qué cambió en consenso (ledgerita_core)

- `state.rs`:
  - Campo nuevo `fees_quemados: u64` — total de fees DESTRUIDOS en los
    Cierres de Libro. Entra al `generate_state_root` (consenso completo:
    destino del fee = pool vivo o quemado, cero ambigüedad).
  - `finalize_checkpoint()` — NUEVA mecánica del Cierre (reemplaza el park
    del Checkpoint 5): pool → fees_quemados íntegro, pool queda en 0. Los
    fees salen del ledger PARA SIEMPRE (deflación decisión del usuario).
    Comentario del park migrado a la semántica nueva con su historia.
  - Gate **g4 → g4'**: `FeePoolRetrocedido` se jubila; nace
    `FeesTotalesRetrocedidos` — el PAR (pool + quemados) es monotónico
    bajo burn (el burn conserva la suma, las TX nuevas solo la suben) con
    sanity de hard cap (pool/quemados/par ≤ 21M: cero puente para overflow
    badge binarios mentirosos).
  - Gate **g5 formalizado**: se jubila el "+1" mágico →
    `genesis::genesis_accounts(network)` (MainNet 0 / TestNet 1, el tesoro
    génesis del faucet).
  - `validate_incoming_snapshot`/`merge_if_fresher` ahora reciben la RED
    (parámetro explícito; el gate ya no adivina el génesis).
  - Snapshot **v2** en el cable: `1 + 4×u64 + num_entries + ...` —
    `fees_quemados` viaja tras el `fee_pool`. Formato SEMCOMPAT: el min-
    length y el orden de campos hacen que el formato viejo NO parsee.
  - `main.rs` (CLI): **`--mint` en MainNet da ERROR** ("la emisión de
    MainNet es estado-derived...") — bootstrap TestNet nomás.
- `genesis.rs`: `genesis_accounts()` + test.
- `storage.rs`: `load_meta`/`save_meta` de 4 claves (key-per-field ⇒
  migración natural: estado de era vieja carga quemados=0, el formato
  nuevo persiste en el primer flush — TEST de migración incluido).

### ② Wiring wallet (net_actor.rs)

- Llamada a `merge_if_fresher` pasa la red (la info ya estaba in scope).
- TODO lo demás hereda gratis: el burn corre en los `finalize_checkpoint`
  existentes del actor (`apply_confirmed`, do_faucet) y del core.

### ③ Tests

- Core: 107 EN VERDE (80 lib). Nuevos/reescritos: burn exacto del Cierre
  (invariancia pool+quemados), distribució del burn por multi-época
  (`500+999` caso real), fees_totales reducido → REJECTED, burn legítimo →
  ADOPTED (pool retrocede pero quemados compensa), pool gigante → REJECTED
  (sanity), allowance g5 por red, migración de meta de 3 claves.
- Wallet: 17 + E2E (~4.9s) EN VERDE. Build sin warnings, `cargo fmt`
  corrido en core.

### ④ RED DEPLOY ATÓMICO — pendiente (próximo movimiento físico)

El `fees_quemados` cambia el state root y el formato de snapshot ⇒
**deploy.ps1 a los 4 nodos en ventana** (peers mezclados NO se entienden
mientra cross-version). El TX de prueba post-deploy debe mostrar el pool
a 0 tras el Cierre siguiente (o intacto si no hubo Cierre aún).

**Siguiente:** redeploy atómico Checkpoint 19 → luego unidad de GOTE
(goteo 500 LIT/época a validadores por Prueba de Tiempo + split 50/50).

---

## Checkpoint 19b — Redeploy atómico del burn a producción (2026-11-07, ~23:40)

Commit del código: `e4c9977`. Ejecutado con un aproach de DOS FASES que
mejora al `deploy.ps1` para cambios de consenso (el script clásico queda
igual para primeros deploys):

- **Fase A (sin downtime):** tar + build en AMBOS hosts con los servicios
  corriendo el binario viejo — Linux mantiene el inode viejo en el proceso
  vivo, el swap de binario le pasa de tapadito a cada servicio.
- **Fase B (ventana de segundos):** `restart` de los 4 demonios
  (MainNet + TestNet en kettle03 y kettle02) tras verificar AMBOS binarios
  nuevos (grep del string `"desactivado (deuda 3b)"` en cada binario remoto
  + mtimes frescos). Service files NO se reescribieron: los `ExecStart`
  con `--connect`/`--listen-port` de Checkpoint 14 quedaron intactos.

### 🔨 Trap del camino atrapada en el acto

El primer intento de Fase A "terminó en 1 s" con `FASE_A_OK`: pasé que el
tar nuevo se subió a **`/tmp/`** pero el script extrae **`/home/user/
ledgerita_core.tar`** — el TAR VIEJO del deploy del 6 de nov (413696 vs
439808 bytes). Detectado ANTES del restart por el chequeo previo
(grep=0 + mtime viejo del binario). Lección: en redeploy, VERIFICAR el
artefacto en la ruta exacta que el build consume, corriendo la
verificación de contenido del binario (no solo el hash del tar) — el
paso la trampa de la verificación de hash de deploy.ps1 que sí se
corrió... contra el archivo equivocado.

### ④ Evidencia post-restart (TODO en verde)

- kettle03 booteó esta noche (estaba apagada al arrancar la tarea; los
  dos demonios se levantaron solos al boot con el binario viejo —
  perfecto para la Fase A sin downtime).
- Seeds escuchando en sus puertos de siempre (36565 MainNet / 46565
  TestNet) con identity persistente (mismos PeerIds, identity nace del
  password del env).
- "Connection refused" transitorio (0.4 s) de kettle02 al pedir directo
  en el arranque — curado por el reintento de 30 s: `Conexión
  establecida` + **`Estado ya sincronizado`** en MainNet (count=2) y
  TestNet (count=4). La raíz del estado CUADRA con snapshot v2 + meta
  migrada (fees_quemados=0 em ambos lados, mismo root ⇒ mismos defaults).
- Cero `🚨 Snapshot RECHAZADO`, cero strikes, cero bans en toda la red.
- La wallet GUI local recompilada con el remache del actor (la vieja no
  entiende snapshot v2) — usuario valida con TX TestNet de punta a punta.

**Siguiente:** TX TestNet de prueba post-deploy (usuario, GUI) → unidad
de GOTE (emisión 500 LIT/época a validadores ponderados por Prueba de
Tiempo + split 50/50 post-reparto). El pool partió 0 tras cada Cierre.

---

## Checkpoint 19c/20 — TX post-redeploy + fee mínimo precargado + Directorio de contactos (2026-11-08)

### ① TX de prueba post-deploy en la red del burn (usuario, GUI)

- 0.1 LIT A→B, fee 0.000001. La wallet-node recompilada conversa sin
  fricción con la red en snapshot v2.
- Seed aplicó la TX a 06:51:22 (`Transacción aplicada peer=
  12D3KooWCzhb3…`) y el gossip llegó a kettle02 el mismo segundo —
  punta a punta completa en la red del burn, y la PRIMERA con un fee
  no nulo en el pool: nace vivo el fee_pool de TestNet (0.000001,
  que se quemará en el primer Cierre de Libro).

### ② UX: fee mínimo precargado (`DEFAULT_FEE_LIT`, commit `b8ac7c0`)

- El consenso acepta fee 0, pero un fee nulo es participación nula al
  burn: mínimo con sentido = 1 sat (0.000001 LIT) PRECARGADO en el
  input del envío (no solo hint). Editable siempre.

### ③ Directorio de contactos (Checkpoint 20)

- **Core `storage.rs`**: tree node-local nuevo `contactos` (JAMÁS
  consenso; los demonios no lo usan ⇒ cero redeploy necesitado).
  Clave = dirección 32B, valor = TAG + etiqueta UTF-8 (tope
  `CONTACTO_LABEL_MAX` 48, tope `CONTACTO_MAX` 50 registros). API:
  `load_contactos` (orden alfabético, registros corruptos se saltan
  sin panic), `save_contacto` (upsert por dirección), `delete_contacto`
  (informa si existía).
- **Wallet GUI**: panel **📖 Directorio (N)** en Enviar: filas etiqueta
  + dirección corta con [usar → llena el receptor] y [✖ → borra]; alta
  manual (etiqueta + dirección con validación); helper testeable
  `guardar_contacto` (tope, upsert, orden alfabético).
- **Contactos de fábrica** (`contactos_de_fabrica(red)`): direcciones
  públicas del laboratorio precargadas SOLO en el primer unlock de una
  red con el book vacío — después es 100% del usuario. TestNet: B
  receptor + nodo kettle02 + nodo kettle03; MainNet: nodos
  kettle02/kettle03. Direcciones extraídas in vivo con `--show-address`
  + password del `/etc/ledgerita/env` (coinciden con los handoffs de
  agosto: bfd54b… / 2e136a… / b1435c… / 6893239…).

### ④ Regresión

- Core: **81** en verde (81 lib; tests +1 directorio de contactos).
- Wallet: **20 + E2E (~4.8 s)** en verde.
- Builds sin warnings; `cargo fmt` en core.

**Siguiente:** unidad de GOTE (emisión 500 LIT/época ponderada por
Prueba de Tiempo + split 50/50 post-reparto).

---

## Checkpoint 21 — GOTE: goteo del tesoro + reparto post-goteo (2026-11-08)

**Commits:** diseño `30a7b7a` (BORRADOR) → `60fa5ec` (APROBADO, firmas
D6–D10 en `DISENO_GOTE.md` §11) → implementación `f07522d`.

### ① Diseño aprobado (doc antes de código)

- **D6 → R1 EQUITATIVA**: elegible = envió ≥1 TX en la época; todos pesan
  1. R2 (ponderar por #TXs) descartada: en la era µLIT, spamear 4096 TXs
  a sí mismo cuesta ~0.004 LIT para capturar ~500 LIT — incentivo
  chatarra-actividad. R1 es Sybil-neutra (N cuentas de un dueño capturan
  exactamente su fracción N/T) y participation = proof-of-use con costo
  (fees quemados).
- **D7 → Constitución B EVER-MINTEADO**: `emitido_total ≤ 21M`; lo
  quemado jamás vuelve a nacer. Seed de migración = supply congelada al
  redeploy → Tesoro útil MainNet ≈ 20,999,990 LIT.
- **D8** ambas redes · **D9** dust del goteo queda EN el tesoro ·
  **D10** TestNet primero.
- La tensión NodeLocal-vs-consenso (ley del Checkpoint 13) resuelta con
  proxy consensual: la participación ES consenso (la secuencia de TX lo
  es), entra al root; al score node-local jamás se le paga.

### ② Implementación core

- `state.rs`: `GOTEO_POR_EPOCA_SAT` (500 LIT) + `emitido_total` +
  `participacion_epoch` (viven en el root; BTreeMap determinista).
  `finalize_checkpoint` con orden EXACTO del doc: elegibles → goteo
  `min(500 LIT, disponible)` en partes iguales con división entera SIN
  mínimo (mintear 1 sat a todos con tesoro casi agotado desbordaría el
  disponible — dust queda en tesoro, D9) → burn 100% del pool en era de
  goteo → era post-goteo (disponible==0): split 50/50 con contabilidad
  EXACTA (quemado+distribuido+carry == pool). Sin elegibles: cero
  emisión, fee_pool ESTACIONADO (Q2: jamás se quema sin destino).
- Migración D7: `State::load` seedea `emitido_total = supply congelada`
  y LA PERSISTE EN EL MISMO load (un reload jamás re-seedea con supply
  ya movida — desincronizaría nodos). Distinción con
  `load_emitido_total() -> Option<u64>` (no default 0 ciego).
- Gate **g6** (`GoteoIncoherente`): emitido ≤ 21M + monotonía
  (snapshot fresco con emitido menor = mentira). Cota por época
  `(500×(epoch+1)+seed)` dejada PARA v2 (requiere seed por red firme).
- Snapshot **v3**: 5×u64 cabecera + entradas de balances 72B +
  num_particip + entradas 40B (participación con cuenta==0 = basura
  rechazada). `merge_if_fresher` adopta emitido+participación.
- `storage.rs`: tree `particip` (load/prune/apply como balances) +
  meta `emitido_total`.

### ③ UX

- Dashboard wallet: filas **"Goteo emitido:"** y **"Tesoro restante:"**
  (derivado `21M − emitido`).
- Monitor TUI: columnas "Goteo (LIT)" / "Tesoro (LIT)". `remote_monitor`:
  JSON + print con el mismo par (compatible auto vía decode v3).

### ④ Regresión + tests GOTE nuevos

- Core: **93** en verde (+12 vs 81): goteo exacto 3 elegibles (dust 2
  sat), sin elegibles (carry), recorte exacto al agotado (emited == techo,
  jamás cruza), split 50/50 con pool impar (49 burn + 2 carry + 50
  distribuido == 101), limpieza de participación (el de la época 1 no
  cobra si no reenvía), emitido/participación obligan root distinto,
  roundtrip v3 + idempotencia, g6 retrocedido/gigante/lado bueno con
  adopción, serie con recorte EXACTO (5 goteos → tesoro en 0 → época
  6 ya en split), migración seed/reload sin re-seedeo.
- Wallet 9+10+E2E y TUI build en verde; `cargo fmt`.

**Siguiente:** REDEPLOY ATÓMICO TestNet (patrón 19b Fase A/B, D10) +
TX de carga para ver el goteo REAL en vivo (4096 TX → 1 Cierre →
balances +500 LIT repartidos); MainNet después.

---

## Checkpoint 21b — Redeploy atómico del GOTE a TestNet + mudanza de IP de la seed (2026-11-09)

Código: commit `f07522d`. Redeploy con el patrón 19b MEJORADO (guardia
D10 más dura que antes). La sesión arrancó con **kettle03 caída**
("bajón" eléctrico) — la red no la veía (`Destination host unreachable`).

### ① Mudanza de IP de kettle03: .61 → .51

- Sin SSH ni ping; el escaneo LAN (herramienta nueva
  `escanear_lan.ps1` — TCP-connect asincrónico 254 IPs × 3 puertos en
  ~1 s + MAC del ARP, la deja el usuario para cazar kettles e
  impresoras 3D) encontró `.51` con SSH; la MAC comparte el OUI con
  kettle02 (34-E8-94-E9-12-84 vs -07-A2) ⇒ misma serie. `hostname` ⇒
  kettle03 confirmado; los DOS demonios ya arrancaron solos al boot con
  el binario viejo (perfecto para la Fase A).
- El host-key de ED25519 en `.51` "cambió" (era otra cosa usando esa IP
  antes): entrada vieja removida de known_hosts re-insertada por
  `accept-new` — la identidad LABORATORIO verificada por hostname +
  servicios + OUI de MAC.
- Parche SYSTEMD (ambos hosts, con sudo, `daemon-reload` sin restart):
  todos los `--connect` de `192.168.68.61` → `.51`. El PeerId de la
  seed (nace del password del env) NO cambió ⇒ multiaddr nueva:
  `/ip4/192.168.68.51/…p2p/12D3KooWNjCgE866sbqjr2626KEC3MGYKvYuVM6MvkuG5VhHvaaS`.

### ② Fase A (sin downtime) con GUARDIA MainNet EXTRA (mejora del 19b)

- **Backup del binario v2 desde `/proc/<MainPID>/exe`** (el inode vivo
  del daemon MainNet — el repositorio verdadero del binario viejo una
  vez que el build reemplaza el path) a
  `/home/user/ledgerita_core_c19_backup` en AMBOS hosts, verificado con
  DOBLE marcador: contiene "deuda 3b" (etiqueta v2 del 19b) y NO
  contiene "snapshot missing participation count" (v3).
- `ExecStart` del servicio **MainNet** apuntado AL BACKUP: si un
  MainNet crashea después del build, levanta v2 — cero fugas del
  binario nuevo hacia la red que D10 manda no tocar todavía. Fase B
  del 21b reinicia SOLO `ledgerita-core-testnet` en ambos hosts.
- Build **ONLINE** en hosts (lección: el 19b ya así lo hacía
  implícitamente — `deploy.ps1` remueve `.cargo/config.toml` tras
  extraer; `local_deps` de vendoring es EXCLUSIVAMENTE Windows-only).
  Tar verificado por hash en la ruta exacta que evalúa el build (la
  trampa del 19b), marcadores en source ok (GOTEO_POR_EPOCA_SAT,
  GoteoIncoherente, emitido_total, snapshot string v3), build Finished
  en 31 s / 15 s, binarios v3 GREPeados + mtimes frescos.
- Gotchas de sesión capturadas: shell SSH no-login no trae cargo en el
  PATH (`cargo: command not found`) ⇒ ejecutar con `bash -l`. Y dos
  trampas de PowerShell: `||` parsea mal dentro de strings para ssh
  (otra vez el patrón script-file), y `Select-String` existe solo de
  este lado (remoto = grep).

### ③ Fase B (ventana de segundos) y resultado

- Restart de `ledgerita-core-testnet` en kettle02 (cliente) y luego
  kettle03 (seed, script-file por el gate del `||`). Arranques v3
  limpios: seed "Estado cargado desde disco. 4 cuentas", mismo PeerId,
  escuchando en `/ip4/192.168.68.51/tcp/46565`.
- Cruce v2↔v3 transitorio: kettle02 vio `WARN Snapshot inválido…
  invalid ed25519 public key` mientras la seed era vieja (el cable
  v2 no parsea como v3: los length-checks del formato nuevo atrapados
  en acción) — curado por el reintento de 30 s cuando ambas punta v3:
  **`Estado ya sincronizado`** (kettle02↔kettle03, root compartido
  `adf3ba9508b7bcf9…`), cero strikes/bans en toda la sesión.
- **Seed de migración D7 verificada en vivo**: snapshot v3 del seed
  (`remote_monitor --json`): `total_supply` 1,000,003.004979 LIT =
  `goteo_emitido` 1,000,003.004979 (la supply CONGELADA al boot
  incluye el génesis-tesoro de TestNet de 1M LIT — descubrimiento:
  TestNet arranca con su tesoro génesis ya "emitido");
  `tesoro_restante` 19,999,996.995021 = 21M − supply exact;
  `tx_count: 7`, `epoch: 0`, `fee_pool: 0.000021`; cross_validation
  `all_match: true`.
- MainNet: los DOS servicios `active` corriendo el INODE viejo; si uno
  cae, levanta el backup v2 — ventana MainNet intacta para SU
  redeploy futuro.
- Wallet GUI local (kettle01) recompilada con GOTE (el exe antiguo
  tenía v2 en el actor): filas "Goteo emitido"/"Tesoro restante" +
  v3 en el actor de red — el usuario re-lee con el `.lnk`.

**Siguiente:** TX de carga TestNet: 4,086 TX faltan para el primer
Cierre del GOTEera (tx_count=7 de 4096) ⇒ ~4089 TXs transmitidos para
ver en vivo nodes: 500 LIT repartidos EN partes iguales a los elegibles
+ `emitido_total` += repartido (energía visible en el dashboard y en el
monitor). Redeploy MainNet DESPUÉS, en SU ventana propia (D10).
