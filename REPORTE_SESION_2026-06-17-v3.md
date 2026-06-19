# Reporte Completo de Sesión — Ledgerita (V3: Remote Monitor + Fix Sync)

**Fecha:** 17 de Junio de 2026 (Noche)
**Props:** @usuario — ¡Sesión BRUTAL! Monitor remoto, bug de sincronización encontrado y corregido, primera transacción en la red real. 🚀

---

## 📋 Índice
1. [Resumen de la sesión](#1-resumen-de-la-sesión)
2. [Stack tecnológico actual](#2-stack-tecnológico-actual)
3. [Estructura del proyecto (completa)](#3-estructura-del-proyecto-completa)
4. [Cambios implementados en esta sesión](#4-cambios-implementados-en-esta-sesión)
5. [Estado actual de la red](#5-estado-actual-de-la-red)
6. [Servidores de producción](#6-servidores-de-producción)
7. [Archivos de recuperación de wallet](#7-archivos-de-recuperación-de-wallet)
8. [Comandos de operación (systemd)](#8-comandos-de-operación-systemd)
9. [Comandos del Remote Monitor](#9-comandos-del-remote-monitor)
10. [Cómo desplegar cambios](#10-cómo-desplegar-cambios)
11. [Decisiones técnicas clave](#11-decisiones-técnicas-clave)
12. [Problemas conocidos](#12-problemas-conocidos)
13. [Pendientes priorizados](#13-pendientes-priorizados)
14. [Arquitectura completa del protocolo](#14-arquitectura-completa-del-protocolo)
15. [Formato de red (Wire Protocol)](#15-formato-de-red-wire-protocol)
16. [Tests](#16-tests)

---

## 1. Resumen de la Sesión

Esta sesión se enfocó en dos grandes logros:

**A. Remote Monitor P2P (`remote_monitor.rs`)** — Herramienta de auditoría que se conecta como peer a uno o más nodos sin detenerlos, descarga el snapshot vía `MSG_SYNC`, y verifica:
- Supply circulante ≤ 21,000,000 LIT
- State Root (recalculado localmente desde el snapshot)
- Fee pool, TX count, época, número de cuentas
- **Cross-validation entre múltiples peers**: detecta si un nodo tiene estado divergente

**B. Bug de sincronización P2P encontrado y corregido** — Durante la primera prueba real con `--mint` y `--send`, el monitor detectó que kettle03 tenía estado incorrecto (0 cuentas) mientras kettle02 tenía el correcto (2 cuentas). La causa: la sincronización `MSG_SYNC` reemplazaba ciegamente el estado local si el state root difería, sin importar qué nodo estuviera más actualizado. Se implementó comparación por **freshness** usando `(epoch, tx_count)`.

**C. Primera transacción real en la red** — Se mintearon 10 LIT en kettle03 y se enviaron 5 LIT a kettle02. La red procesó correctamente la transacción y ambos nodos convergieron al mismo estado después del fix.

---

## 2. Stack Tecnológico Actual

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
| Systemd | (ambos servidores) |

---

## 3. Estructura del Proyecto (Completa)

```
Ledgerita/
├── inicio.md                                         [Visión general del protocolo]
├── REPORTE_SESION_2026-06-16.md                       [Reporte Fase 2 (persistencia + P2P)]
├── REPORTE_SESION_2026-06-17.md                       [Reporte Fase 3a (--send, --mint, meta)]
├── REPORTE_SESION_2026-06-17-v2.md                    [Reporte V2 (systemd + infra)]
├── REPORTE_SESION_2026-06-17-v3.md                    [ESTE REPORTE (monitor remoto + fix sync)]
├── sesion-2026-06-15.md                               [Bitácora día 1]
├── sesion-2026-06-16.md                               [Bitácora día 2]
├── wallet_recovery.env                                [⚠️ RECUPERACIÓN — contraseña + direcciones]
├── .gitignore                                         [target/, *.log, .gemini/, local_deps/, wallet_recovery.env]
├── deploy.ps1                                         [Script de deploy automatizado]
│
├── ledgerita_core/
│   ├── Cargo.toml                                     [ed25519-dalek, libp2p, sled, orion, tokio, sha2]
│   ├── Cargo.lock
│   ├── .cargo/config.toml                             [Vendor -> local_deps/ (SOLO LOCAL)]
│   ├── local_deps/                                    [Gitignored, regenerable con cargo vendor]
│   ├── scripts/
│   │   ├── ledgerita-core.service                     [Systemd service file]
│   │   └── setup-systemd.sh                           [Instalador one-shot del servicio]
│   ├── src/
│   │   ├── lib.rs                                     [pub mod: crypto, transaction, state, network, error, serialize, storage]
│   │   ├── main.rs                                    [CLI principal con --listen-port, --mint, --send]
│   │   ├── crypto.rs                                  [Address, wallet encriptada con orion]
│   │   ├── transaction.rs                             [Transaction struct con nonce]
│   │   ├── state.rs                                   [State, checkpoint cada 4096 TXs, snapshots]
│   │   ├── storage.rs                                 [Sled wrapper (3 trees: balances, nonces, meta)]
│   │   ├── serialize.rs                               [Encode/decode manual (sin serde)]
│   │   ├── error.rs                                   [StorageError, SerializationError]
│   │   ├── network.rs                                 [P2P node, RawCodec, MSG_TX/MSG_SYNC, fix sync]
│   │   └── bin/
│   │       ├── monitor.rs                             [Monitor de estado LOCAL (nodo DETENIDO)]
│   │       └── remote_monitor.rs                      [Monitor REMOTO P2P (nodo EN VIVO)] ← NUEVO
│   └── target/
```

---

## 4. Cambios Implementados en Esta Sesión

### 4.1 Remote Monitor P2P — `src/bin/remote_monitor.rs` (ARCHIVO NUEVO, ~230 líneas)

**Descripción:** Herramienta de auditoría que se conecta como peer P2P a nodos en producción **sin detenerlos**, descarga el snapshot completo vía `MSG_SYNC`, y corre validaciones.

**CLI flags:**
- `--connect <multiaddr>` — **Obligatorio** (repetible para cross-validation). Debe incluir `/p2p/<PeerId>`.
- `--timeout <secs>` — Timeout total en segundos (default: 30).

**Validaciones por peer:**
1. **Supply circulante** ≤ 21,000,000 LIT (alert 🚨 si excede)
2. **State Root** — muestra prefijo hex de 8 bytes
3. **Fee Pool, TX Count, Época, Cuentas** — estadísticas del estado

**Cross-validation (multi-peer):**
- Si hay ≥ 2 peers, compara state roots entre todos
- Si todos coinciden → ✅
- Si algún root difiere → 🚨 DISCREPANCIA (exit code 1)

**Identidad:** Temporal (genera keypair ed25519 nuevo en cada ejecución). No necesita wallet ni `LEDGERITA_PASSWORD`.

**Dependencias:** Ninguna nueva. Reutiliza `RawCodec` de `network.rs`, `State` de `state.rs`, y las mismas de libp2p.

**Detalles técnicos:**
- Envía `MSG_SYNC` con dummy root ([0u8; 32]) al conectar
- Decodifica snapshot con `State::decode_snapshot()`
- No modifica ningún estado local — solo lectura
- Exit code 0 si todo bien, 1 si hay anomalías

**Uso:**
```bash
# One-shot a un peer
cargo run --bin remote_monitor -- \
  --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW...

# Cross-validation entre 2 peers
cargo run --bin remote_monitor -- \
  --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW... \
  --connect /ip4/192.168.68.60/tcp/<puerto>/p2p/12D3KooWGrah...

# Con timeout personalizado
cargo run --bin remote_monitor -- \
  --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW... \
  --timeout 15
```

### 4.2 Fix: Sincronización P2P por Freshness — `src/network.rs` (MODIFICADO)

**Bug detectado:** El monitor remoto reveló que kettle03 tenía estado incorrecto (0 cuentas) mientras kettle02 tenía el correcto (2 cuentas) tras una transacción.

**Causa raíz:** En el handler de `MSG_SYNC` response, la lógica reemplazaba el estado local si el state root era diferente, **sin importar qué estado fuera más nuevo**:

```rust
// ANTES (BUG): reemplazaba ciegamente si roots diferían
if local_root != remote_root {
    s.balances = remote.balances; // ❌ sobrescribe aunque local sea más nuevo
    // ...
}
```

**Fix implementado (`network.rs:388`):** Comparación por **freshness** usando `(epoch, tx_count)`:

```rust
// AHORA: compara freshness antes de reemplazar
if local_root != remote_root {
    let local_fresh = (s.epoch, s.tx_count);
    let remote_fresh = (remote.epoch, remote.tx_count);
    if remote_fresh > local_fresh {
        // Adoptar estado remoto (es más nuevo)
    } else if local_fresh > remote_fresh {
        // Conservar estado local (es más nuevo)
    } else {
        // Misma fresh pero diferente root → divergencia
    }
}
```

**Comportamiento después del fix:**
- `remote_fresh > local_fresh` → ✅ adoptar estado remoto (el peer está más actualizado)
- `local_fresh > remote_fresh` → 📤 conservar local (nosotros estamos más actualizados)
- Misma freshness pero roots diferentes → ⚠️ divergencia (posible corrupción)
- Roots iguales → ✅ ya sincronizado

**Otros cambios:**
- `MSG_TX` y `MSG_SYNC` ahora son `pub const` en `network.rs` para reutilización desde `remote_monitor.rs`
- Logs más descriptivos en la sincronización (muestra epoch y TX count de ambos peers)

### 4.3 Primera Transacción Real en la Red (PRUEBA)

Se ejecutó el siguiente comando en kettle03 (con servicios detenidos):

```bash
LEDGERITA_PASSWORD="obegtBl4U5CPqzSMJ3mOhEvDWGIHfyL6" \
  /home/user/ledgerita_core/target/release/ledgerita_core \
  --mint b1435c81522ac1e2b0f503986ca93bde92542b5d4cab4b9a9fd21253667e4cba 10000000 \
  --send 6893239f5850555f5701f24342bfbef5afdab94835f08d6e5f352899d4a17f28 5000000 5 \
  --listen-port 36565
```

**Resultado:**
- **Mint:** 10,000,000 base units (10 LIT) a kettle03
- **Send:** 5,000,000 base units (5 LIT) a kettle02 con fee 5
- **kettle03 balance final:** 4,999,995 LIT (4.999995 LIT)
- **kettle02 balance final:** 5,000,000 LIT (5.000000 LIT)
- **Fee pool:** 5 (0.000005 LIT)
- **TX count:** 1
- **State Root:** `94aa9ccf311d53e5...`
- **Supply total:** 9,999,995 (9.999995 LIT) ✅ ≤ 21M

El monitor remoto **detectó la discrepancia** del bug de sync, y después del fix **confirmó la convergencia** de ambos nodos al mismo estado correcto.

---

## 5. Estado Actual de la Red

```
kettle03 (semilla) ──── 192.168.68.61:36565 ──── ACTIVE (running)
     │
     │ (conexión automática al iniciar)
     │
kettle02 (cliente) ──── 192.168.68.60 ──── ACTIVE (running)
     ├── Conectado a kettle03
     ├── Ping: ~12ms
     ├── 2 cuentas (kettle03 + kettle02)
     ├── 1 transacción procesada
     └── Fee pool: 5 (0.000005 LIT)
```

**Verificación en vivo (17 Jun 2026 ~18:45 -04):**
```
remote_monitor:

kettle03 (seed):
  ✅ Supply     : 9.999995 / 21000000.000000 LIT
  ✅ State Root : 94aa9ccf311d53e5...
  📊 Fee Pool   : 0.000005 LIT
  📊 TX Count   : 1
  📊 Época      : 0
  📊 Cuentas    : 2

kettle02 (client):
  ✅ Supply     : 9.999995 / 21000000.000000 LIT
  ✅ State Root : 94aa9ccf311d53e5...
  📊 Fee Pool   : 0.000005 LIT
  📊 TX Count   : 1
  📊 Época      : 0
  📊 Cuentas    : 2

Cross-Validation: ✅ Todos los peers (2) comparten el mismo State Root
Resumen: ✅ Red saludable — 0 anomalía(s) detectada(s)
```

**Ambos nodos conectados 24/7 con reinicio automático y sincronización corregida.**

---

## 6. Servidores de Producción

| Servidor | IP | Sistema | Rust | Rol |
|---|---|---|---|---|
| **kettle02** | 192.168.68.60 | Debian 12 (bookworm) | 1.96.0 | Cliente (--connect a kettle03) |
| **kettle03** | 192.168.68.61 | Ubuntu | 1.96.0 | Semilla (--listen-port 36565) |

**Credenciales:**
- Usuario: `user`
- Password SSH: `clavefacil`
- Llave SSH: `~/.ssh/id_ed25519_ledgerita`
- Password sudo: `clavefacil` (misma que SSH)

**Directorios en servidores:**
```bash
/home/user/ledgerita_core/                              # Código fuente
/home/user/ledgerita_core/target/release/ledgerita_core # Binario nodo
/home/user/ledgerita_core/target/release/monitor         # Binario monitor local
/home/user/ledgerita_core/target/release/remote_monitor  # Binario monitor remoto
~/.ledgerita/wallet.key                                  # Wallet encriptada
~/.ledgerita/state/                                      # Base de datos Sled
/etc/ledgerita/env                                       # LEDGERITA_PASSWORD (solo root)
```

**Alias SSH:**
```
Host kettle02
    HostName 192.168.68.60
    User user
    StrictHostKeyChecking accept-new

Host kettle03
    HostName 192.168.68.61
    User user
    StrictHostKeyChecking accept-new
```

---

## 7. Archivos de Recuperación de Wallet

**⚠️ IMPORTANTE:** Este archivo contiene las credenciales de la wallet. NO está versionado en git.

**Ruta local:** `C:\Users\user\Documents\job\Ledgerita\wallet_recovery.env`

**Contenido:**
```
WALLET_PASSWORD=obegtBl4U5CPqzSMJ3mOhEvDWGIHfyL6
KETTLE02_WALLET_ADDRESS=6893239f5850555f5701f24342bfbef5afdab94835f08d6e5f352899d4a17f28
KETTLE02_PEER_ID=12D3KooWGrahZ1gW5pFBu9Q1U1QEryPyEUGh8rCGGJJzuuPoHLdu
KETTLE03_WALLET_ADDRESS=b1435c81522ac1e2b0f503986ca93bde92542b5d4cab4b9a9fd21253667e4cba
KETTLE03_PEER_ID=12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1
KETTLE03_IP=192.168.68.61
```

**Balances actuales (después de la TX de prueba):**
- **kettle03:** 4,999,995 LIT (4.999995 LIT)
- **kettle02:** 5,000,000 LIT (5.000000 LIT)
- **Fee pool:** 5 (0.000005 LIT)

**Si se pierde la contraseña o el wallet:**
1. Eliminar `~/.ledgerita/wallet.key` en el servidor
2. Ejecutar nodo con nueva contraseña → se genera wallet nuevo
3. Actualizar `wallet_recovery.env` con la nueva dirección y PeerId
4. El estado (balances) de otras cuentas se conserva en Sled

---

## 8. Comandos de Operación (Systemd)

```bash
# Ver logs en vivo del nodo
ssh -i ~/.ssh/id_ed25519_ledgerita kettle03
sudo journalctl -u ledgerita-core -f

# Últimas 50 líneas
sudo journalctl -u ledgerita-core -n 50 --no-pager

# Estado del servicio
systemctl status ledgerita-core

# Reiniciar servicio (después de deploy)
sudo systemctl restart ledgerita-core

# Detener servicio
sudo systemctl stop ledgerita-core

# Ver puerto de escucha
sudo journalctl -u ledgerita-core -n 2 --no-pager | grep "escuchando"

# Ver conexiones activas
sudo journalctl -u ledgerita-core -n 5 --no-pager | grep "Conexión establecida"

# Ver logs de sincronización (después del fix)
sudo journalctl -u ledgerita-core -n 5 --no-pager | grep "sincronizado\|reciente\|divergente"
```

### Comandos Útiles del Nodo

```bash
# Mostrar dirección de wallet
sudo LEDGERITA_PASSWORD="$(cat /etc/ledgerita/env | cut -d= -f2)" /home/user/ledgerita_core/target/release/ledgerita_core --show-address

# O más simple (después de source):
export $(cat /etc/ledgerita/env | xargs)
./target/release/ledgerita_core --show-address

# Enviar TX (requiere fondos)
LEDGERITA_PASSWORD="..." ./target/release/ledgerita_core \
  --mint <mi_direccion> 1000000 \
  --send <receiver_hex> 500 5

# Tests
cd /home/user/ledgerita_core
source ~/.cargo/env
cargo test --release
```

---

## 9. Comandos del Remote Monitor

### Local (Windows)
```bash
cd C:\Users\user\Documents\job\Ledgerita\ledgerita_core

# One-shot a kettle03
cargo run --bin remote_monitor -- \
  --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1

# Cross-validation con kettle02 (necesita puerto actual)
cargo run --bin remote_monitor -- \
  --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW... \
  --connect /ip4/192.168.68.60/tcp/<PUERTO_ACTUAL>/p2p/12D3KooWGrah...

# Cross-validation con timeout extendido
cargo run --bin remote_monitor -- \
  --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW... \
  --connect /ip4/192.168.68.60/tcp/<PUERTO>/p2p/12D3KooWGrah... \
  --timeout 20
```

### En servidores (binario compilado en release)
```bash
# Obtener puerto actual de kettle02
ssh kettle02 "ss -tlnp | grep ledgerita"

# Ejecutar monitor remoto contra ambos peers
LEDGERITA_PASSWORD="obegtBl4U5CPqzSMJ3mOhEvDWGIHfyL6" \
  ./target/release/remote_monitor \
  --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW... \
  --connect /ip4/192.168.68.60/tcp/<PORT>/p2p/12D3KooWGrah...
```

**Nota:** El monitor local (`monitor`) requiere nodo DETENIDO. El remote (`remote_monitor`) funciona con nodo EN VIVO.

---

## 10. Cómo Desplegar Cambios

### Opción A: Script automatizado (recomendado)

```powershell
Set-Location C:\Users\user\Documents\job\Ledgerita
.\deploy.ps1
```

### Opción B: Manual paso a paso

```powershell
# 1. Compilar y testear local
Set-Location C:\Users\user\Documents\job\Ledgerita\ledgerita_core
cargo test --release
cargo build --release

# 2. Crear tar (excluyendo target y local_deps)
Set-Location C:\Users\user\Documents\job\Ledgerita
Remove-Item ledgerita_core.tar -ErrorAction SilentlyContinue
tar cf ledgerita_core.tar --exclude="target" --exclude="local_deps" ledgerita_core

# 3A. Subir a kettle03 (SCP)
scp -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" ledgerita_core.tar kettle03:~

# 3B. Subir a kettle02 (SFTP)
$batch = Join-Path $env:TEMP "upload_ledgerita.txt"
Set-Content -Path $batch -Value "put ledgerita_core.tar"
sftp -b $batch -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" kettle02:
Remove-Item -LiteralPath $batch -ErrorAction SilentlyContinue

# 4. Extraer y compilar en servidores
ssh -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" kettle03 "
  cd /home/user && tar xf ledgerita_core.tar &&
  rm -f /home/user/ledgerita_core/.cargo/config.toml &&
  cd /home/user/ledgerita_core &&
  source ~/.cargo/env &&
  cargo build --release
"

ssh -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" kettle02 "
  cd /home/user && tar xf ledgerita_core.tar &&
  rm -f /home/user/ledgerita_core/.cargo/config.toml &&
  cd /home/user/ledgerita_core &&
  source ~/.cargo/env &&
  cargo build --release
"

# 5. Reiniciar servicios
ssh -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" kettle03 "echo clavefacil | sudo -S systemctl restart ledgerita-core"
ssh -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" kettle02 "echo clavefacil | sudo -S systemctl restart ledgerita-core"
```

### Notas importantes para deploy:
- **Siempre eliminar** `.cargo/config.toml` después de extraer tar en servidores
- **Contraseña sudo** es `clavefacil` (usar `echo 'clavefacil' | sudo -S comando`)
- **SCP a kettle02** es inestable para archivos >10KB — usar SFTP
- **kettle03** SCP funciona bien

---

## 11. Decisiones Técnicas Clave

1. **Identidad temporal del Remote Monitor**: No necesita wallet ni `LEDGERITA_PASSWORD` porque genera keypair ed25519 nuevo cada ejecución. Es solo lectura, no firma transacciones.

2. **Freshness por `(epoch, tx_count)`**: Se usa la tupla `(epoch, tx_count)` como métrica de qué estado es más nuevo. Tupla compara por epoch primero, luego por TX count. Un nodo con epoch 1 (post-checkpoint) es más nuevo que uno con epoch 0 aunque tenga más TXs.

3. **Dummy root en MSG_SYNC**: El remote monitor envía `[0u8; 32]` como state root en la solicitud MSG_SYNC (no tiene estado propio). El nodo remoto no valida este root, solo responde con su snapshot.

4. **Cross-validation**: Si hay ≥ 2 peers, se comparan state roots. Si todos coinciden → red saludable. Si alguno difiere → alerta de discrepancia.

5. **Misma freshness pero roots diferentes**: Se loguea como divergencia (posible corrupción). El estado local se conserva.

6. **Sled no elimina claves al hacer flush con mapa vacío**: `State::flush()` itera las claves actuales del mapa. Si el mapa está vacío (como ocurría con el bug), las claves viejas en Sled NO se eliminan. Esto salvó los balances cuando ocurrió el bug — al reiniciar, el nodo cargó las claves existentes de Sled. Esto es un comportamiento a considerar: `flush()` debería hacer un clear del árbol primero para ser correcto.

---

## 12. Problemas Conocidos

### Operativos
1. **SCP a kettle02**: Inestable para archivos >10KB. Usar SFTP con batch file.
2. **Conexión a kettle02**: Timeouts intermitentes. Usar `ConnectTimeout=30`.
3. **`sudo` sin TTY**: Los comandos sudo requieren `echo 'clavefacil' | sudo -S` cuando se ejecutan via SSH sin terminal interactivo.
4. **`.cargo/config.toml` con `local_deps`**: El tar incluye este archivo. Siempre eliminar después de extraer en servidores: `rm -f ledgerita_core/.cargo/config.toml`.

### Técnicos
5. **Remote Monitor requiere puerto actual de kettle02**: Como kettle02 usa puerto dinámico, hay que obtenerlo con `ss -tlnp | grep ledgerita` antes de ejecutar el monitor contra él.
6. **Build local en Windows**: Usa `.cargo/config.toml` con `local_deps/` regenerado vía `cargo vendor`. Si `cargo vendor` falla por rutas largas, comentar el config y compilar directo de crates.io.
7. **`State::flush()` no elimina claves viejas**: Si el mapa de balances está vacío y se hace flush, las claves existentes en Sled no se eliminan. Esto fue beneficioso durante el bug de sync (preservó los balances), pero puede causar estados inconsistentes si se necesita resetear a 0 cuentas.

---

## 13. Pendientes Priorizados

### 🔴 Alta Prioridad

| # | Tarea | Descripción | Archivos |
|---|---|---|---|
| 1 | **Tests de integración P2P** | Test que inicia dos nodos en memoria, conecta, envía TX, verifica que el segundo recibe y aplica. | `tests/` (nuevo) |
| 2 | **Tests del Remote Monitor** | Tests unitarios para `remote_monitor.rs`: `format_amount`, parsing de args, validación de snapshot. | `bin/remote_monitor.rs` |

### 🟡 Media Prioridad

| # | Tarea | Descripción | Archivos |
|---|---|---|---|
| 3 | **Múltiples --connect en service file** | El flag `--connect` se puede repetir en CLI pero configurarlo en el service file con múltiples peers es manual. | `main.rs`, `scripts/ledgerita-core.service` |
| 4 | **Reducir ruido en logs** | "Respuesta enviada" se imprime para cada mensaje. Demasiado verbose para producción. | `network.rs` |
| 5 | **Mejorar `State::flush()`** | Debería limpiar el árbol Sled antes de escribir, o al menos eliminar claves que ya no existen en el estado en memoria. | `state.rs`, `storage.rs` |

### 🟢 Baja Prioridad

| # | Tarea | Descripción | Archivos |
|---|---|---|---|
| 6 | **TestNet formal** | Después de estabilizar producción. Requiere: chain ID / network magic en handshake P2P, génesis alternativo con faucet. | Nuevos módulos |
| 7 | **Systemd: logging size** | Configurar `SystemMaxUse=50M` en journald para evitar que logs crezcan infinito. | `/etc/systemd/journald.conf` |
| 8 | **Script `setup.ps1`** | Script que ejecuta `cargo vendor` automáticamente para builds locales. | `setup.ps1` |

---

## 14. Arquitectura Completa del Protocolo

```
                    ┌──────────────────────┐
                    │      main.rs         │
                    │  CLI + Wallet load   │
                    │  --listen-port N     │
                    │  --mint / --send     │
                    └────────┬─────────────┘
                             │
              ┌──────────────┼──────────────┐
              ▼              ▼              ▼
       ┌──────────┐  ┌────────────┐  ┌──────────────┐
       │ crypto   │  │ transaction│  │   network    │
       │ .rs      │  │ .rs        │  │   .rs        │
       │ Address  │  │ Tx struct  │  │ Swarm P2P    │
       │ Wallet   │  │ sign/verify│  │ dial + listen│
       │ encrypt  │  │ nonce      │  │ MSG_TX/SYNC  │
       └──────────┘  └─────┬──────┘  └──────┬───────┘
                           │                │
                           ▼                ▼
                    ┌──────────────┐ ┌───────────────────┐
                    │   state.rs   │ │ remote_monitor.rs │
                    │  BTreeMap    │ │  Auditor P2P     │
                    │  balances    │ │  MSG_SYNC +       │
                    │  nonces      │ │  validaciones     │
                    │  fee_pool    │ │  cross-validation │
                    │  epoch       │ └───────────────────┘
                    │  apply_tx()  │
                    │  checkpoint  │
                    │  snapshots   │
                    └──────┬───────┘
                           │
                           ▼
                    ┌──────────────┐
                    │  storage.rs  │
                    │  Sled DB     │
                    │  3 trees     │
                    └──────────────┘
```

### Flujo de una Transacción
1. Usuario firma `Transaction` con su `SigningKey` (wallet)
2. Nodo emisor valida firma → `is_signature_valid()` (Prueba de Fórmula)
3. Nodo emisor aplica al `State` → `apply_transaction()` (descuenta saldo, acredita receptor, valida nonce)
4. TX se propaga a pares vía P2P (`MSG_TX` broadcast a peers conectados)
5. Nodo receptor valida firma, chequea mempool (deduplicación via SHA-256)
6. Nodo receptor aplica TX localmente y reenvía a sus demás peers
7. Cada 4096 TXs → `finalize_checkpoint()` distribuye fee pool, genera nuevo state root

### Flujo de Sincronización P2P (después del fix)
1. Al establecerse conexión, ambos nodos envían `MSG_SYNC` con su state root
2. Cada nodo responde con su snapshot completo
3. Al recibir respuesta, se compara `(epoch, tx_count)`:
   - **Remoto más reciente** → adoptar estado remoto
   - **Local más reciente** → conservar estado local
   - **Misma freshness, roots diferentes** → log de divergencia

### Identidad Unificada
```
semilla 32 bytes (getrandom) → derive_from_seed()
  ├── ed25519_dalek::SigningKey  → Wallet (firmar transacciones)
  └── libp2p::identity::Keypair  → Nodo (PeerId P2P)
```

---

## 15. Formato de Red (Wire Protocol)

```
Request/Response (RawCodec):
  [4 bytes big-endian length][payload]

Payload (MSG_TX = 0x01):
  [0x01][encoded Transaction (152 bytes)]

Payload (MSG_SYNC = 0x02):
  Request:  [0x02][state root (32 bytes)]
  Response: [0x02][snapshot (variable)]

Snapshot format:
  [epoch:8][fee_pool:8][tx_count:8][num_entries:4]
    por cada entry: [address:32][balance:8][nonce:8]
```

---

## 16. Tests

**29 tests pasan en todos los entornos (local + servidores):**
- crypto: 7 tests
- transaction: 2 tests
- state: 8 tests
- serialize: 4 tests
- storage: 4 tests
- network: 1 test
- monitor: 2 tests
- doc-tests: 2
- remote_monitor: 0 tests (pendiente agregar)

```bash
# Ejecutar tests
cd ledgerita_core
cargo test --release
```

**Binarios disponibles en servidores:**
- `target/release/ledgerita_core` — Nodo principal P2P
- `target/release/monitor` — Monitor local (requiere nodo detenido)
- `target/release/remote_monitor` — Monitor remoto P2P (nodo en vivo) ← NUEVO

---

*Fin del reporte V3. ¡Ha sido una sesión INCREÍBLE! Encontramos y corregimos un bug crítico de sincronización, construimos el monitor remoto que lo detectó, y ejecutamos la primera transacción real en la red Ledgerita. Nos vemos en la próxima para los tests de integración P2P. 🚀*

*Recuerda leer `wallet_recovery.env` antes de empezar la siguiente sesión — contiene las credenciales de los wallets actuales.*
