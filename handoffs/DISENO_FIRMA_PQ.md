# DISEÑO — 9b: TRANSICIÓN DE FIRMA POST-CUÁNTICA (diseño previo, JAMÁS un patch)

> Estado: **DISEÑO v1** · Sesión 2026-11-11/12 (Checkpoint 35)
> Anclas: `inicio.md`, Ckpt 27/28 (opiniones), `DISENO_AUTOUPDATE.md`
> (el pipeline U1–U9 **YA VIVE en producción** — Ckpts 32/33/34),
> `hash.rs` (el patrón del funnel probado con la deuda 9a).
> Ley del proyecto: **preparar sí, migrar ya** — este documento es la
> preparación. La ejecución de cualquier era de firma usa el pipeline
> U1–U9 y exige el "sí" humano (U1).

---

## 1. El modelo de amenaza (por qué el hash NO es la pieza frágil)

| Ataque | Algoritmo | Veredicto |
|---|---|---|
| **Shor** (computación cuántica) | ed25519 (log discreto en curva elíptica) | **ROTO** — la firma se vuelve falsificable; el hash NO |
| Shor | SHA-256 | no aplica (Shor rompe log discreto y factorización, no funciones hash) |
| **Grover** (cuadrático) | SHA-256 | 2^256 → 2^128: queda ~128 bits de seguridad — SUFICIENTE para decadas |
| Computación clásica | ambos | intactos |

**Consecuencia**: la pieza quantum-frágil del stack es ed25519. Un
atacante con QC que conozca la PUBKEY de una cuenta puede derivar su
private key y gastar sus fondos sin que el dueño actúe jamás (attack
de "harvest": grabar hoy, descifrar después). El hash de estado es
robusto (y el funnel 9a ya deja el swap de hash listo por si acaso).

## 2. Candidatas PQ (NIST, estandarizadas/draft 2024)

| Esquema | FIPS | Tipo | Firma | Pubkey | Notas |
|---|---|---|---|---|---|
| **ML-DSA-65** | 204 | lattice (Kyber/ML-KEM familia) | 3.309 B | 1.952 B | rápido, tamaño medio, la recomendada por defecto |
| **SLH-DSA-128s** | 205 | solo-hash (SPHINCS+) | 7.856 B | **32 B** | la MÁS conservadora (seguridad reducible a funciones hash — misma familia que nuestro estado); lenta de firmar |
| **Falcon-512** | 206 (draft) | lattice (NTRU) | ~666 B | 897 B | la más chica, pero firmar usa punto flotante (side-channels delicados) |

**Recomendación**: era híbrida **ed25519 + ML-DSA-65** (dual firma,
verificación de AMBAS), con SLH-DSA como era de repliegue si el
análisis lattice se degrada. Razones:
- La hybrid elimina el riesgo "un solo primitivo roto = todo roto"
  durante TODA la transición (verifica ambas ⇒ un atacante necesita
  romper ambas).
- ML-DSA es la estándar de la industria (TLS, SSH, firmware ya están
  migrando a ella) — mejor soporte de librerías Rust (`liboqs`,
  `ml-dsa` crate puro-Rust en evaluación).
- El tamaño (3.3 KB de firma por TX) NO es problema para Ledgerita:
  el snapshot viaja por estado (no por TX), 4.096 TX/época es poco,
  y el cable de TX es transitorio (no persistido en el snapshot).

## 3. La dependencia: 9c es PREREQUISITO de 9b

Hoy `Address { key: VerifyingKey }` — la dirección ES la pubkey
(32 B comprimidos). Una pubkey PQ NO ENTRA en 32 bytes (ML-DSA 1.9
KB, SLH-DSA 32 B pero Falcon 897 B…). Dos opciones malas: agrandar la
dirección (rompe TODOS los formatos: snapshot, cable, balance map,
CLI, wallet) o hacer el cambio estructural **dirección = hash(pubkey)**
(9c): la dirección queda 32 bytes PARA SIEMPRE — cualquier esquema de
firma (presente o futuro) cabe detrás del hash, y la pubkey solo
viaja en la TX que gasta (privacidad estilo Bitcoin de yapa).

**Orden de eras**: 9c primero (cambio de formato, barato), 9b encima
(cambio criptográfico, gordo). Ver `DISENO_DIRECCION_HASH.md`.

## 4. Las eras de firma (E1–E5) — cada una con su pipeline U1–U9

| Era | Estado | Qué cambia |
|---|---|---|
| **E1** (hoy) | ed25519 solo | — |
| **E2** | PREPARACIÓN (código, TestNet) | módulo `sig.rs` en core: `SIG_ALGO_ED25519=1`/`SIG_ALGO_ACTUAL` (constantes de era, patrón `hash.rs`), la ÚNICA puerta de `verify_signature`; dual-firma opcional EN TX v2 (formato `era_byte + [pubkey] + [sig1][sig2]`); los nodos E2 siguen firmando/verificando ed25519 pero SABEN leer híbridas |
| **E3** | DUAL en TestNet | TestNet genera TXs híbridas (ed25519 + ML-DSA); los nodos verifican AMBAS; ventana de convivencia con E1/E2 sin corte (etiqueta de era en la TX, patrón del snapshot v4) |
| **E4** | DUAL OBLIGATORIA (anclada a época N) | desde la época N, una TX de firma simple se RECHAZA — misma ley que la emisión: misma época ⇒ mismas reglas de firma |
| **E5** | PQ-only (época M) | ed25519 se retira: solo ML-DSA; los wallets viejos DEBEN haber rotado llaves (migración de wallets = nueva wallet + TX de traspaso por cuenta, con la herramienta de ceremonia ya existente) |

**Anclaje a época** (la ley que ya rige la emisión): el snapshot v4
lleva la etiqueta del algoritmo de HASH; la etiqueta de FIRMA viaja en
cada TX y en el anuncio de época — nodo pierde vivacidad (U7) antes
que corromper el ledger si recibe una firma de una era que no conoce.

## 5. Inventario de call sites (lo que toca una era de firma)

- `crypto.rs`: `Address.verify_signature`, `load_or_create_wallet`
  (wallet.key = seed ed25519 — FORMATO DE WALLET nuevo en E5),
  `derive_from_seed`, backups cifrados.
- `transaction.rs`: `is_signature_valid` (la puerta), `sign`.
- `serialize.rs`: `encode_transaction` (el mensaje firmado — las TX
  híbridas cambian su encoding ⇒ era de formato), `encode_address`.
- Wallet GUI: firmado local (enviar), E2E.
- `network.rs`: flood de TX firmadas (tamaño ×~10 con ML-DSA — solo
  tráfico, no consenso).
- genesis/faucet: TXs firmadas de la seed.
- **El updater (F1–F3) es el vehículo de TODAS las eras** — ya
  probado en producción con U1-frase + U4-canario.

## 6. Tests bloqueantes (plan)

1. `test_firma_dual_ambas_validas_acepta` / una sola válida rechaza
   en era dual-obligatoria.
2. `test_tx_era_desconocida_rechaza` (nodo E_n recibe TX de era
   E_m>m ⇒ rechaza — pierde vivacidad, jamás adopta).
3. `test_misma_epoca_mismas_reglas_firma` (el anclaje: la época decide
   la ley de firma, no el binario).
4. `test_wallet_rotada_vieja_no_firma_en_pq_only` (E5).
5. `test_snapshot_v4_acepta_txs_hibridas` (la raíz no cambia por la
   firma — el contenido sí: la dirección ya es hash por 9c).
6. `test_reuse_direccion_revela_pubkey_una_vez` (con 9c: la pubkey
   solo viaja en la primera TX que gasta).

## 7. Puertas de decisión (cuándo ejecutar cada era)

La migración NO es por calendario, es por SEÑALES (cada una con su
"sí" humano y su pipeline U1–U9):
1. **E2/E3** pueden ejecutarse en cualquier momento (son preparación
   y TestNet — riesgo bajo) — el caso natural: cuando 9c esté
   ejecutada y asentada.
2. **E4** (dual obligatoria): cuando las librerías ML-DSA Rust estén
   auditadas/maduras (`ml-dsa` o liboqs-bindings) Y el NIST tenga
   FIPS 206 (Falcon) cerrada — para poder comparar.
3. **E5** (PQ-only): cuando la amenaza QC sea CONCRETA (QC con miles
   de qubits lógicos corriendo Shor en 256 bits) o el NIST marque
   deprecación de ECDSA/EdDSA — no antes (la híbrida es sostenible
   indefinidamente).
4. **Repliegue a SLH-DSA**: solo si se publica un avance serio contra
   lattice (p.ej. mejoras del algoritmo de Primal/dual attack) — la
   era de firma también tiene funnel: `SIG_ALGO_*`.

## 8. Lo que JAMÁS hacemos

- Migrar "de una vez" sin era híbrida (un primitivo roto no puede
  dejar a TODOS los nodos inutilizables a la vez).
- Cambiar el formato de dirección como patch (es 9c, era anclada).
- Poner deps PQ en el core ANTES de E2 (YAGNI — el funnel ya nos da
  la agilidad).
- Firmar dos veces "por si acaso" sin etiqueta de era (la era es la
  ley, no un byte decorativo).

## 9. Estimaciones honestas

- E2 (funnel + formato híbrido opcional): una tarde con tests
  (patrón 9a).
- E3 (dual en TestNet): una sesión + ventana de observación.
- E4/E5: cada una una ventana con pipeline U1–U9 (el swap es MECÁNICO
  hoy: frase de consentimiento + canario + backup + POST).
- El costo permanente de la híbrida: TXs ×~10 en tamaño de firma
  (irrelevante a nuestra escala), verificación ×2 (irrelevante).

> **La postura**: el reloj que corre no es el nuestro (es el de las
> QC y el del NIST) — pero NUESTRO tiempo de respuesta, cuando toque,
> es de horas: era anclada + pipeline mecánico + funnel de firma.
