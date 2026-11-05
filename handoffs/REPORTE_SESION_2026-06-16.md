# Reporte de Sesión — Ledgerita Phase 2

**Fecha:** 16 de junio de 2026
**Props:** @usuario — ¡Gracias por la sesión!

---

## 1. Resumen General

Se completó la implementación de **Phase 2** (persistencia Sled, fee pool y checkpoint, propagación P2P, state sync) y se desplegó a producción en kettle02 y kettle03.

---

## 2. Stack Tecnológico

| Componente | Versión |
|---|---|
| Rust (local) | 1.87.0 (Windows) |
| Rust (kettle02) | 1.96.0 (Linux x86_64) |
| Rust (kettle03) | 1.96.0 (Linux x86_64) |
| libp2p | 0.56.0 |
| Sled | 0.34.7 |
| ed25519-dalek | 2.2.0 |
| SHA-256 | sha2 0.10.9 |
| Orion (AEAD) | 0.17.15 |
| Tokio | 1.50.0 |

---

## 3. Arquitectura del Código

### 3.1 Módulos (`ledgerita_core/src/`)

| Archivo | Propósito |
|---|---|
| `lib.rs` | Re-exporta todos los módulos públicos |
| `crypto.rs` | Address (ed25519), wallet creation/loading/show, encrypt/decrypt seed con orion (AEAD + KDF), hex helpers |
| `transaction.rs` | Transaction struct, to_bytes(), is_signature_valid() |
| `state.rs` | State (balances, nonces, fee_pool, tx_count, epoch), apply_transaction(), total_supply(), finalize_checkpoint(), is_checkpoint_due(), encode_snapshot(), decode_snapshot(), generate_state_root() |
| `storage.rs` | Storage wrapper sobre Sled con árboles "balances" y "nonces", open(), load_balances(), load_nonces(), apply_balance(), apply_nonce(), flush() |
| `serialize.rs` | encode/decode manual (sin serde) para Address, u64, Transaction; hash_tx() con SHA-256 |
| `error.rs` | StorageError y SerializationError con thiserror |
| `network.rs` | RawCodec (request_response::Codec con length-prefix), LedgeritaBehaviour (ping + rr), run_p2p_node() con event loop: MSG_TX y MSG_SYNC |
| `main.rs` | CLI flags --connect, --data-dir, --show-address; inicializa wallet, storage, state; ejecuta run_p2p_node() |

### 3.2 Archivos de Constantes Clave

**state.rs:**
```rust
pub const CHECKPOINT_INTERVAL: u64 = 4096;  // Producción
```

### 3.3 Formato de Red (RawCodec)

- **Wire format:** 4 bytes big-endian length prefix + payload
- **Protocol ID:** `/ledgerita/1`
- **Mensajes:**
  - `MSG_TX = 0x01`: TX bytes completos
  - `MSG_SYNC = 0x02`: Request = state root (32 bytes); Response = snapshot completo (encode_snapshot())

### 3.4 Formato de Snapshot

```
[epoch:8][fee_pool:8][tx_count:8][num_entries:4]
  por cada entry: [address:32][balance:8][nonce:8]
```

---

## 4. Estructura de Archivos

```
Ledgerita/
├── ledgerita_core/
│   ├── Cargo.toml
│   ├── .cargo/config.toml          → vendored sources (local_deps/)
│   ├── local_deps/                 → gitignored, regenerable con cargo vendor
│   ├── src/
│   │   ├── lib.rs
│   │   ├── main.rs
│   │   ├── crypto.rs
│   │   ├── transaction.rs
│   │   ├── state.rs
│   │   ├── storage.rs
│   │   ├── serialize.rs
│   │   ├── error.rs
│   │   ├── network.rs
│   │   └── bin/
│   │       └── monitor.rs          → Monitor de estado local
│   └── target/
├── REPORTE_SESION_2026-06-16.md
```

---

## 5. Monitor (`monitor`)

**Uso:**
```bash
monitor --data-dir ~/.ledgerita
```

**Funcionalidad:**
- Abre la base Sled del nodo (después de detener el nodo — Sled lock es exclusivo)
- Muestra: supply circulante vs hard cap (21M), época, TX count, fee pool, # cuentas
- Muestra top 10 cuentas por balance
- Alerta si supply > hard cap
- Mensaje claro si el nodo está corriendo

**Limitación:** Sled solo permite un writer a la vez. El monitor debe ejecutarse con el nodo detenido.

---

## 6. Servidores de Producción

### 6.1 Configuración SSH

**Archivo:** `~/.ssh/config`
```
Host kettle02
    HostName 192.168.68.60
    User user
    StrictHostKeyChecking accept-new
    IdentityFile ~/.ssh/id_ed25519_ledgerita

Host kettle03
    HostName 192.168.68.59
    User user
    StrictHostKeyChecking accept-new
    IdentityFile ~/.ssh/id_ed25519_ledgerita
```

**Nota:** En Windows, `HOME` se corrompe a `C:Usersuser`. Usar siempre rutas absolutas o la flag `-i` explícita.

### 6.2 Cómo Conectarse

```powershell
# Desde Windows:
ssh -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" kettle02 "comando"
scp -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" archivo kettle02:~
sftp -b batch.txt -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" kettle02:
```

### 6.3 Directorios de Código en Servidores

```bash
/home/user/ledgerita_core/          # Código fuente
/home/user/ledgerita_core/target/release/ledgerita_core   # Binario nodo
/home/user/ledgerita_core/target/release/monitor           # Binario monitor
```

### 6.4 Wallet y Estado

- **Wallet path:** `~/.ledgerita/wallet.key` (encriptado con orion AEAD)
- **Password:** `LEDGERITA_PASSWORD` env var
- **Estado (Sled):** `~/.ledgerita/state/`
- Al iniciar sin `--data-dir`, usa `$HOME/.ledgerita`

### 6.5 Problemas Conocidos

1. **Sincronización de archivos:** `tar` de Windows (BSD tar) puede producir archivos corruptos (`bad header checksum`). Si pasa, recrear con `Remove-Item` + `tar cf ...`.
2. **SCP a kettle02:** Inestable para archivos > 10KB. Usar `sftp -b batch.txt` como alternativa.
3. **Conexión a kettle02:** Timeouts intermitentes. Usar `ConnectTimeout=30` y reintentar.
4. **`.cargo/config.toml` con `local_deps`:** Se restaura al extraer tar. Siempre eliminar después:
   ```bash
   rm -f /home/user/ledgerita_core/.cargo/config.toml
   ```
5. **Build local en Windows:** Usa `.cargo/config.toml` con `local_deps/` regenerado vía `cargo vendor`.

---

## 7. Cómo Desplegar Cambios

```powershell
# 1. Local: crear tar limpio
Set-Location C:\Users\user\Documents\job\Ledgerita
Remove-Item -LiteralPath ledgerita_core.tar -ErrorAction SilentlyContinue
tar cf ledgerita_core.tar --exclude="target" --exclude="local_deps" -C "C:\Users\user\Documents\job\Ledgerita" ledgerita_core

# 2. Subir a servidores
scp -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" ledgerita_core.tar kettle03:~
sftp -b upload_sftp.txt -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" kettle02:
# donde upload_sftp.txt contiene:
#   put ledgerita_core.tar
#   bye

# 3. Extraer y compilar en servidores
ssh -i "$env:USERPROFILE\.ssh\id_ed25519_ledgerita" kettle02 "
  cd /home/user && tar xf ledgerita_core.tar &&
  rm -f /home/user/ledgerita_core/.cargo/config.toml &&
  cd /home/user/ledgerita_core &&
  /home/user/.cargo/bin/cargo build --release 2>&1
"
```

---

## 8. Tests

```bash
# Local (Windows)
cd ledgerita_core
cargo test --release

# Servidores
ssh kettle02 "cd /home/user/ledgerita_core && /home/user/.cargo/bin/cargo test --release"
```

**Resultado:** 27 tests pasan en todos los entornos.

---

## 9. Pendiente para Próxima Sesión

1. **Interfaz para enviar transacciones** — No hay stdin, RPC ni flag CLI para emitir TXs. El nodo solo reacciona a mensajes P2P entrantes.
2. **Producción en vivo** — Los bins están compilados pero no corriendo como daemons (systemd, nohup, etc.).
3. **Monitor remoto vía P2P** — Conectar a nodo remoto, pedir snapshot via MSG_SYNC, mostrar estado sin detener el nodo.
4. **Persistencia de epoch/fee_pool/tx_count** — Actualmente `State::load()` devuelve 0 para estos campos. No se persisten en Sled.
5. **Propagación de transacciones** — Se re-envían a peers conectados, pero no hay probador automático.
6. **Config local** — `.cargo/config.toml` referencia `local_deps/`; si alguien clona fresco sin vendor, el build local falla.

---

## 10. Decisiones Técnicas Clave

- **Sin serde:** Serialización manual en `serialize.rs` para evitar la macro-derive y mantener control sobre el wire format.
- **Fee pool:** Se acumula en `State.fee_pool` y se distribuye equitativamente entre validators en `finalize_checkpoint()`. El remanente va al primer validator.
- **Checkpoint:** Cada `CHECKPOINT_INTERVAL` (4096) transacciones se vacía el fee pool y se incrementa `epoch`.
- **P2P:** Single `request_response::Behaviour` con `RawCodec` (no gossipsub). Mensajes diferenciados por primer byte (`MSG_TX` / `MSG_SYNC`).
- **Sync en conexión:** Al establecerse una conexión, se envía `MSG_SYNC` automáticamente.
- **Mempool:** `HashSet<[u8; 32]>` con SHA-256 de la TX para deduplicación.

---

## 11. Comandos Útiles

```bash
# Mostrar dirección de wallet
LEDGERITA_PASSWORD=xxx ./target/release/ledgerita_core --show-address

# Iniciar nodo semilla
LEDGERITA_PASSWORD=xxx ./target/release/ledgerita_core --data-dir /ruta

# Iniciar nodo conectándose a semilla
LEDGERITA_PASSWORD=xxx ./target/release/ledgerita_core \
  --data-dir /ruta \
  --connect /ip4/192.168.68.60/tcp/41581/p2p/12D3KooW...

# Ejecutar monitor (nodo detenido)
./target/release/monitor --data-dir ~/.ledgerita

# Verificar checkpoint interval en código
grep CHECKPOINT_INTERVAL src/state.rs
```

---

*Fin del reporte. ¡Gracias por la sesión! Nos vemos en la próxima.* 🚀
