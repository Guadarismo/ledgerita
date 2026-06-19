# Reporte de Sesión — Ledgerita (Completo)
**Fecha:** 2026-06-18 — 14:38 UTC-3

---

## Resumen

Dos fixes principales en la sesión de hoy:

1. **Fix: Orden de broadcast P2P** — Las TXs pendientes ahora se envían **después** de completar el handshake de sync, no antes. Esto previene replay attacks entre peers recién conectados.

2. **Migración a `tracing`** — El nodo (`network.rs`) ahora usa logging estructurado con `tracing` en vez de `println!`/`eprintln!`. Filtrable por nivel via `RUST_LOG`.

**Tests: 38 passed, 0 failed** (27 unit + 2 monitor + 8 remote_monitor + 1 integración P2P)

---

## Sistema de Archivos

```
C:\Users\user\Documents\job\Ledgerita\
├── inicio.md                                    [Visión general del protocolo]
├── REPORTE_SESION_2026-06-16.md                  [Reporte Fase 2]
├── REPORTE_SESION_2026-06-17.md                  [Reporte sesión anterior]
├── REPORTE_SESION_2026-06-17-v2.md               [Reporte sesión anterior v2]
├── REPORTE_SESION_2026-06-17-v3.md               [Reporte sesión anterior v3]
├── REPORTE_SESION_2026-06-18.md                  [ESTE REPORTE]
├── sesion-2026-06-15.md                          [Bitácora día 1]
├── sesion-2026-06-16.md                          [Bitácora día 2]
├── wallet_recovery.env                           [Wallet recovery — NO SUBIR A GIT]
├── deploy.ps1                                    [Script de deploy a servidores]
├── opinion de deepseek del proyecto 15-06-2026.txt
└── ledgerita_core/
    ├── Cargo.toml
    ├── Cargo.lock
    ├── .cargo/config.toml                        [Vendor -> local_deps/]
    ├── local_deps/                               [Regenerado con: cargo vendor local_deps]
    ├── src/
    │   ├── lib.rs                                [pub mod: crypto, transaction, state, network, error, serialize, storage]
    │   ├── main.rs                               [CLI principal — subscriber tracing init]
    │   ├── crypto.rs                             [Address, wallet encriptada (Argon2i + XChaCha20-Poly1305)]
    │   ├── transaction.rs                        [Transaction {sender, receiver, amount, fee, nonce, signature}]
    │   ├── state.rs                              [State, checkpoint cada 4096 TXs, snapshots, state root SHA-256]
    │   ├── storage.rs                            [Sled wrapper: 3 trees — balances, nonces, meta]
    │   ├── serialize.rs                          [Encode/decode manual (sin serde)]
    │   ├── error.rs                              [StorageError, SerializationError]
    │   ├── network.rs                            [P2P node, RawCodec, run_p2p_node — ahora con tracing]
    │   ├── bin/
    │   │   ├── monitor.rs                        [Monitor local (nodo detenido)]
    │   │   └── remote_monitor.rs                 [Monitor remoto P2P (se conecta como peer)]
    │   └── tests/
    │       └── p2p_integration.rs                [Test integración: 2 nodos, sync + TX]
    └── target/                                   [Build artifacts — gitignored]
```

---

## Stack Tecnológico

| Componente | Versión | Propósito |
|---|---|---|
| Rust (local) | 1.87.0 (MSVC) | Compilador |
| Rust (servidores) | 1.96.0 | Compilador en kettle02/kettle03 |
| libp2p | 0.56.0 | Red P2P descentralizada |
| ed25519-dalek | 2.2.0 | Firmas digitales |
| Sled | 0.34.7 | Persistencia KV embebida |
| Orion | 0.17.15 | Encriptación wallet (Argon2i + XChaCha20-Poly1305) |
| SHA-256 | sha2 0.10.9 | Hashing |
| Tokio | 1.50.0 (local) / 1.52.3 (servidores) | Async runtime |
| **tracing** | **0.1.44** | **Logging estructurado (NUEVO)** |
| **tracing-subscriber** | **0.3.23** | **Subscriber con env-filter (NUEVO)** |
| Serialización | Manual | Sin serde, sin macros |

---

## Módulos del Código (Explicación Detallada)

### `crypto.rs` (343 líneas)
- **`Address`**: Wrapper de `VerifyingKey` de ed25519. Implementa `PartialEq`, `Eq`, `Ord`, `Hash`.
- **Métodos**: `new()`, `from_signing_key()`, `from_hex()`, `to_hex()`, `verify_signature()`.
- **Wallet**: Encriptada con Orion. Formato `hex(salt):hex(ciphertext)`. Argon2i (3 pases, 65536 KB) + XChaCha20-Poly1305.
- **`load_or_create_wallet()`**: Crea wallet nueva o carga existente. Migración automática de legacy (hex plano) a encriptado.
- **`show_address()`**: Lee dirección sin exponer clave privada.
- **Identidad unificada**: Una semilla de 32 bytes deriva tanto `SigningKey` (wallet) como `Keypair` (P2P PeerId).
- **Tests**: 7 tests.

### `transaction.rs` (99 líneas)
- **`Transaction`**: `{sender: Address, receiver: Address, amount: u64, fee: u64, nonce: u64, signature: Option<Signature>}`
- **Métodos**: `to_bytes()` (serializa para firma), `sign()`, `is_signature_valid()` — corazón de la "Prueba de Fórmula".
- **Tests**: 2 tests.

### `state.rs` (397 líneas)
- **`State`**: `{balances: BTreeMap<Address, u64>, nonces: BTreeMap<Address, u64>, fee_pool: u64, tx_count: u64, epoch: u64}`
- **`CHECKPOINT_INTERVAL`** = 4096.
- **`mint()`**: Crea LIT del Tesoro Génesis (sin firma).
- **`apply_transaction()`**: Valida firma, nonce anti-replay, fondos, aplica saldos, fee pool.
- **`is_checkpoint_due()`**: True si `tx_count > 0 && tx_count % 4096 == 0`.
- **`finalize_checkpoint()`**: Distribuye fee_pool entre validators, incrementa epoch, genera state root.
- **`generate_state_root()`**: SHA-256(epoch + fee_pool + tx_count + todos los pares address/balance/nonce).
- **`encode_snapshot()`/`decode_snapshot()`**: Serialización completa para sync P2P.
- **Tests**: 8 tests.

### `storage.rs` (202 líneas)
- **`Storage`**: Sled con 3 árboles: `balances`, `nonces`, `meta`.
- **Meta keys**: `"fee_pool"`, `"tx_count"`, `"epoch"` → u64 big-endian.
- **Tests**: 4 tests.

### `serialize.rs` (153 líneas)
- Encode/decode manual de Address (32 bytes), u64 (8 bytes), Transaction (152 bytes).
- **`hash_tx()`**: SHA-256 de TX serializada para dedup en mempool.
- **Tests**: 5 tests.

### `network.rs` (498 líneas)
- **`RawCodec`**: request_response Codec con prefijo de 4 bytes big-endian de longitud.
- **`LedgeritaBehaviour`**: ping + request_response (protocol ID: `/ledgerita/1`).
- **Mensajes**: `MSG_TX = 0x01`, `MSG_SYNC = 0x02`.
- **`run_p2p_node()`**: Event loop principal con **tracing logging** (info/warn/error).
- **Flujo de conexión corregido hoy**: ConnectionEstablished → MSG_SYNC → al completar sync → broadcast de TXs pendientes.
- **Pending broadcast**: `Arc<Mutex<Vec<Vec<u8>>>>` compartido (compartido por request/response sync handlers).
- **Tests**: 1 test.

### `main.rs` (236 líneas)
- **Flags CLI**: `--connect`, `--data-dir`, `--listen-port`, `--show-address`, `--send <receiver_hex> <amount> [fee]`, `--mint <address_hex> <amount>`.
- **Password**: `LEDGERITA_PASSWORD` env var.
- **Subscriber tracing**: Inicializado al inicio de main, output a stderr, filtrable por `RUST_LOG`.
- **Flujo**: Parse args → init subscriber → load wallet → open storage → apply mint → build TXs → run P2P node.

### `bin/monitor.rs` (~130 líneas)
- **Monitor local**: Requiere nodo detenido (Sled lock exclusivo).
- **Flags**: `--data-dir <path>`.
- **Muestra**: Supply vs hard cap (21M), época, TX count, fee pool, cuentas, top 10 balances.
- **Tests**: 2 tests.

### `bin/remote_monitor.rs` (402 líneas)
- **Monitor remoto P2P**: Se conecta como peer a uno o más nodos, pide snapshot via MSG_SYNC.
- **Flags**: `--connect <multiaddr>` (repetible), `--timeout <secs>`.
- **Verifica**: supply ≤ 21M, cross-validate state roots entre múltiples peers.
- **Tests**: 8 tests.

### `tests/p2p_integration.rs` (248 líneas)
- Test que crea 2 nodos libp2p con `TestBehaviour`, los conecta, hace sync por freshness, envía TX y verifica convergencia de estado.
- Usa `tokio::spawn` + `watch::channel` para comunicación entre tasks.
- **No usa `run_p2p_node`** — replica la lógica P2P directamente para aislar el test.

---

## Wire Protocol (P2P)

```
Request/Response:
  [4 bytes big-endian length][payload]

Payload (MSG_TX = 0x01):
  [0x01][encoded Transaction (152 bytes)]

Payload (MSG_SYNC = 0x02):
  Request:  [0x02][state root (32 bytes)]
  Response: [0x02][snapshot (variable)]
```

### Sync freshness rule
```
let local = (state.epoch, state.tx_count);
let remote = (snapshot.epoch, snapshot.tx_count);
if remote > local { adoptar estado remoto }
```

---

## Cambios Implementados Hoy (2026-06-18)

### 1. Fix orden de broadcast P2P
**Archivos:** `network.rs`, `main.rs`

| Cambio | Detalle |
|---|---|
| `pending_broadcast` | Variable local → `Arc<Mutex<Vec<Vec<u8>>>>` compartido |
| `ConnectionEstablished` | Eliminado broadcast — ahora solo envía MSG_SYNC |
| MSG_SYNC request handler | Broadcast de TXs pendientes **después** de responder con snapshot |
| MSG_SYNC response handler | Broadcast de TXs pendientes **después** de adoptar/confirmar estado |
| `main.rs` | Creación del Arc compartido + pase a `run_p2p_node` |

### 2. Migración a tracing
**Archivos:** `Cargo.toml`, `main.rs`, `network.rs`

| Cambio | Detalle |
|---|---|
| `Cargo.toml` | + `tracing = "0.1"`, + `tracing-subscriber = { features = ["env-filter"] }` |
| `main.rs:30-34` | `tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).with_writer(std::io::stderr).init()` |
| `network.rs` | ~45 `println!`/`eprintln!` → `info!`, `warn!`, `error!` con campos estructurados |
| `local_deps/` | Regenerado con `cargo vendor local_deps` |

### Niveles de log
```
RUST_LOG=error        # Solo errores (producción silenciosa)
RUST_LOG=warn         # Errores + advertencias
RUST_LOG=info         # Default recomendado
RUST_LOG=debug        # Detalle completo
RUST_LOG=network=trace  # Máximo detalle de red
```

### Se mantuvieron como println!/eprintln!
- CLI errors/help en `main.rs`, `monitor.rs`, `remote_monitor.rs`
- Output estructurado de monitores (tablas, reportes)
- Mensajes de wallet en `crypto.rs`

---

## Identidad y Wallet

### Wallet Recovery (NO SUBIR A GIT)
Archivo: `wallet_recovery.env`
```
WALLET_PASSWORD=obegtBl4U5CPqzSMJ3mOhEvDWGIHfyL6
KETTLE02_WALLET_ADDRESS=6893239f5850555f5701f24342bfbef5afdab94835f08d6e5f352899d4a17f28
KETTLE02_PEER_ID=12D3KooWGrahZ1gW5pFBu9Q1U1QEryPyEUGh8rCGGJJzuuPoHLdu
KETTLE03_WALLET_ADDRESS=b1435c81522ac1e2b0f503986ca93bde92542b5d4cab4b9a9fd21253667e4cba
KETTLE03_PEER_ID=12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1
KETTLE03_IP=192.168.68.61
```

### Servidores de Producción

| Servidor | IP | Sistema | Rust | Rol |
|---|---|---|---|---|
| **kettle03** | 192.168.68.61 | Ubuntu | 1.96.0 | Seed node (puerto 36565) |
| **kettle02** | 192.168.68.60 | Debian 12 | 1.96.0 | Cliente |

**Acceso SSH:** `ssh -i ~/.ssh/id_ed25519_ledgerita kettle03`
**Password SSH:** `clavefacil`

---

## Próximos Pasos (Priorizados)

### 🔴 Alta Prioridad
1. **Cross-validation en `remote_monitor.rs`** — Tests para `check_supply_limit` y `cross_validate_roots`. Soporte para `--json` output.
2. **Verificar `main.rs:run_p2p_node`** — Confirmar que el fix de ordenamiento (broadcast post-sync) está correctamente integrado (ya hecho hoy, verificar en próxima sesión).

### 🟡 Media Prioridad
3. **Test de stress P2P** — Múltiples TXs concurrentes entre N nodos.
4. **Test de fork/resolución** — Simular 2 nodos con estados divergentes, verificar sync por freshness.
5. **Test de reconexión** — Nodo se desconecta y reconecta, verificar resincronización.
6. **Refactor test de integración** — Extraer helpers (`build_swarm`, `wait_for_listen_addr`, `sync_state`) para reuso.

### 🟢 Baja Prioridad
7. **Documentación** — Comentarios en `RawCodec`, `State::apply_transaction`, `network::run_p2p_node`.
8. **TestNet formal** — Chain ID / network magic en handshake P2P, génesis alternativo.
9. **Setup script `local_deps/`** — Script que ejecute `cargo vendor` automáticamente.

---

## Comandos Útiles

```powershell
# Tests completos (release)
cd ledgerita_core; cargo test --release

# Tests con stdout visible
cd ledgerita_core; cargo test --release -- --nocapture

# Solo test de integración P2P
cd ledgerita_core; cargo test --test p2p_integration --release -- --nocapture

# Compilar release
cd ledgerita_core; cargo build --release

# Ejecutar nodo seed
$env:LEDGERITA_PASSWORD="obegtBl4U5CPqzSMJ3mOhEvDWGIHfyL6"
cd ledgerita_core\target\release; .\ledgerita_core.exe --listen-port 36565

# Ejecutar nodo cliente
$env:LEDGERITA_PASSWORD="obegtBl4U5CPqzSMJ3mOhEvDWGIHfyL6"
cd ledgerita_core\target\release; .\ledgerita_core.exe --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1

# Enviar TX (mint + send + connect)
$env:LEDGERITA_PASSWORD="..."
.\ledgerita_core.exe --mint <mi_direccion_hex> 1000000 --send <receiver_hex> 500 5 --connect /ip4/.../tcp/.../p2p/...

# Monitor local (nodo DETENIDO)
.\monitor.exe --data-dir ~/.ledgerita

# Monitor remoto P2P
.\remote_monitor.exe --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1

# Desplegar a servidores
cd C:\Users\user\Documents\job\Ledgerita; .\deploy.ps1

# Regenerar vendored deps (si se agregan nuevas dependencias)
cd ledgerita_core; Remove-Item local_deps -Recurse -Force; cargo vendor local_deps
```

---

## Notas Técnicas Importantes

1. **Sled lock exclusivo**: Monitor local requiere nodo **detenido**.
2. **SCP a kettle02**: Inestable para archivos >10KB. Usar SFTP.
3. **`.cargo/config.toml` en servidores**: Siempre eliminar después de extraer tar (`rm -f ledgerita_core/.cargo/config.toml`).
4. **`cargo vendor` en Windows**: Puede fallar por rutas largas. Si pasa, comentar `.cargo/config.toml` y compilar directo de crates.io.
5. **1 LIT = 1,000,000 satoshis**: El monitor y remote_monitor muestran satoshis como decimal de 6 posiciones.
6. **Hard cap**: 21,000,000 LIT = 21,000,000,000,000 satoshis.
7. **Checkpoint interval**: 4096 transacciones.
8. **Sin TestNet**: No hay separación de redes. Todo usa el mismo protocolo y génesis.

---

*¡Gracias por la sesión! Nos vemos en la próxima. 🚀*
