# Diseño Formal: Consenso BFT Ligero (`QuorumCertificate`) sobre State Root — Ledgerita

**Fecha:** 2026-11-15  
**Checkpoint:** 38 (`handoffs/PLAN_CORRECCION_VULNERABILIDADES_2026-11-13.md`)  
**Estado:** Especificación aprobada para implementación en Checkpoints 39, 40 y 41.  
**Relacionado con:** `DISENO_DEFENSA_RED.md` (Gates `g1`–`g6`), `handoffs/DISENO_GOTE.md` (Cierre de Libro / `finalize_checkpoint`), `handoffs/REPORTE_SESION_2026-11-13_AUDITORIA_Y_OPINION.md` (Hallazgos Críticos #1 y #2).

---

## 1. Modelo de Amenazas y Problema a Resolver

Las compuertas actuales (`g1`–`g6` en `state.rs::validate_incoming_snapshot`) garantizan consistencia aritmética local (emisión total, balances no negativos, límite de emisión por época y coincidencia de versión de `HASH_ALGO_ACTUAL`), pero **no autentican la autoridad ni el consenso** detrás de un `LedgerState` entrante:

1. **Inyección de Estado Arbitrario (Auditoría — Vulnerabilidad Crítica #1):**  
   Cualquier peer conectado que envíe un `MSG_SYNC` con `remote.epoch > local.epoch` (o mayor `tx_count` dentro de la misma época) y que respete las cotas matemáticas de `g1`–`g6` puede sobrescribir el estado local en `merge_if_fresher`, fabricando balances propios o revertiendo transacciones legítimas sin haber sido validado por la red.
2. **Inflación de Épocas / Sybil en Cierre de Libro (Auditoría — Vulnerabilidad Crítica #2):**  
   Un nodo malicioso puede ejecutar localmente miles de transacciones entre cuentas propias en milisegundos, avanzar `epoch` unilateralmente y forzar al resto de nodos honestos a aceptar su estado como "más fresco".

**Objetivo del diseño:**  
Exigir que todo avance de época (`epoch > 0`) esté respaldado por un **Certificado de Quórum (`QuorumCertificate`, QC)** verificable criptográficamente mediante firmas `ed25519` de al menos $M$-de-$N$ validadores autorizados (`ValidatorSet`), vinculando el certificado al `State Root` (`finalize_checkpoint`) y al `chain_id` de la red.

---

## 2. Arquitectura de Validadores (`ValidatorSet`)

### 2.1 Identidad y Configuración por Red (`NetworkId`)
Cada red (`Mainnet`, `Testnet`, `Devnet`, según `NetworkId`) define un `ValidatorSet` determinista y explícito:

```rust
pub struct ValidatorInfo {
    /// Índice único del validador en el conjunto (0..N-1).
    pub id: u8,
    /// Clave pública ed25519 (32 bytes) que identifica al validador.
    pub pubkey: [u8; 32],
}

pub struct ValidatorSet {
    /// Lista ordenada de validadores autorizados (máximo 255).
    pub validators: Vec<ValidatorInfo>,
    /// Umbral mínimo M de firmas válidas requeridas (M <= N, M >= 1).
    pub threshold: u8,
}
```

- **Topología inicial de producción / staging:**
  - `Mainnet` / `Testnet`: $N = 3$ validadores (`kettle01`, `kettle02`, `kettle03`), umbral $M = 2$ (tolera $f = 1$ fallo o nodo bizantino).
  - `Devnet` / tests unitarios: configurable ($N \ge 1$, $M \ge 1$; permite $1$-de-$1$ o $2$-de-$3$ en tests de integración).
- **Invariantes del `ValidatorSet`:**
  - No puede haber `id` duplicados ni `pubkey` duplicadas.
  - `threshold >= 1` y `threshold as usize <= validators.len()`.
  - Las claves públicas de los validadores de red se cargan de forma determinista por `NetworkId` (o configuración explícita inyectada al iniciar el nodo/test), nunca desde un peer remoto.

---

## 3. Certificado de Quórum (`QuorumCertificate`) y Firma Canónica

### 3.1 Estructura de Datos
Ubicación propuesta: `ledgerita_core/src/quorum.rs` (Checkpoint 39).

```rust
pub const QC_DOMAIN_TAG: &[u8] = b"LEDGERITA_QC_V1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatorSignature {
    pub validator_id: u8,
    pub signature: [u8; 64], // ed25519::Signature::to_bytes()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuorumCertificate {
    pub chain_id: u8,
    pub epoch: u64,
    pub state_root: [u8; 32],
    pub signatures: Vec<ValidatorSignature>,
}
```

### 3.2 Mensaje Canónico de Firma (Domain Separation)
Para prevenir ataques de repetición entre cadenas (`Mainnet` vs `Testnet`), entre épocas o confusión con firmas de transacciones, cada validador firma exactamente el siguiente buffer canónico:

```text
msg = b"LEDGERITA_QC_V1" || chain_id (1B) || epoch.to_be_bytes() (8B) || state_root (32B)
```

Longitud fija: `15 + 1 + 8 + 32 = 56 bytes`.  
El campo `state_root` corresponde al hash criptográfico devuelto por `LedgerState::compute_state_root()` / `finalize_checkpoint()` (SHA-256 sobre el estado canónico en `HASH_ALGO_ACTUAL`).

### 3.3 Reglas de Verificación Pura (`verify_quorum`)
La función `QuorumCertificate::verify(&self, expected_chain_id: u8, vset: &ValidatorSet) -> Result<(), QuorumError>` debe comprobar en orden estricto:
1. **`chain_id` coincide:** `self.chain_id == expected_chain_id`.
2. **Sin duplicados de validador:** Ningún `validator_id` aparece más de una vez en `self.signatures` (previene inflar el conteo con la misma firma repetida $M$ veces).
3. **Pertenece al `ValidatorSet`:** Cada `validator_id` existe en `vset.validators`.
4. **Firma `ed25519` válida:** Cada `signature` verifica con `VerifyingKey::from_bytes(&validator.pubkey)` sobre el mensaje canónico de 56 bytes usando `verify_strict` (previene maleabilidad de firmas).
5. **Umbral alcanzado:** El número de firmas válidas distintas es `>= vset.threshold as usize`.

Cualquier fallo retorna un `QuorumError` tipado (`ChainIdMismatch`, `DuplicateValidator`, `UnknownValidator`, `InvalidSignature`, `InsufficientQuorum`).

---

## 4. Protocolo P2P de Votación (`MSG_VOTE = 0x03`)

### 4.1 Tipos de Mensaje en `network.rs`
Se mantiene compatibilidad con `build_msg` / `split_msg`:
- `MSG_TX: u8 = 0x01` (existente)
- `MSG_SYNC: u8 = 0x02` (existente)
- `MSG_VOTE: u8 = 0x03` (nuevo — Checkpoint 41)

### 4.2 Flujo de Cierre de Libro y Formación del QC
1. **Propuesta / Cierre de Época:**  
   Cuando un nodo validador alcanza la condición de Cierre de Libro y ejecuta `finalize_checkpoint()`, calcula el `state_root` de la nueva época `E`.
2. **Emisión de Voto (`MSG_VOTE`):**  
   Si el nodo posee una clave de validador configurada (`ValidatorSigner` con `SigningKey` envuelta en `Zeroizing`), firma el mensaje canónico `(chain_id, E, state_root)` y difunde un paquete `MSG_VOTE`:
   ```text
   MSG_VOTE payload:
   chain_id (1B) | epoch (8B BE) | state_root (32B) | validator_id (1B) | signature (64B)
   ```
   Tamaño fijo de payload: `106 bytes`.
3. **Agregación en `VoteCollector`:**  
   Cada nodo mantiene un colector en memoria indexado por `(epoch, state_root)` que valida cada `MSG_VOTE` entrante contra `ValidatorSet` antes de almacenarlo.
   - Si un voto tiene firma inválida o `validator_id` desconocido, se descarta y se registra un strike en `PeerGuard`.
   - Cuando el conjunto de votos válidos para `(E, state_root)` alcanza `vset.threshold`, el nodo ensambla el `QuorumCertificate` y lo adjunta a su `LedgerState` (`latest_qc: Option<QuorumCertificate>`).
4. **Difusión por `MSG_SYNC`:**  
   Al enviar su snapshot vía `MSG_SYNC`, el nodo incluye su `latest_qc` en el formato Snapshot v5 para que nodos no-validadores (observadores/wallets) o validadores rezagados puedan verificar y adoptar la época certificada.

---

## 5. Gate `g7` en `state.rs` y Formato Snapshot v5

### 5.1 Nueva Compuerta `g7` en `validate_incoming_snapshot`
Se añade `g7` al pipeline de `state.rs::validate_incoming_snapshot` (tras `g1`–`g6`):

| Gate | Condición de Rechazo | `GateReason` | Comportamiento en `merge_if_fresher` |
| :--- | :--- | :--- | :--- |
| **`g7a`** | `remote.epoch > 0` y `remote.latest_qc.is_none()` (o `remote.epoch > local.epoch` sin QC) | `FaltaCertificadoQuorum` | Rechaza el snapshot; no actualiza estado local. |
| **`g7b`** | `qc.epoch != remote.epoch` o `qc.state_root != remote_computed_root` | `CertificadoQuorumInvalido` | Rechaza el snapshot; penaliza al peer vía `PeerGuard`. |
| **`g7c`** | `qc.verify(local.chain_id, vset)` falla (`UnknownValidator`, `InvalidSignature`, `DuplicateValidator`, `InsufficientQuorum`, `ChainIdMismatch`) | `CertificadoQuorumInvalido` | Rechaza el snapshot; penaliza al peer vía `PeerGuard`. |

> **Nota sobre Época Génesis (`epoch == 0`):**  
> El estado inicial en `epoch == 0` no requiere `QuorumCertificate` para permitir el arranque limpio de la red. Todo avance a `epoch >= 1` sí exige `g7` de forma obligatoria en redes con `ValidatorSet` activo.

### 5.2 Extensión del Wire Format: Snapshot v4 -> Snapshot v5
Formato actual v4 en `state.rs::encode_snapshot`:
```text
chain_id(1B) | epoch(8B) | fee_pool(8B) | fees_quemados(8B) | tx_count(8B) | emitido_total(8B)
| num_entries(4B) | [addr(32B) + balance(8B) + nonce(8B)]*
| num_particip(4B) | [addr(32B) + count(8B)]*
| hash_algo(1B)
```

Extensión **Snapshot v5** (añadida al final tras `hash_algo(1B)`):
```text
| has_qc(1B: 0x00 o 0x01)
  [si has_qc == 0x01]:
  | qc_chain_id(1B) | qc_epoch(8B) | qc_state_root(32B) | num_sigs(1B)
  | [validator_id(1B) + signature(64B)] * num_sigs
```

- **Validación de límites en `decode_snapshot`:**
  - `has_qc` distinto de `0x00`/`0x01` -> error de decodificación (`SnapshotCorrupto`).
  - `num_sigs` acotado (por ejemplo `<= 64` y `num_sigs as usize <= MAX_QC_SIGNATURES`) para evitar asignaciones de memoria arbitrarias ante payloads maliciosos.
  - Si faltan bytes al final (snapshot v4 heredado de un peer sin actualizar), `decode_snapshot` rechaza en producción cuando `epoch > 0` (por `g7a: FaltaCertificadoQuorum`), manteniendo el rechazo seguro por defecto (*fail-closed*).

---

## 6. Integración con `PeerGuard` (Defensa Activa)

En concordancia con `DISENO_DEFENSA_RED.md`:
- Un peer que envíe un `MSG_SYNC` rechazado por `g7b` o `g7c` (QC falsificado, `state_root` manipulado, firma inválida o validadores duplicados) incurre en `StrikeKind::SnapshotInvalido` -> **baneo inmediato de 60 minutos**.
- Un peer que envíe un `MSG_VOTE` con firma criptográfica inválida o `validator_id` inexistente incurre en `StrikeKind::FirmaInvalida` / `SnapshotInvalido` (strike inmediato), protegiendo al `VoteCollector` contra inundación de votos basura.
- Un peer que envíe un `MSG_SYNC` con `g7a` (sin QC, por ejemplo un nodo antiguo v4 intentando sincronizar una época `> 0`) es rechazado sin aplicar su estado y recibe un strike estándar por snapshot inválido.

---

## 7. Checklist de Pruebas Bloqueantes (Checkpoints 39–41)

Ninguno de los Checkpoints 39, 40 o 41 podrá cerrarse sin que estas pruebas compilen y pasen en `cargo test --release`:

### Checkpoint 39 (`quorum.rs`)
- [x] `test_qc_valido_2_de_3`: QC con 2 firmas válidas distintas de un `ValidatorSet` 2-de-3 verifica correctamente.
- [x] `test_qc_rechaza_umbral_insuficiente`: QC con 1 firma válida en esquema 2-de-3 falla con `InsufficientQuorum`.
- [x] `test_qc_rechaza_validador_duplicado`: QC con la misma firma válida repetida 2 veces falla con `DuplicateValidator`.
- [x] `test_qc_rechaza_firma_alterada`: QC con 1 bit alterado en `signature` o `state_root` falla con `InvalidSignature`.
- [x] `test_qc_rechaza_validador_desconocido`: QC firmado por clave fuera de `ValidatorSet` falla con `UnknownValidator`.
- [x] `test_qc_rechaza_chain_id_distinto`: QC válido en `Testnet` falla al verificarse en `Mainnet` con `ChainIdMismatch`.

### Checkpoint 40 (`state.rs` Gate `g7` + Snapshot v5)
- [x] `test_g7_rechaza_epoch_sin_qc`: `merge_if_fresher` rechaza snapshot con `epoch > 0` sin `QuorumCertificate` (`FaltaCertificadoQuorum`).
- [x] `test_g7_rechaza_qc_root_distinto`: snapshot cuyo contenido fue alterado después de firmar el QC falla en `g7` (`CertificadoQuorumInvalido`).
- [x] `test_g7_acepta_snapshot_con_qc_valido`: `encode_snapshot` -> `decode_snapshot` (v5) -> `merge_if_fresher` acepta estado con QC 2-de-3 válido y actualiza `epoch` y `latest_qc`.
- [x] `test_decode_snapshot_v5_bounds`: payload truncado o `num_sigs` excesivo retorna error limpio sin pánico.

### Checkpoint 41 (`network.rs` `MSG_VOTE` + `PeerGuard`)
- [x] `test_msg_vote_forma_qc_al_alcanzar_umbral`: recepción de 2 `MSG_VOTE` válidos en `VoteCollector` produce automáticamente un `QuorumCertificate` verificable.
- [x] `test_msg_vote_invalido_penaliza_peer`: `MSG_VOTE` con firma inválida no se agrega al `VoteCollector` y registra strike en `PeerGuard`.
- [x] `test_sync_con_qc_falso_banea_peer`: recepción de `MSG_SYNC` que falla `g7` dispara baneo en `PeerGuard`.
