# 📋 Reporte de Sesión — Ledgerita (AUTOCONTENIDO)
**Fecha y hora:** 2026-08-02 11:37 (hora local)
**Ultimo commit:** `545dbb8`
**Tests:** 70 passed, 0 failed (debug y release)
**Estado git:** limpio

> ⚠️ **LEER PRIMERO**: Este reporte es **autocontenido** para retomar en una sesión
> nueva sin contexto previo. La IA no recuerda nada entre sesiones. Contiene
> TODO lo necesario: credenciales, direcciones, PeerIds, traps, comandos y estado actual.

---

## 🎯 Resumen de la sesión (2026-08-02)

Sesión doble: **TestNet formal** (el último pendiente del roadmap) + **revisión con lupa** (auditoría de crypto + backups automáticos).

1. **TestNet formal implementado** (aislamiento MainNet↔TestNet por chain id + génesis con faucet por CLI).
2. **TestNet desplegado y verificado en vivo** en los servidores (seed kettle03 puerto 46565 + cliente kettle02).
3. **Faucet en vivo probado**: 0.005 LIT minteados y propagados entre nodos.
4. **Revisión con lupa**: auditoría de `crypto.rs`/`serialize.rs` → APROBADA (primitivas veteadas, sin crypto casera).
5. **Fix de seguridad**: `wallet.key` de 0644 → **0600** (código + servidores).
6. **Backups automáticos**: script + cron diario 04:15 en ambos servidores, restore test verificado.

---

## 🛠️ Commits de la sesión (en orden)

| Commit | Descripción |
|---|---|
| `2c10848` | TestNet formal: aislamiento por chain id + genesis con faucet + deploy parametrizado por red |
| `51c4f5f` | TestNet desplegado y verificado en vivo + faucet propagado + mainnet intacto + reporte |
| `3413c42` | Revisión con lupa en reporte: riesgos priorizados |
| `545dbb8` | Mini-auditoria OK: wallet.key a 0600 + backups automaticos con cron + restore test |

---

## 🌐 ESTADO ACTUAL DE LA RED (verificado 2026-08-02 ~11:00)

### MainNet (producción, intacta y funcionando)
| Nodo | IP | Rol | Puerto | PeerId | Wallet |
|---|---|---|---|---|---|
| kettle03 | 192.168.68.61 | Seed | 36565 | `12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1` | `b1435c81522ac1e2b0f503986ca93bde92542b5d4cab4b9a9fd21253667e4cba` |
| kettle02 | 192.168.68.64 | Cliente | dinámico | `12D3KooWGrahZ1gW5pFBu9Q1U1QEryPyEUGh8rCGGJJzuuPoHLdu` | `6893239f5850555f5701f24342bfbef5afdab94835f08d6e5f352899d4a17f28` |

- Ledger: supply 10.000095 LIT, tx_count 1, epoch 0, 2 cuentas, state_root `c0198d1fcf725b28`, fee_pool 0.000005 LIT. Snapshot `snap.0000000000000334`.
- Service: `ledgerita-core.service`. Data dir: `~/.ledgerita`. Env: `/etc/ledgerita/env`.

### TestNet (NUEVA, desplegada esta sesión)
| Nodo | Rol | Puerto | PeerId testnet | Wallet testnet |
|---|---|---|---|---|
| kettle03 | Seed | **46565** | `12D3KooWNjCgE866sbqjr2626KEC3MGYKvYuVM6MvkuG5VhHvaaS` | `bfd54b3153d851854b09dcf9f47063abff13e2768d2e486054997853ba67f97f` |
| kettle02 | Cliente | dinámico | `12D3KooWCvE6SWUeqx3j5US3ya3Gs9Mh3yKZLYubxEqxDVtAymit` | `2e136a85a6ca17678637e1e219bc2db601889a7375cfc790594ceb3d1b0138dd` |

- Service: `ledgerita-core-testnet.service`. Data dir: `~/.ledgerita-testnet` (aislado).
- Génesis aplicada en ambos (tesoro faucet 1M LIT). Faucet probado: 0.005 LIT → seed wallet, propagado (`count=2`).
- Sync confirmado: ping ~7-27ms, "Estado ya sincronizado".
- **Los 4 servicios están activos** (mainnet + testnet en ambos hosts).

---

## 🔑 CREDENCIALES Y ACCESOS (CRÍTICO — no se recuerdan entre sesiones)

- **SSH key**: `$env:USERPROFILE\.ssh\id_ed25519_ledgerita`. Hosts aliases en `~/.ssh/config`: `kettle02`, `kettle03`, `celular`.
- **Sudo en servidores**: password `<SUDO_PASSWORD>` → `echo '<SUDO_PASSWORD>' | sudo -S -p '' <cmd>`.
  - ⚠️ En kettle03 el `sudo -n` a veces rechaza → usar sudo con password.
- **Wallet password**: `<VER_SECRETS_LOCAL.md>` (idéntica en mainnet/testnet; está en `/etc/ledgerita/env` y `wallet_recovery.env` gitignored).
- **Env files**: `/etc/ledgerita/env` (root, 600) contiene `LEDGERITA_PASSWORD=...` + `RUST_LOG=info`. **NO debe tener BOM** (rompe parsing de systemd).
- **Sudoers** (`/etc/sudoers.d/ledgerita-deploy`, NOPASSWD): cp service mainnet/testnet, mkdir /etc/ledgerita, tee/chmod env, systemctl daemon-reload, enable/start/restart/stop de `ledgerita-core` Y `ledgerita-core-testnet`, cat env. Referencia local: `C:\Users\user\AppData\Local\Temp\opencode\ledgerita-deploy-sudoers`.

---

## 🔍 AUDITORÍA DE CRIPTO (revisión con lupa — RESULTADO: APROBADO)

**Primitivas VETEADAS, sin crypto casera. No requiere cambios.**
- Firmas: `ed25519-dalek` (ed25519). `Address` = clave pública ed25519 (32 bytes).
- Wallet: `orion` = **Argon2i** (t=3, m=64MiB, salt 16B random) + **XChaCha20-Poly1305** (`aead::seal/open`). Formato: `salt_hex:ciphertext_hex`.
- Seed: `getrandom` (32 bytes). Misma seed → ed25519 `SigningKey` + libp2p `Keypair` (identidad única address=PeerId).
- `serialize.rs`: TX fija 152B = sender(32)+receiver(32)+amount(8)+fee(8)+nonce(8)+sig(64), big-endian, con checks de longitud.
- **Fix aplicado**: `wallet.key` ahora se escribe con **0600** (era 0644 world-readable). En código (`crypto.rs::write_wallet_key`, cfg unix) y `chmod 600` aplicado a los archivos existentes.

### Hallazgos menores (documentados, no bloqueantes)
- `decode_transaction` acepta bytes extra (>152) — inofensivo (firma solo valida el mensaje del remitente), opcional hacerlo estricto.
- Argon2i t=3/m=64MiB son los defaults de Argon2 — aceptable; se puede subir iteraciones en el futuro.
- Consenso = merge por `tx_count` ("gana el que tiene más txs"): **NO es consenso real** (sin PoW/PoS/finalidad). Límite de diseño; no tocar sin plan explícito.
- Caps del faucet son node-local (tree `faucet` fuera del snapshot): acotados por el tesoro en consenso, pero "total dado" puede diferir entre nodos. Límite conocido.

---

## 💾 BACKUPS AUTOMÁTICOS (implementados esta sesión)

- Script: `ledgerita_core/scripts/ledgerita-backup.sh` (commiteado). Instalado en ambos servidores en `/home/user/ledgerita-backup.sh` (chmod 700).
- Qué respalda: `~/.ledgerita/state` + `~/.ledgerita-testnet/state` + `wallet.key` de ambos → tar.gz en `/home/user/backups/ledgerita/` con timestamp.
- **Zero-downtime** (no detiene servicios, no requiere sudo). Consistencia por crash-recovery de sled (una copia a mitad de escritura se repara al restaurar; el backup puede quedar ligeramente atrás = correcto para DR).
- **Cron diario 04:15**: `15 4 * * * /home/user/ledgerita-backup.sh` (crontab de `user` en ambos hosts). Rotación: borra backups >7 días.
- **Restore test verificado** en kettle03: tar extraído a /tmp, `--show-address` descifró wallet → `b1435c81...` (correcto). Permisos 0600 preservados.

---

## 📋 PENDIENTES PARA PRÓXIMA SESIÓN

### Opcional (baja prioridad)
- **Healthcheck de nodos**: comprobar `systemctl is-active ledgerita-core*` + sync con alerta (log/mail). Es lo único de la prioridad 2 que no se hizo.
- **Reset de mainnet** (solo si se decide romper compatibilidad de estado): `sudo systemctl stop ledgerita-core` → `rm -rf /home/user/.ledgerita/state` (en AMBOS nodos) → `sudo systemctl start ledgerita-core`. La génesis nueva solo se aplica en data-dir vacío.
- **Reset de testnet**: `sudo systemctl stop ledgerita-core-testnet` → `rm -rf /home/user/.ledgerita-testnet/state` → `start` (replica génesis).
- **Más faucet testnet**: `ledgerita_core --network testnet --data-dir <dir> --faucet <ADDR> <monto_satoshis>` (nodo detenido; requiere `sudo cat /etc/ledgerita/env` para la password). Para reconectar el cliente si cambia el seed: `.\deploy.ps1 -Network testnet -SeedPeerId <ID>`.
- Exploratorio: docstrings restantes, UX CLI/monitor, ampliación protocolo.

---

## 🧪 TESTS (70 passed, debug y release)

```
Unit tests (lib)         43 passed  (incluye faucet, genesis, network)
monitor                  2 passed
remote_monitor          15 passed
helpers_advanced         4 passed
network_isolation        2 passed
P2P integration          1 passed
P2P stress               1 passed
P2P fork                 1 passed
P2P reconnect            1 passed
```

---

## ⚠️ TRAMPAS CRÍTICAS (no recordadas entre sesiones)

1. **`deploy.ps1` requiere BOM UTF-8** (PowerShell 5.1 lee sin BOM los emojis como ANSI corruptos).
2. **NUNCA ejecutar `deploy.ps1` con `2>&1` o pipe de stderr**: con `$ErrorActionPreference="Stop"`, el stderr de cargo dispara `NativeCommandError`. Ejecutar plano: `.\deploy.ps1 -Network testnet`.
3. **SSH desde PowerShell**: en strings double-quoted, `$(...)` se expande LOCALMENTE. Para comandos con subshells remotos usar variable single-quoted: `$cmd = '...'; ssh -i $key host $cmd`.
4. **Sled lock exclusivo**: `monitor.exe` local y el CLI con `--faucet`/`--mint` requieren el nodo DETENIDO. Tras usar `--faucet`, el proceso queda corriendo como nodo P2P → matarlo (`kill <pid>` / `pkill`) antes de reiniciar el servicio.
5. **`/etc/ledgerita/env`**: root-only (600). Para leerlo como `user`: `eval "$(sudo cat /etc/ledgerita/env)"` (cat está en NOPASSWD). El binary lee `LEDGERITA_PASSWORD` del env.
6. **Tar del deploy**: `tar cf $TarFile -C $ProjectRoot --exclude="ledgerita_core/target" --exclude="ledgerita_core/local_deps" ledgerita_core` (obligatorio `-C` con path relativo, sin trailing slash).
7. **`.cargo/config.toml` en servidores**: borrar tras extraer tar (`rm -f ledgerita_core/.cargo/config.toml`) — referencia `local_deps` que no existe ahí (el deploy ya lo hace).
8. **Framing con magic**: `[chain_id u8][msg_type u8][payload]`. Chain IDs Mainnet `0x01` / Testnet `0x02`. Protocolos `/ledgerita/1/mainnet` y `/ledgerita/1/testnet`. El snapshot lleva chain_id como primer byte. El transporte TCP conecta entre redes, pero la negociación falla → `OutboundFailure` (aislamiento real).
9. **1 LIT = 1,000,000 satoshis**; hard cap 21,000,000 LIT = `21_000_000_000_000` sat. Checkpoint cada 4096 TX. Supply real = `total_supply() + fee_pool`.
10. **Génesis** se aplica solo en data-dir vacío (`should_apply_genesis`). Dirección faucet = seed `[0u8;32]`. Default faucet 1000 LIT; `--faucet` rechazado en mainnet.
11. **`pkill -f` con patrón con espacios falla** ("only one pattern") → usar patrón simple (ej: `pkill -f testnet`) o kill por PID.
12. **journalctl en kettle02 requiere sudo**.
13. **BOM**: `setup-systemd.sh`/service files — copiar con permisos root y sin BOM.

---

## 💻 COMANDOS ÚTILES

```powershell
# Tests completos (debug o release)
cd ledgerita_core; cargo test            # debug
cd ledgerita_core; cargo test --release  # release

# Deploy mainnet / testnet
.\deploy.ps1
.\deploy.ps1 -Network testnet -SeedPeerId 12D3KooWNjCgE866sbqjr2626KEC3MGYKvYuVM6MvkuG5VhHvaaS

# SSH / sudo / logs
ssh -i ~/.ssh/id_ed25519_ledgerita kettle03
echo '<SUDO_PASSWORD>' | sudo -S -p '' systemctl restart ledgerita-core
ssh -i ~/.ssh/id_ed25519_ledgerita kettle03 "echo '<SUDO_PASSWORD>' | sudo -S -p '' journalctl -u ledgerita-core -f"

# Estado de servicios (ambas redes)
ssh -i ~/.ssh/id_ed25519_ledgerita kettle03 "systemctl is-active ledgerita-core ledgerita-core-testnet"
ssh -i ~/.ssh/id_ed25519_ledgerita kettle02 "systemctl is-active ledgerita-core ledgerita-core-testnet"

# Remote monitor mainnet (servicio RUNNING)
.\ledgerita_core\target\release\remote_monitor.exe --connect /ip4/192.168.68.61/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1
# Remote monitor testnet
.\ledgerita_core\target\release\remote_monitor.exe --network testnet --connect /ip4/192.168.68.61/tcp/46565/p2p/12D3KooWNjCgE866sbqjr2626KEC3MGYKvYuVM6MvkuG5VhHvaaS

# Backup manual (en el servidor)
bash /home/user/ledgerita-backup.sh

# Backup/restore test
tar tzf /home/user/backups/ledgerita/ledgerita-20260802-112853.tar.gz
tar xzf <backup>.tar.gz -C /tmp/restore
```

---

## 📁 ESTRUCTURA DEL REPO (relevante)

```
Ledgerita/
├── deploy.ps1                          # Deploy parametrizado (-Network, -SeedPeerId)
├── wallet_recovery.env                 # Credenciales wallet (GITIGNORED)
├── REPORTE_HANDOFF_2026-08-02.md       # Reporte de sesión previo (deltas)
├── REPORTE_SESION_2026-08-02_1137.md   # ESTE reporte (autocontenido)
├── REPORTE_SESION_2026-08-02.md        # Reporte detallado anterior
├── REPORTE_FINAL_SESION_2026-07-03.md  # Histórico
└── ledgerita_core/
    ├── .cargo/config.toml              # Source-replacement → local_deps (SOLO local)
    ├── local_deps/                     # Vendored crates (gitignored, regenerable)
    ├── scripts/
    │   ├── ledgerita-core.service      # systemd mainnet (se preserva en deploy)
    │   ├── ledgerita-core-testnet.service # systemd testnet (seed, 46565)
    │   ├── ledgerita-backup.sh         # NUEVO: backups automáticos (cron 04:15)
    │   ├── setup-systemd.sh
    │   └── vendor-local-deps.ps1
    ├── src/
    │   ├── main.rs                     # CLI flags, génesis, faucet, mint/send
    │   ├── network.rs                  # NetworkId (chain id/magic), P2P
    │   ├── state.rs                    # Máquina de estados, snapshot, merge
    │   ├── crypto.rs                   # ed25519 + wallet encriptada (orion)
    │   ├── serialize.rs                # Serialización manual (sin serde)
    │   ├── storage.rs                  # Sled (trees: balances, nonces, faucet)
    │   ├── transaction.rs
    │   ├── genesis.rs                  # NUEVO: génesis testnet + faucet treasury
    │   └── faucet.rs                   # NUEVO: apply_faucet con caps y hard cap
    ├── src/bin/                        # monitor.rs, remote_monitor.rs
    └── tests/                          # helpers/mod.rs, helpers_advanced.rs, network_isolation.rs, p2p_*.rs
```

---

## 🧠 PARA ENTENDER EL SISTEMA (resumen rápido)

- **Ledgerita**: blockchain de aprendizaje desde cero en Rust (edition 2024). Cuenta = clave pública ed25519. Sin serde (serialización manual). Sled para storage.
- **Transacción** (152B firmada): sender, receiver, amount, fee, nonce (anti-replay). Fees van al fee_pool.
- **Consenso**: los nodos intercambian snapshots completos; gana el estado con más txs (`merge_if_fresher`). Sin PoW/PoS — es replicación de estado con resolución por frescura.
- **P2P**: libp2p, protocolo `/ledgerita/1/<red>`, framing con chain_id. Retry dial cada 30s. TXs pendientes se envían después del sync.
- **TestNet**: misma base de código, génesis distinta (tesoro faucet) y chain_id `0x02`. Faucet mintea del tesoro (no emisión nueva) respetando hard cap 21M y cap por dirección.

---

*Reporte autocontenido generado el 2026-08-02 11:37. Para retomar en nueva sesión SIN pérdida de contexto. ¡Éxito en la próxima sesión! 🚀*
