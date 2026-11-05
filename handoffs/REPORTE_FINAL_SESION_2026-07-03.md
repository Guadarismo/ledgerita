# Ledgerita — Reporte Final Completo
**Fecha:** 2026-07-03 07:03 UTC-3
**Último commit:** `694ff66` → `(este commit)`
**Tests:** 50 passed, 0 failed
**Red:** 2 nodos productivos sincronizados (kettle03 seed, kettle02 cliente)

---

## 📋 ¿Qué es Ledgerita?

Protocolo contable ultra-ligero, descentralizado y autónomo en Rust. No usa blockchain tradicional — emplea "Cierre de Libro" (checkpoints cada 4096 TXs) y "Prueba de Fórmula" (validación determinista, sin minería).

- **Unidad:** Ledgerita (LIT) — 1 LIT = 1,000,000 satoshis
- **Hard cap:** 21,000,000 LIT
- **Checkpoint interval:** 4096 transacciones
- **Sin serde, sin minería, sin TestNet**

---

## 🏗️ Arquitectura del Código

```
Ledgerita/
├── inicio.md                           # Visión general del protocolo
├── deploy.ps1                          # Script de deploy a servidores
├── wallet_recovery.env                 # Credenciales de wallet (NO SUBIR A GIT)
├── REPORTE_FINAL_CONTEXTO.md           # Contexto completo del proyecto
├── REPORTE_SESION_2026-07-03.md        # Sesión previa: reconnect test + test manual producción
├── REPORTE_SESION_2026-07-03-v2.md     # Esta sesión: fix mint + retry P2P
├── REPORTE_FINAL_SESION_2026-07-03.md  # ESTE ARCHIVO — reporte final consolidado
│
└── ledgerita_core/                     # PROYECTO RUST
    ├── Cargo.toml                      # 11 dependencias (ed25519-dalek, libp2p, sled, orion...)
    ├── Cargo.lock
    ├── .cargo/config.toml              # Vendor -> local_deps/
    ├── local_deps/                     # Vendored crates (cargo vendor)
    ├── src/
    │   ├── lib.rs                      # pub mod: 7 módulos
    │   ├── main.rs                     # CLI (240 líneas)
    │   ├── crypto.rs                   # Address + Wallet encriptada (343 líneas)
    │   ├── transaction.rs              # TX con firma ed25519 (99 líneas)
    │   ├── state.rs                    # Estado + Checkpoints + SyncResult (430 líneas)
    │   ├── network.rs                  # P2P libp2p event-loop (523 líneas) ← MODIFICADO
    │   ├── storage.rs                  # Sled wrapper (290 líneas)
    │   ├── serialize.rs                # Encode/decode manual (153 líneas)
    │   ├── error.rs                    # Errores tipados (thiserror)
    │   └── bin/
    │       ├── monitor.rs              # Monitor local (nodo DETENIDO)
    │       └── remote_monitor.rs       # Monitor remoto P2P con --json
    └── tests/
        ├── helpers/
        │   └── mod.rs                  # TestBehaviour, build_test_swarm, wait_for_listen_addr, TempTestDirs
        ├── p2p_integration.rs          # 2 nodos, sync + TX (186 líneas)
        ├── p2p_stress.rs               # 3 nodos, 10 TXs concurrentes (346 líneas)
        ├── p2p_fork.rs                 # 2 nodos divergentes + resolución (290 líneas)
        └── p2p_reconnect.rs            # 2 nodos, reconexión con misma identidad (302 líneas)
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

### `lib.rs`
```rust
pub mod crypto;
pub mod transaction;
pub mod state;
pub mod network;
pub mod error;
pub mod serialize;
pub mod storage;
```

### `crypto.rs` — Address + Wallet (343 líneas)
- **Address**: Wrapper de `ed25519_dalek::VerifyingKey` (32 bytes). Implementa `PartialEq`, `Eq`, `Ord`, `Hash`.
  - `from_hex()`, `to_hex()`, `verify_signature()`
- **Wallet**: Encriptada con Orion (Argon2i + XChaCha20-Poly1305). Formato: `hex(salt):hex(ciphertext)`.
- `load_or_create_wallet()`: Crea o carga wallet. Migración automática de hex plano a encriptado.
- **Identidad unificada**: Una semilla de 32 bytes deriva `SigningKey` (wallet) + `Keypair` (P2P PeerId).
- **Password**: vía variable de entorno `LEDGERITA_PASSWORD`.

### `transaction.rs` — Prueba de Fórmula (99 líneas)
```rust
Transaction { sender, receiver, amount, fee, nonce, signature }
```
- `to_bytes()`: serializa para firma
- `sign()`, `is_signature_valid()`: verificación ed25519

### `state.rs` — Estado Global + Checkpoints (430 líneas)
```rust
State { balances: BTreeMap<Address,u64>, nonces: BTreeMap<Address,u64>, fee_pool, tx_count, epoch }
```
- `CHECKPOINT_INTERVAL = 4096`
- `mint(addr, amount)`: crear LIT del Tesoro Génesis
- `apply_transaction(tx)`: valida firma → nonce anti-replay → balance → aplica
- `total_supply()`: suma de balances. **Importante**: no incluye fee_pool. Supply real = `total_supply() + fee_pool`
- `merge_if_fresher(remote, storage) -> Result<SyncResult, StorageError>`: sync por freshness
  - `SyncResult::AlreadySynced` | `RemoteAdopted` | `LocalNewer` | `Divergent`
- `finalize_checkpoint(validators)`: distribuye fee_pool, avanza epoch, genera root
- `generate_state_root()`: SHA-256 del estado completo
- `encode_snapshot()` / `decode_snapshot()`: serialización para sync P2P

### `network.rs` — Nodo P2P (523 líneas) ⬅️ MODIFICADO
- **`RawCodec`**: prefijo 4 bytes BE longitud + payload
- **`LedgeritaBehaviour`**: ping + request_response (`/ledgerita/1`)
- **Mensajes**: `MSG_TX = 0x01`, `MSG_SYNC = 0x02`
- **`run_p2p_node()`**: event loop principal con tracing
- **`extract_peer_id()`** (nueva): extrae PeerId del multiaddr
- **Retry automático**: peers pendientes se reintentan cada 30s via `tokio::select!`
- **Broadcast**: TXs se re-envían a todos los peers conectados (excepto el emisor)

### `storage.rs` — Persistencia Sled (290 líneas)
- 3 árboles: `balances`, `nonces`, `meta`
- Meta keys: `"fee_pool"`, `"tx_count"`, `"epoch"`
- `prune_balances()` y `prune_nonces()` eliminan claves stale antes de insertar

### `serialize.rs` — Encode/Decode manual (153 líneas, sin serde)
- Address (32 bytes), u64 (8 bytes BE), Transaction (152 bytes con sig)
- `hash_tx()`: SHA-256 de TX serializada → dedup en mempool

### `error.rs` — Errores tipados (17 líneas)
```rust
StorageError { Db, Serialization, Io }
SerializationError { InvalidData }
```

### `main.rs` — CLI (240 líneas)
- `--connect <multiaddr>`: conectar a peers (formato completo: `/ip4/.../tcp/.../p2p/<PeerId>`)
- `--data-dir <path>`: directorio de datos (default: ~/.ledgerita)
- `--listen-port <port>`: puerto de escucha
- `--show-address`: muestra dirección de wallet
- `--send <receiver_hex> <amount> [fee]`: preparar TX (nonce auto-incrementado por cada flag --send)
- `--mint <address_hex> <amount>`: crear LIT desde Tesoro. **Ahora incrementa tx_count**
- Password: `$env:LEDGERITA_PASSWORD="..."`
- **Orden**: `--mint` se aplica ANTES de `--send` y de conectar

### `bin/monitor.rs` — Monitor local
- Requiere nodo **DETENIDO** (Sled lock exclusivo)
- Muestra supply, top 10 cuentas, epoch

### `bin/remote_monitor.rs` — Monitor remoto P2P
- Se conecta como peer a nodos P2P, solicita snapshot via MSG_SYNC
- Flags: `--connect` (obligatorio, repetible, debe incluir `/p2p/<PeerId>`), `--timeout`, `--json`/`-j`
- Validación: `check_supply_limit()`, `cross_validate_roots()`

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

| Nodo | IP | OS | Rol | Puerto | PeerId |
|---|---|---|---|---|---|
| kettle03 | 192.168.68.61 | Ubuntu | Seed | 36565 | `12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1` |
| kettle02 | 192.168.68.64 | Debian 12 | Cliente | dinámico | `12D3KooWGrahZ1gW5pFBu9Q1U1QEryPyEUGh8rCGGJJzuuPoHLdu` |

**Estado actual:** Ambos nodos sincronizados (AlreadySynced)
- Supply: 10.000095 LIT
- State Root: `c0198d1fcf725b28...`
- tx_count: 1, epoch: 0, 2 cuentas

**Wallet password:** `<VER_SECRETS_LOCAL.md>` (en `wallet_recovery.env`)
**SSH key:** `~/.ssh/id_ed25519_ledgerita`
**SSH password sudo:** `<SUDO_PASSWORD>`
**SSH config:**
```
Host kettle02
    HostName 192.168.68.64
    User user
    StrictHostKeyChecking accept-new

Host kettle03
    HostName 192.168.68.61
    User user
```

---

## 🧪 Tests (50 passed)

```
Unit tests (lib)         29 passed
  crypto:     7  (encrypt/decrypt, wallet creation, legacy upgrade, wrong password)
  tx:         2  (valid/invalid signature)
  state:      8  (execution, funds, determinism, replay, nonce, fee, checkpoint, snapshot)
  serialize:  5  (address, u64, TX with/without sig, hash uniqueness)
  storage:    6  (balance, nonce, roundtrip, flush, prune_balances, prune_nonces)
  network:    1  (sync request format)

monitor                 2 passed  (format_amount)
remote_monitor         15 passed  (supply_limit 3, roots 4, fmt 3, peers, etc.)
P2P integration         1 passed  (2 nodos, sync + TX)
P2P stress              1 passed  (3 nodos, 10 TXs concurrentes)
P2P fork                 1 passed  (fork detection + resolución)
P2P reconnect            1 passed  (reconexión misma identidad)
───────────────────────────────────────
TOTAL                   50 passed
```

---

## 🛠️ Cambios de Esta Sesión (2026-07-03)

### 1. Fix: `--mint` ahora incrementa `tx_count`
**Archivo:** `src/main.rs:173`
- Se agregó `s.tx_count += 1` después de cada `s.mint()`
- Evita divergencia irresoluble cuando solo un nodo recibe un mint
- `state.rs::mint()` NO se modificó para no romper tests de setup génesis

### 2. Fix: Retry de dial P2P
**Archivo:** `src/network.rs`
- Nueva función `extract_peer_id()` para extraer PeerId del multiaddr
- `pending_peers: HashMap<PeerId, Multiaddr>` trackea peers no conectados
- `tokio::time::interval` cada 30s reintenta dialear peers pendientes
- Event loop envuelto en `tokio::select!` para timer + event handler simultáneos
- `ConnectionEstablished` remueve el peer de la lista de reintentos

---

## 🟡 Pendientes para Próxima Sesión

### Media Prioridad
1. **Generalizar `wait_for_listen_addr`** — Soporte para IPv6 y multiaddr múltiples en `tests/helpers/mod.rs`
2. **`TempTestDirs` con labels** — Poder crear N directorios con nombres personalizados

### Baja Prioridad
3. **Reducir ruido de logs** — `ResponseSent` event en `network.rs` cambiar a `debug!`/`trace!`
4. **Documentación de código** — Docstrings en `RawCodec`, `State::apply_transaction`, `network::run_p2p_node`
5. **TestNet formal** — Chain ID / network magic en handshake P2P, génesis alternativo con faucet
6. **Script `local_deps/`** — Script que ejecute `cargo vendor local_deps` automáticamente
7. **Límite de logs systemd** — Configurar `SystemMaxUse=50M` en journald
8. **Re-deployar binario actualizado en producción** — Compilar release actual y subir a kettle03/kettle02
9. **Corregir IP de kettle02 en deploy.ps1** — Cambiar de `192.168.68.60` a `192.168.68.64`

---

## ⚠️ Notas Técnicas Importantes

1. **Sled lock exclusivo**: Monitor local requiere nodo DETENIDO
2. **SCP a kettle02**: Inestable para archivos >10KB. Usar SFTP con batch file (deploy.ps1 ya lo maneja)
3. **`.cargo/config.toml` en servidores**: Siempre eliminar después de extraer tar (`rm -f ledgerita_core/.cargo/config.toml`)
4. **1 LIT = 1,000,000 satoshis**: `format_amount()` divide entre 1,000,000 para display
5. **Hard cap**: 21,000,000 LIT = 21,000,000,000,000 satoshis
6. **Checkpoint interval**: 4096 transacciones
7. **Sin serde**: Serialización manual en `serialize.rs`
8. **Sin TestNet**: No hay separación de redes. Todo usa el mismo protocolo y génesis
9. **Para crear un nuevo test P2P**: copiar estructura de `tests/p2p_fork.rs` o `tests/p2p_reconnect.rs`, usar `mod helpers;` e importar `build_test_swarm`, `wait_for_listen_addr`, `TempTestDirs`, y `State::merge_if_fresher`
10. **Orden de broadcast P2P**: TXs pendientes se envían DESPUÉS del sync
11. **Prune antes que insert**: `State::flush()` llama a `prune_balances()` y `prune_nonces()` antes de insertar
12. **Misma identidad P2P para reconexión**: `identity::Keypair` implementa Clone
13. **Equivalencia supply**: `total_supply()` solo suma balances. Supply real = `total_supply() + fee_pool`
14. **--connect debe incluir /p2p/<PeerId>**: El formato completo es obligatorio (validado en remote_monitor.rs)
15. **Retry de dial P2P**: Cada 30s via `tokio::select!`. Solo para peers en `--connect`. No-readiciona al desconectarse.

---

## 💻 Comandos Útiles

```powershell
# Tests completos
cd ledgerita_core; cargo test

# Tests con stdout visible
cd ledgerita_core; cargo test -- --nocapture

# Test específico
cd ledgerita_core; cargo test --test p2p_reconnect -- --nocapture

# Compilar release
cd ledgerita_core; cargo build --release

# Ejecutar nodo seed local
$env:LEDGERITA_PASSWORD="<VER_SECRETS_LOCAL.md>"
.\ledgerita_core\target\release\ledgerita_core.exe --listen-port 36565

# Conectar a producción
.\ledgerita_core\target\release\ledgerita_core.exe --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1

# Remote monitor
.\ledgerita_core\target\release\remote_monitor.exe --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1

# Desplegar a servidores
.\deploy.ps1

# SSH
ssh -i ~/.ssh/id_ed25519_ledgerita kettle03
ssh -i ~/.ssh/id_ed25519_ledgerita kettle02 'journalctl -u ledgerita-core -f'

# Niveles de log
$env:RUST_LOG="info"
$env:RUST_LOG="debug"
$env:RUST_LOG="network=trace"
```
