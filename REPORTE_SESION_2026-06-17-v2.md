# Reporte Completo de Sesión — Ledgerita (V2: Systemd + Infraestructura)

**Fecha:** 17 de Junio de 2026 (Tarde/Noche)
**Props:** @usuario — ¡Gracias infinitas por la sesión! Increíble trabajo en equipo.

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
9. [Cómo desplegar cambios](#9-cómo-desplegar-cambios)
10. [Decisiones técnicas clave](#10-decisiones-técnicas-clave)
11. [Problemas conocidos](#11-problemas-conocidos)
12. [Pendientes priorizados](#12-pendientes-priorizados)
13. [Arquitectura completa del protocolo](#13-arquitectura-completa-del-protocolo)
14. [Formato de red (Wire Protocol)](#14-formato-de-red-wire-protocol)

---

## 1. Resumen de la Sesión

Esta sesión se enfocó en llevar los nodos Ledgerita a **producción real como daemons 24/7**. Se implementaron:

- **Flag `--listen-port <puerto>`**: Puerto fijo para nodos (esencial para systemd)
- **Systemd service**: Daemon con reinicio automático, logging integrado, manejo seguro de contraseña
- **Nuevos wallets**: Se generaron wallets frescos en ambos servidores (contraseña guardada en `wallet_recovery.env`)
- **Conexión P2P automática**: kettle02 se conecta a kettle03 al arrancar vía `--connect`
- **Script de deploy automatizado**: `deploy.ps1` compila, sube, construye e instala en un solo comando
- **Archivo de recuperación**: `wallet_recovery.env` con contraseña y direcciones (guardado LOCAL, no en git)

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
├── REPORTE_SESION_2026-06-17-v2.md                    [ESTE REPORTE (systemd + infra)]
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
│   │   ├── main.rs                                    [CLI principal con --listen-port]
│   │   ├── crypto.rs                                  [Address, wallet encriptada con orion]
│   │   ├── transaction.rs                             [Transaction struct con nonce]
│   │   ├── state.rs                                   [State, checkpoint cada 4096 TXs, snapshots]
│   │   ├── storage.rs                                 [Sled wrapper (3 trees: balances, nonces, meta)]
│   │   ├── serialize.rs                               [Encode/decode manual (sin serde)]
│   │   ├── error.rs                                   [StorageError, SerializationError]
│   │   ├── network.rs                                 [P2P node, RawCodec, MSG_TX/MSG_SYNC]
│   │   └── bin/
│   │       └── monitor.rs                             [Monitor de estado local]
│   └── target/
```

---

## 4. Cambios Implementados en Esta Sesión

### 4.1 Flag `--listen-port <puerto>` (CÓDIGO)

**Archivos modificados:**
- `ledgerita_core/src/main.rs` — nuevo flag CLI `--listen-port` (default 0 = puerto aleatorio)
- `ledgerita_core/src/network.rs` — `run_p2p_node()` acepta `listen_port: u16`, lo usa en `swarm.listen_on()`

**Detalle del cambio en `network.rs:143`:**
```rust
// ANTES (puerto aleatorio siempre):
swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

// AHORA (puerto configurable):
let listen_addr = format!("/ip4/0.0.0.0/tcp/{}", listen_port);
swarm.listen_on(listen_addr.parse()?)?;
```

**Detalle del cambio en `main.rs:`**
- Nuevo flag `--listen-port <port>` en el parser de argumentos (línea ~103)
- Se pasa como último argumento a `run_p2p_node()`
- Si no se especifica, `listen_port = 0` → comportamiento idéntico al anterior (puerto aleatorio)
- Muestra `📡 Puerto de escucha: <port>` si se especifica un puerto distinto de 0

**Por qué puerto fijo y no aleatorio:**
- Sin puerto fijo, `--connect` no funciona confiablemente después de reinicios
- Los puertos aleatorios no aportan seguridad real (la red es LAN, la seguridad está en ed25519 + nonce)
- Permite firewall rules, logs predecibles, y systemd confiable

### 4.2 Systemd Service (ARCHIVOS NUEVOS)

**Archivo:** `ledgerita_core/scripts/ledgerita-core.service`

```ini
[Unit]
Description=Ledgerita Protocol Core Node
After=network.target
Wants=network-online.target

[Service]
Type=simple
User=user
Group=user
WorkingDirectory=/home/user/ledgerita_core
ExecStart=/home/user/ledgerita_core/target/release/ledgerita_core
EnvironmentFile=/etc/ledgerita/env
Restart=always
RestartSec=10
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
```

**Puntos clave:**
- `EnvironmentFile=/etc/ledgerita/env` — contraseña fuera del service file, permisos 600
- `Restart=always` + `RestartSec=10` — revive automáticamente tras crash con 10s de pausa
- `LimitNOFILE=65535` — necesario para libp2p (múltiples conexiones)
- `ExecStart` se edita en cada servidor para agregar flags específicos

**Archivo:** `ledgerita_core/scripts/setup-systemd.sh`
- Script one-shot que crea `/etc/ledgerita/env`, copia service file, recarga systemd, habilita servicio
- Solicita editar la contraseña real antes de iniciar

### 4.3 Deploy Script (ARCHIVO NUEVO)

**Archivo:** `deploy.ps1`

Script PowerShell que automatiza el ciclo completo:
1. `cargo test --release` — verifica tests locales
2. `cargo build --release` — compila release
3. Crea tarball excluyendo `target/` y `local_deps/`
4. Sube a kettle03 (SCP) y kettle02 (SFTP)
5. SSH a cada servidor: extrae tar, elimina `.cargo/config.toml`, compila con `cargo build --release`
6. Instala service file, habilita systemd

**Uso:**
```powershell
.\deploy.ps1
```

### 4.4 Nuevos Wallets (INFRAESTRUCTURA)

Se generaron wallets NUEVOS en ambos servidores (los anteriores tenían contraseña desconocida).

**Proceso:**
1. Eliminar wallet viejos: `rm -f ~/.ledgerita/wallet.key`
2. Ejecutar nodo con nueva contraseña → `load_or_create_wallet()` genera seed fresh de 32 bytes
3. Capturar dirección y PeerId de la salida
4. Configurar `/etc/ledgerita/env` con `LEDGERITA_PASSWORD=...`
5. Iniciar servicio systemd

**Datos de wallet guardados en:** `wallet_recovery.env` (NO versionado en git — está en `.gitignore`)

### 4.5 Conexión P2P Automática (INFRAESTRUCTURA)

- **kettle03**: Corre como semilla en puerto fijo **36565**
- **kettle02**: Se conecta automáticamente a kettle03 vía `--connect` en el `ExecStart` del service

---

## 5. Estado Actual de la Red

```
kettle03 (semilla) ──── 192.168.68.61:36565 ──── ACTIVE (running)
     │
     │ (conexión automática al iniciar)
     │
kettle02 (cliente) ──── 192.168.68.60 ──── ACTIVE (running)
     └── Conectado a kettle03
     └── Ping: ~7ms
     └── Estado sincronizado (0 cuentas, wallets nuevos)
```

**Verificación en vivo (17 Jun 2026 18:15 -04):**
```
kettle03:
  📡 Escuchando en: /ip4/192.168.68.61/tcp/36565
  ✅ Conexión establecida con: PeerId("12D3KooWGrahZ1gW5pFBu9Q1U1QEryPyEUGh8rCGGJJzuuPoHLdu") [kettle02]
  🔄 Apretón de Manos (Ping): 9ms

kettle02:
  🔗 Conectando a: /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1
  ✅ Conexión establecida con: PeerId("12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1") [kettle03]
  🔄 Apretón de Manos (Ping): 7ms
  ✅ Estado ya sincronizado
```

**Ambos nodos conectados 24/7 con reinicio automático en caso de fallo.**

---

## 6. Servidores de Producción

| Servidor | IP | Sistema | Rust | Rol |
|---|---|---|---|---|
| **kettle02** | 192.168.68.60 | Debian 12 (bookworm) | 1.96.0 | Cliente (--connect a kettle03) |
| **kettle03** | 192.168.68.61 | Ubuntu | 1.96.0 | Semilla (--listen-port 36565) |

**Nota importante:** La IP de kettle03 cambió de 192.168.68.59 a 192.168.68.61. El archivo `~/.ssh/config` local ya está actualizado.

**Credenciales:**
- Usuario: `user`
- Password SSH: `clavefacil`
- Llave SSH: `~/.ssh/id_ed25519_ledgerita`
- Password sudo: `clavefacil` (misma que SSH)

**Directorios en servidores:**
```bash
/home/user/ledgerita_core/                              # Código fuente
/home/user/ledgerita_core/target/release/ledgerita_core # Binario nodo
/home/user/ledgerita_core/target/release/monitor         # Binario monitor
~/.ledgerita/wallet.key                                  # Wallet encriptada (NUEVA)
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

# Monitor (nodo DETENIDO)
./target/release/monitor --data-dir ~/.ledgerita

# Tests
cd /home/user/ledgerita_core
source ~/.cargo/env
cargo test --release
```

---

## 9. Cómo Desplegar Cambios

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
tar cf ledgerita_core.tar --exclude="target" --exclude="local_deps" -C "C:\Users\user\Documents\job\Ledgerita" ledgerita_core

# 3A. Subir a kettle03 (SCP)
scp -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" ledgerita_core.tar kettle03:~

# 3B. Subir a kettle02 (SFTP)
echo "put ledgerita_core.tar" > upload_sftp.txt
sftp -b upload_sftp.txt -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" kettle02:
Remove-Item upload_sftp.txt

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
ssh -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" kettle03 "echo 'clavefacil' | sudo -S systemctl restart ledgerita-core"
ssh -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" kettle02 "echo 'clavefacil' | sudo -S systemctl restart ledgerita-core"
```

### Notas importantes para deploy:
- **Siempre eliminar** `.cargo/config.toml` después de extraer tar en servidores (referencia a `local_deps/` que no existe)
- **Contraseña sudo** es `clavefacil` (usar `echo 'clavefacil' | sudo -S comando`)
- **SCP a kettle02** es inestable para archivos >10KB — usar SFTP
- **kettle03** SCP funciona bien

---

## 10. Decisiones Técnicas Clave

1. **Puerto fijo vs aleatorio**: Elegimos puerto fijo (`--listen-port`) porque la seguridad por oscuridad no aporta nada en LAN, y el costo operativo de puerto aleatorio (no poder conectar confiablemente tras reinicios) es alto.

2. **Contraseña en EnvironmentFile**: `/etc/ledgerita/env` con permisos 600, fuera del service file. La contraseña nunca aparece en `ps aux` ni en logs de systemd.

3. **kettle03 como semilla**: Corre en puerto 36565, escucha conexiones entrantes. Es el punto de anclaje de la red.

4. **kettle02 como cliente**: Se conecta automáticamente a kettle03 via `--connect`. No necesita puerto fijo (solo hace conexiones salientes).

5. **Wallets nuevos**: Se generaron wallets frescos porque la contraseña anterior era desconocida. Los wallets se guardan en `wallet_recovery.env`.

6. **Identidad unificada**: Una semilla de 32 bytes deriva tanto `SigningKey` (wallet) como `Keypair` (P2P PeerId). Esto significa que la dirección de wallet y el PeerId del nodo están vinculados criptográficamente.

7. **Sin serde**: Serialización manual en `serialize.rs` para control total sobre wire format y cero dependencias de macros.

---

## 11. Problemas Conocidos

### Operativos
1. **SCP a kettle02**: Inestable para archivos >10KB. Usar SFTP con batch file.
2. **Conexión a kettle02**: Timeouts intermitentes. Usar `ConnectTimeout=30`.
3. **`sudo` sin TTY**: Los comandos sudo requieren `echo 'clavefacil' | sudo -S` cuando se ejecutan via SSH sin terminal interactivo.
4. **`.cargo/config.toml` con `local_deps`**: El tar incluye este archivo. Siempre eliminar después de extraer en servidores: `rm -f ledgerita_core/.cargo/config.toml`.

### Técnicos
5. **Monitor requiere nodo detenido**: Sled solo permite un writer a la vez. El monitor debe ejecutarse con el nodo **detenido**.
6. **Build local en Windows**: Usa `.cargo/config.toml` con `local_deps/` regenerado vía `cargo vendor`. Si `cargo vendor` falla por rutas largas, comentar el config y compilar directo de crates.io.
7. **Wallet nuevos = 0 saldo**: Los nuevos wallets no tienen LIT. Usar `--mint` para crear fondos de prueba del Tesoro Génesis.

---

## 12. Pendientes Priorizados

### 🔴 Alta Prioridad

| # | Tarea | Descripción | Archivos |
|---|---|---|---|
| 1 | **Monitor remoto P2P** | Auditoría de nodos SIN detenerlos. Se conecta como peer, pide snapshot via `MSG_SYNC`, verifica: (a) supply <= 21M, (b) state root coincide, (c) fee pool consistente, (d) cross-validation entre peers. | `bin/remote_monitor.rs` (nuevo), `network.rs` |
| 2 | **Tests de integración P2P** | Test que inicia dos nodos en memoria, conecta, envía TX, verifica que el segundo recibe y aplica. | `tests/` (nuevo) |

### 🟡 Media Prioridad

| # | Tarea | Descripción | Archivos |
|---|---|---|---|
| 3 | **Múltiples --connect** | Permitir que un nodo se conecte a varios peers (actualmente el flag --connect se puede repetir en CLI pero no en el service file fácilmente). | `main.rs`, `network.rs` |
| 4 | **Reducir ruido en logs** | El log de "Respuesta enviada" se imprime para cada mensaje. Demasiado verbose para producción. | `network.rs` |

### 🟢 Baja Prioridad

| # | Tarea | Descripción | Archivos |
|---|---|---|---|
| 5 | **TestNet formal** | Después de estabilizar producción. Requiere: chain ID / network magic en handshake P2P, génesis alternativo con faucet. | Nuevos módulos |
| 6 | **Systemd: logging size** | Configurar `SystemMaxUse=50M` en journald para evitar que logs crezcan infinito. | `/etc/systemd/journald.conf` |
| 7 | **Script `setup.ps1`** | Script que ejecuta `cargo vendor` automáticamente para builds locales. | `setup.ps1` |

---

## 13. Arquitectura Completa del Protocolo

```
                    ┌──────────────────────┐
                    │      main.rs         │
                    │  CLI + Wallet load   │
                    │  --listen-port N     │
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
       └──────────┘  └─────┬──────┘  └──────────────┘
                           │
                           ▼
                    ┌──────────────┐
                    │   state.rs   │
                    │  BTreeMap    │
                    │  balances    │
                    │  nonces      │
                    │  fee_pool    │
                    │  epoch       │
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

### Identidad Unificada
```
semilla 32 bytes (getrandom) → derive_from_seed()
  ├── ed25519_dalek::SigningKey  → Wallet (firmar transacciones)
  └── libp2p::identity::Keypair  → Nodo (PeerId P2P)
```

---

## 14. Formato de Red (Wire Protocol)

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

## 15. Tests

**29 tests pasan en todos los entornos (local + servidores):**
- crypto: 7 tests
- transaction: 2 tests
- state: 8 tests
- serialize: 4 tests
- storage: 4 tests
- network: 1 test
- monitor: 2 tests
- doc-tests: 2

```bash
# Ejecutar tests
cd ledgerita_core
cargo test --release
```

---

*Fin del reporte V2. ¡Ha sido una sesión espectacular! Nos vemos en la próxima para construir el monitor remoto P2P. 🚀*

*Recuerda leer `wallet_recovery.env` antes de empezar la siguiente sesión — contiene las credenciales de los wallets actuales.*
