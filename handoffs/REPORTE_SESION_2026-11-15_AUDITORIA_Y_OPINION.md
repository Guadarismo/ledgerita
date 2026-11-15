# 🛡️ Auditoría Integral y Opinión Técnica — Ledgerita (2026-11-15)

**Fecha:** 2026-11-15 13:56 (hora local)  
**Tipo de documento:** Revisión completa de código, arquitectura, despliegue, seguridad y dependencias  
**Commit auditado:** `450e95f` (rama `main` tras completar Checkpoints 37–45)  
**Estado del working tree:** `deploy.ps1` (modificado sin commit), `ledgerita_monitor_tui/Cargo.lock` (modificado sin commit), `consulta10-11-2026.txt` (sin seguimiento)  
**Suites verificadas en esta sesión:**  
- `ledgerita_core`: **175 passed; 0 failed; 1 ignored** (`tests/p2p_quorum_live.rs`)  
- `ledgerita_wallet`: **22 passed; 0 failed** (10 lib + 11 bin + 1 E2E)  
- `ledgerita_monitor_tui`: **5 passed; 0 failed**

---

## 🎯 1. Resumen Ejecutivo y Opinión General

Entre el 13 y el 15 de noviembre de 2026 Ledgerita dio un salto de madurez notable al ejecutar los 9 checkpoints del [PLAN_CORRECCION_VULNERABILIDADES_2026-11-13.md](file:///C:/Users/user/Documents/job/Ledgerita/handoffs/PLAN_CORRECCION_VULNERABILIDADES_2026-11-13.md): se incorporó `zeroize` para higiene de llaves, el módulo `quorum.rs` con validación Ed25519 estricta (`verify_strict`), el Gate `g7` y Snapshot v5, resolución determinista de bifurcaciones (`TxUndo` + `resolve_fork_if_divergent`), direcciones hash Fase D2a (`SHA-256(pubkey)` + TX v3), soporte DNS en multiaddrs y multifirma 2-de-3 en `release.rs`, `release_tool` y `updater`.

**¿Va bien el proyecto?**  
**Sí en diseño conceptual y disciplina de desarrollo, pero NO está listo aún para abrirlo a terceros ni para superar su primer Cierre de Libro (`tx_count == 4096`) con quórum BFT activo.**  

Esta segunda auditoría completa (leyendo código fuente de core, wallet, monitor TUI, tests de integración, scripts de despliegue, systemd, dependencias y git) revela que, aunque los **tests unitarios pasan**, hay **bugs bloqueantes de vivacidad (auto-baneos entre nodos honestos)** y **brechas entre módulos** que hacen que varias mitigaciones de los Checkpoints 39–45 funcionen solo de forma parcial en runtime real.

---

## 🚨 2. Vulnerabilidades y Fallos Críticos (Prioridad Inmediata)

### 🔴 C1. Bloqueo intra-época del quórum (`g7`), pérdida de QC al reiniciar y bypass en época 0
* **Ubicación:** [state.rs](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/state.rs#L174), [L480-L490](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/state.rs#L480-L490), [network.rs](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/network.rs#L701-L739) y [storage.rs](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/storage.rs).
* **Mecanismo doble:**
  1. **Auto-baneo desde la primera TX de la época 1:** Los votos y certificados `latest_qc` solo se firman en los cierres de época (`tx_count % 4096 == 0`), sellando `(epoch, checkpoint_state_root)`. Sin embargo, para `remote.epoch > 0`, el Gate `g7` ([state.rs:487-488](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/state.rs#L487-L488)) ejecuta `qc.verify(chain_id, remote.epoch, &remote.generate_state_root(), vset)`. En cuanto entra la TX `4097`, el `state_root` actual cambia mientras `latest_qc.state_root` sigue apuntando al cierre anterior: todo snapshot intra-época falla `g7` (`CertificadoQuorumInvalido`) y dispara baneo de **60 minutos** contra nodos honestos.
  2. **Pérdida al reiniciar:** `State::load` ([state.rs:174](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/state.rs#L174)) inicializa `latest_qc: None` y `State::flush` nunca lo persiste en `sled`. Reiniciar un nodo en `epoch >= 1` borra su QC y provoca `FaltaCertificadoQuorum`.
  3. **Bypass en época 0:** La condición `(remote.epoch > 0 || remote.epoch > self.epoch)` omite `g7` por completo mientras ambos nodos estén en `epoch == 0` (`tx_count` 0..4095), permitiendo inyección de balances sin QC antes del primer cierre.
  4. **Quórum desactivado por defecto y ausente en MainNet/Wallet:** `validator_set` es `None` salvo que se pase `--validators-file`. Ni `ledgerita_wallet` ([net_actor.rs:944](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_wallet/src/net_actor.rs#L944)), ni `remote_monitor`, ni `ledgerita_monitor_tui`, ni [ledgerita-core.service](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/scripts/ledgerita-core.service) verifican `g7`. Además TestNet quedó en 2-de-2 (cero tolerancia a caída de 1 nodo).
* **Impacto:** Parálisis y ban mutuo de la red al cruzar `tx_count > 4096` con `--validators-file`; falsificación trivial de saldos en wallet y en época 0.
* **Corrección recomendada:** Persistir `latest_qc` en `sled`; distinguir `last_checkpoint_root` (sellado por `latest_qc`) vs verificación de deltas/TXs firmadas dentro de la época (o probar transición intra-época desde el `checkpoint_root` certificado); exigir `ValidatorSet` embebido por `NetworkId` (3 validadores, quórum 2-de-3) también en Wallet y monitores.

---

### 🔴 C2. Auto-baneo entre nodos honestos por respuestas `MSG_TX` y `MSG_VOTE` sin framing
* **Ubicación:** [network.rs](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/network.rs#L675-L775), [L850-L886](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/network.rs#L850-L886) y [defense.rs](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/defense.rs#L34-L36).
* **Mecanismo:** Cuando un nodo recibe `MSG_TX` o `MSG_VOTE`, responde con texto plano ASCII (`b"OK"`, `b"DUPLICATE"`, `b"OK_VOTE"`, `b"REJECT:..."`, `b"INVALID_SIG"`). Sin embargo, el manejador `Message::Response` ([network.rs:876-886](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/network.rs#L876-L886)) exige que toda respuesta de longitud `>= 2` comience con `[chain_id, msg_type, ...]` vía `split_msg`. Como `'O'` (`0x4F`), `'D'` (`0x44`), `'R'` (`0x52`), etc. no coinciden con `0x01`/`0x02`, `split_msg` devuelve `None` y aplica `StrikeKind::ChainIdSpam`.
* **Impacto:** Al superar 20 confirmaciones de TX/votos en una ventana de 5 minutos (`CHAINID_SPAM_LIMIT = 20`), **los nodos honestos se banean entre sí por 30 minutos** en operación normal.
* **Corrección recomendada:** Enmarcar las respuestas en protocolo `[chain_id, msg_type_ack, ...]` o distinguir explícitamente las respuestas ASCII esperadas antes de invocar `split_msg`.

---

### 🔴 C3. Denegación de servicio (OOM / Panic pre-autenticación) en `RawCodec`
* **Ubicación:** [network.rs](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/network.rs#L211-L233) y [remote_monitor.rs](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/bin/remote_monitor.rs#L176).
* **Mecanismo:** `RawCodec::read_request` y `read_response` leen un `u32` big-endian del stream TCP/Noise y asignan directamente `vec![0u8; len]` **sin tope máximo de frame**, antes de validar `PeerGuard` o `chain_id`.
* **Impacto:** Cualquier peer conectado que envíe `0xffffffff` fuerza un intento de reserva de ~4 GiB y tumba el proceso del nodo por OOM/panic. (`Message::Response` tampoco verifica `guard.is_banned` antes de procesar snapshots.)
* **Corrección recomendada:** Imponer `MAX_FRAME_BYTES` acotado (por ejemplo 4–8 MiB acorde a snapshots esperados) y rechazar `len > MAX_FRAME_BYTES` con error de I/O antes de asignar memoria.

---

### 🔴 C4. Bypass de nonces monótonos en Gate `g3`
* **Ubicación:** [state.rs](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/state.rs#L388-L394) y [L884-L886](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/state.rs#L884-L886).
* **Mecanismo:** `g3` itera sobre `&remote.nonces`, y `decode_snapshot` descarta entradas con `nonce == 0`. Un snapshot remoto que omita una dirección en `nonces` (o la envíe en `0`) nunca dispara `NonceRetrocedido` contra `self.nonces`.
* **Impacto:** Permite resetear el nonce de cuentas existentes en sincronizaciones sin prueba de quórum (época 0 / sin `validators`), habilitando replay de transacciones previas.
* **Corrección recomendada:** Iterar `for (addr, local_nonce) in &self.nonces` verificando que `remote.nonces.get(addr).copied().unwrap_or(0) >= *local_nonce`.

---

### 🔴 C5. Error aritmético post-goteo en `finalize_checkpoint` y choque con `g4'`
* **Ubicación:** [state.rs](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/state.rs#L414) y [L703-L704](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/state.rs#L703-L704).
* **Mecanismo:** En la era post-goteo (`disponible == 0`), [state.rs:703-704](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/state.rs#L703-L704) calcula:
  ```rust
  let carried = mitad - distribuido;
  let quemado = pool - mitad - carried;
  ```
  Como `mitad == distribuido + carried`, restar `carried` además de `mitad` **destruye `carried` satoshis**. Adicionalmente, al repartir ~50% del `fee_pool` hacia `balances`, la suma `fee_pool + fees_quemados` **disminuye**, por lo que el Gate `g4'` (`fees_totales_remoto >= fees_totales_local`) rechazará como inválido cualquier sync legítimo tras un cierre post-goteo.
* **Corrección recomendada:** Corregir `let quemado = pool - mitad;` (`quemado + distribuido + carried == pool`) y ajustar `g4'` considerando el split post-goteo o contabilizando `fees_distribuidos`.

---

### 🔴 C6. Fragilidad en `resolve_fork_if_divergent` y límite de profundidad 1 en `TxUndo`
* **Ubicación:** [state.rs](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/state.rs#L277-L316), [L576-L606](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/state.rs#L576-L606), [network.rs](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/network.rs#L941-L961) y [faucet.rs](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/faucet.rs#L77-L78).
* **Hallazgos:**
  1. Si `rolled_back_tx` se reaplica con éxito (`reapplied == true`) y lleva `tx_count` a un múltiplo de `4096`, ni `resolve_fork_if_divergent` ni `network.rs` llaman a `finalize_checkpoint()`, dejando el nodo desfasado de época.
  2. Si `!reapplied`, [network.rs:953-960](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/network.rs#L953-L960) encola la TX **inválida/rechazada** en `pending_broadcast` para re-difundirla a los peers.
  3. `last_undo` guarda solo 1 TX: una divergencia de `>= 2` TXs o tras un reinicio descarta las TXs anteriores (incluso dejando atascado un `nonce` futuro si se reaplicó solo la segunda TX). `apply_faucet` y `--mint` incrementan `tx_count` sin invalidar `last_undo`, pudiendo revertir una TX anterior equivocada.
  4. `resolve_fork_if_divergent` comprueba `local_root < remote_root` **antes** de validar gates `g1`–`g7`, evitando penalizar snapshots inválidos cuyo hash sea mayor que el local; y sin QC en época 0 un atacante puede minar un `remote_root < local_root` en ~2 hashes SHA-256 para forzar rollbacks cada 30s.

---

## 🟠 3. Hallazgos de Severidad Alta (Criptografía, Wallet, Updater e Infraestructura)

### 🟠 H1. Ed25519 no estricto (`verify()`), replay entre redes y Sybil gratis en GOTE
* **Verificación laxa de firmas:** `quorum.rs:252` usa `verify_strict`, pero `Address::verify_signature` / `verify_signature_with_pubkey` ([crypto.rs:98, 118](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/crypto.rs#L98-L118)) y `release.rs` ([L124, 199](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/release.rs#L124)) usan `.verify()`, aceptando puntos de orden pequeño / torsión y firmas maleables (cambiando `hash_tx`).
* **Replay MainNet ↔ TestNet:** `Transaction::to_bytes()` ([transaction.rs:53-62](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/transaction.rs#L53-L62)) firma `sender || receiver || amount || fee || nonce` **sin `chain_id`**. Una TX válida firmada para TestNet puede reinyectarse en MainNet si el remitente reutiliza llave/dirección en ambas redes.
* **TXs de `amount == 0, fee == 0` y abuso de GOTE:** Al aceptarse `fee == 0` y `amount == 0`, cualquier atacante puede generar cientos de cuentas vacías enviándose `0` satoshis entre sí para capturar la mayoría del `goteo` de `500 LIT` por época sin costo.
* **Faucet y emisión en TestNet:** `testnet_faucet_address()` ([genesis.rs:21-23](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/genesis.rs#L21-L23)) deriva de `seed = [0u8; 32]` pública en código (cualquiera puede gastar los `1,000,000 LIT` vía P2P); `apply_genesis` y `apply_faucet` tampoco incrementan `emitido_total`, permitiendo que TestNet exceda `HARD_CAP_SAT`.

---

### 🟠 H2. Persistencia no atómica en Sled y riesgos de panic/overflow
* **Crash safety (`State::flush`):** [state.rs:179-195](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/state.rs#L179-L195) ejecuta `prune_*` e inserciones separadas sobre 4 árboles `sled` distintos (`balances`, `nonces`, `meta`, `particip`) sin batch atómico ni `db.flush()`. Una interrupción durante la escritura deja el estado inconsistente en disco.
* **Overflows/Panics:** `apply_transaction` ([state.rs:246](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/state.rs#L246)) suma `receiver + tx.amount` sin `checked_add`; `State::total_supply()` ([state.rs:319](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/state.rs#L319)) usa `.values().sum()` (pánico en debug / wrap en release, invocable remotamente en `net_actor.rs:947` y `remote_monitor`); `hex_decode` ([crypto.rs:301](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/crypto.rs#L301)) y `unhex` ([release.rs:236](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/release.rs#L236)) indexan `&s[i..i+2]` pudiendo paniquear ante caracteres UTF-8 multibyte (e `unhex` trunca strings impares).

---

### 🟠 H3. `ledgerita_wallet`: sin D2a, sin quórum y exposición de contraseña en memoria
* **Sin quórum ni fork resolution:** `WalletActor` llama `merge_if_fresher(..., None)` ([net_actor.rs:944](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_wallet/src/net_actor.rs#L944)), confía ciegamente en cualquier snapshot remoto y marca `pv.synced = true` antes de evaluar errores ([L949](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_wallet/src/net_actor.rs#L949)); además no llama a `resolve_fork_if_divergent` ante `SyncResult::Divergent`.
* **Sin soporte Fase D2a para el usuario:** La wallet crea identidades y firma transacciones exclusivamente en formato legado **D1** ([main.rs:392, 552](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_wallet/src/main.rs#L392)), exponiendo la pubkey en texto de dirección desde la creación (aunque sí puede enviar a receptores D2).
* **Memoria y bloqueo:** `WalletApp.password` permanece en RAM durante toda la sesión desbloqueada (necesario para firmar en cada envío), no existe botón de bloqueo manual ni auto-lock por inactividad, `do_restaurar_respaldo` no limpia `password` en rama `Err`, y `decrypt_seed` / respaldos legado sin cifrar manejan `Vec<u8>` / `String` sin `Zeroizing`.

---

### 🟠 H4. Brechas operativas en `updater.rs` y `deploy.ps1` respecto al flujo multifirma 2-de-3
Aunque la lógica base de verificación 2-de-3 ya está implementada y testeada en [release.rs](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/release.rs) y [release_tool.rs](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/bin/release_tool.rs), el despliegue en producción aún no la aprovecha y presenta fallos en `updater`:
1. **Multisig opcional por CLI:** En [updater.rs](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/bin/updater.rs#L97-L105), la constante de llave oficial solo existe dentro de `#[cfg(test)]`; producción acepta cualquier `--pubkey` pasado por parámetro, y si es una sola llave aplica 1-de-1. Conviene fijar las 3 pubkeys oficiales generadas (`a3b99cd6...`, `2a733f8e...`, `fb4fc38e...`) como política obligatoria 2-de-3 por defecto.
2. **Bypass de revocación por mayúsculas:** `llave_revocada` compara hex sensible a mayúsculas/minúsculas mientras `unhex` acepta mayúsculas; una llave revocada pasada en uppercase pasa la comprobación.
3. **TOCTOU, RCE de fuente no firmada y backup silencioso:** `updater.rs` verifica `release.bin` en memoria en el paso 1, luego corre `cargo test --release --lib` sobre `{dir}/fuente` (directorio **no cubierto por la firma del manifiesto**) y vuelve a leer `release.bin` desde disco en el paso 4 sin verificar que no haya cambiado. Además `backup_proc_exe` no valida `status.success()` del comando `cp`.
4. **Conflicto MainNet vs TestNet:** `validar_guardias_manifest` ([updater.rs:77-83](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/bin/updater.rs#L77-L83)) rechaza manifiestos `red: mainnet` y ambas redes systemd apuntan al mismo `PATH_BINARIO` (`/home/user/ledgerita_core/target/release/ledgerita_core`), por lo que actualizar TestNet pisa el binario en disco de MainNet.
5. **`deploy.ps1` fuera de diseño U2/U5 y borrando quórum:** [deploy.ps1](file:///C:/Users/user/Documents/job/Ledgerita/deploy.ps1) borra `.cargo/config.toml` y compila en vivo con internet en los nodos (`local_deps` excluido), sobrescribe el servicio TestNet sin los flags `--validators-file` / `--validator-id` / `--validator-key-file` del Ckpt 41, deja MainNet sin flags de red ni quórum, y mantiene la IP vieja `.61` en su ayuda de la línea 203.

---

### 🟠 H5. Higiene de secretos, permisos de systemd e historial de Git
* **Llaves de release en workstation:** Las 3 nuevas llaves (`release_key_1..3.seed`) y el archivo legado `release_key.seed` existen fuera de git en `C:\Users\user\Documents\respaldos-ledgerita\` (bien protegido de commits), pero aún conviven en el mismo disco hasta que `2` y `3` se muevan físicamente a pendrive/máquina secundaria. `release_tool gen-key` tampoco aplica `chmod 0600` en Unix.
* **`.gitignore` incompleto:** Excluye `release_key.seed`, `SECRETS_LOCAL.md`, `wallet_recovery.env` y `local_deps/`, pero **no** cubre patrones genéricos `*.seed`, `*.key`, `validators.txt` ni `validator.key`. Además `rustup-init.exe` (~10 MB) está trackeado en git.
* **Credenciales históricas en docs:** Se confirmó que los archivos privados (`SECRETS_LOCAL.md`, `release_key.seed`, `wallet_recovery.env`) **nunca** entraron al historial de git en los 81 commits; sin embargo, contraseñas en texto claro de operación siguen presentes en [HANDOFF.md](file:///C:/Users/user/Documents/job/Ledgerita/HANDOFF.md#L128) (líneas 128 y 220) y ~15 reportes anteriores en `handoffs/` (tal como reconoce `HANDOFF.md:290`).
* **Endurecimiento del sistema operativo:** Los servicios systemd corren bajo el usuario `user` (mismo dueño del código fuente, llaves y backups) sin directivas de aislamiento (`NoNewPrivileges=true`, `ProtectSystem=strict`, `PrivateTmp=true`) y con puertos P2P escuchando en `0.0.0.0`. Asimismo, el ejecutable `main.rs` comparte por defecto `~/.ledgerita` si un operador olvida `--data-dir` al correr `--network testnet`.

---

## 🟡 4. Hallazgos Medios y Deuda Técnica

1. **Riesgo 6 (Snapshot monolítico cada 30s) sigue abierto:** El respondedor de `MSG_SYNC` ([network.rs:779-795](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/network.rs#L779-L795)) no compara el `state_root` de 32 bytes que envía el solicitante: serializa y transmite todo el snapshot completo cada 30 segundos por cada peer aunque ambos estén idénticos.
2. **Seeds de fallback y DNS asíncrono:** `default_fallback_seeds` ([network.rs:36-52](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/network.rs#L36-L52)) y `default_seeds` ([net_actor.rs:81](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_wallet/src/net_actor.rs#L81)) no tienen llamadores en runtime, siguen con IPs LAN privadas (`.60`/`.64`), y las entradas secundarias de `.64` omiten `/p2p/<PeerId>`. Además `resolve_multiaddr_dns` ejecuta `ToSocketAddrs` bloqueante dentro del loop Tokio.
3. **`PeerGuard` y recursos en memoria (`defense.rs`, `network.rs`, `quorum.rs`):**
   * `PeerGuard::prune()` ([defense.rs:392-401](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/defense.rs#L392-L401)) elimina toda la entrada `PeerScore` al vencer un baneo (anulando el backoff exponencial en reincidencias) y nunca limpia peers sin strikes ni persiste baneos por IP o en Sled (un atacante cambia su `PeerId` y vuelve a entrar).
   * `VoteCollector::prune_below` es código muerto; un único validador comprometido puede llenar las 64 ranuras del colector con épocas futuras para desalojar votos vigentes; y `MSG_VOTE` no se retransmite (no hay gossip entre nodos no productores).
   * `mempool` (`HashSet<[u8; 32]>`) crece sin límite; y varios errores P2P (`decode_snapshot` fallido, `apply_transaction` inválido, `MSG_SYNC < 32B`, `msg_type` desconocido) no suman strikes.
4. **Carencias en `ledgerita_monitor_tui` y `remote_monitor`:**
   * `ledgerita_monitor_tui` no resuelve DNS, no reintenta reconexión automática cuando cae un peer, no acepta `-n` (pese a anunciarlo en `--help`), acepta `--refresh 0` (busy-loop) e ignora `latest_qc`.
   * `remote_monitor` puede entrar en panic si un `PeerId` Base58 mide menos de 18 bytes (`&[..18]` en [L397](file:///C:/Users/user/Documents/job/Ledgerita/ledgerita_core/src/bin/remote_monitor.rs#L397)), no valida `g1`–`g7` ni quórum (solo `total_supply <= HARD_CAP_SAT` e igualdad entre peers consultados), y no escapa saltos de línea en la salida JSON.
5. **Tests de integración con cobertura engañosa o desalineada:**
   * Cuatro suites (`p2p_fork.rs`, `p2p_integration.rs`, `p2p_reconnect.rs`, `p2p_stress.rs`) codifican snapshots `Mainnet` pero invocan `merge_if_fresher(NetworkId::Testnet, ...)`.
   * `p2p_fork.rs` nunca ejercita `resolve_fork_if_divergent` (fuerza el desempate enviando otra TX externa).
   * `p2p_quorum_live.rs` (ignorado) solo verifica que envió el snapshot falso (`epoch = 99`), sin comprobar que el nodo remoto realmente aplicara el rechazo/ban.
6. **Dependencias y builds portables:**
   * `ledgerita_core` tiene 375/375 crates vendored en `local_deps/` (excelente), pero `deploy.ps1` no los usa; `ledgerita_wallet` y `ledgerita_monitor_tui` carecen de `.cargo/config.toml` offline.
   * `sled 0.34.7` lleva sin mantenimiento desde 2021 (arrastrando `fxhash` y `parking_lot 0.11` / `instant`).
   * El paquete portable de wallet ([empaquetar_portable.ps1](file:///C:/Users/user/Documents/job/Ledgerita/empaquetar_portable.ps1)) genera un `SHA256SUMS.txt` útil contra corrupción, pero sin firma Ed25519 contra manipulación.

---

## 🧭 5. Hoja de Ruta Recomendada (Próxima Fase de Endurecimiento)

Si quieres llevar el proyecto de forma ordenada al siguiente nivel sin perder estabilidad en tu TestNet de laboratorio, recomiendo este orden:

1. **Bloque Crítico de Estabilidad y Consenso (antes de llegar a 4096 TXs):**
   - **Arreglar el framing de respuestas `MSG_TX`/`MSG_VOTE`** (`network.rs`) para eliminar el auto-baneo tras 20 TXs.
   - **Poner tope `MAX_FRAME_BYTES` en `RawCodec`** para cerrar el OOM DoS con 5 líneas.
   - **Corregir `g3`** (iterar `self.nonces`) **y `g7`** (persistir `latest_qc` en `sled`, separar `checkpoint_root` firmado de las TXs intra-época o emitir cadena de deltas/pruebas intra-época, y definir `ValidatorSet` 2-de-3 por defecto por red — agregando idealmente un 3er validador ligero).
   - **Corregir la fórmula post-goteo y `g4'`** en `finalize_checkpoint`, además del bug de `reapplied == true` sin `finalize_checkpoint()` y `!reapplied` en `pending_broadcast`.
2. **Bloque Criptográfico y Almacenamiento:**
   - Cambiar todas las firmas a **`verify_strict()`** y añadir **`chain_id`** dentro de `Transaction::to_bytes()` (junto con `fee >= MIN_FEE` para neutralizar el farming gratis de GOTE).
   - Hacer **`State::flush` atómico** (batch `sled` + `storage.flush()`) y reemplazar sumas directas por `checked_add` / `try_fold`.
3. **Bloque Wallet D2a + Quórum + Despliegue Real 2-de-3:**
   - Llevar **Fase D2a (`new_d2`)** y **verificación de `ValidatorSet`** a `ledgerita_wallet` y `remote_monitor`/`TUI`.
   - Endurecer `updater.rs` con las **3 pubkeys oficiales embebidas por defecto** (umbral 2-de-3 obligatorio), comparación lowercase en revocación, lectura desde bytes verificados en RAM sin TOCTOU, binarios separados por red y reparación de `--red mainnet`.
   - Actualizar [deploy.ps1](file:///C:/Users/user/Documents/job/Ledgerita/deploy.ps1) para no borrar `--validators-file` en TestNet ni usar IPs `.61` viejas, y separar físicamente `release_key_2.seed` y `release_key_3.seed` fuera de la PC principal.
4. **Antes de hacer público el repositorio (opcional por ahora):**
   - Añadir `*.seed`, `*.key`, `validators.txt` a `.gitignore`, retirar `HANDOFF.md` / `handoffs/` secretos en texto plano y purgar historial si se publica en GitHub.

---

*Auditoría generada el 2026-11-15 sobre commit `450e95f`.* 🛡️
