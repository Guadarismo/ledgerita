# DISEÑO: Defensa de Red (B + C + G)

> **Estado: v1 APROBADO — 2026-11-06 (decisiones de §5 cerradas). Sin código
> todavía**, como pide el plan (`HANDOFF.md` → unidad 4: "documento de diseño
> ANTES de código").
>
> Alcance: paquete único de tres piezas de la deuda técnica:
> - **G.** Gate de invariantes en la adopción de snapshots (el hallazgo crítico).
> - **B.** Baneo node-local de peers.
> - **C.** Registro de reputación ("Prueba de Tiempo") — información para el
>   selector de fees de la deuda A (3b), NO entrada al consenso en v1.
>
> Principios de `inicio.md` que gobiernan este diseño:
> 1. **Zero-trust**: cada nodo valida todo por sí mismo (wallet-node incluida).
> 2. **Determinismo**: NUNCA entra al estado de consenso dato node-local
>    (lección inscrita por el hallazgo A — Checkpoint 5).
> 3. **Defensa node-local**: en v1 no hay gossip de bans ni reputación
>    gossiped — cada nodo es la guardia de sus propios ojos.
> 4. Fee_pool estacionado (park 2026-11-05) es consenso: cualquier regla que
>    dependa de él debe valerse de invariantes deterministas.

---

## 1. G — Gate de invariantes antes de ADOPTAR snapshots (Prioridad 1)

### El ataque viable hoy

`state.rs::merge_if_fresher` adopta cualquier snapshot con `(epoch, tx_count)`
mayores **sin validar nada del contenido**. Un peer malicioso puede fabricar
un snapshot con saldos gigantes, supply inflado, nonces retrocedidos… y el
nodo ingenuo lo traga y lo replica en su próximo `MSG_SYNC` (viral).

### El gate (función pura de local + snapshot, cero aleatoriedad)

Nueva función en `state.rs`: `validate_incoming_snapshot(&self, remote: &State)
-> Result<(), GateReason>` — llamada por `merge_if_fresher` en la rama de
**ADOPCIÓN** (snapshot más fresco que el local). Invariantes aprobadas (v1):

| # | Invariante | Regla | Razonamiento |
|---|---|---|---|
| g1 | Hard cap absoluto | suma CHECKEADA de `remote.balances` ≤ `HARD_CAP_SAT` | 21M LIT es constitucional; `checked_add` para que un mentiroso con balances ~u64::MAX no colgue el nodo con overflow |
| g2 | Rango de época coherente | `remote.epoch * 4096 <= remote.tx_count <= (remote.epoch+1) * 4096` | el epoch SOLO avanza con un checkpoint (cada 4096 TX); cota superior INCLUSIVA: `tx_count == (epoch+1)*4096` es la ventana transitoria "checkpoint due, aún no finalizado" |
| g3 | Nonces monótonicos | para toda dirección en `remote.nonces ∩ local.nonces`: `remote.nonce >= local.nonce` | los nonces solo crecen con las TXs; retrocederlos es reescritura de historia |
| g4 | Fee_pool monotónico bajo park | si `local.tx_count > 0`: `remote.fee_pool >= local.fee_pool` | con park vigente, el fee_pool NO pisa ni se distribuye; los fees se acumulan idénticamente en toda la red → un snapshot "más fresco" con pool menor es mentira (invariante que ganamos gratis con la decisión park del Checkpoint 5) |
| g5 | Cuentas plausibles (débil) | `\|remote.balances − local.balances\| <= remote.tx_count + \|local.balances\| + 1` | toda cuenta desconocida para el local debió nacer de ≥1 TX/mint del snapshot; la holgura +1 es la cuenta de génesis compartida (fee_pool/tesoro determinista, NO cuenta como TX). El borrador usaba `tx_count + 1` absoluto, pero castigaría génesis de varios minteos; la forma relativa caza igual al mentiroso (N cuentas falsas con pocas TX) |

**Contrapeso muerto pensado (y descartado):** "supply no crece nunca por
sync" NO sirve: los faucets legítimos del operador agrandan el supply en un
nodo y se PROPAGAN legítimamente por sync. Por eso g4 se fija en el
fee_pool (determinista entre nodos) y el hard cap absoluto (g1), no en la
dirección del supply.

**Refinamientos de implementación (Checkpoint 11):**
1. El gate corre SOLO en la rama de adopción (`remote_fresh > local_fresh`).
   Un snapshot atrasado pero honesto (nonces/supply menores) no es mentira:
   no se adopta por frescura, y castigarlo con strikes banearía peers
   legítimos rezagados que se alejaron temporalmente. La rama `Divergent`
   (misma frescura, root distinto) tampoco corre el gate: esa divergencia
   es el problema de consenso aparte, con su propio WARN.
2. g2 lleva cota superior inclusiva (ventana transitoria de checkpoint
   pendiente) y g5 pasó de la fórmula absoluta del borrador a la relativa
   de la tabla (los génesis con varios minteos son legítimos).

### Qué hacer al rechazar

1. **NO adoptar** (obvio) y **NO reenviar** ese snapshot a terceros.
2. `WARN` con PeerId + `GateReason` (diagnóstico: cuál invariante).
3. Contar el evento al PeerGuard (§2): snapshot inválido por gate = el
   evento más grave (intenta estafar el estado), un solo strike pesado.
4. La reputación del peer cae (§3); con el umbral → ban.

El rechazo del snapshot NUNCA toca el estado local: queda exactamente como
estaba, y en el próximo sync (30 s) pedirá a otro nodo / el mismo de nuevo.
Un nodo honesto no se pierde nada; un atacante pierde su tiempo.

### Tests de la unidad (TODOS en verde, Checkpoint 11)

En `ledgerita_core/src/state.rs`:
- `test_snapshot_mentiroso_supply_inflado_rejectado` (g1) ✔
- `test_snapshot_supply_desbordado_rejectado` (g1 con overflow de suma,
  añadido en la implementación: checked_add, sin panic) ✔
- `test_snapshot_epoch_mentiroso_rejectado` (g2: epoch=3 con tx_count=100) ✔
- `test_snapshot_epoch_probado_aceptado` (g2 lado bueno: ventana inclusiva
  del checkpoint pendiente) ✔
- `test_snapshot_nonces_retrocedidos_rejectado` (g3) ✔
- `test_snapshot_fee_pool_reducido_rejectado` (g4: tx_count mayor con pool menor) ✔
- `test_snapshot_cuentas_gratis_rejectado` (g5: 9 cuentas nuevas con 3 TX,
  1 dirección compartida con el local) ✔
- `test_snapshot_legitimo_aceptado` (génesis + faucet + TX con fee → adopción) ✔
- `test_gate_no_toca_estado_local_al_rechazar` ✔

---

## 2. B — PeerGuard: baneo node-local (Prioridad 2)

### Lugar

Nueva unidad **`ledgerita_core/src/defense.rs`** (pública, sin estado global):
`PeerGuard`, una estructura por nodo instanciada en el loop de red (core) y
en el actor de red (wallet). Nada de nuevo consenso: los bans viven en
memoria del proceso y **nunca** en el estado ni en el state root.

```rust
pub struct PeerGuard {
    scores: HashMap<PeerId, PeerScore>, // HashMap: node-local, el orden NO importa
}
pub struct PeerScore {
    strikes: VecDeque<(Instant, StrikeKind)>, // ventana rodante
    banned_until: Option<Instant>,
    ban_count: u32,                            // para el backoff exponencial
    conectado_desde: Option<Instant>,          // semilla de C (Prueba de Tiempo)
}
enum StrikeKind { TxInvalid, SnapshotInvalido, ChainIdSpam, RequestSpam }
```

### Reglas de strike (constantes ajustables al tope del archivo)

| Evento | Strike | Efecto tras el umbral |
|---|---|---|
| TX inválida (decode/firma/apply interno) | 1 leve | 3 leves en 10 min ⇒ ban 30 min |
| Mensaje con chain_id / tipo desconocido | 1 leve (rate-based: >20 en 5 min ⇒ 1 grave) | ídem |
| **Snapshot rechazado por gate G** | 1 grave | 1 grave ⇒ **ban inmediato 60 min** (intenta inflar el estado) |
| Flood de requests/response-vacíos | 1 grave | ban 10 min |

- Backoff: cada ban repetido del mismo peer se extiende `min(30min * 2^(n-1), 24h)`.
- Expiración: el peer recobra el dial derecho; los strikes expiran con la
  ventana (10 min) sin dejar cicatriz permanente — la "prisión" es el ban,
  no la reputación eterna (eso es asunto de C).

### Acciones al estar baneado

- `dial(baneado)` → skip con `WARN` (para no patearnos a nosotros mismos en
  el retry de 30 s).
- Requests entrantes del baneado → respuesta mínima y NO cuesta CPU/E/S
  (nada de decode ni apply); `BADNET`/`EMPTY` cortesía.
- NO se desconecta de un golpe: la conexión activa se deja cerrar naturalmente
  (evitar churn), pero ningún request de un baneado toca el estado.

---

## 3. C — Reputación node-local: Prueba de Tiempo (Prioridad 3)

### Qué es (y qué NO es)

La visión de `inicio.md`: los fees van "a los validadores que mantienen
viva la red" — demostrados estables y honestos. La **Prueba de Tiempo** aquí
es la evidencia acumulada de ESTABILIDAD + HONESTIDAD por peer, medida
localmente por cada nodo durante la convivencia (la pieza B ya lleva la
honestidad: strikes negativos):

| Métrica (por PeerScore) | Fuente |
|---|---|
| `antiguedad`: tiempo desde la PRIMERA conexión establecida | `conectado_desde` persistente (tras re-conexiones se conserva la PRIMERA por clave PeerId en el tree `peers`, node-local storage) |
| `uptime_ratio`: pings OK / pings enviados (ventana rodante 1 h) | eventos `ping::Event` que ya llegan al behaviour |
| `contribuciones`: TX válidas aceptadas + snapshots servidos correctos | esos accepts ya existen en los handlers |

### Para qué sirve AHORA (v1) y para qué NO

- **Sí:** prioridad de dial (peers antiguos primero), tolerancia de
  strikes antes del ban (peer con antigüedad + uptime alto merece 1 strike
  más de margen — el ban sigue siendo por repetición, jamás por sospecha),
  y base de datos para el selector definitivo de la deuda A (3b).
- **NO:** el score JAMÁS entra al `State`/state root/selector de fee
  recipients como dato del score local. La lección del hallazgo A es ley:
  el reparto de fees DEBE ser función pura del estado pre-checkpoint de
  TODOS los nodos. La pregunta "¿a qué direcciones le toca?" se resolverá
  en el diseño de 3b con reglas derivables del estado público (y la
  variante "burn parcial" a la espera ahí); la reputación local informa
  QUIÉN se permite ayudar a la red (conexiones), no QUIÉN cobra.

### Storage node-local (futurible)

Nuevo tree `peers` en la `Storage` existente (`PeerId -> (primera_conexion,
contadores)`), a baja frecuencia (flush junto con cada checkpoint). TODO es
node-local → cero riesgo de divergencia por construcción (NO vive en el
snapshot de consenso, nunca viaje por `MSG_SYNC`).

---

## 4. Orden de implementación (dentro de la unidad)

1. **G primero** — es la única que bloquea un ataque activo hoy: gate en
   `state.rs` + tests + wiring mínimo (log + strike al PeerGuard).
2. **B segundo** — `defense.rs` común para `network.rs` (core) y
   `net_actor.rs` (wallet): misma regla de juego en ambos lados.
3. **C tercero** — métricas y tablas del PeerGuard; storage `peers` al final.
4. **Redeploy core** (deploy.ps1 mainnet+testnet, kettle03/kettle02) tras
   G+B en verde: los binarios de producción de agosto aún no cumplen el
   park de fees.

## 5. Decisiones (cerradas 2026-11-06 con el usuario)

1. **Ban inmediato al primer snapshot rechazado por gate G** (60 min) — no
   3-strikes: intentar inflar el estado no es ruido de conexión, es
   adulterio. ✔ DECIDIDO.
2. **El gate G aplica en ambos lados** (demonios core Y wallet-node): mismo
   juego, defense en ambos lados; la wallet ya tiene los PeerViews para
   mostrar el evento 🚨 en el dashboard. ✔ DECIDIDO.
3. **Umbrales como constantes** con los defaults de la tabla §2 al tope del
   archivo; flags del CLI solo más tarde si un caso operativo real los
   exige. ✔ DECIDIDO.

## 6. Fuera de alcance v1 (para no inflar la unidad)

- Gossip de bans/reputación entre nodos (protocolo nuevo completo, v0.2+).
- Filtrado de snapshots por firma de nodo (no existe aún firma de nodo
  distinta de la TX; fork de protocolo).
- Rotación de validadores / stake: Fase 3+, tras 3b.
- Deuda conocida referida al margen: el test flaky
  `test_temp_dirs_with_labels` se investiga en unidad separada.
