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
