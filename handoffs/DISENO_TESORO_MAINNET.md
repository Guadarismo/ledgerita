# DISEÑO — Tesoro MainNet: goteo estado-derived + burn de fees (Deuda 3b / Deuda E)

> Estado: BORRADOR v1 para decisión del usuario · Sesión 2026-11-07
> Origen: HANDOFF ítem 5 (Deuda E) + nota "revisar en 3b" del gate g5 (`state.rs` L205)
> + reporte de deuda A ("el reparto a validadores NO existe" — el gap más serio vs la visión).

---

## 1. Lo que dice `inicio.md` (la brújula)

| Disposición de inicio.md | Traducción técnica |
|---|---|
| *"El total de monedas nace protegido matemáticamente en una cuenta 'Tesoro' **sin clave privada**, in-hackeable"* | El tesoro NO puede ser una cuenta (toda cuenta con saldo exige su clave para gastar ⇒ alguien la tiene). Es una **regla de estado**: tesoro virtual, emisión determinista. |
| *"Por época se libera un **micro-goteo del Tesoro** y se reparte **proporcionalmente** entre los nodos que demostraron estabilidad, conexión y honestidad"* | Opción A: emisión por Cierre de Libro + reparto ponderado por **Prueba de Tiempo** (las métricas de la unidad C ya en producción: antigüedad, uptime_ratio, contribuciones de `PeerLocal`). |
| *"Finalizado el goteo, la red se sostiene **redistribuyendo las comisiones** a los validadores"* | El fee_pool NO se quema a perpetuidad: transita a reparto. El burn es TRANSITORIO/imparcial, no el destino final de los fees. |
| *"SIN Halving, Distribución Equitativa"* | Goteo plano (sin curva Mostrar): misma cantidad por época hasta agotar el tesoro. |
| *"Cada 2,000 bloques el Cierre de Libro"* | **DESACUERDO ACEPTADO** (decisión del usuario 2026-11-07): el código usa `CHECKPOINT_INTERVAL = 4096` TX y queda ASÍ (`state.rs` L10). inicio.md se desvía aquí por edad del documento; esta decisión queda registrada como la primera desviación oficial. La unidad mínima 10⁻⁸ vs 10⁻⁶ del código: pendiente de decidir (🎯8, no bloquea). |

## 2. Estado de hoy (hechos verificables)

- **MainNet**: sin génesis; el tesoro de arranque lo mintea el operador con `--mint <addr> <amount>` — supply actual ≈ 10.000095 LIT (mints de pruebas de la era Checkpoint 1–4 + fondeo del faucet TestNet). El poder de mintea está en quien corre el daemon con el flag. *Cualquier* nodo puede mintear localmente y ganar frescura (el gate solo protege el hard cap, g2).
- **TestNet**: tesoro génesis = 1M LIT en la cuenta de seed cero (`genesis.rs` L21-23). La dir es derivable por cualquiera (¡y su CLAVE también: la seed [0u8;32] es pública!) — inofensivo en TestNet, ANTI-patrón para MainNet ya censado (Deuda E).
- **fee_pool**: se acumula en consenso (park, Checkpoint 5) y NO forma parte del supply (`total_supply()` suma solo balances, `state.rs` L135). Los fees pagados YA hoy están fuera de circulación — solo falta decidir si se destruyen (burn) o se reservan para reparto.
- **Gate g5**: allowance fija `+1` justificada informalmente ("la cuenta de génesis no cuenta como TX"). Con tesoro virtual esta justificación se formaliza (⇒ §5).

## 3. Decisión ya tomada por el usuario (2026-11-07)

- ✅ **Opción A es la esencia** (100% fidelidad a inicio.md): emisión 100% estado-derived, tesoro virtual sin clave.
- ✅ **Deflación SI** (pedido explícito): burn de fees como política real, no solo transitoria.
- ✅ **Opción C muere como permiso**: `--mint` queda como herramienta de BOOTSTRAP (TestNet faucet) — en MainNet se desactiva tras esta unidad (con guardia de red).
- ✅ **4096 queda**, desviación del "2000 bloques" de inicio.md registrada (§1).

## 4. Diseño — Tesoro virtual (Opción A)

### 4.1 El tesoro no es una cuenta: es un contador

Nuevo campo de consenso en `State`:

```rust
pub emitido_total: u64,   // LIT ya liberados del tesoro por el goteo
```

- `TESORO_TOTAL = HARD_CAP_SAT = 21_000_000 LIT` (constante, no campo: cap ya existe como g2/invariante).
- **Tesoro disponible** = `HARD_CAP_SAT − emitido_total − total_supply()` — derivado SIEMPRE del estado, jamás almacenado: dos nodos con el mismo estado derivan el mismo tesoro. Nada que robar: no hay clave privada porque no hay cuenta.
- **Regla del goteo**: `goteo(epoca) = min(TESORO_POR_EPOCA, tesoro_disponible_previo_a_esta_emision)` — el último goteo se recorta, y el tesoro muere exactamente en 21M (`total_supply + emitido_total ≤ HARD_CAP_SAT` por construcción).

### 4.2 La emisión SOLO ocurre con reparto activo (folio del goteo)

Cuando el Cierre de Libro corre y el módulo de reparto está activo:

1. Se calcula `goteo` (§5).
2. Se emite a los validadores honestos con el peso de Prueba de Tiempo (peer_i_recibe = goteo × peso_i / Σpesos).
3. `emitido_total += goteo`.

**Si el módulo de reparto no corre o no hay validadores elegibles: NO se emite nada esa época y el tesoro NO pierde su turno** (el disponible es derivado, no un calendario que expira). Esto permite implementar el burn de fees HOY y el goteo EN SU UNIDAD, sin deuda interna de "días faltados".

⚠️ NOTA ANTI-REGLAS-RÍGIDAS: el estado ganará `emitido_total` cuando la unidad de goteo aterrice; antes de eso NO se tocó el state-root de la unidades nuevas — el estado de MainNet/TestNet en producción NO cambia en esta unidad salvo el burn de fees (ver §6).

## 5. Diseño — Burn de fees (deflación, donde toca HOY)

### 5.1 Regla del Cierre (determinista, consenso)

En `finalize_checkpoint()` (Cada 4096 TX):

```rust
// Burn (deflación, decisión usuario 2026-11-07):
self.fees_quemados = self.fees_quemados.saturating_add(self.fee_pool);
self.fee_pool = 0;
// Y en la unidad de goteo (FUTURO además):
// self.emitido_total += goteo; // reparto a los validadores honestados
self.epoch += 1;
```

- `fees_quemados: u64` = acumulación de TODOS los fees destruidos desde el génesis. Vive en el estado (state root) como `fee_pool`, `epoch`, `tx_count`.
- Efecto monetario: `total_supply()` baja POR LOS FEES PAGADOS (el sender ya los perdió de su balance al pagarlos — eso ya es deflactuoso hoy) — y el burn hace esa salida del ledger IRREVERSIBLE y AUDITABLE: los fees dejados de estacionar se destruyen de verdad.
- La historia del burn es AUDITABLE: `fees_quemados` en cada snapshot muestra el total destruido; el delta por época mide la contribución deflacionaria de esa era (grato para reports/monitor).

### 5.2 El gate g4 se rediseña con la pareja (pool + quemados)

Hoy g4 dice *"bajo park, fee_pool es monotónico"* — el burn rompe la monotonicidad pura del pool: en cada Cierre el pool cae a 0 y el quemado sube. Nueva invariante que SÍ es monotónica bajo burn:

```
g4': fees_totales(remote) = remote.fee_pool + remote.fees_quemados
— en un snapshot MÁS fresco (mayor epoch/tx_count que el local):
  fees_totales(remote) ≥ fees_totales(local) — los fees tormentosos
  solo ENTREGAN de dos formas (pool o quemado) y jamás se crean de la nada.
```

- Ventana del g2 aplicada: fees_totales(remote) puede exceder fees_totales(local) a lo sumo por los fees de las hasta `CHECKPOINT_INTERVAL` TX disputadas (misma ventana INCLUSIVA de la cota superior de g2).

  ```rust
  const FEES_MAX_G4: u64 = 20 * 1_000_000; // 20 LIT de fee por TX (🎯: hoy NO existe fee-max en apply_transaction; eliminarlo o formalizarlo en apply es PRERREQUISITO del g4' — sanity cap: fee >> razonable se rechaza como TxInvalid)
  let coste_adicional_max = (remote.tx_count - local.tx_count) * FEES_MAX_G4;
  ```
- Implementación literal con esas constantes + comentario del park (Checkpoint 5) migrado a la nueva semántica.
- NO hay que tocar g2/g3/g5 por el burn.

## 6. Opción C — Bootstrap al modo funeral

- `--mint` sigue existiendo pero con guardia de red: en `NetworkId::Mainnet` DA ERROR (`"La emisión en MainNet es estado-derived: --mint queda solo como bootstrap de TestNet"`). Si algún día se necesita bootstrap MainNet con operador SIDE, será un protocolo propio (fuera de scope; JAMÁS implícito).
- Los ~10 LIT ya minteados en MainNet son HISTORIA INMUTABLE (están en el estado y viajan por consenso): nada cosmético retroactivo; esta unidad documenta esa era como "fase de laboratorio del operador" y el tesoro virtual arranca a partir de ahí.
- TestNet GENESIS sigue con tesoro 1M en seed-cero (es el faucet de la red de pruebas; con emisión propia posterior ese pool vive polución-detached de MainNet).

## 7. Gate g5 — formalización de la holgura (+1 → allowlist génesis)

Hoy: `nuevas_cuentas ≤ remote.tx_count + |local| + 1`.

- El `+1` real era para la cuenta de tesoro génesis de TestNet (nunca nació de una TX): pasa a ser **transparente y por red**: `allowance_g5 = genesis::GENESIS_ACCOUNTS(network)` con mainnet 0, testnet 1.
- En MainNet el resultado es idéntico (0 hoy y después) — cero impacto de producción; en TestNet el cambio es equivalente (sigue siendo un +1, solo que con ORIGEN especificado).

## 8. Goteo — parámetros a decidir (🎯, no bloquea el presente)

Curva (inicio.md: "sin halving, distribución equitativa" → **GOTEO PLANO**):

**🎯 Q1 (curva): plano.** Misma cantidad por época hasta agotar el tesoro virtual. Candidatos:

| Plan | Goteo/época | Épocas para 21M | TXs totales (×4096) |
|---|---|---|---|
| a) agresivo | 1,000 LIT | 21,000 | ~86M |
| b) medio | 500 LIT | 42,000 | ~172M |
| c) micro-goteo largo | 100 LIT | 210,000 | ~860M |

**🎯 Q2 (política del pool tras la unidad de reparto): 50% burn / 50% validadores** — split fijo estado-derived, y nunca poza muerta: si una época no tiene validadores elegibles, esa MITAD queda ESTACIONADA (se añade al pool de la siguiente época, no se quema). La decisión final la tomas cuando aterrice la unidad de reparto — nada de hard-code retroactivo de algo ya quemado.

→ **El burn 100% de HOY no es la política final; es la era PRE-reparto.** Aterrizar la unidad de reparto moverá el split a 50/50 (o lo que decidas entonces) — eso se hace con SU checkpoint, jamás como edit retroactivo de algo ya quemado.

## 9. Plan de implementación (unidad actual — Checkpoint 19)

1. [ ] **core/state.rs**: campo `fees_quemados: u64` (incluido en root hash y serialización/storage meta — el mismo wiring de `fee_pool`: `Storage::load_meta/save_meta` ahora lleva 4 campos).
2. [ ] Burn en `finalize_checkpoint()`: pool → quemados, pool = 0.
3. [ ] g4 → g4' (par monotónico pool+quemados con la ventana del g2; comentario del park migrado a la nueva regla).
4. [ ] g5 formalizado: `GENESIS_ACCOUNTS(network)` en `genesis.rs` (mainnet 0 / testnet 1) y wire al allowance.
5. [ ] `--mint` guardia en MainNet (error claro, mensaje en español).
6. [ ] Tests bloqueantes NUEVOS:
   - burn mueve pool → quemados en el Cierre (invariancia exacta: pool_before + fees_quemados_before == fees_quemados_after, pool_after == 0).
   - root cambia con el nuevo campo (pool+/quemados+ entrará al hash; roots entre nodos siguen idénticos — determinismo probado).
   - g4': monotónico bajo burn en la ventana de frescura, y RECHAZO si fees_totales inflado (unit + property test con fee aleatorio ≤ FEES_MAX_G4).
   - g5 con allowance 0/1 por red.
   - `--mint` en mainnet rechaza (test de integración del CLI).
7. [ ] Regresión completa core+wallet, E2E TestNet en vivo: burn real bajo el monitor (con 4096 TX de carga, el pool TestNet se aterriza a 0 todos los Cierres — **NOTA: el pool TestNet queda en 0 tras cada Cierre hasta que lleguen fees nuevos; el balances dashboard queda intacto**).
8. [ ] Docs: HANDOFF 5 → hecha; nota `Deuda E RESUELTA` al frente del HANDOFF; reporte Checkpoint 19.

### Consecuencia de rollout (aviso honesto): migración de estado

Los nodos existentes MainNet/TestNet producen estado sin el campo nuevo → al arrancar el binario nuevo, `load_meta` con 3 campos mensajeros: `fees_quemados = 0` (default), persistir el formato NUEVO al primer flush. Peers mixtos durante el deploy: el binario viejo root NO incluye quemados → mismatch de root cross-version. **REDEPLOY ATÓMICO: actualizar TODOS los nodos juntos, en ventana breve (patrón deploy.ps1 del Checkpoint 14).** Costo conocido y pequeño (TestNet 2 nodos, MainNet 2 nodos).

## 10. Que NO cambia

- Modelo de cuentas / TX / faucet TestNet node-local de la wallet (Checkpoint 6b).
- Genesis TestNet (1M en seed-cero).
- PeerGuard/Prueba de Tiempo en producción (las métricas siguen recolectándose node-local; el REPARTO del goteo las consumirá en su unidad, hoy no se tocan).
- La unidad mínima 10⁻⁶ (🎯8, decisión aparte — la wallet, faucet e historial ya corren con 6 decimales).

---

## Registro de decisiones (para firmar al aprobar)

| # | Punto | Decisión |
|---|---|---|
| D1 | Opción esencia | A (estado-derived) + B transitoria + C funeraria — ✅ usuario |
| D2 | Deflación | Sí — ✅ usuario ("me gusta deflación") |
| D3 | Interval de Cierre | 4096 TX queda — ✅ usuario ("a dejar cada 4096") |
| D4 | Mecánica explicada (goteo vs burn = dos tuberías; neto: circulante sube con el goteo y decae lento por fees tras agotarlo) | ✅ usuario: "me encanta la idea" (2026-11-07) |
| D5 | Recomendaciones adoptadas | Q1: plano b) 500 LIT/época · Q2: 50% burn / 50% validadores (poza muerta prohibida) · Q8: 10⁻⁶ queda — ✅ usuario (firma "empezamos") |
| 🎯 Q1 | ~~Goteo plano: ¿a/b/c?~~ | ✅ RESUELTO (b) — sus constantes se concretan en la unidad del goteo |
| 🎯 Q2 | ~~Política post-reparto~~ | ✅ RESUELTO (50/50) — se activa en la unidad de reparto |
| 🎯 Q8 | ~~Unidad 10⁻⁶ vs 10⁻⁸~~ | ✅ RESUELTO (10⁻⁶) — nota documental, sin cambios de código |
| 🎯 Q8 | Unidad 10⁻⁶ vs 10⁻⁸ | PENDIENTE (recomendación: quedar en 10⁻⁶ — ya es el std de toda la app) |
