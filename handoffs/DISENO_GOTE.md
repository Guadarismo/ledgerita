# DISEÑO — GOTE: goteo del tesoro virtual + reparto post-goteo

> Estado: **APROBADO v1** · Sesión 2026-11-08 (D6–D10 firmadas ver §11)
> Anclas: `DISENO_TESORO_MAINNET.md` (D1–D5 FIRMADAS, commit `477c306`),
> Checkpoint 13 (Prueba de Tiempo node-local) y la visión de `inicio.md`
> ("micro-goteo del Tesoro… redistribuyendo las comisiones… SIN Halving,
> Distribución Equitativa").

---

## 1. Lo que está FIRMADO ya (no se re-discute)

| Ley | Origen |
|---|---|
| Emisión 100% estado-derived, tesoro virtual **sin clave** | D1/D2 (3b) |
| Goteo **PLANO** sin halving, 500 LIT/época | D5 (3b) |
| Post-goteo (tesoro agotado): fees **50% burn / 50% validadores** | D5 (3b), decisión final de la unidad |
| **Poza muerta prohibida**: todo fee tiene destino legal (pool, carry, distribución, burn) | D5 (3b) |
| Si la época **no tiene elegibles: NO hay goteo y el tesoro NO pierde el turno** (el disponible es derivado, no un calendario) | §4.2 del diseño del tesoro |
| 4096 TX/época queda (desviación oficial de inicio.md) | D5 (3b) |
| **El score node-local JAMÁS paga el goteo** — el reparto es función pura del estado público | Checkpoint 13, lección del hallazgo A |

## 2. El problema central del diseño: los pesos no pueden salir de `PeerLocal`

inicio.md dice "reparte proporcionalmente entre los nodos que demostraron
estabilidad, conexión y honestidad" — y la unidad C midió exactamente eso.
Pero `PeerLocal` (antigüedad, uptime_ratio) es **node-local**: vive en el
tree `peers` de cada nodo, corre con reloj local y JAMÁS viaja en el
snapshot. Si el Cierre lo usara para pagar, dos nodos derivarían salidos
distintos del MISMO epoch → divergencia de consenso. Es el mismo vicio del hallazgo original del Checkpoint 5: un dato node-local
mezclado con saldos. **PROHIBIDO por ley.**

### Las opciones honestas (todas función pura del estado público)

| Opción | Peso | Anti-Sybil | Costo de estado |
|---|---|---|---|
| **R1 (equitativa)** — elegible = envió ≥1 TX en la época; todo elegible pesa **1** | nivel | **Neutra**: un dueño con N cuentas captura N/T del goteo (T = total elegibles) — exactamente su fracción de la red, ni un sat más | mapa de participación por época |
| **R2 (por participación)** — peso = #TXs enviadas en la época | grave: en la era µLIT, spamear 4096 TXs hacia sí mismo cuesta ~0.004 LIT de fees y captura ~todo el goteo (500 LIT) — incentivo chatarra-actividad | mapa con contadores |
| R3 (por saldo) — peso = balance | mide riqueza, no estabilidad — ya descartada (3b) | nada |

⚠️ **Nota anti-Sybil bajo R1:** cada cuenta nueva debió ENVIAR una TX (fees
se queman) — participación = proof-of-use con costo. Un dueño nunca captura
más que su fracción honesta del total de participantes.

**Recomendación: R1.** Es literalmente "Distribución Equitativa" de
inicio.md, cero curvas, cero gameo rentable, y honra a la Prueba de
Tiempo por su proxy consensual: operar la red (una TX en la época) es
demostración de vida. El mapa guarda el **contador** (u64), no solo
presencia: la puerta queda abierta para ponderar en v2 (antigüedad +
participación; solo cuando exista identidad de nodo en consenso —
desviación registrada como TODO de v2).

## 3. Mecánica de consenso (determinista, función pura)

### 3.1 Estado nuevo (Snapshot v3 — el root vuelve a cambiar)

```rust
pub emitido_total: u64,                           // LIT ya minteado del tesoro (goteo); consenso
pub participacion_epoch: BTreeMap<Address, u64>,  // TXs enviadas ESTA época; se vacía en cada Cierre
```

- `participacion_epoch` crece en `apply_transaction` (sender += 1). La
  secuencia de TX ya es de consenso ⇒ el mapa es determinista entre nodos
  ⇒ entra al root OBLIGATORIO (sin él, divergencia oculta).
- Snapshot v3: `emitido_total` (1×u64) + entradas `(addr, count)` de
  participación tras las de balances. **Redeploy ATÓMICO** requerido.

### 3.2 El Cierre de Libro (orden exacto, TODO dentro de `finalize_checkpoint`)

En cada Cierre (época clausura), en este orden:

1. `goteo_piso = min(GOTEO_POR_EPOCA, tesoro_disponible)`
   — `GOTEO_POR_EPOCA = 500 LIT` (D5); tesoro disponible ver §4.
2. `elegibles = cuentas con participacion_epoch[c] >= 1`.
3. Si `goteo_piso > 0` y hay elegibles:
   - `parte = goteo_piso / elegibles.len()` (división entera).
   - Cada elegible: `balances[c] += parte` (MINT directo — el tesoro no
     es cuenta, nadie cobra por proxy).
   - `emitido_total += parte * elegibles.len()` (el POLVO queda en el
     tesoro, no emite; ver D9).
4. Si NO hay elegibles: cero emisión (folio intacto, D firmada) y el
   `fee_pool` queda ESTACIONADO (carry a la época siguiente — Q2 firmada,
   jamás se quema sin destino legal).
5. Post-goteo (cuando `tesoro_disponible == 0`, era redistribución):
   - `mitad = fee_pool / 2` → repartida entre elegibles con la misma
     mecánica (dust de esta mitad → carry del pool, poza muerta prohibida).
   - `fees_quemados += fee_pool − mitad − dust_carried`.
6. `participacion_epoch.clear()` SIEMPRE (época nueva nace en blanco).
7. `fee_pool` queda con su carry; `epoch += 1`.

Todo esto es una única función pura del estado: los nodos derivan
byte a byte el mismo resultado → roots idénticos (el test del
Checkpoint 5 sigue valiendo).

### 3.3 Por qué el goteo es inmune al doble-firmado

- El tesoro NO es cuenta: nadie firma nada. `emitido_total` es otro
  contador como `fees_quemados`. Ni el operador tiene "el tesoro".
- Toda la emisión ocurre DENTRO de `finalize_checkpoint` que producía
  roots idénticos entre nodos desde el Checkpoint 5.

## 4. 🎯 D7 — Constitución del techo (dos lecturas de "21M LIT")

| Constitución | Regla | Consecuencia |
|---|---|---|
| **A — techo SIMULTÁNEO** | `emitido_total + total_supply ≤ 21M SIEMPRE` | El burn REABRE tesoro (supply baja → disponible sube). "21M = tope de monedas vivas". Deflación parcial re-compensada. |
| **B — techo EVER-MINTEADO** (recomendada) | `emitido_total ≤ 21M` (total ya minteado jamás) | El burn es irreversible EN ABSOLUTO: lo quemado no vuelve jamás a nacer. BTC-style "21M ever". |

Migración en B: primera corrida del binario nuevo, `emitido_total` se
SEEDea con `total_supply()` congelada en el momento del redeploy
(~10 LIT MainNet, ~3 LIT TestNet): "todo LIT que ya existe consumió su
parte del tesoro" ⇒ Tesoro útil MainNet ≈ 20.999.990 LIT. Riesgo B
honesto: el seed requiere que TODOS los nodos migren con la MISMA
supply (ventana atómica ya conocida del deploy del 19b).

## 5. 🎯 D8 — ¿En qué redes corre?

| Red | Goteo | Comentario |
|---|---|---|
| MainNet | SÍ desde su redeploy | La visión del tesoro es MainNet. |
| TestNet | RECOMENDADO SÍ (lab vivo primero) | Se prueba el goteo REAL con TX de carga antes de tocar MainNet. El faucet del laboratorio sigue funcionando (toolbox de emergencia), y el accounting (`--faucet` mints también suben `supply`) es uniforme. |

Faucet TestNet NO es poza muerta ni clave del tesoro: es toolbox del
operador de laboratorio con sus caps existentes.

## 6. Gate g6 (nuevo, con la emisión)

Snapshot ENTRANTE más fresco:
- `emitido_total ≤ HARD_CAP_SAT` (techo mensurable, 21M).
- **Monotonía**: snapshot más fresco con `emitido_total` MENOR que el
  local = mentira (el tesoro solo emite, no des-emite).
- **Coherencia con el epoch** (cota por época+constante de turno):
  `emitido_remoto ≤ GOTEO_POR_EPOCA × (epoch_remoto + 1) + seed_red`
  — el goteo solo emite 500 LIT/época; `seed_red` = el monto con el que
  nació `emitido_total` en la migración (para MainNet) o 0 acumulado
  conocible (TestNet, donde el faucet sigue su propia vida: ver D8).
- Participación incluida: el snapshot v3 DEBE traer entradas
  `(addr, count)` con formato válido (longitud exacta por tag) — sin
  participación no parsea (los peers viejos no se entienden, por eso la
  ventana atómica).

## 7. UX/GUI (pequeño, después de la implementación)

- Dashboard: fila `Goteo emitido:` (columna con `emitido_total`) y
  `Tesoro restante:` (derivado en vivo `21M − emitido − supply`).
- El monitor TUI lee el snapshot v3 out-of-the-box (usa
  `decode_snapshot`), solo agrega la fila.

## 8. Tests bloqueantes (plan)

1. Goteo exacto: 3 elegibles (participaciones 1,1,1) → +166.666.666 sat
   c/u, `emitido_total + = 500.000.000 − 2` (dust de 2 sat queda en el
   tesoro), balances exactos.
2. Sin elegibles ⇒ cero emisión + `fee_pool` carry intacto (D firmadas).
3. Tesoro agotado: el último goteo se recorta exacto al disponible y
   luego `emitted+supply == 21M` (para A) / `emitted == 21M` (para B).
4. Post-goteo 50/50: mitad distribuida entre elegibles, dust → carry,
   resto → quemados; split exacto con fee_pool impar.
5. `participacion_epoch` limpiado en cada Cierre (mapa vacío, epoch+1,
   root determinista entre nodos cada uno).
6. g6: emitido retrocedido → REJECTED; emitido gigante → REJECTED;
   participación corrupta (longitud) → snapshot v3 rechazado.
7. Root idéntico entre nodos con wallets distintas y mismas
   participaciones.
8. Snapshot v3 roundtrip con count de participación grande (u64).
9. Serie de 50 Cierres seguidos: `emitted+supply` jamás pisa
   `HARD_CAP_SAT` (invariante constitucional).
10. Regresión completa core + wallet + E2E; g4'/burn del 19 no se toca
    (el goteo no altera el accounting de fees salvo en la era post-goteo).

## 9. Rollout (aviso político honesto)

- Este es otro cambio de root: **redeploy atómico**
  (la ventana del 19b replicada) — D10 define el orden.
- TestNet será el lab del goteo real (con TX de carga);
  MainNet migra después con la constitución elegida (D7).
- Los epochs 0..k ACTUALES ya formarán folio: `emitido_total` se queda
  en 0 hasta el primer Cierre CON elegibles (folio de la ley firmada).

---

## 10. Decisions a firmar (🎯) → VER §11 FIRMADAS

(El borrador planteaba las opciones; todas resueltas en §11.)

---

## 11. Registro de decision — firmadas 2026-11-08

- **D6 — Peso del reparto: R1 EQUITATIVA** ✅. Elegible = envió ≥1 TX en
  la época; todo elegible pesa 1. R2 descartada (incentivo chatarra).
- **D7 — Constitución del techo: B EVER-MINTEADO** ✅. Lo quemado jamás
  vuelve a nacer; 21M = total histórico minteado (drip incluye seed de
  migración = supply congelada al redeploy). disponible = 21M − emitted.
- **D8 — Redes: MainNet + TestNet** ✅ (lab TestNet primero, con el
  faucet toolbox vivo contra el mismo accounting).
- **D9 — Dust del reparto: QUEDA EN EL TESORO** ✅ (emitido_total solo
 suma lo efectivamente repartido; el polvo queda disponible para
 futuros goteos — nunca poza muerta: sigue siendo emisión pendiente).
- **D10 — Rollout: TestNet PRIMERO** (lab con TX de carga), MainNet
  después, en su propia ventana atómica ✅.
