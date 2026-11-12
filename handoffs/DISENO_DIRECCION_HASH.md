# DISEÑO — 9c: DIRECCIÓN = HASH(PUBKEY) (diseño previo, JAMÁS un patch)

> Estado: **DISEÑO v1** · Sesión 2026-11-11/12 (Checkpoint 35)
> Anclas: `inicio.md`, Ckpt 28 (el hallazgo), Ckpt 29 (la herramienta
> de ceremonia de bulk — el vehículo de la migración),
> `DISENO_AUTOUPDATE.md` (pipeline U1–U9 vivo en producción),
> `DISENO_FIRMA_PQ.md` (9c es PREREQUISITO de 9b).
> Ley: es transición de era (cambia el formato en TODO el estado) —
> se ejecuta anclada a época, con el "sí" humano (U1), JAMÁS un patch.

---

## 1. El hallazgo (por qué esto importa)

Hoy `Address { key: VerifyingKey }`: la dirección **ES** la pubkey
(32 B del punto comprimido ed25519). Consecuencia: el ledger publica
la clave pública de TODAS las cuentas, para siempre — incluso
cuentas que jamás gastaron.

**Bitcoin hace lo contrario**: dirección = hash160(pubkey) — la
pubkey solo se revela cuando la cuenta gasta por primera vez. Un
atacante con QC (Shor) puede:
- contra Bitcoin: atacar SOLO las cuentas que ya gastaron (pubkey
  expuesta) — la gran mayoría de las cuentas USTED-ahorradoras está
  detrás de hashes sin pubkey;
- contra Ledgerita hoy: atacar **TODAS** las cuentas (pubkey al
  alcance de cualquiera desde el snapshot).

Con una QC en el horizonte, la superficie de ataque de Ledgerita es
MAYOR que la de Bitcoin por diseño — este hardening la iguala.

## 2. El formato nuevo (era de dirección D2)

- **D1 (hoy)**: `address = pubkey` (32 B).
- **D2 (propuesta)**: `address = SHA-256(pubkey)` (32 B) — **el
  formato NO cambia de tamaño** (el balance map, el snapshot, el
  cable y todas las UI siguen viendo 32 bytes) pero el contenido pasa
  a ser OPAQUE: un hash de la pubkey, no la pubkey.
- La pubkey viaja en la **TX que gasta** (campo `pubkey_gastadora:
  [u8; 32]` en TX v3) y el nodo verifica:
  1. `sha256(pubkey_gastadora) == sender` (la cuenta es dueña de esa
     pubkey — la dirección ES el compromiso),
  2. ed25519: `pubkey_gastadora.verify(mensaje, firma)` (la firma es
     de esa pubkey),
  3. `nonce` secuencial como siempre.
- Efecto de privacidad de yapa: una cuenta que nunca gastó publica
  CERO info sobre su llave. La pubkey solo entra al rastro cuando la
  cuenta gasta — y cada reuso de dirección después de gastar mantiene
  la misma superficie que Bitcoin (los hashes no esconden pubkeys
  ya-reveladas: es el límite de reuso, no de formato).

## 3. El anclaje y la migración (era anclada a época N)

La ley de siempre: **misma época ⇒ mismas reglas**. El snapshot
lleva ya la etiqueta de era (v4 lleva el byte de hash-algo; el
formato de cuenta migra junto):

| Era | Estado | Cuentas |
|---|---|---|
| **D1** (hoy) | address = pubkey | las 3 de MainNet + TestNet viven acá |
| **D2a** (preparación) | código en core: TX v3 con `pubkey_gastadora`, verificación del compromiso, **acepta cuentas D1 y D2** (etiqueta de era por cuenta en el snapshot — patrón v4) | convivencia total |
| **D2b** (época N: D2 obligatoria para GASTAR) | una TX con sender D1 se rechaza: para gastar hay que migrar primero | las cuentas D1 quedan congeladas-hasta-migrar (no queman fondos — el saldo espera) |
| **D2c** (época M: D1 retirada) | snapshot con cuentas D1 = formato legacy solo lectura; los balances D1 sin reclamar quedan visibles hasta que alguien (el dueño) migre | — |

**El vehículo de migración ya existe**: la herramienta de ceremonia
(Ckpt 29) hizo 4.096 TX de bulk con contabilidad al satoshi — la
migración D1→D2 es UNA TX POR CUENTA (la seed manda el saldo de la
cuenta vieja a la cuenta nueva = hash de una wallet NUEVA), puede ser
bulk para muchas cuentas o una por una para las 3 de MainNet.

**Para las 3 cuentas MainNet hoy**: la seed (504.99 LIT) y kettle02
(5 LIT) migran a wallets nuevas D2; el usuario migra SU wallet con la
GUI (nueva wallet → TX de traspaso). Opcional: quemar y re-emitir
NO — los fondos se TRASPASAN (la emisión total no cambia jamás).

## 4. Inventario de call sites (lo que toca la era D2)

- `crypto.rs`: `Address` — en D2 el tipo queda igual por fuera (32
  B) pero `from_signing_key` pasa a `sha256(pubkey)`; `verify_signature`
  se convierte en `verify_signature_contra_compromiso(pubkey, ...)`.
- `transaction.rs`: TX v3 — campo `pubkey_gastadora` opcional en D2a,
  obligatorio para sender en D2b; `is_signature_valid` verifica el
  compromiso.
- `serialize.rs`: `encode_address`/`decode_address` (sin cambio de
  tamaño, con etiqueta de era de cuenta en el snapshot → **snapshot
  v5**: `era_cuenta` byte por sección o al final, patrón v4).
- `state.rs`: balances map — la KEY cambia de significado (ya no es
  la pubkey): la migración es un re-mapeo por TX de traspaso (no un
  rehash en disco — el estado se transforma por CONSENSO, no por
  migración de datos).
- Wallet GUI: mostrar/crear wallet D2, firma con pubkey incluida.
- CLI `--show-address`: la dirección impresa cambia de significado.
- genesis/faucet: nacen directamente D2.
- Updater (U1–U9): vehículo del swap de era, como siempre.

## 5. Tests bloqueantes (plan)

1. `test_d2_compromiso_verifica` — TX v3: sha256(pubkey)==sender ∧
   firma válida ⇒ acepta; pubkey que no compromete ⇒ RECHAZA.
2. `test_d1_gasta_en_d2b_rechaza` (anclaje: época decide la ley).
3. `test_d1_recibe_en_d2_acepta` (recibir jamás se bloquea).
4. `test_migracion_preserva_saldo_y_emision` (la TX de traspaso no
   toca la emisión total ni la cuota — contabilidad al satoshi).
5. `test_snapshot_v5_leyendo_v4_legado` (compatibilidad del cable).
6. `test_pubkey_no_revelada_gasta_una_vez` (privacidad de yapa: la
   primera TX revela, las siguientes no agregan nada).

## 6. Puertas de decisión

1. **D2a** puede ejecutarse cuando se quiera (preparación, riesgo
   bajo, TestNet).
2. **D2b** (obligatoriedad): después de que 9a/9b-E2 estén asentadas
   y ANTES de cualquier ventaja QC real — el orden natural es
   D2a → E2/E3 (firma híbrida preparada) → D2b → E4 (dual
   obligatoria) → E5/D2c (retiros juntos si el calendario lo pide).
3. **La migración de las 3 cuentas MainNet**: SU ventana, SU "sí"
   humano, con la herramienta de ceremonia y verificación de raíz.

## 7. Lo que JAMÁS hacemos

- Rehashear cuentas EN DISCO por fuera del consenso (el estado se
  transforma por TXs de traspaso — el ledger es la única verdad).
- Agrandar la dirección a 1.9 KB para meter pubkeys PQ (rompe todos
  los formatos — el hash es la solución estructural).
- Retirar D1 sin la era de aceptación-ambas (la ventana sin corte es
  el patrón probado del snapshot v4).
- Patch caliente en MainNet sin anclaje de época (jamás).

## 8. Estimación honesta

- D2a: una tarde con tests (el patrón 9a lo mide).
- D2b + migración de cuentas: una sesión con ceremonia (el Ckpt 29
  es el ensayo general — ya se hizo una vez).
- El costo permanente: TXs +32 B (la pubkey) — irrelevante.

> **La postura**: 9c es el hardening que hace al ledger QUANTUM-
> PACIENTE: sin él, cada cuenta es una pubkey esperando una QC; con
> él, el ledger expone compromisos (hashes) y las pubkeys viven
> escondidas hasta su primer gasto — el modelo que Bitcoin puso a
> prueba durante 15 años, con nuestras herramientas de era ya
> probadas en producción.
