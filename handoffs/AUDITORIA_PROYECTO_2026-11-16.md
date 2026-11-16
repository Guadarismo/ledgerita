# Auditoría Integral de Seguridad y Arquitectura — Ledgerita (2026-11-16)

- **Fecha:** 2026-11-16 (`00:48 -04:00`)
- **Alcance:** Auditoría post-cierre de Checkpoints 46–52 (commit HEAD `0f521af`) sobre `C:\Users\user\Documents\job\Ledgerita` (`ledgerita_core`, `ledgerita_wallet`, `ledgerita_monitor_tui`, `deploy.ps1`, `scripts/`, `tests/` y raíz del repositorio).
- **Referencia de diseño:** [inicio.md](file:///C:/Users/user/Documents/job/Ledgerita/inicio.md)

---

## 1. Opinión Honesta: ¿Va bien el proyecto?

**Sí en los cimientos estructurales; todavía NO listo para despliegue sin parches previos.**

### Lo que ya alcanzó nivel sólido tras Checkpoints 46–52
1. **Criptografía y formato de transacciones (`crypto.rs`, `release.rs`, `transaction.rs`, `serialize.rs`, `quorum.rs`):**
   - Firmas Ed25519 verificadas con `.verify_strict(...)`, rechazando escalares maleables (`S >= L`) y puntos de orden pequeño.
   - Dominio separado en firmas (`chain_id` dentro de `Transaction::to_bytes()`, etiqueta `LEDGERITA_QC_V1` en `QuorumCertificate`) y codificación canónica exacta `TX v4` (`153 B` D1 / `185 B` D2), cerrando el replay entre MainNet y TestNet y cualquier byte sobrante al decodificar.
   - Decodificación hex sobre bytes (`hex_decode` / `unhex`), sin panics con UTF-8 multibyte.
   - Claves privadas de validadores y de liberación 2-de-3 generadas fuera de Git (`respaldos-ledgerita/`).
2. **Consenso contable y poda de estado (`state.rs`, `storage.rs`):**
   - Persistencia multi-árbol atómica en `sled` (`balances`, `nonces`, `particip`, `meta` incluyendo `latest_qc`, `last_checkpoint_root` y `epoch_txs`) con `flush()`.
   - El **Hidro-Checkpointing podable (`Snapshot v6`)** cumple la visión de [inicio.md](file:///C:/Users/user/Documents/job/Ledgerita/inicio.md): las épocas cerradas se validan con `QuorumCertificate` 2-de-3, el diario `epoch_txs` (`<= 4096` TXs) permite auditar matemáticamente (`Prueba de Fórmula`) las TXs intra-época y se poda físicamente en disco al cerrar cada época.
   - Elegibilidad GOTE (`tx.fee >= 1 && tx.amount >= 1`) cierra el farming gratuito conservando micropagos M2M desde 1 satoshi.
3. **Defensa de red base (`network.rs`, `defense.rs`):**
   - Protocolos libp2p aislados por red, tope de frame `MAX_FRAME_BYTES = 8 MiB` en `RawCodec` (lectura y escritura), resolución DNS asíncrona con timeout de 5 s y verificación BFT 2-de-3 también del lado de `HidroLedger`.

### Por qué NO conviene desplegar todavía tal cual está
Activar `ValidatorSet` por defecto, introducir `epoch_txs` en Gate `g7`, enmarcar respuestas y separar rutas MainNet/TestNet en los últimos checkpoints **reveló interacciones rotas entre módulos** que antes estaban dormidas:
- **Auto-baneo entre nodos honestos:** puede ocurrir durante la ventana de cierre de época (`4096 * k` mientras aún no llega el 2.º voto `MSG_VOTE`) o en desincronizaciones intra-época por `g3` (`NonceRetrocedido`).
- **Ruptura de herramientas de TestNet y monitoreo:** `--faucet` / `do_faucet` / `--mint` rompen `g7`, `remote_monitor` falla siempre en `g6` porque usa `State::new()` como baseline, y `monitor_tui` no valida ningún gate `g1`–`g7`.
- **Pipeline `deploy.ps1` ↔ `updater.rs` desalineado:** compila e instala nombres de binario distintos (`ledgerita_core` vs `ledgerita_core_testnet`), borra flags de validador al desplegar y `validar_guardias_manifest` aún usa `.contains(...)`.
- **Fallo de auto-bloqueo y estado offline en `HidroLedger`:** `AUTO_LOCK_SECS` nunca bloquea sin interacción previa y al mover el mouse resetea el contador antes de evaluar expiración; la UI puede mostrar `"confirmado por la red"` sin estar conectada.

---

## 2. Vulnerabilidades y Bugs Encontrados (Post-Commit `0f521af`)

### 🔴 Críticas (Ruptura operativa, bypass de seguridad o auto-bloqueo de red)

1. **Desalineación crítica `deploy.ps1` / `systemd` vs `updater.rs` + canario engañable**
   - **Dónde:** `ledgerita_core/src/bin/updater.rs:62-64, 315-337, 458`; `deploy.ps1:131, 143`; `scripts/ledgerita-core-testnet.service:11`; `scripts/ledgerita-core.service:11`.
   - **Problema:** `updater.rs` instala TestNet en `/home/user/ledgerita_core/target/release/ledgerita_core_testnet`, pero tanto `deploy.ps1:143` como `ledgerita-core-testnet.service:11` ejecutan `ExecStart=.../ledgerita_core` (mismo binario de MainNet) y `deploy.ps1:132-152` sobreescribe siempre el `.service` borrando `--validator-id` / `--validator-key-file`. Al actualizar TestNet con `updater`, reinicia el binario viejo sin cambiar nada, reporta `APPLY_OK`, y `canario_testnet_verde` (`updater.rs:326`) lee el archivo `ledgerita_core_testnet` en disco (no `/proc/<pid>/exe`), autorizando pasar a MainNet aunque TestNet jamás haya corrido el release nuevo.

2. **Bypass de guardia de red en `updater.rs` por `.contains(...)`**
   - **Dónde:** `ledgerita_core/src/bin/updater.rs:88-93`.
   - **Problema:** `validar_guardias_manifest` evalúa `m.red.contains("mainnet")` / `m.red.contains("testnet")`. Un manifiesto con `red: testnet-solo` o `red: no-mainnet-test` pasa la validación para una red que no le corresponde (`"ambas"` o token exacto debe validarse por igualdad / lista exacta).

3. **`remote_monitor.rs` falla permanentemente contra redes reales (`baseline = State::new()`) y oculta caídas totales**
   - **Dónde:** `ledgerita_core/src/bin/remote_monitor.rs:341-356, 364-418, 481-495`.
   - **Problema:** Instancia `let baseline = State::new();` (`epoch = 0, emitido_total = 0`) y ejecuta `validate_incoming_snapshot_with_quorum` con él. En TestNet con génesis (`emitido_total = 1_000_000_000_000`) o MainNet real (`epoch = 1, emitido_total = 510_000_095`), Gate `g6` (`cota_goteo = 0` o `500_000_000`) rechaza **siempre** con `GoteoIncoherente`. Además, cuando ese peer falla el gate sigue sumando `healthy += 1`, y si **todos** los peers dan timeout (`ps.state.is_none()`), reporta `"status":"healthy"` con `exit code 0`.

4. **`ledgerita_monitor_tui` no valida ninguno de los gates `g1`–`g7`**
   - **Dónde:** `ledgerita_monitor_tui/src/main.rs:35-37, 245-269`.
   - **Problema:** Solo chequea `total_supply() <= HARD_CAP_SAT` (con `saturating_add`). Un nodo atacante que sirva snapshots con `epoch = 999`, `latest_qc = None`, nonces retrocedidos o emisión falsificada se muestra en verde como `"OK ✓"`.

---

### 🟠 Altas (Auto-baneos, DoS/OOM, colisión de red, pérdida de TXs y fallos de Wallet)

5. **TXs gratuitas `amount == 0 && fee == 0` rompen Gate `g5` (`CuentasImplausibles`), banean nodos honestos y permiten state-bloat**
   - **Dónde:** `ledgerita_core/src/state.rs:372-431, 685-696`; `ledgerita_core/src/network.rs:954-965, 1268-1288`.
   - **Mecanismo:** Un atacante sin saldo genera claves nuevas y envía TXs `0/0` válidamente firmadas. `apply_transaction` inserta **2** nuevas entradas con saldo `0` en `balances` (`sender` y `receiver`) por solo `+1` en `tx_count`. Un nodo recién iniciado evalúa `nuevas_cuentas (2) > costo_disponible (1)` en Gate `g5` (en MainNet desde la primera TX, en TestNet tras 3 TXs), rechaza el snapshot legítimo y banea 60 min al seed honesto.

6. **Incompatibilidad total entre Faucet / `--mint` de TestNet y Gate `g7` (`ValidatorSet = Some`)**
   - **Dónde:** `ledgerita_core/src/faucet.rs:48-96`; `ledgerita_core/src/genesis.rs:21-24`; `ledgerita_core/src/main.rs:322-395, 468`; `ledgerita_core/src/state.rs:714-781`; `ledgerita_wallet/src/net_actor.rs:366, 836-879`.
   - **Mecanismo:** Tanto `--faucet`/`--mint` como `WalletActor::do_faucet` incrementan `tx_count` y alteran `balances` **sin** firma en `epoch_txs` ni `QuorumCertificate`, además de no sumar `amount` a `emitido_total` en `apply_faucet` (permitiendo exceder `HARD_CAP_SAT` en futuros cierres). Como `main.rs` y `wallet` activan `default_validator_set(Testnet)` por defecto, cualquier peer honesto que reciba ese snapshot lo rechaza en `g7` (`FaltaCertificadoQuorum` / `CertificadoQuorumInvalido`) y banea 60 min al origen.

7. **Auto-baneo entre nodos honestos en ventanas de cierre (`latest_qc == None`), forks con `g3` (`NonceRetrocedido`) y sin `finalize_checkpoint` en `merge_if_fresher`**
   - **Dónde:** `ledgerita_core/src/state.rs:452, 599-604, 761-764, 944-970, 1049-1052, 1211-1217`; `ledgerita_core/src/network.rs:444-446, 1268-1288`.
   - **Mecanismos:**
     - Al cerrar `epoch` en `finalize_checkpoint()`, `latest_qc` queda en `None` brevemente hasta reunir el 2.º voto `MSG_VOTE`; si un par sincroniza en ese lapso, recibe `FaltaCertificadoQuorum` y **banea 60 min** al validador/seed honesto (lo mismo pasa si `WalletActor` alcanza `4096`, dado que la wallet ignora `MSG_VOTE`).
     - En `merge_if_fresher_with_quorum` (`remote_fresh > local_fresh`), `g3` se valida sin rebobinar el sufijo local divergente en `epoch_txs`: si el nodo local tenía 1 TX de Alice y la mayoría avanzó con 2 TXs de Bob, el nodo local rechaza a la mayoría por `NonceRetrocedido` y la banea 60 min.
     - En `merge_if_fresher_with_quorum` (`state.rs:953-970`) falta llamar `finalize_checkpoint()` cuando el remoto está en borde `tx_count == (epoch + 1) * 4096`; si luego entra TX 4097 local, `epoch_txs.len() < CHECKPOINT_INTERVAL` la omite del diario y rompe `g7`.
     - `resolve_fork_with_reapplied` (`state.rs:1049-1052`) aplica desempate léxico (`local_root < remote_root`) sin dar prioridad a un remoto que sí tenga `QuorumCertificate` 2-de-3 válido sobre un local sin QC.
     - Si alguien llama a `run_p2p_node` con `validator_set: None`, `process_incoming_vote` devuelve `Ok(None)` sin validar firma y `MSG_VOTE` retransmite votos falsos a la red.

8. **Regresión en `default_data_dir(network)` colisiona MainNet y TestNet en discos heredados**
   - **Dónde:** `ledgerita_core/src/main.rs:24-35`.
   - **Problema:** `if !scoped.exists() && base.join("state").exists() { base }` no exige `network == NetworkId::Mainnet`. Si existe un `~/.ledgerita/state` previo y se arranca `--network testnet` sin `--data-dir`, abre la wallet y BD de MainNet con reglas TestNet.

9. **Fallas de `PeerGuard` (anti-DoS anulado primeros 10 min tras reboot + fuga en RAM/disco)**
   - **Dónde:** `ledgerita_core/src/defense.rs:107-112, 227-235, 342-371, 396-417`; `ledgerita_core/src/network.rs:826`.
   - **Problemas:**
     - `now.checked_sub(STRIKE_WINDOW).unwrap_or(now)` convierte `cuts` en `now` cuando el uptime del SO es menor a 10 min (`Instant` monotónico), vaciando la cola de strikes en cada llamada y **anulando bans de `TxInvalid` y `ChainIdSpam` durante los primeros 10 min tras reiniciar el host**; igual en `import_records`, borrando la antigüedad (`antiguedad = 0`) de veteranos > uptime.
     - `ConnectionEstablished` siempre marca `primera_conexion = Some(now)`, pero `PeerGuard::prune` al superar `MAX_PEER_SCORES` retiene todo peer con `primera_conexion.is_some()`. Un atacante que rote `PeerId` y complete handshakes crece `scores` y el árbol `peers` de `sled` sin límite real.

10. **`RawCodec` pre-asigna hasta 8 MiB antes de leer, sin límite de conexiones ni bloqueo al conectar baneados**
    - **Dónde:** `ledgerita_core/src/network.rs:282-283, 638-658, 822-849, 1349`.
    - **Problema:** `read_frame_with_limit` hace `vec![0u8; len]` antes de leer los bytes del socket; `SwarmBuilder` carece de `connection-limits`; `InboundFailure` (como exceder 8 MiB o timeouts de lectura) no suma strikes en `PeerGuard`; y `ConnectionEstablished` no verifica `guard.is_banned`.

11. **Secuestro y pérdida silenciosa de `pending_broadcast` vía `MSG_SYNC_ALREADY` / `MSG_SYNC`**
    - **Dónde:** `ledgerita_core/src/network.rs:488-520, 1061, 1150, 1308-1325`.
    - **Problema:** `flush_pending_broadcast` ejecuta `pb.clear()` sin comprobar si hay peers honestos conectados ni si `send_request` tuvo éxito, y se dispara ante cualquier `MSG_SYNC` entrante (incluidos monitores pasivos) o cualquier respuesta `MSG_SYNC` / `MSG_SYNC_ALREADY` sin correlacionar `request_id`.

12. **`seen_votes` sin tope en época actual y evicción de votos en `VoteCollector`**
    - **Dónde:** `ledgerita_core/src/network.rs:1104-1106`; `ledgerita_core/src/quorum.rs:447, 492`.
    - **Problema:** `seen_votes.retain(|&(ep, _, _)| ep >= s.epoch)` nunca elimina votos de la época vigente si un solo validador bizantino envía >2048 raíces distintas; y 64 votos firmados por un único validador para `s.epoch + 1` saturan `MAX_COLLECTOR_ENTRIES = 64` y pueden evictar el `(s.epoch, checkpoint_root)` legítimo antes de completar quórum.

13. **HidroLedger (`ledgerita_wallet`): `AUTO_LOCK_SECS` roto, exposición prolongada de `password` y fugas de defensa en `net_actor.rs`**
    - **Dónde:** `ledgerita_wallet/src/main.rs:282, 384-395, 407-483, 579-594, 759, 1518-1527`; `ledgerita_wallet/src/net_actor.rs:394, 595, 771, 958-967, 1118, 1155-1199`.
    - **Problemas:**
      - `WalletApp::ui` no programa `ctx.request_repaint_after`, por lo que `eframe` duerme en reposo; y al volver a mover el ratón, el `if !i.events.is_empty()` actualiza `last_activity = Instant::now()` antes del `else if`, **impidiendo que `AUTO_LOCK_SECS` bloquee jamás**.
      - `self.password` no se zeroiza tras `unlock()` (permanece en claro para `build_signed_tx` y `do_export_respaldo`, y `TextEdit` deja copias al realocar); `crypto.rs:172` no envuelve `plaintext` en `Zeroizing` ni valida `plaintext.len() == 32` antes de `copy_from_slice`.
      - En `net_actor.rs`: `self.mempool` no tiene `MAX_MEMPOOL_LEN` ni se limpia al cerrar época; falta `is_banned` en `ConnectionEstablished` y `Message::Response`; falta manejar `MSG_SYNC_ALREADY`; y `decode_snapshot` corrupto no aplica strike.

14. **Cobertura P2P y CLI de monitoreo con fallos activos**
    - **Dónde:** `ledgerita_core/tests/{p2p_fork,p2p_integration,p2p_reconnect,p2p_stress,p2p_quorum_cycle,p2p_quorum_live}.rs`; `ledgerita_monitor_tui/src/main.rs:86, 110, 350-358, 440`; `ledgerita_core/src/bin/remote_monitor.rs:123, 145, 208-229`; `ledgerita_core/src/bin/monitor.rs:6-46, 100`.
    - **Problemas:**
      - 5 de 6 tests de integración siguen usando harnesses manuales en vez de `run_p2p_node_with_quorum`; `p2p_quorum_cycle` no ejercita votación BFT viva (`local_validator: None`); y `p2p_quorum_live.rs:87` espera `b"INVALID_VOTE"` plano roto tras el framing de Ckpt 46.
      - `ledgerita_monitor_tui:440` conserva `pid.to_base58()[..18]` panickeable en `PeerId` cortos y `.expect()` en el hilo de red deja la UI muda.
      - `remote_monitor` imprime banner en stdout aún con `--json` (rompiendo parsers) y documenta `-n` sin implementarlo.
      - `monitor.rs:100` imprime satoshis crudos de `fee_pool` etiquetados como `LIT` y apunta solo a `~/.ledgerita/state`.

---

### 🟡 Medias y Bajas (Resumen prioritario)

15. **Pipeline de actualización (`updater.rs` & `release_tool.rs`):**
    - `updater` recompila `cargo test --release --lib` sobre `fuente/ledgerita_core` **sin hash/firma que vincule `fuente/` con `release.bin`**, ni usa `--offline --locked`.
    - `cmd_apply` acepta `--pubkey <pk1>,<pk2>` arbitrarias por CLI (pudiendo anular `DEFAULT_RELEASE_PUBKEYS_2_OF_3`); `cmd_verify` no exige umbral 2-de-3; `backup_proc_exe` falla si el servicio está caído (`MainPID == 0`) y sobreescribe backups sin timestamp.
16. **Rendimiento y CPU bajo carga:**
    - `verify_quorum_and_epoch_journal` verifica primero hasta 4096 TXs (8192 checks Ed25519 + 2 clones de `State`) **antes** de comprobar las 2 firmas rápidas de `qc.verify`.
    - `MSG_TX` y `MSG_VOTE` vuelven a validar firmas antes de consultar caché o bajo repeticiones sin strike (`clear()` total de mempool a 8192 agrava esto), y `commit_state_with_quorum` ejecuta `fsync` + reescritura completa de todos los árboles en cada TX (`O(N^2)` en `epoch_txs` durante la época).
    - `resolve_multiaddr_dns` (`retry_interval`, 5 s/peer) y `sled::flush` corren sobre el hilo del loop P2P.
17. **Wallet UX / Fork Choice / Storage:**
    - `WalletActor` usa `resolve_fork_if_divergent` (en lugar de `resolve_fork_with_reapplied`) perdiendo el re-gossip de TXs reaplicadas; `use_d2` se congela antes de sincronizar la primera vez; el indicador `"confirmado por la red"` no verifica `pv.connected` ni resetea `pv.synced = false` en `ConnectionClosed`; `send_status` se borra en ~16 ms si hubo un `last_broadcast` anterior; `storage::load_tx_history` devuelve orden por hash SHA-256 en lugar de cronológico; `save_contacto` no trunca etiquetas > 192 B antes de guardar.
18. **Bordes `epoch == 0` y `--mint`:**
    - En `epoch == 0`, si un peer manda solo un sufijo `remote.epoch_txs` apoyado en `self.generate_state_root()`, `state.rs:967` reemplaza `self.epoch_txs` truncando el diario completo de la época (`[MEDIA-1]`).
    - `--mint` en TestNet no valida `HARD_CAP_SAT` ni actualiza `emitido_total`.
19. **Higiene CLI, scripts y raíz del repositorio:**
    - `ledgerita_core/src/main.rs`: `LEDGERITA_PASSWORD` vivo en memoria indefinidamente; acepta password vacía `""`; `--send` aplica TXs antes de sincronizar; `&hex[..16]` en `--mint` puede panickear con whitespace Unicode; `main.rs` y `show_address` siguen en D1 mientras Wallet ya usa D2.
    - `scripts/ledgerita-backup.sh:18-20` apunta a `/home/user/.ledgerita/wallet.key` (desalineado de `~/.ledgerita/mainnet`); `scripts/ledgerita-healthcheck.sh:23` busca un log que pasó a `debug!`; `rustup-init.exe` (~10 MB) sigue *tracked* en el índice de Git aunque se añadió a `.gitignore`; `consulta10-11-2026.txt` sigue untracked en raíz y `HANDOFF.md` conserva encabezados/IP `.61` obsoletos en su mitad superior.

---

## 3. Alineación con `inicio.md`

| Pilar de `inicio.md` | Estado actual | Brecha detectada |
| :--- | :--- | :--- |
| **Hidro-Checkpointing (poda, ~1 GB acotado)** | **Muy alineado** (`Snapshot v6` + poda en `sled`) | `PeerGuard::scores` + tree `peers`, `mempool` en Wallet y `seen_votes` en época aún crecen sin poda efectiva bajo ataque Sybil/bizantino. |
| **Prueba de Fórmula (Zero-Trust determinista)** | **Fuerte en core TX/QC/D2**, parcial en periferia | `ledgerita_monitor_tui` no corre `g1`–`g7`; `remote_monitor` falla con `baseline = State::new()`; TX `0/0` desde cuentas nuevas rompe `g5`; `g3` sin unwind puede auto-banear nodos honestos. |
| **Prueba de Tiempo (anti-Sybil por antigüedad)** | **Implementada y persistente** | `checked_sub.unwrap_or(now)` borra la antigüedad de veteranos y desactiva bans de strikes en los primeros 10 min tras reiniciar el SO. |
| **Tesoro Génesis sin clave privada + M2M AI-Ready** | **Alineado en MainNet/GOTE** (`testnet_faucet_address` keyless SHA-256, micropagos `fee >= 1 && amount >= 1`) | El TestNet Faucet (`--faucet` / `do_faucet`) quedó roto frente a `g7` porque mintea sin TX firmada ni actualizar `emitido_total`; Wallet y Core difieren en D2 vs D1 por defecto. |

---

## 4. Plan Sugerido si se Ejecuta una Nueva Ronda de Pulido (Pre-Despliegue)

1. **Estabilidad BFT y Zero-Trust sin auto-baneos (Core):**
   - Bloquear TXs `amount == 0 && fee == 0` desde cuentas inexistentes (o exigir `tx.fee >= 1` global si el protocolo lo admite, ajustando tests).
   - Comprobar `qc.verify` **antes** del unwind/replay en `g7` (`epoch >= 1`).
   - En `merge_if_fresher_with_quorum`: finalizar checkpoint pendiente en borde `4096` tras adoptar, priorizar `ValidatorSet` QC sobre ramas sin QC en forks, y **no** banear 60 min por `FaltaCertificadoQuorum` transitorio ni por `g3` si hay un prefijo común en `epoch_txs` (intentar fork/resync controlado).
   - Arreglar `epoch == 0` sufijo parcial en `state.rs:967` (acumular/verificar desde inicio real de época), `PeerGuard::prune` / `checked_sub` post-reboot (`saturating_sub` / guardar edad acumulada), `seen_votes` + `VoteCollector` por época actual vs futura, `flush_pending_broadcast` (solo limpiar tras envío real y sin aceptar `MSG_SYNC_ALREADY` como trigger sin `request_id`), `connection-limits` e `is_banned` en `ConnectionEstablished`.
   - Resolver el diseño del Faucet TestNet compatible con `g7` (p. ej. cuenta operadora/faucet financiada en Génesis con clave controlada en TestNet enviando `Transaction` normal en `epoch_txs` y actualizando `emitido_total`, en vez de mutaciones mágicas sin firma).
2. **Wallet, TUI y Monitores:**
   - Arreglar `AUTO_LOCK_SECS` (`request_repaint_after` + chequear expiración antes de resetear `last_activity`), zeroize inmediato de `password` / `decrypt_seed`, `mempool` acotada + `MSG_VOTE` + `resolve_fork_with_reapplied` + `pv.connected` en dashboard de `HidroLedger`.
   - Arreglar `remote_monitor` (baseline consciente de red/estado o `--baseline-from` + `--json` limpio + `-n` + conteo correcto de fallos), `monitor.rs` (`format_amount` + rutas por red) y `ledgerita_monitor_tui` (`g1`–`g7`, `[..len.min(18)]`, sin `expect()` silencioso).
3. **Despliegue, `updater.rs` e Higiene:**
   - Alinear un único criterio de nombres/rutas en `deploy.ps1`, `scripts/*.service` y `updater.rs`, verificar `/proc/<pid>/exe` real en `canario_testnet_verde`, igualdad exacta en `m.red`, prohibir `--pubkey` arbitrario en MainNet, restringir `default_data_dir` fallback solo a `Mainnet` (o chequear marca de red), des-trackear `rustup-init.exe` (`git rm --cached`) y actualizar `HANDOFF.md` y tests E2E reales sobre `run_p2p_node_with_quorum`.
