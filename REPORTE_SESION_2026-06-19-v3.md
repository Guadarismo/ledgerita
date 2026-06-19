# Reporte de Sesión — Ledgerita (Test fork/resolución P2P)
**Fecha:** 2026-06-19 — 12:28 UTC-3

---

## Resumen

1. **Plan trazado** para `tests/p2p_fork.rs` — fork detection + resolution por freshness.
2. **Implementado** `tests/p2p_fork.rs` (176 líneas) — 2 nodos divergen, detectan `SyncResult::Divergent`, luego A se vuelve más fresco y B adopta vía `RemoteAdopted`.

**Tests: 49 passed, 0 failed, 0 warnings.**

---

## Cambios de la Sesión

### `tests/p2p_fork.rs` — CREADO (176 líneas)

**Escenario de fork:**

```
Génesis: Alice=10000, Bob=10000, Charlie=10000

Fase 1 — Divergencia:
  Nodo A: Alice→Bob 100 (nonce=1)  → tx_count=1, root_A
  Nodo B: Charlie→Bob 200 (nonce=1) → tx_count=1, root_B (≠ root_A)
  Sync A↔B → ambos detectan SyncResult::Divergent

Fase 2 — Resolución:
  Nodo A: Alice→Bob 50 (nonce=2)   → tx_count=2, root_A2
  Nodo B re-sincroniza → merge_if_fresher → RemoteAdopted
  Estados convergen: tx_count=2, roots iguales
```

**Verificaciones:**
- State roots iguales tras resolución
- `tx_count == 2` en ambos nodos
- Alice = 9850, Bob = 10150, Charlie = 10000 (TX de Charlie en B fue revertida)
- Supply total = 30000 (no se crea ni destruye dinero)

**Detalles de implementación:**
- Señalización vía `Arc<AtomicU64>` para divergencia contada por ambos nodos
- `watch::channel<bool>` para resolver final
- `trigger_resync` flag en nodo B: al detectar `Divergent`, envía un segundo MSG_SYNC
- Nodo A aplica TX fresca al detectar divergencia y envía MSG_SYNC inmediatamente
- Timeout 10s por fase
- Usa `mod helpers;` e importa `build_test_swarm`, `wait_for_listen_addr`, `TempTestDirs`

---

## Resumen de Tests (49 passed)

```
test result: ok. 49 passed; 0 failed; 0 warnings (0.00s)

  Unit tests (lib)         31 passed  (crypto:7, tx:2, state:8, serialize:5, storage:6, network:1)
  monitor                  2 passed
  remote_monitor          15 passed
  P2P integration          1 passed
  P2P stress               1 passed
  P2P fork                 1 passed  ← NUEVO
```

---

## Estado Actual del Proyecto

### Archivos del proyecto (raíz)
```
Ledgerita/
├── inicio.md                              # Visión general del protocolo
├── .gitignore                             # target/, *.log, .gemini/, local_deps/, wallet_recovery.env
├── deploy.ps1                             # Script de deploy a servidores
├── wallet_recovery.env                    # Wallet credentials — NO SUBIR A GIT
├── opinion de deepseek del proyecto 15-06-2026.txt
├── REPORTE_FINAL_CONTEXTO.md              # Contexto completo del proyecto
├── REPORTE_SESION_2026-06-16.md           # Fase 2 — Persistencia Sled
├── REPORTE_SESION_2026-06-17.md           # Fase 3a — Flags CLI + TX
├── REPORTE_SESION_2026-06-17-v2.md        # Systemd/Infra
├── REPORTE_SESION_2026-06-17-v3.md        # Remote Monitor + Sync Fix
├── REPORTE_SESION_2026-06-18.md           # Broadcast Fix + tracing
├── REPORTE_SESION_2026-06-18-v2.md        # Broadcast Fix (resumen)
├── REPORTE_SESION_2026-06-19.md           # Refactor Test Helpers + merge_if_fresher
├── REPORTE_SESION_2026-06-19-v2.md        # Fix stale keys + Stress test
├── REPORTE_SESION_2026-06-19-v3.md        # ESTE REPORTE — Fork test
├── sesion-2026-06-15.md                   # Bitácora día 1
├── sesion-2026-06-16.md                   # Bitácora día 2
│
└── ledgerita_core/                        # PROYECTO RUST
    ├── Cargo.toml                         # 11 dependencias
    ├── Cargo.lock
    ├── .cargo/config.toml                 # Vendor -> local_deps/
    ├── local_deps/                        # Vendored crates (cargo vendor)
    ├── src/
    │   ├── lib.rs                         # pub mod: 7 módulos
    │   ├── main.rs                        # CLI (240 líneas)
    │   ├── crypto.rs                      # Address + Wallet encriptada (343 líneas)
    │   ├── transaction.rs                 # TX con firma ed25519 (99 líneas)
    │   ├── state.rs                       # Estado + Checkpoints + SyncResult (430 líneas)
    │   ├── network.rs                     # P2P libp2p event-loop (495 líneas)
    │   ├── storage.rs                     # Sled wrapper (290 líneas)
    │   ├── serialize.rs                   # Encode/decode manual (153 líneas)
    │   ├── error.rs                       # Errores tipados (thiserror)
    │   └── bin/
    │       ├── monitor.rs                 # Monitor local (nodo DETENIDO)
    │       └── remote_monitor.rs          # Monitor remoto P2P con --json
    └── tests/
        ├── helpers/
        │   └── mod.rs                     # TestBehaviour, build_test_swarm, wait_for_listen_addr, TempTestDirs
        ├── p2p_integration.rs             # 2 nodos, sync + TX (186 líneas)
        ├── p2p_stress.rs                  # 3 nodos, 10 TXs concurrentes (346 líneas)
        └── p2p_fork.rs                    # 2 nodos divergentes + resolución (176 líneas) ← NUEVO
```

### Dependencias (`Cargo.toml`)
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

### Wire Protocol (P2P)
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

Sync freshness rule (merge_if_fresher):
  if (remote.epoch, remote.tx_count) > (local.epoch, local.tx_count) → RemoteAdopted
  if (local.epoch, local.tx_count) > (remote.epoch, remote.tx_count) → LocalNewer
  if equal but roots differ → Divergent
  if roots equal → AlreadySynced
```

### Red de Producción
| Nodo | IP | OS | Rol | Puerto | Wallet (hex prefix) | PeerId |
|---|---|---|---|---|---|---|
| kettle03 | 192.168.68.61 | Ubuntu | Seed | 36565 | `b1435c81522a...` | `12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1` |
| kettle02 | 192.168.68.60 | Debian 12 | Cliente | dinámico | `6893239f5850...` | `12D3KooWGrahZ1gW5pFBu9Q1U1QEryPyEUGh8rCGGJJzuuPoHLdu` |

**Wallet password (producción):** `obegtBl4U5CPqzSMJ3mOhEvDWGIHfyL6` (en `wallet_recovery.env`)
**SSH:** `ssh -i ~/.ssh/id_ed25519_ledgerita kettle03`, password: `clavefacil`

---

## Pendientes para Próxima Sesión

### 🟡 Media Prioridad
1. ~TEST DE FORK/RESOLUCIÓN — HECHO~ ✅
2. **Test de reconexión** — `tests/p2p_reconnect.rs`: Nodo se desconecta y reconecta, verificar resincronización. Usar helpers existentes.
3. **`wait_for_listen_addr` generalizar** — Soporte para IPv6 y multiaddr múltiples.
4. **`TempTestDirs` con labels** — Poder crear N directorios con nombres personalizados.

### 🟢 Baja Prioridad
5. **Reducir ruido de logs** — `ResponseSent` event en `network.rs:475` cambiar a `debug!`/`trace!`
6. **Documentación de código** — Docstrings en `RawCodec`, `State::apply_transaction`, `network::run_p2p_node`
7. **TestNet formal** — Chain ID / network magic en handshake P2P, génesis alternativo con faucet
8. **Script `local_deps/`** — Script que ejecute `cargo vendor local_deps` automáticamente
9. **Límite de logs systemd** — Configurar `SystemMaxUse=50M` en journald

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
10. **Para crear un nuevo test P2P**: copiar estructura de `tests/p2p_fork.rs`, usar `mod helpers;` e importar `build_test_swarm`, `wait_for_listen_addr`, `TempTestDirs`, y `State::merge_if_fresher`.
11. **Orden de broadcast P2P**: TXs pendientes se envían DESPUÉS del sync (fix 2026-06-18). El broadcast se dispara en los handlers de MSG_SYNC request y response.
12. **Prune antes que insert**: `State::flush()` llama a `prune_balances()` y `prune_nonces()` antes de insertar (fix 2026-06-19-v2). Esto evita claves stale en Sled.

---

## Comandos Útiles

```powershell
# Tests completos
cd ledgerita_core; cargo test

# Tests con stdout visible
cd ledgerita_core; cargo test -- --nocapture

# Test de fork/resolución
cd ledgerita_core; cargo test --test p2p_fork -- --nocapture

# Test de integración P2P
cd ledgerita_core; cargo test --test p2p_integration -- --nocapture

# Stress test
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

*¡Gracias por la increíble sesión! Nos vemos en la próxima para seguir construyendo Ledgerita juntos. 🚀*
