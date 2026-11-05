# Reporte Completo de Sesión — Ledgerita
**Fecha:** 17 de Junio de 2026
**Props:** @usuario — ¡Gracias por todo!

---

## 1. Descripción del Proyecto

**Ledgerita** es un protocolo de registro de activos digitales ultra-ligero, autónomo y energéticamente sostenible. No es una blockchain tradicional — es un sistema de "Cierre de Libro" periódico (checkpointing) que permite podar el historial manteniendo el disco en tamaño predecible.

| Concepto | Detalle |
|---|---|
| Nombre del sistema | Ledgerita (Protocolo Core) |
| Nombre de la Wallet | HidroLedger (Interfaz de usuario) |
| Unidad de medida | Ledgerita (LIT) |
| Unidad mínima | Satoshi de Ledgerita ($10^{-8}$) |
| Suministro máximo | 21,000,000 LIT |
| Checkpoint interval | 4,096 transacciones |
| Lenguaje | Rust |
| Criptografía | ed25519-dalek (firmas) |
| Networking | libp2p 0.56.0 |
| Persistencia | Sled 0.34.7 |
| Encriptación wallet | Orion (Argon2i + XChaCha20-Poly1305) |
| Serialización | Manual (sin serde) |

## 2. Stack Tecnológico

| Componente | Versión |
|---|---|
| Rust (local Windows) | 1.87.0 (MSVC) |
| Rust (kettle02 Debian) | 1.96.0 |
| Rust (kettle03 Ubuntu) | 1.96.0 |
| libp2p | 0.56.0 |
| Sled | 0.34.7 |
| ed25519-dalek | 2.2.0 |
| SHA-256 | sha2 0.10.9 |
| Orion (AEAD) | 0.17.15 |
| Tokio (local) | 1.50.0 |
| Tokio (servidores) | 1.52.3 |

## 3. Estructura del Proyecto

```
Ledgerita/
├── inicio.md                                    [Visión general del protocolo]
├── REPORTE_SESION_2026-06-16.md                  [Reporte Fase 2]
├── REPORTE_SESION_2026-06-17.md                  [Este reporte]
├── sesion-2026-06-15.md                          [Bitácora día 1]
├── sesion-2026-06-16.md                          [Bitácora día 2]
├── .gitignore                                    [target/, *.log, .gemini/, local_deps/]
├── ledgerita_core/
│   ├── Cargo.toml
│   ├── Cargo.lock
│   ├── .cargo/config.toml                        [Vendor -> local_deps/]
│   ├── local_deps/                               [Gitignored, regenerable con cargo vendor]
│   ├── src/
│   │   ├── lib.rs                                [pub mod: crypto, transaction, state, network, error, serialize, storage]
│   │   ├── main.rs                               [CLI principal]
│   │   ├── crypto.rs                             [Address, wallet encriptada]
│   │   ├── transaction.rs                        [Transaction struct]
│   │   ├── state.rs                              [State, checkpoint, snapshots]
│   │   ├── storage.rs                            [Sled wrapper]
│   │   ├── serialize.rs                          [Encode/decode manual]
│   │   ├── error.rs                              [StorageError, SerializationError]
│   │   ├── network.rs                            [P2P node, RawCodec]
│   │   └── bin/
│   │       └── monitor.rs                        [Monitor de estado local]
│   └── target/
```

## 4. Módulos del Código (Explicación Detallada)

### `crypto.rs` (343 líneas)
- **`Address`**: Wrapper de `VerifyingKey` de ed25519. Implementa `PartialEq`, `Eq`, `Ord`, `Hash`.
- **Métodos**: `new()`, `from_signing_key()`, `from_hex()` (parsea hex de 64 chars), `to_hex()`, `verify_signature()`.
- **Wallet**: Encriptada con Orion. Formato `hex(salt):hex(ciphertext)`. Usa Argon2i (3 pases, 65536 KB) + XChaCha20-Poly1305.
- **`load_or_create_wallet()`**: Crea wallet nueva si no existe, o carga existente. Migración automática de legacy (hex plano) a encriptado.
- **`show_address()`**: Lee dirección sin exponer clave privada.
- **Identidad unificada**: Una semilla de 32 bytes deriva tanto `SigningKey` (wallet) como `Keypair` (P2P PeerId).
- **Tests**: 7 tests (encrypt/decrypt, wrong password, wallet creation, legacy upgrade).

### `transaction.rs` (99 líneas)
- **`Transaction`**: `{sender: Address, receiver: Address, amount: u64, fee: u64, nonce: u64, signature: Option<Signature>}`
- **`to_bytes()`**: Serializa para firma (sender + receiver + amount + fee + nonce).
- **`sign()`**: Adjunta firma.
- **`is_signature_valid()`**: Verifica firma contra sender — corazón de la "Prueba de Fórmula".
- **Tests**: 2 tests (firma válida, firma inválida tras alterar amount).

### `state.rs` (395 líneas)
- **`State`**: `{balances: BTreeMap<Address, u64>, nonces: BTreeMap<Address, u64>, fee_pool: u64, tx_count: u64, epoch: u64}`
- **`CHECKPOINT_INTERVAL`** = 4096 (constante pública).
- **`mint()`**: Crea LIT del Tesoro Génesis (sin firma, solo para administración/faucet).
- **`apply_transaction()`**: Valida firma, nonce anti-replay, fondos suficientes, aplica saldos, acumula fee.
- **`is_checkpoint_due()`**: True si `tx_count > 0 && tx_count % 4096 == 0`.
- **`finalize_checkpoint()`**: Distribuye fee_pool entre validators equitativamente, remanente al primero, incrementa epoch, genera state root.
- **`generate_state_root()`**: SHA-256 de (epoch + fee_pool + tx_count + todos los pares address/balance/nonce).
- **`encode_snapshot()`** / **`decode_snapshot()`**: Serialización completa del estado para sync P2P. Formato: `[epoch:8][fee_pool:8][tx_count:8][num_entries:4][entries...: (address:32)(balance:8)(nonce:8)]`.
- **`load()`** / **`flush()`**: Lee/escribe balances, nonces, y metadatos (fee_pool, tx_count, epoch) desde Sled.
- **Tests**: 8 tests (TX válida, fondos insuficientes, determinismo state root, replay attack, nonce antiguo, fee pool, checkpoint, snapshot roundtrip).

### `storage.rs` (180 líneas)
- **`Storage`**: Wrapper sobre Sled con 3 árboles: `balances`, `nonces`, `meta`.
- **Métodos**: `open()`, `load_balances()`, `load_nonces()`, `load_meta()` (fee_pool/tx_count/epoch), `apply_balance()`, `apply_nonce()`, `save_meta()`, `flush()`.
- **Meta keys**: `"fee_pool"`, `"tx_count"`, `"epoch"` → valores u64 big-endian.
- **Tests**: 4 tests (balance write/read, nonce write/read, state full roundtrip, flush persistence).

### `serialize.rs` (153 líneas)
- **Encode/decode manual** (sin serde, sin macros).
- **`encode_address()`** / **`decode_address()`**: 32 bytes.
- **`encode_u64()`** / **`decode_u64()`**: 8 bytes big-endian.
- **`encode_transaction()`** / **`decode_transaction()`**: 152 bytes (32+32+8+8+8+64).
- **`hash_tx()`**: SHA-256 de la TX serializada.
- **Tests**: 4 tests (address roundtrip, u64 roundtrip, TX with sig, TX no sig, hash uniqueness).

### `network.rs` (450 líneas)
- **`RawCodec`**: Implementa `request_response::Codec` con prefijo de 4 bytes big-endian de longitud.
- **`LedgeritaBehaviour`**: `ping` + `request_response` (protocol ID: `/ledgerita/1`).
- **Mensajes**:
  - `MSG_TX = 0x01`: Transacción completa (wire = encoded Transaction bytes).
  - `MSG_SYNC = 0x02`: Request = state root (32 bytes); Response = snapshot completo.
- **`run_p2p_node()`**: Event loop principal.
  - Al conectarse: envía `MSG_SYNC` automáticamente, broadcast de TXs pendientes.
  - Al recibir `MSG_TX`: valida firma, chequea mempool (deduplicación), aplica TX, reenvía a demás peers.
  - Al recibir `MSG_SYNC`: responde con snapshot. Si el snapshot local difiere del remoto, reemplaza estado.
  - Si `is_checkpoint_due()` después de aplicar TX, ejecuta `finalize_checkpoint()`.
- **Identidad unificada**: Misma seed → `SigningKey` (wallet) y `Keypair` (P2P).
- **Tests**: 1 test (sync request format).

### `main.rs` (178+ líneas)
- **Flags CLI**:
  - `--connect <multiaddr>`: Conectar a peer (repetible).
  - `--data-dir <path>`: Directorio de datos (default: `~/.ledgerita`).
  - `--show-address`: Muestra dirección de wallet y sale.
  - `--send <receiver_hex> <amount> [fee]`: Crear, firmar y emitir TX (repetible, nonces auto-incrementales).
  - `--mint <address_hex> <amount>`: Crear LIT del Tesoro (repetible, se aplica antes de --send).
- **Password**: `LEDGERITA_PASSWORD` env var (obligatoria).
- **Flujo**: Parse args → load wallet → open storage → load state → apply --mint → build --send TXs → run P2P node.
- **Fix importante en parsing**: El `--send` handler usa `i -= 1` para compensar el `i += 1` del bottom del while loop, evitando saltar el siguiente flag.

### `bin/monitor.rs` (150 líneas)
- **Monitor de estado local**: Requiere nodo detenido (Sled lock exclusivo).
- **Flags**: `--data-dir <path>`.
- **Muestra**: Supply circulante vs hard cap (21M), época, TX count, fee pool, # cuentas, top 10 por balance.
- **Alerta**: Si supply > hard cap ("Inflación detectada").
- **Formato**: LIT se muestra como decimal con 6 decimales (1 LIT = 1,000,000 satoshis).
- **Tests**: 2 tests (format_amount).

## 5. Formato de Red (Wire Protocol)

```
Request/Response:
  [4 bytes big-endian length][payload]

Payload (MSG_TX = 0x01):
  [0x01][encoded Transaction (152 bytes)]

Payload (MSG_SYNC = 0x02):
  Request:  [0x02][state root (32 bytes)]
  Response: [0x02][snapshot (variable)]
```

## 6. Servidores de Producción

| Servidor | IP | Sistema | Rust | Acceso |
|---|---|---|---|---|
| **kettle02** | 192.168.68.60 | Debian 12 (bookworm) | 1.96.0 | `ssh -i ~/.ssh/id_ed25519_ledgerita kettle02` |
| **kettle03** | 192.168.68.59 | Ubuntu | 1.96.0 | `ssh -i ~/.ssh/id_ed25519_ledgerita kettle03` |

**Directorios en servidores:**
```bash
/home/user/ledgerita_core/                              # Código fuente
/home/user/ledgerita_core/target/release/ledgerita_core # Binario nodo
/home/user/ledgerita_core/target/release/monitor         # Binario monitor
~/.ledgerita/wallet.key                                  # Wallet encriptada
~/.ledgerita/state/                                      # Base de datos Sled
```

**Alias SSH configurados** en `~/.ssh/config` (tanto local como servidores).
**Password SSH:** `<SUDO_PASSWORD>`

## 7. Cómo Desplegar Cambios

```powershell
# 1. Crear tar (excluyendo target y local_deps)
Set-Location C:\Users\user\Documents\job\Ledgerita
Remove-Item ledgerita_core.tar -ErrorAction SilentlyContinue
tar cf ledgerita_core.tar --exclude="target" --exclude="local_deps" -C "C:\Users\user\Documents\job\Ledgerita" ledgerita_core

# 2A. Subir a kettle03 (SCP directo)
scp -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" ledgerita_core.tar kettle03:~

# 2B. Subir a kettle02 (SFTP — SCP es inestable para archivos >10KB)
echo "put ledgerita_core.tar" > upload_sftp.txt
sftp -b upload_sftp.txt -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" kettle02:
Remove-Item upload_sftp.txt

# 3. Extraer y compilar
ssh -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" kettle03 "
  cd /home/user && tar xf ledgerita_core.tar &&
  rm -f /home/user/ledgerita_core/.cargo/config.toml &&
  cd /home/user/ledgerita_core &&
  source ~/.cargo/env &&
  cargo build --release
"
```

## 8. Comandos Útiles

```bash
# Mostrar dirección de wallet
LEDGERITA_PASSWORD="tu-clave" ./target/release/ledgerita_core --show-address

# Iniciar nodo semilla (escucha)
LEDGERITA_PASSWORD="tu-clave" ./target/release/ledgerita_core

# Iniciar nodo conectándose
LEDGERITA_PASSWORD="tu-clave" ./target/release/ledgerita_core \
  --connect /ip4/<IP>/tcp/<PORT>/p2p/<PEER_ID>

# Enviar TX (--mint + --send + --connect)
LEDGERITA_PASSWORD="tu-clave" ./target/release/ledgerita_core \
  --mint <mi_direccion_hex> 1000000 \
  --send <receiver_hex_64_chars> 500 5 \
  --connect /ip4/192.168.68.59/tcp/36565/p2p/12D3KooW...

# Múltiples TXs (nonces auto-incrementales)
LEDGERITA_PASSWORD="tu-clave" ./target/release/ledgerita_core \
  --send <addr_a> 100 1 \
  --send <addr_b> 200 2 \
  --connect /ip4/.../p2p/...

# Monitor (nodo detenido)
./target/release/monitor --data-dir ~/.ledgerita

# Ejecutar tests
Set-Location C:\Users\user\Documents\job\Ledgerita\ledgerita_core
cargo test --release
```

## 9. Pruebas Realizadas y Resultados

**27 tests unitarios + 2 doc-tests = 29 tests pasan en todos los entornos.**

```
crypto::tests::test_encrypt_decrypt_roundtrip ................ ok
crypto::tests::test_wrong_password_fails .................... ok
crypto::tests::test_different_seeds_different_output ....... ok
crypto::tests::test_same_seed_different_output_different_salt ok
crypto::tests::test_wallet_creation_and_loading ............ ok
crypto::tests::test_legacy_plaintext_upgrade ............... ok
crypto::tests::test_wallet_wrong_password_fails ............ ok
state::tests::test_valid_transaction_execution ............. ok
state::tests::test_insufficient_funds ...................... ok
state::tests::test_state_root_determinism .................. ok
state::tests::test_replay_attack_detected .................. ok
state::tests::test_old_nonce_rejected ...................... ok
state::tests::test_fee_goes_to_pool ........................ ok
state::tests::test_checkpoint_distributes .................. ok
state::tests::test_snapshot_roundtrip ...................... ok
transaction::tests::test_valid_transaction_signature ....... ok
transaction::tests::test_invalid_transaction_signature_altered_amount ok
serialize::tests::test_address_roundtrip ................... ok
serialize::tests::test_u64_roundtrip ....................... ok
serialize::tests::test_transaction_roundtrip_with_sig ...... ok
serialize::tests::test_transaction_roundtrip_no_sig ........ ok
serialize::tests::test_hash_uniqueness ..................... ok
storage::tests::test_balance_write_read .................... ok
storage::tests::test_nonce_write_read ...................... ok
storage::tests::test_state_full_roundtrip .................. ok
storage::tests::test_flush_persistence ..................... ok
network::tests::test_sync_request_format ................... ok
bin/monitor::tests::test_format_amount ..................... ok
bin/monitor::tests::test_format_amount_edge_cases .......... ok
```

**Prueba de conexión real (17 Jun 2026):**
```
kettle03 (seed):
  📡 Escuchando en: /ip4/192.168.68.59/tcp/36565
  PeerId: 12D3KooWCJfVpJN3E8zMCtNNwZsMkJ6eDAevwYxSgqnAHwHPnXeg

kettle02 (--connect + --send):
  🔗 Intentando conectar a: /ip4/192.168.68.59/tcp/36565/p2p/12D3KooW...
  ✅ Conexión establecida
  🔄 Ping: 7ms
  ✅ TX preparada (nonce 1)
  📤 1 TX pendiente por enviar
  ⚠️ TX local rechazada: Insufficient funds (esperado — wallet vacía)
```

## 10. Decisiones Técnicas Clave (Recordatorio)

1. **Sin serde**: Serialización manual en `serialize.rs` — control total sobre wire format, cero macros.
2. **Fee pool**: Se acumula en `State.fee_pool`, se distribuye equitativamente entre validators en `finalize_checkpoint()`. El remanente va al primer validator.
3. **Checkpoint**: Cada 4096 transacciones se vacía el fee pool y se incrementa `epoch`.
4. **P2P**: `request_response::Behaviour` con `RawCodec`. Mensajes: `MSG_TX` (0x01) y `MSG_SYNC` (0x02). Protocol ID: `/ledgerita/1`.
5. **Sync en conexión**: Al establecerse conexión, se envía `MSG_SYNC` automáticamente.
6. **Mempool**: `HashSet<[u8; 32]>` con SHA-256 de la TX para deduplicación.
7. **Identidad unificada**: Una semilla de 32 bytes deriva tanto `SigningKey` (wallet) como `Keypair` (P2P PeerId).
8. **Wallet encriptada**: Formato `hex(salt):hex(ciphertext)` con orion (Argon2i + XChaCha20-Poly1305). Password vía `LEDGERITA_PASSWORD`.
9. **Migración automática**: Wallets legacy (hex plano) se re-encriptan al primer acceso.
10. **Formato LIT**: 1 LIT = 1,000,000 satoshis (monitor muestra como decimal de 6 posiciones).
11. **Sin TestNet**: No hay separación de redes. Todo usa el mismo protocolo y génesis.

## 11. Problemas Conocidos (Operativos)

1. **SCP a kettle02**: Inestable para archivos >10KB. Usar SFTP con batch file.
2. **Conexión a kettle02**: Timeouts intermitentes. Usar `ConnectTimeout=30`.
3. **`.cargo/config.toml` con `local_deps`**: Siempre eliminar después de extraer tar en servidores: `rm -f ledgerita_core/.cargo/config.toml`.
4. **Build local en Windows**: Usa `.cargo/config.toml` con `local_deps/` regenerado vía `cargo vendor`. Si `cargo vendor` falla por rutas largas, comentar el config y compilar directo de crates.io.
5. **`tar` de Windows (BSD tar)**: Puede producir archivos corruptos (`bad header checksum`). Si pasa, recrear con `Remove-Item` + `tar cf ...`.
6. **Monitor**: Sled solo permite un writer a la vez. El monitor debe ejecutarse con el nodo **detenido**.

## 12. Cambios Implementados en la Sesión de Hoy (17 Jun 2026)

### Sesión Principal (Mañana/Tarde)
- **Flag `--send <receiver_hex> <amount> [fee]`**: CLI para crear, firmar y emitir TXs a la red.
- **Nonce auto-incremental**: Consulta último nonce desde Sled.
- **Aplicación local + broadcast P2P**: TX se aplica localmente y se envía a peers al conectarse.
- **`Address::from_hex()`**: Nuevo método para parsear direcciones hex de 64 chars.
- **Fix parsing de args**: `--send` ya funciona antes o después de `--connect` (doble incremento corregido con `i -= 1`).
- **Despliegue a producción**: Binarios compilados y subidos a kettle02 y kettle03.

### Bonus (Tarde-Noche)
- **Persistencia de metadatos**: Nuevo árbol `meta` en Sled para `fee_pool`, `tx_count`, `epoch`. `State::load()` y `State::flush()` actualizados.
- **Flag `--mint <address_hex> <amount>`**: Crea LIT del Tesoro Génesis para pruebas. Se aplica antes de `--send`.

## 13. Pendientes Priorizados para la Próxima Sesión

### 🔴 Alta Prioridad
| # | Tarea | Descripción | Archivos |
|---|---|---|---|
| 1 | **Systemd para producción** | Crear servicio systemd para que los nodos corran como daemons 24/7. Importante para el juego de carreras. | `ledgerita_core.service` (nuevo) |
| 2 | **Script de deploy** | Automatizar tar + scp/sftp + ssh build. | `deploy.ps1` (nuevo) |

### 🟡 Media Prioridad
| # | Tarea | Descripción | Archivos |
|---|---|---|---|
| 3 | **Monitor remoto P2P** | Auditoría de nodos sin detenerlos. Conecta como peer, pide snapshot via `MSG_SYNC`, verifica: (a) supply <= 21M, (b) state root recalcula y coincide, (c) fee pool consistente, (d) cross-validation entre múltiples peers. | `bin/remote_monitor.rs` (nuevo), `network.rs` |
| 4 | **Tests de integración P2P** | Test que inicia dos nodos en memoria, conecta, envía TX, verifica que el segundo recibe y aplica. | `tests/` (nuevo) |

### 🟢 Baja Prioridad
| # | Tarea | Descripción | Archivos |
|---|---|---|---|
| 5 | **TestNet formal** | Después de estabilizar producción y abrir a terceros. Requiere: chain ID / network magic en handshake P2P, génesis alternativo con faucet, scripts de deploy para testnet. | Nuevos módulos |
| 6 | **Setup script `local_deps/`** | Script que ejecuta `cargo vendor` automáticamente para builds locales. | `setup.ps1` / `setup.sh` |

## 14. Hacia un Monitor Remoto P2P con Detección de Nodos Corruptos

Como se discutió en la sesión, el monitor remoto P2P puede funcionar como **auditor de integridad** sin detener el nodo. La idea es:

### Qué detectaría
1. **Supply inflado (>21M)**: Descarga el snapshot vía `MSG_SYNC`, calcula `total_supply()`, lo compara contra el hard cap.
2. **State root inconsistente**: Recalcula `generate_state_root()` localmente a partir del snapshot. Si no coincide con el root que el nodo reportó, el nodo está mintiendo o corrupto.
3. **Fee pool mal contabilizado**: Verifica que el fee pool sea consistente con el número de transacciones y los fees esperados.
4. **Cross-validation**: Si el monitor consulta múltiples peers y N-1 devuelven un root consistente pero 1 nodo difiere, el outlier es marcado como sospechoso.

### Próximos pasos para implementarlo
- El monitor se conecta como peer P2P normal (usa la misma identidad que la wallet local o una temporal).
- Envía `MSG_SYNC` con su propio state root, recibe el snapshot del peer remoto.
- Compara y reporta discrepancias.
- Si hay varios peers, puede pedirle a cada uno el snapshot y hacer votación por state root.

---

*Fin del reporte. ¡Gracias por la sesión, ha sido increíble! Nos vemos en la próxima. 🚀*
