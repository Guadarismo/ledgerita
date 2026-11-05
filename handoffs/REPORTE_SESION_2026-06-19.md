# Reporte de Sesión — Ledgerita (Refactor Test Helpers + merge_if_fresher)
**Fecha:** 2026-06-19 — 15:14 UTC-3

---

## Resumen

1. **Extracción de helpers P2P** — `tests/helpers/mod.rs` creado con `TestBehaviour`, `build_test_swarm()`, `wait_for_listen_addr()`, `TempTestDirs`.
2. **`State::merge_if_fresher()`** — Nuevo método en `state.rs` que centraliza la lógica de sync por freshness. Elimina duplicación en `network.rs` y tests.
3. **Refactor `network.rs`** — MSG_SYNC handler simplificado de ~45 líneas inline a ~25 usando `merge_if_fresher`.
4. **Refactor `p2p_integration.rs`** — Reducido de 248→186 líneas, usando helpers y `merge_if_fresher`.

**Tests: 45 passed, 0 failed, 0 warnings.**

---

## Sistema de Archivos Completo

```
C:\Users\user\Documents\job\Ledgerita\
├── inicio.md                                   [Visión general del protocolo]
├── .gitignore                                  [target/, *.log, .gemini/, local_deps/, wallet_recovery.env]
├── deploy.ps1                                  [Script de deploy a servidores]
├── wallet_recovery.env                         [Wallet credentials — NO SUBIR A GIT]
├── opinion de deepseek del proyecto 15-06-2026.txt
├── REPORTE_SESION_2026-06-16.md                [Fase 2 — Persistencia Sled]
├── REPORTE_SESION_2026-06-17.md                [Fase 3a — Flags CLI + TX]
├── REPORTE_SESION_2026-06-17-v2.md             [Systemd/Infra]
├── REPORTE_SESION_2026-06-17-v3.md             [Remote Monitor + Sync Fix]
├── REPORTE_SESION_2026-06-18.md                [Broadcast Fix + tracing]
├── REPORTE_SESION_2026-06-18-v2.md             [Broadcast Fix (resumen)]
├── REPORTE_SESION_2026-06-19.md                [ESTE REPORTE]
├── sesion-2026-06-15.md                        [Bitácora día 1]
├── sesion-2026-06-16.md                        [Bitácora día 2]
│
└── ledgerita_core/                             [PROYECTO RUST]
    ├── Cargo.toml                              [Package manifest — ver abajo]
    ├── Cargo.lock
    ├── .cargo/config.toml                      [Vendor -> local_deps/ (local builds)]
    ├── src/
    │   ├── lib.rs                              [pub mod: crypto, transaction, state, network, error, serialize, storage]
    │   ├── main.rs                             [CLI: --connect, --send, --mint, --listen-port, --show-address]
    │   ├── crypto.rs                           [Address (ed25519), wallet encrypt/decrypt (Orion)]
    │   ├── transaction.rs                      [Transaction {sender, receiver, amount, fee, nonce, sig}]
    │   ├── state.rs                            [State, checkpoint cada 4096 TXs, snapshots, state root, merge_if_fresher]
    │   ├── storage.rs                          [Sled: 3 trees (balances, nonces, meta)]
    │   ├── serialize.rs                        [Encode/decode manual (sin serde)]
    │   ├── error.rs                            [StorageError, SerializationError]
    │   ├── network.rs                          [P2P con libp2p, RawCodec, run_p2p_node, tracing logging]
    │   ├── bin/
    │   │   ├── monitor.rs                      [Monitor local (nodo DETENIDO)]
    │   │   └── remote_monitor.rs               [Monitor remoto P2P con --json output]
    ├── tests/
    │   ├── helpers/
    │   │   └── mod.rs                          [NUEVO — Helpers P2P compartidos]
    │   └── p2p_integration.rs                  [Test integración: 2 nodos, sync + TX]
    └── target/                                 [Build artifacts — gitignored]
```

---

## Dependencias (`Cargo.toml`)

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

## Módulos del Código

### `lib.rs` — 7 líneas
```rust
pub mod crypto;
pub mod transaction;
pub mod state;
pub mod network;
pub mod error;
pub mod serialize;
pub mod storage;
```

### `crypto.rs` — Address + Wallet encriptada
- **`Address`**: Wrapper de `ed25519_dalek::VerifyingKey` (32 bytes). Implementa `PartialEq`, `Eq`, `Ord`, `Hash`, `Clone`.
- Métodos: `new()`, `from_signing_key()`, `from_hex()`, `to_hex()`, `verify_signature()`.
- **Wallet**: Encriptada con Orion (Argon2i + XChaCha20-Poly1305). Formato: `hex(salt):hex(ciphertext)`.
- `load_or_create_wallet(data_dir, password)`: Crea o carga wallet. Migración automática de hex plano.
- **Identidad unificada**: Una semilla de 32 bytes deriva `SigningKey` (wallet) y `Keypair` (P2P PeerId).
- **Tests**: 7.

### `transaction.rs` — Proof of Formula
- `Transaction { sender, receiver, amount, fee, nonce, signature }`.
- `to_bytes()`: serializa para firma. `sign()`, `is_signature_valid()`.
- **Tests**: 2.

### `state.rs` — State + SyncResult + merge_if_fresher (428 líneas)
- `State { balances: BTreeMap, nonces: BTreeMap, fee_pool, tx_count, epoch }`.
- `CHECKPOINT_INTERVAL = 4096`.
- `mint(addr, amount)`: crea LIT del Tesoro Génesis.
- `apply_transaction(tx)`: valida firma → nonce anti-replay → balance → aplica.
- `total_supply()`: suma de todos los balances.
- `merge_if_fresher(&mut self, remote, storage) -> Result<SyncResult, StorageError>`: **NUEVO**.
- `finalize_checkpoint(validators)`: distribuye fee_pool, avanza epoch, genera root.
- `generate_state_root()`: SHA-256 de estado completo.
- `encode_snapshot()` / `decode_snapshot()`: serialización para sync P2P.

**`SyncResult` enum** (nuevo):
```rust
pub enum SyncResult {
    AlreadySynced,   // roots iguales, no hay nada que hacer
    RemoteAdopted,   // estado remoto más fresco, adoptado + persistido
    LocalNewer,      // estado local más fresco, no se modifica nada
    Divergent,       // misma época/tx_count pero roots diferentes
}
```

- **Tests**: 8 (originales) + 0 nuevos (merge_if_fresher se prueba indirectamente via P2P integration test).

### `storage.rs` — Sled wrapper (202 líneas)
- 3 árboles: `balances` (clave=32 bytes address, valor=8 bytes BE), `nonces` (clave=32 bytes, valor=8 bytes BE), `meta` (clave=string, valor=8 bytes BE).
- Meta keys: `"fee_pool"`, `"tx_count"`, `"epoch"`.
- **Tests**: 4.

### `serialize.rs` — Encode/decode manual (153 líneas)
- Address (32 bytes), u64 (8 bytes BE), Transaction (152 bytes).
- `hash_tx()`: SHA-256 de TX serializada → dedup en mempool.
- **Tests**: 5.

### `network.rs` — P2P node (495 líneas)
- **`RawCodec`**: prefijo 4 bytes BE longitud + payload.
- **`LedgeritaBehaviour`**: ping + request_response (`/ledgerita/1`).
- **Mensajes**: `MSG_TX = 0x01`, `MSG_SYNC = 0x02`.
- **`run_p2p_node()`**: event loop principal con tracing.
- **Broadcast ordering fix** (Jun 18): TXs pendientes se envían solo DESPUÉS del sync.
- **Sync handler** usa `s.merge_if_fresher(&remote, &storage)`.
- **Tests**: 1 (test_sync_request_format).

### `main.rs` — CLI (240 líneas)
- Flags: `--connect`, `--data-dir`, `--listen-port`, `--show-address`, `--send <receiver_hex> <amount> [fee]`, `--mint <address_hex> <amount>`.
- Password via `LEDGERITA_PASSWORD`.
- `pending_broadcast`: `Arc<Mutex<Vec<Vec<u8>>>>` compartido con `run_p2p_node`.

### `bin/remote_monitor.rs` — Remote P2P Monitor (521 líneas)
- Se conecta como peer a nodos P2P, solicita snapshot via MSG_SYNC.
- **Flags**: `--connect` (repetible), `--timeout <secs>`, `--json`/`-j` (NUEVO).
- Funciones: `check_supply_limit(state)`, `cross_validate_roots(roots)`, `format_amount()`, `root_prefix()`, `fmt_addr()`, `extract_peer_id()`.
- **Tests**: 15 (8 originales + 7 nuevos de supply_limit y cross_validate_roots).

### `bin/monitor.rs` — Local Monitor (~130 líneas)
- Requiere nodo DETENIDO (Sled lock exclusivo).
- Flag: `--data-dir <path>`.
- **Tests**: 2.

---

## Wire Protocol (P2P)

```
Request/Response:
  [4 bytes big-endian length][payload]

MSG_TX (0x01):
  [0x01][encoded Transaction (152 bytes)]

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

## Test Helpers (`tests/helpers/mod.rs`) — NUEVO

```rust
// tests/helpers/mod.rs — 80 líneas

#[derive(NetworkBehaviour)]
pub struct TestBehaviour {
    pub rr: request_response::Behaviour<RawCodec>,
}

pub fn build_test_swarm(key) -> Swarm<TestBehaviour>
pub async fn wait_for_listen_addr(swarm) -> Multiaddr
pub struct TempTestDirs { pub a: PathBuf, pub b: PathBuf }
    // new() — crea directorios temporales
    // Drop — limpia automáticamente
```

**Cómo usar en nuevos tests:**
```rust
mod helpers;
use helpers::{build_test_swarm, wait_for_listen_addr, TempTestDirs};

let dirs = TempTestDirs::new();
let key = identity::Keypair::generate_ed25519();
let mut swarm = build_test_swarm(key);
swarm.listen_on("/ip4/127.0.0.1/tcp/0".parse().unwrap()).unwrap();
let addr = wait_for_listen_addr(&mut swarm).await;
```

El event loop match usa `helpers::TestBehaviourEvent::Rr(...)`.

---

## Cambios de la Sesión (2026-06-19)

### 1. `tests/helpers/mod.rs` — CREADO (80 líneas)
| Helper | Propósito |
|--------|-----------|
| `TestBehaviour` | Behaviour P2P mínimo para tests (solo request_response) |
| `build_test_swarm(key)` | Construye swarm con Noise + Yamux + TCP |
| `wait_for_listen_addr(swarm)` | Loop hasta obtener dirección IPv4 |
| `TempTestDirs` | Crea/limpia directorios temporales con Drop automático |

### 2. `src/state.rs` — MODIFICADO (+28 líneas)
| Cambio | Detalle |
|--------|---------|
| `SyncResult` enum (4 variantes) | Resultado tipado de merge_if_fresher |
| `merge_if_fresher(&mut self, remote, storage)` | Lógica de sync centralizada: compara roots, adopta si más fresco, persiste |

### 3. `src/network.rs` — MODIFICADO (~20 líneas menos)
| Cambio | Detalle |
|--------|---------|
| Import `SyncResult` | `use crate::state::{State, SyncResult}` |
| MSG_SYNC response handler | Reemplazadas ~45 líneas inline por `s.merge_if_fresher(&remote, &storage)` + match |

### 4. `tests/p2p_integration.rs` — MODIFICADO (248→186 líneas)
| Cambio | Detalle |
|--------|---------|
| `TestBehaviour` eliminado | Ahora importa de `helpers` |
| `build_swarm` eliminado | Ahora usa `build_test_swarm` de `helpers` |
| Lógica de sync eliminada | Ahora usa `s.merge_if_fresher(&remote, &storage)` |
| Temp dirs manuales eliminados | Ahora usa `TempTestDirs::new()` con Drop automático |

---

## Resumen de Tests

```
test result: ok. 45 passed; 0 failed

  Unit tests (lib)       27 passed
    crypto:   7  (encrypt/decrypt, wallet creation, legacy upgrade, wrong password)
    tx:       2  (valid/invalid signature)
    state:    8  (execution, insufficient funds, determinism, replay, nonce,
                  fee pool, checkpoint, snapshot roundtrip)
    serialize:5  (address, u64, TX with/without sig, hash uniqueness)
    storage:  4  (balance, nonce, roundtrip, flush)
    network:  1  (sync request format)

  monitor              2 passed  (format_amount)
  remote_monitor      15 passed  (8 original + 7 nuevos: supply_limit 3, roots 4)
  P2P integration      1 passed  (2 nodos, sync + TX, merge_if_fresher)
```

---

## Red de Producción

| Nodo | IP | OS | Rol | Puerto | Wallet (hex prefix) | PeerId |
|------|----|----|-----|--------|---------------------|--------|
| kettle03 | 192.168.68.61 | Ubuntu | Seed | 36565 | `b1435c81522a...` | `12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1` |
| kettle02 | 192.168.68.60 | Debian 12 | Cliente | dinámico | `6893239f5850...` | `12D3KooWGrahZ1gW5pFBu9Q1U1QEryPyEUGh8rCGGJJzuuPoHLdu` |

**Wallet password (producción):** `<VER_SECRETS_LOCAL.md>` (en `wallet_recovery.env`)
**SSH:** `ssh -i ~/.ssh/id_ed25519_ledgerita kettle03`, password: `<SUDO_PASSWORD>`

Los nodos corren como daemons systemd con auto-restart (servicio: `ledgerita-core`).

---

## Comandos Útiles

```powershell
# Tests completos
cd ledgerita_core; cargo test

# Tests con stdout visible
cd ledgerita_core; cargo test -- --nocapture

# Solo test de integración P2P
cd ledgerita_core; cargo test --test p2p_integration -- --nocapture

# Compilar release
cd ledgerita_core; cargo build --release

# Ejecutar nodo seed
$env:LEDGERITA_PASSWORD="<VER_SECRETS_LOCAL.md>"
.\ledgerita_core.exe --listen-port 36565

# Ejecutar nodo cliente
.\ledgerita_core.exe --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1

# Remote monitor (texto)
.\remote_monitor.exe --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1

# Remote monitor (JSON)
.\remote_monitor.exe --json --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1

# Monitor local (nodo DETENIDO)
.\monitor.exe --data-dir ~/.ledgerita

# Desplegar a servidores
cd C:\Users\user\Documents\job\Ledgerita; .\deploy.ps1

# Niveles de log
$env:RUST_LOG="info"
$env:RUST_LOG="debug"
$env:RUST_LOG="network=trace"
```

---

## Pendientes para Próxima Sesión

### 🟡 Media Prioridad
1. **Test de stress P2P** — Múltiples TXs concurrentes entre N nodos. YA hay helpers (`build_test_swarm`, `wait_for_listen_addr`, `merge_if_fresher`). Crear `tests/p2p_stress.rs`.
2. **Test de fork/resolución** — Simular 2 nodos con estados divergentes, verificar sync por freshness. Crear `tests/p2p_fork.rs`.
3. **Test de reconexión** — Nodo se desconecta y reconecta, verificar resincronización. Crear `tests/p2p_reconnect.rs`.
4. **Refactor extra** — `wait_for_listen_addr` podría generalizarse para soportar IPv6 y/dev/multiaddr. `TempTestDirs` podría aceptar labels.
5. **`State::flush()` no limpia claves viejas** — Si el mapa en memoria es más pequeño que el estado persistido, quedan stale keys en Sled. Fix en `storage.rs`.

### 🟢 Baja Prioridad
6. **Reducir ruido de logs** — Mensaje "Respuesta enviada" (`ResponseSent` event en `network.rs:481`) muy verboso. Cambiar a `debug!` o `trace!`.
7. **Documentación de código** — Comentarios/docstrings en `RawCodec`, `State::apply_transaction`, `network::run_p2p_node`.
8. **TestNet formal** — Chain ID / network magic en handshake P2P, génesis alternativo con faucet.
9. **Script `local_deps/`** — Script que ejecute `cargo vendor local_deps` automáticamente.
10. **Límite de logs systemd** — Configurar `SystemMaxUse=50M` en journald.

---

## Notas Técnicas Importantes

1. **Sled lock exclusivo**: Monitor local (`monitor.exe`) requiere nodo DETENIDO.
2. **SCP a kettle02**: Inestable para archivos >10KB. Usar SFTP con batch file.
3. **`.cargo/config.toml` en servidores**: Siempre eliminar después de extraer tar (`rm -f ledgerita_core/.cargo/config.toml`).
4. **1 LIT = 1,000,000 satoshis**: `format_amount()` divide entre 1,000,000 para display.
5. **Hard cap**: 21,000,000 LIT = 21,000,000,000,000 satoshis.
6. **Checkpoint interval**: 4096 transacciones.
7. **Sin serde**: Serialización manual en `serialize.rs`.
8. **Sin TestNet**: No hay separación de redes. Todo usa el mismo protocolo y génesis.
9. **El event loop de test usa `helpers::TestBehaviourEvent::Rr(...)`** para matchear eventos P2P.
10. **Para crear un nuevo test P2P**: copiar estructura de `tests/p2p_integration.rs`, usar `mod helpers;` e importar `build_test_swarm`, `wait_for_listen_addr`, `TempTestDirs`, y `State::merge_if_fresher`.

---

*¡Gracias por la sesión! Nos vemos en la próxima. 🚀*
