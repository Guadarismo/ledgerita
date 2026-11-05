# Reporte de Sesión — Ledgerita (Fix broadcast P2P)

**Fecha:** 2026-06-18  
**Proyecto:** Ledgerita — blockchain educativa en Rust (edition 2024)  
**Stack:** libp2p 0.56, sled, ed25519-dalek, sin serde (serialización manual)

---

## Lo que se hizo

### Fix: Orden de broadcast de TXs pendientes en P2P

**Problema detectado:**
En `network.rs:run_p2p_node`, cuando se establecía una conexión entrante/saliente (`ConnectionEstablished`), el código hacía:
1. Broadcast de `pending_broadcast` (TXs locales) a **todos** los peers conectados
2. Luego enviaba `MSG_SYNC` al peer remoto

Esto causaba que el peer remoto recibiera la TX primero, la aplicara, y luego al recibir el snapshot vía MSG_SYNC, el estado ya incluyera la TX → el peer veía `nonce` repetido y la rechazaba como **replay attack**.

**Solución implementada:**

Se movió el broadcast de TXs pendientes para que ocurra **después** de completar el handshake de sync, no antes:

1. **`pending_broadcast`** pasó de ser variable local (`Vec<Vec<u8>>`) a `Arc<Mutex<Vec<Vec<u8>>>>` compartido, permitiendo su acceso desde múltiples puntos del event loop.

2. **`ConnectionEstablished`** ahora solo envía `MSG_SYNC` (el root) — sin broadcast.

3. El broadcast se dispara en dos lugares:
   - **MSG_SYNC request handler** (al recibir petición de sync de un peer): después de responder con el snapshot, se hace broadcast de las TXs pendientes a **todos** los peers conectados.
   - **MSG_SYNC response handler** (al recibir respuesta de sync de un peer): después de adoptar el estado remoto o confirmar que el local es más fresco, se hace broadcast a **todos** los peers.

**Archivos modificados:**
- `ledgerita_core/src/network.rs` — Lógica del broadcast movida a los sync handlers
- `ledgerita_core/src/main.rs` — Creación del `Arc<Mutex<Vec<Vec<u8>>>>` y pase a `run_p2p_node`

**Resultado de tests:**
```
38 passed; 0 failed
  - 27 tests de unidad (lib)
  - 2 tests de monitor
  - 8 tests de remote_monitor
  - 1 test de integración P2P
```

---

## Próximos pasos (pendientes por prioridad)

### 🔴 Alta Prioridad
1. **Logging profesional** — Reemplazar `eprintln!` con `tracing` o `log` para debug controlado en producción
2. **Cross-validation en `remote_monitor`** — Tests para `check_supply_limit`, `cross_validate_roots`
3. **Output `--json` en `remote_monitor`** — Para integración con herramientas externas

### 🟡 Media Prioridad
4. **Test de stress P2P** — Múltiples TXs concurrentes entre N nodos
5. **Test de fork/resolución** — Simular dos nodos con estados divergentes y verificar sync por freshness
6. **Test de reconexión** — Nodo B se desconecta y reconecta, verificar resincronización
7. **Refactor test de integración** — Extraer helpers (`build_swarm`, `wait_for_listen_addr`, `sync_state`) para reuso

### 🟢 Baja Prioridad
8. **Documentación** — Comentarios en `RawCodec`, `State::apply_transaction`, `network::run_p2p_node`
9. **TestNet formal** — Chain ID / network magic en handshake P2P, génesis alternativo con faucet
10. **Setup script `local_deps/`** — Script que ejecute `cargo vendor` automáticamente
