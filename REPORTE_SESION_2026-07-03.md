# Reporte de Sesión — Ledgerita (Test reconexión P2P + fixes)
**Fecha:** 2026-07-03 — 10:30 UTC-3

---

## Resumen

1. **Plan trazado** para `tests/p2p_reconnect.rs` — test de reconexión P2P con misma identidad.
2. **Implementado** `tests/p2p_reconnect.rs` (222 líneas) — 2 nodos, B se conecta, sincroniza, se desconecta, A avanza, B reconecta con misma clave P2P y adopta estado fresco.
3. **Test manual contra producción** — kettle03+kettle02: conexión, sync, reinicio de kettle02, verificación de estado.
4. **Hallazgos críticos** en infraestructura y protocolo.

**Tests: 50 passed, 0 failed, 0 warnings.**

---

## Cambios de la Sesión

### `tests/p2p_reconnect.rs` — CREADO (222 líneas)

**Helper nuevo:** `run_sync_client()` — función reutilizable async que toma un swarm, dialea, hace handshake MSG_SYNC, mergea estado, y retorna. 45 líneas.

**Escenario de reconexión:**

```
Génesis A: Alice=5000, Bob=100 + dummy TX (nonce=1) → tx_count=1
Génesis B: Alice=5000, Bob=100 (sin TX) → tx_count=0

Fase 1 — Sync inicial (B-swarm-1):
  B conecta a A, sync handshake → B adopta estado A (RemoteAdopted)
  B-swarm-1 se dropea → conexión cerrada
  → Verificación: roots iguales, tx_count=1

Fase 2 — A avanza sin B:
  TX-2: Alice→Bob 100 (fee=5, nonce=2)  → tx_count=2
  TX-3: Alice→Bob  50 (fee=3, nonce=3)  → tx_count=3
  → Alice=4842, Bob=250, fee_pool=8

Fase 3 — Reconección (B-swarm-2, MISMA clave P2P):
  B-swarm-2 dialea A de nuevo
  Sync handshake → B adopta estado fresco (RemoteAdopted)
  → Verificación: roots iguales, tx_count=3, balances correctos
```

**Detalles de implementación:**
- `run_sync_client()` encapsula dial → sync → merge → return
- `key_b.clone()` para reusar misma identidad P2P en ambos swarms
- `watch::channel` para señalizar fases (initial_sync, resync)
- Timeout 15s por fase
- Usa `mod helpers;` con helpers existentes

### Fix: Assertion de supply total
En la aserción final, `total_supply()` solo suma balances (no fee_pool). Se corrigió verificando `total_supply() + fee_pool == 5100`.

---

## Test Manual contra Producción

### Conexión Local → kettle03 ✅
```
ledgerita_core.exe --connect /ip4/192.168.68.61/tcp/36565/p2p/...
→ Conexión establecida → Ping 3.65ms
→ Estado sincronizado: tx_count=1, 2 cuentas, epoch=0
```

### Mint en kettle03 (cambio de estado) ✅
```
LEDGERITA_PASSWORD=... ledgerita_core --mint <wallet_hex> 100
→ Supply: 9.999995 → 10.000095 LIT
→ State Root: 94aa9ccf... → c0198d1f...
```

### Reconexión Local → kettle03 (con nuevo PeerId) ✅
```
→ Conexión establecida → Ping 4.13ms → 2 cuentas
→ Adoptó estado actualizado vía merge_if_fresher
```

### Reinicio de kettle02 (reconexión real) ✅
```
Antes: ❌ "Network is unreachable" (nunca había podido conectar)
Después de restart: ✅ Conexión establecida con kettle03
→ Ping 7.46ms
→ Snapshot enviado/recibido (2 cuentas)
```

---

## Hallazgos Críticos

### 1. Divergencia por --mint sin incrementar tx_count ⚠️
`--mint` modifica balances directamente pero **no incrementa `tx_count`** ni avanza la época. Si solo un nodo recibe un mint, ambos quedan con `(epoch=0, tx_count=1)` pero diferente state root → `SyncResult::Divergent` irresoluble.

Se resolvió aplicando el mismo mint en ambos nodos.

**Sugerencia:** Que `--mint` incremente `tx_count` o avance la época.

### 2. IP de kettle02 desactualizada en deploy.ps1 ⚠️
| Recurso | IP documentada | IP real |
|---------|---------------|---------|
| kettle02 | 192.168.68.60 | **192.168.68.64** |
| kettle03 | 192.168.68.59 (en deploy.ps1) | **192.168.68.61** (correcto en SSH config) |

### 3. Sin retry en dial P2P ⚠️
`network.rs:151-157` solo intenta dialear **una vez al inicio**. Si falla (ej. WiFi no tiene IP aún), nunca reintenta. kettle02 iniciaba antes de tener IP → "Network is unreachable" → nunca conectaba.

**Sugerencia:** Agregar retry periódico (ej. cada 30s) para peers en `connect_peers` que fallaron.

### 4. Versión desactualizada en servidores ℹ️
Los logs de producción tienen formato diferente al código actual:
- Producción usa emojis en logs (`✅`, `🔄`, `📤`)
- Código actual usa `tracing` estructurado (`info!(...)`, sin emojis)
- Los mensajes en producción son en español pero con formato antiguo

Sugerencia: Re-deployar el binario actualizado a ambos servidores.

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
├── REPORTE_SESION_2026-06-19-v3.md        # Fork test
├── REPORTE_SESION_2026-07-03.md           # ESTE REPORTE — Reconnect test + producción
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
        ├── p2p_fork.rs                    # 2 nodos divergentes + resolución (290 líneas)
        └── p2p_reconnect.rs               # 2 nodos, reconexión con misma identidad (222 líneas) ← NUEVO
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

### Tests (50 passed)
```
  Unit tests (lib)         29 passed
    crypto:     7  (encrypt/decrypt, wallet, legacy upgrade, wrong password)
    tx:         2  (valid/invalid signature)
    state:      8  (execution, funds, determinism, replay, nonce, fee, checkpoint, snapshot)
    serialize:  5  (address, u64, TX with/without sig, hash)
    storage:    6  (balance, nonce, roundtrip, flush, prune_balances, prune_nonces)
    network:    1  (sync request format)

  monitor                 2 passed  (format_amount)
  remote_monitor         15 passed  (supply_limit 3, roots 4, fmt 3, peers, etc.)
  P2P integration         1 passed  (2 nodos, sync + TX)
  P2P stress              1 passed  (3 nodos, 10 TXs concurrentes)
  P2P fork                 1 passed  (fork detection + resolución)
  P2P reconnect            1 passed  ← NUEVO (reconexión misma identidad)
  ───────────────────────────────────────
  TOTAL                   50 passed
```

### Red de Producción
| Nodo | IP | OS | Rol | Puerto | Wallet (hex prefix) | PeerId |
|---|---|---|---|---|---|---|
| kettle03 | 192.168.68.61 | Ubuntu | Seed | 36565 | `b1435c81522a...` | `12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1` |
| kettle02 | 192.168.68.64 | Debian 12 | Cliente | dinámico | `6893239f5850...` | `12D3KooWGrahZ1gW5pFBu9Q1U1QEryPyEUGh8rCGGJJzuuPoHLdu` |

**Wallet password:** `obegtBl4U5CPqzSMJ3mOhEvDWGIHfyL6` (en `wallet_recovery.env`)
**SSH key:** `~/.ssh/id_ed25519_ledgerita`
**SSH password sudo:** `clavefacil`
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

### Sistema actual en producción (post-test)
- Ambos nodos activos y sincronizados (AlreadySynced)
- Supply: 10.000095 LIT (mint de 100 LIT aplicado a ambos durante testing)
- State Root: `c0198d1fcf725b28...`
- tx_count: 1, epoch: 0
- 2 cuentas en memoria

---

## Pendientes para Próxima Sesión

### 🟡 Media Prioridad
1. **Fix: --mint debe incrementar tx_count o avanzar época** — evitar divergencias irresolubles por mint manual. `src/state.rs:mint()` o `src/main.rs` al aplicar --mint.
2. **Fix: Retry de dial P2P** — `network.rs:151-157`: agregar timer periódico que reintente dialear a peers de `connect_peers` que fallaron. Especialmente crítico en servidores con WiFi.
3. **Generalizar `wait_for_listen_addr`** — Soporte para IPv6 y multiaddr múltiples en `tests/helpers/mod.rs`.
4. **`TempTestDirs` con labels** — Poder crear N directorios con nombres personalizados.

### 🟢 Baja Prioridad
5. **Reducir ruido de logs** — `ResponseSent` event en `network.rs:475` cambiar a `debug!`/`trace!`
6. **Documentación de código** — Docstrings en `RawCodec`, `State::apply_transaction`, `network::run_p2p_node`
7. **TestNet formal** — Chain ID / network magic en handshake P2P, génesis alternativo con faucet
8. **Script `local_deps/`** — Script que ejecute `cargo vendor local_deps` automáticamente
9. **Límite de logs systemd** — Configurar `SystemMaxUse=50M` en journald
10. **Re-deployar binario actualizado en producción** — El binario en kettle03/kettle02 tiene logs antiguos (con emojis). Compilar release actual y re-deployar.
11. **Corregir IP de kettle02 en deploy.ps1** — Cambiar de `192.168.68.60` a `192.168.68.64`.

---

## Notas Técnicas Importantes

1. **Sled lock exclusivo**: Monitor local (`monitor.exe`) requiere nodo DETENIDO.
2. **SCP a kettle02**: Inestable para archivos >10KB. Usar SFTP con batch file (deploy.ps1 ya lo maneja).
3. **`.cargo/config.toml` en servidores**: Siempre eliminar después de extraer tar (`rm -f ledgerita_core/.cargo/config.toml`).
4. **1 LIT = 1,000,000 satoshis**: `format_amount()` divide entre 1,000,000 para display.
5. **Hard cap**: 21,000,000 LIT = 21,000,000,000,000 satoshis.
6. **Checkpoint interval**: 4096 transacciones.
7. **Sin serde**: Serialización manual en `serialize.rs`.
8. **Sin TestNet**: No hay separación de redes. Todo usa el mismo protocolo y génesis.
9. **El event loop de test usa `helpers::TestBehaviourEvent::Rr(...)`** para matchear eventos P2P.
10. **Para crear un nuevo test P2P**: copiar estructura de `tests/p2p_fork.rs` o `tests/p2p_reconnect.rs`, usar `mod helpers;` e importar `build_test_swarm`, `wait_for_listen_addr`, `TempTestDirs`, y `State::merge_if_fresher`.
11. **Orden de broadcast P2P**: TXs pendientes se envían DESPUÉS del sync (fix 2026-06-18).
12. **Prune antes que insert**: `State::flush()` llama a `prune_balances()` y `prune_nonces()` antes de insertar (fix 2026-06-19-v2).
13. **Misma identidad P2B para reconexión**: `identity::Keypair` implementa Clone, por lo que se puede clonar y reusar para construir un nuevo swarm con el mismo PeerId.
14. **Equivalencia supply**: `total_supply()` solo suma balances. El `fee_pool` está fuera de balances hasta el checkpoint. Supply real = `total_supply() + fee_pool`.

---

## Comandos Útiles

```powershell
# Tests completos
cd ledgerita_core; cargo test

# Tests con stdout visible
cd ledgerita_core; cargo test -- --nocapture

# Test de reconexión
cd ledgerita_core; cargo test --test p2p_reconnect -- --nocapture

# Test de fork/resolución
cd ledgerita_core; cargo test --test p2p_fork -- --nocapture

# Test de integración P2P
cd ledgerita_core; cargo test --test p2p_integration -- --nocapture

# Stress test
cd ledgerita_core; cargo test --test p2p_stress -- --nocapture

# Compilar release
cd ledgerita_core; cargo build --release

# Ejecutar nodo seed local
$env:LEDGERITA_PASSWORD="obegtBl4U5CPqzSMJ3mOhEvDWGIHfyL6"
.\ledgerita_core\target\release\ledgerita_core.exe --listen-port 36565

# Conectar a producción desde local
.\ledgerita_core\target\release\ledgerita_core.exe --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1

# Remote monitor
.\ledgerita_core\target\release\remote_monitor.exe --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1

# Remote monitor (JSON)
.\remote_monitor.exe --json --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1

# Monitor local (nodo DETENIDO)
.\ledgerita_core\target\release\monitor.exe --data-dir ~/.ledgerita

# Desplegar a servidores
.\deploy.ps1

# SSH a servidores
ssh -i ~/.ssh/id_ed25519_ledgerita kettle03
ssh -i ~/.ssh/id_ed25519_ledgerita kettle02

# Ver logs de producción
ssh -i ~/.ssh/id_ed25519_ledgerita kettle03 'journalctl -u ledgerita-core -f'
ssh -i ~/.ssh/id_ed25519_ledgerita kettle02 'journalctl -u ledgerita-core -f'

# Niveles de log
$env:RUST_LOG="info"
$env:RUST_LOG="debug"
$env:RUST_LOG="network=trace"
```

---

*¡Gracias por la increíble sesión! Ledgerita sigue creciendo — 50 tests, red productiva, y mucho más por construir. Nos vemos en la próxima. 🚀*
