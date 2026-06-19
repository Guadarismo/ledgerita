# Ledgerita — Reporte Completo de Contexto del Proyecto
**Generado:** 2026-06-19 12:02 (hora local)
**Último commit:** `12eb2f1` — Wallet persistente + conexion activa P2P + dialing + CLI flags
**Tests:** 48 passed, 0 failed

---

## 📋 ¿Qué es Ledgerita?

Protocolo contable ultra-ligero, descentralizado y autónomo en Rust. No usa blockchain tradicional — emplea "Cierre de Libro" (checkpoints periódicos) y "Prueba de Fórmula" (validación determinista, sin minería).

- Unidad: **Ledgerita (LIT)**
- Unidad mínima: **Satoshi de Ledgerita** (1 LIT = 1,000,000 satoshis)
- Hard cap: **21,000,000 LIT** (= 21,000,000,000,000 satoshis)
- Checkpoint interval: **4096** transacciones
- Sin TestNet, sin serde, sin minería

---

## 🏗️ Arquitectura del Código

```
Ledgerita/
├── inicio.md                         # Visión general del protocolo
├── deploy.ps1                        # Script de deploy a servidores
├── wallet_recovery.env               # Credenciales de wallet (NO SUBIR A GIT)
├── REPORTE_FINAL_CONTEXTO.md         # ESTE ARCHIVO
├── REPORTE_SESION_*.md               # Reportes de sesiones anteriores
│
└── ledgerita_core/                   # PROYECTO RUST
    ├── Cargo.toml                    # Dependencias (ver sección abajo)
    ├── src/
    │   ├── lib.rs                    # pub mod: 7 módulos
    │   ├── main.rs                   # CLI (240 líneas)
    │   ├── crypto.rs                 # Address + Wallet encriptada (343 líneas)
    │   ├── transaction.rs            # TX con firma ed25519 (99 líneas)
    │   ├── state.rs                  # Estado + Checkpoints + Sync (430 líneas)
    │   ├── network.rs                # P2P libp2p event-loop (495 líneas)
    │   ├── storage.rs                # Sled wrapper (290 líneas)
    │   ├── serialize.rs              # Encode/decode manual (153 líneas)
    │   ├── error.rs                  # Errores tipados (thiserror)
    │   └── bin/
    │       ├── monitor.rs            # Monitor local (nodo DETENIDO)
    │       └── remote_monitor.rs     # Monitor remoto P2P con --json
    └── tests/
        ├── helpers/
        │   └── mod.rs                # Helpers P2P compartidos
        ├── p2p_integration.rs        # Test integración 2 nodos
        └── p2p_stress.rs             # Test stress 3 nodos, 10 TXs
```

---

## 📦 Dependencias (Cargo.toml)

| Dependencia | Versión | Propósito |
|---|---|---|
| ed25519-dalek | 2.2.0 | Firmas digitales ED25519 |
| libp2p | 0.56.0 | Red P2P (TCP, Noise, Yamux, Ping, request-response) |
| sha2 | 0.10.9 | SHA-256 para state root y hash_tx |
| getrandom | 0.2 | Generación de números aleatorios |
| tokio | 1.50.0 | Async runtime (multi-thread, macros, time, sync) |
| orion | 0.17 | Encriptación wallet (Argon2i + XChaCha20-Poly1305) |
| sled | 0.34 | Persistencia KV embebida |
| thiserror | 1 | Errores tipados |
| async-trait | 0.1 | Traits asíncronos para RawCodec |
| futures | 0.3 | StreamExt, AsyncRead, AsyncWrite |
| tracing | 0.1 | Logging estructurado |
| tracing-subscriber | 0.3 | Subscriber con env-filter |

---

## 📁 Módulos en Detalle

### `lib.rs` — Punto de entrada
```rust
pub mod crypto;
pub mod transaction;
pub mod state;
pub mod network;
pub mod error;
pub mod serialize;
pub mod storage;
```

### `crypto.rs` — Address + Wallet
- **`Address`**: Wrapper de `ed25519_dalek::VerifyingKey` (32 bytes). Implementa `PartialEq`, `Eq`, `Ord`, `Hash`.
  - `from_hex()`, `to_hex()`, `verify_signature()`
- **Wallet**: Encriptada con Orion. Formato: `hex(salt):hex(ciphertext)`.
- `load_or_create_wallet()`: Crea o carga wallet. Migración automática de hex plano a encriptado.
- **Identidad unificada**: Una semilla de 32 bytes deriva `SigningKey` (wallet) + `Keypair` (P2P PeerId).
- **Password**: vía variable de entorno `LEDGERITA_PASSWORD`.
- Tests: 7

### `transaction.rs` — Prueba de Fórmula
```rust
Transaction { sender, receiver, amount, fee, nonce, signature }
```
- `to_bytes()`: serializa para firma
- `sign()`, `is_signature_valid()`: verificación ed25519
- Tests: 2

### `state.rs` — Estado Global + Checkpoints
```rust
State { balances: BTreeMap<Address,u64>, nonces: BTreeMap<Address,u64>, fee_pool, tx_count, epoch }
```
- `CHECKPOINT_INTERVAL = 4096`
- `mint(addr, amount)`: crear LIT del Tesoro Génesis
- `apply_transaction(tx)`: valida firma → nonce anti-replay → balance → aplica
- `total_supply()`: suma de balances
- `merge_if_fresher(remote, storage) -> Result<SyncResult, StorageError>`: sync por freshness
  - `SyncResult::AlreadySynced` | `RemoteAdopted` | `LocalNewer` | `Divergent`
- `finalize_checkpoint(validators)`: distribuye fee_pool, avanza epoch, genera root
- `generate_state_root()`: SHA-256 del estado completo
- `encode_snapshot()` / `decode_snapshot()`: serialización para sync P2P
- Tests: 8

### `network.rs` — Nodo P2P
- **`RawCodec`**: prefijo 4 bytes BE longitud + payload
- **`LedgeritaBehaviour`**: ping + request_response (`/ledgerita/1`)
- **Mensajes**: `MSG_TX = 0x01`, `MSG_SYNC = 0x02`
- `run_p2p_node()`: event loop principal con tracing
- **Broadcast**: TXs se re-envían a todos los peers conectados (excepto el emisor)
- Tests: 1

### `storage.rs` — Persistencia Sled
- 3 árboles: `balances`, `nonces`, `meta`
- Meta keys: `"fee_pool"`, `"tx_count"`, `"epoch"`
- **IMPORTANTE**: `prune_balances()` y `prune_nonces()` eliminan claves stale (agregado 2026-06-19)
- Tests: 6

### `serialize.rs` — Encode/Decode manual (sin serde)
- Address (32 bytes), u64 (8 bytes BE), Transaction (152 bytes con sig)
- `hash_tx()`: SHA-256 de TX serializada → dedup en mempool
- Tests: 5

### `error.rs` — Errores tipados
```rust
StorageError { Db, Serialization, Io }
SerializationError { InvalidData }
```

### `main.rs` — CLI
- `--connect <multiaddr>`: conectar a peers
- `--data-dir <path>`: directorio de datos (default: ~/.ledgerita)
- `--listen-port <port>`: puerto de escucha
- `--show-address`: muestra dirección de wallet
- `--send <receiver_hex> <amount> [fee]`: preparar TX
- `--mint <address_hex> <amount>`: crear LIT desde Tesoro
- Password: `$env:LEDGERITA_PASSWORD="..."`

### `bin/monitor.rs` — Monitor local
- Requiere nodo **DETENIDO** (Sled lock exclusivo)
- Muestra supply, top 10 cuentas, epoch
- Tests: 2

### `bin/remote_monitor.rs` — Monitor remoto P2P
- Se conecta como peer a nodos P2P, solicita snapshot via MSG_SYNC
- Flags: `--connect`, `--timeout`, `--json`/`-j`
- Validación: `check_supply_limit()`, `cross_validate_roots()`
- Tests: 15

---

## 📡 Wire Protocol (P2P)

```
Request/Response:
  [4 bytes big-endian length][payload]

MSG_TX (0x01):
  Request:  [0x01][encoded Transaction (152 bytes)]
  Response: "OK" | "INVALID" | "INVALID_SIG" | "DUPLICATE" | "REJECT:..."

MSG_SYNC (0x02):
  Request:  [0x02][state root (32 bytes)]
  Response: [0x02][snapshot (variable)]

Snapshot format:
  [epoch:8][fee_pool:8][tx_count:8][num_entries:4]
  [entries...: (address:32)(balance:8)(nonce:8)]

Sync freshness rule:
  if (remote.epoch, remote.tx_count) > (local.epoch, local.tx_count)
    → adopt remote state
```

---

## 🌐 Red de Producción

| Nodo | IP | OS | Rol | Puerto | Wallet (hex prefix) | PeerId |
|---|---|---|---|---|---|---|
| kettle03 | 192.168.68.61 | Ubuntu | Seed | 36565 | `b1435c81522a...` | `12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1` |
| kettle02 | 192.168.68.60 | Debian 12 | Cliente | dinámico | `6893239f5850...` | `12D3KooWGrahZ1gW5pFBu9Q1U1QEryPyEUGh8rCGGJJzuuPoHLdu` |

**Wallet password (producción):** `obegtBl4U5CPqzSMJ3mOhEvDWGIHfyL6` (en `wallet_recovery.env`)
**SSH:** `ssh -i ~/.ssh/id_ed25519_ledgerita kettle03`, password: `clavefacil`

Los nodos corren como daemons systemd con auto-restart (servicio: `ledgerita-core`).

---

## 🧪 Tests (48 passed)

```
Unit tests (lib)         31 passed
  crypto:     7  (encrypt/decrypt, wallet creation, legacy upgrade, wrong password)
  tx:         2  (valid/invalid signature)
  state:      8  (execution, insufficient funds, determinism, replay, nonce,
                  fee pool, checkpoint, snapshot roundtrip)
  serialize:  5  (address, u64, TX with/without sig, hash uniqueness)
  storage:    6  (balance, nonce, roundtrip, flush, prune_balances, prune_nonces)
  network:    1  (sync request format)

monitor                 2 passed  (format_amount)
remote_monitor         15 passed  (8 original + 7: supply_limit 3, roots 4)
P2P integration         1 passed  (2 nodos, sync + TX, merge_if_fresher)
P2P stress              1 passed  (3 nodos, 10 TXs concurrentes)  ← NUEVO
```

---

## 🛠️ Cambios de la Última Sesión (2026-06-19)

### 1. Fix stale keys en Sled
**Archivos:** `src/storage.rs`, `src/state.rs`
- `storage.rs`: métodos `prune_balances()` y `prune_nonces()` que iteran el árbol Sled y eliminan claves no presentes en el mapa activo
- `state.rs`: `flush()` ahora llama a `prune_*` **antes** de hacer inserts
- **Orden importante**: prune → insert, para que el árbol refleje exactamente el mapa en memoria
- Tests: `test_prune_removes_stale_balances`, `test_prune_removes_stale_nonces`

### 2. Stress test P2P
**Archivo:** `tests/p2p_stress.rs` (250 líneas)
- **helpers**: `TempTestDirs` extendido con campo `c` para 3 nodos
- 3 nodos con estado génesis idéntico (A=10000, B=10000, C=10000)
- 10 TXs concurrentes: A→B (5), B→C (3), C→A (2)
- Topología: seed A, B dialea A, C dialea A y B
- **Bug corregido**: El contador `AtomicU64` no consideraba TXs adoptadas vía `merge_if_fresher`. Fix: usar `state.tx_count` directamente
- Ejecución: 1.54s

---

## 📋 Pendientes para Próxima Sesión

### 🟡 Media Prioridad
1. **Test de fork/resolución** — `tests/p2p_fork.rs`: Simular 2 nodos con estados divergentes, verificar sync por freshness. Usar helpers existentes.
2. **Test de reconexión** — `tests/p2p_reconnect.rs`: Nodo se desconecta y reconecta, verificar resincronización.
3. **`wait_for_listen_addr` generalizar** — Soporte para IPv6 y multiaddr múltiples.
4. **`TempTestDirs` con labels** — Poder crear N directorios con nombres personalizados.

### 🟢 Baja Prioridad
5. **Reducir ruido de logs** — `ResponseSent` event en `network.rs:481` cambiar a `debug!`/`trace!`
6. **Documentación de código** — Docstrings en `RawCodec`, `State::apply_transaction`, `network::run_p2p_node`
7. **TestNet formal** — Chain ID / network magic en handshake P2P, génesis alternativo con faucet
8. **Script `local_deps/`** — Script que ejecute `cargo vendor local_deps` automáticamente
9. **Límite de logs systemd** — Configurar `SystemMaxUse=50M` en journald

---

## 💻 Comandos Útiles

```powershell
# Tests completos
cd ledgerita_core; cargo test

# Tests con stdout visible
cd ledgerita_core; cargo test -- --nocapture

# Solo test de integración P2P
cd ledgerita_core; cargo test --test p2p_integration -- --nocapture

# Solo stress test
cd ledgerita_core; cargo test --test p2p_stress -- --nocapture

# Compilar release
cd ledgerita_core; cargo build --release

# Ejecutar nodo seed
$env:LEDGERITA_PASSWORD="obegtBl4U5CPqzSMJ3mOhEvDWGIHfyL6"
.\ledgerita_core\target\release\ledgerita_core.exe --listen-port 36565

# Ejecutar nodo cliente
.\ledgerita_core\target\release\ledgerita_core.exe --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1

# Remote monitor (texto)
.\ledgerita_core\target\release\remote_monitor.exe --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1

# Remote monitor (JSON)
.\remote_monitor.exe --json --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1

# Monitor local (nodo DETENIDO)
.\ledgerita_core\target\release\monitor.exe --data-dir ~/.ledgerita

# Desplegar a servidores
.\deploy.ps1

# Niveles de log
$env:RUST_LOG="info"
$env:RUST_LOG="debug"
$env:RUST_LOG="network=trace"
```

---

## ⚠️ Notas Técnicas Importantes

1. **Sled lock exclusivo**: Monitor local requiere nodo DETENIDO
2. **SCP a kettle02**: Inestable para archivos >10KB. Usar SFTP con batch file
3. **`.cargo/config.toml` en servidores**: Siempre eliminar después de extraer tar (`rm -f ledgerita_core/.cargo/config.toml`)
4. **1 LIT = 1,000,000 satoshis**: `format_amount()` divide entre 1,000,000 para display
5. **Hard cap**: 21,000,000 LIT = 21,000,000,000,000 satoshis
6. **Checkpoint interval**: 4096 transacciones
7. **Sin serde**: Serialización manual en `serialize.rs`
8. **Sin TestNet**: No hay separación de redes. Todo usa el mismo protocolo y génesis
9. **Para crear un nuevo test P2P**: copiar estructura de `tests/p2p_stress.rs`, usar `mod helpers;` e importar `build_test_swarm`, `wait_for_listen_addr`, `TempTestDirs`, y `State::merge_if_fresher`
10. **Wallet recovery**: `wallet_recovery.env` contiene password y direcciones — NO SUBIR A GIT
11. **El event loop de test usa `helpers::TestBehaviourEvent::Rr(...)`** para matchear eventos P2P

---

*Generado el 2026-06-19. ¡Buena suerte en la próxima sesión! 🚀*
