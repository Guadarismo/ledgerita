//! Consenso BFT ligero mediante Certificados de Quórum (`QuorumCertificate`).
//!
//! Implementa las estructuras, firma canónica con separación de dominio
//! (`LEDGERITA_QC_V1`), verificación pura ed25519 ($M$-de-$N$) y serialización
//! binaria determinista especificadas en `handoffs/DISENO_QUORUM_BFT.md`.

use std::collections::{HashMap, HashSet};

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use thiserror::Error;

/// Etiqueta de separación de dominio para firmas de Certificado de Quórum.
pub const QC_DOMAIN_TAG: &[u8] = b"LEDGERITA_QC_V1";

/// Longitud fija del mensaje canónico firmado por cada validador (15 + 1 + 8 + 32).
pub const QC_SIGNING_MSG_LEN: usize = 56;

/// Límite máximo de firmas permitidas en un `QuorumCertificate` (anti-DoS).
pub const MAX_QC_SIGNATURES: usize = 64;

/// Tamaño fijo de la cabecera de un QC serializado: chain_id(1) + epoch(8) + root(32) + num_sigs(1).
const QC_HEADER_LEN: usize = 1 + 8 + 32 + 1;

/// Tamaño fijo de cada entrada de firma en un QC serializado: validator_id(1) + signature(64).
const QC_SIG_ENTRY_LEN: usize = 1 + 64;

/// Tamaño fijo del payload de un mensaje `MSG_VOTE`: chain_id(1) + epoch(8) + root(32) + validator_id(1) + sig(64).
pub const VOTE_PAYLOAD_LEN: usize = 1 + 8 + 32 + 1 + 64;

/// Errores de validación, verificación o decodificación de quórum.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum QuorumError {
    #[error("el conjunto de validadores no puede estar vacío")]
    EmptyValidatorSet,
    #[error("el umbral ({threshold}) debe estar entre 1 y {num_validators}")]
    InvalidThreshold { threshold: u8, num_validators: usize },
    #[error("demasiados validadores ({0} > {MAX_QC_SIGNATURES})")]
    TooManyValidators(usize),
    #[error("id de validador duplicado en ValidatorSet: {0}")]
    DuplicateValidatorIdInSet(u8),
    #[error("clave pública duplicada en ValidatorSet para validador {0}")]
    DuplicatePubkeyInSet(u8),
    #[error("clave pública ed25519 inválida para validador {0}")]
    InvalidValidatorPubkey(u8),
    #[error("chain_id no coincide (esperado {expected}, recibido {got})")]
    ChainIdMismatch { expected: u8, got: u8 },
    #[error("epoch no coincide (esperado {expected}, recibido {got})")]
    EpochMismatch { expected: u64, got: u64 },
    #[error("state_root no coincide con el certificado")]
    StateRootMismatch,
    #[error("validador duplicado en el certificado: {0}")]
    DuplicateValidator(u8),
    #[error("validador desconocido en el certificado: {0}")]
    UnknownValidator(u8),
    #[error("firma ed25519 inválida del validador {0}")]
    InvalidSignature(u8),
    #[error("quórum insuficiente: {got} firmas válidas, se requieren {required}")]
    InsufficientQuorum { got: usize, required: u8 },
    #[error("certificado binario corrupto o truncado")]
    MalformedEncoding,
    #[error("número de firmas ({0}) excede el máximo permitido ({MAX_QC_SIGNATURES})")]
    TooManySignatures(usize),
}

/// Información pública de un validador autorizado dentro de un `ValidatorSet`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatorInfo {
    /// Índice único del validador (0..N-1).
    pub id: u8,
    /// Clave pública ed25519 (32 bytes).
    pub pubkey: [u8; 32],
}

/// Conjunto determinista de validadores autorizados y umbral $M$-de-$N$.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatorSet {
    pub validators: Vec<ValidatorInfo>,
    pub threshold: u8,
}

impl ValidatorSet {
    /// Construye y valida las invariantes de un `ValidatorSet`.
    pub fn new(validators: Vec<ValidatorInfo>, threshold: u8) -> Result<Self, QuorumError> {
        if validators.is_empty() {
            return Err(QuorumError::EmptyValidatorSet);
        }
        if validators.len() > MAX_QC_SIGNATURES {
            return Err(QuorumError::TooManyValidators(validators.len()));
        }
        if threshold == 0 || (threshold as usize) > validators.len() {
            return Err(QuorumError::InvalidThreshold {
                threshold,
                num_validators: validators.len(),
            });
        }

        let mut seen_ids = HashSet::with_capacity(validators.len());
        let mut seen_keys = HashSet::with_capacity(validators.len());
        for v in &validators {
            if !seen_ids.insert(v.id) {
                return Err(QuorumError::DuplicateValidatorIdInSet(v.id));
            }
            if !seen_keys.insert(v.pubkey) {
                return Err(QuorumError::DuplicatePubkeyInSet(v.id));
            }
            VerifyingKey::from_bytes(&v.pubkey)
                .map_err(|_| QuorumError::InvalidValidatorPubkey(v.id))?;
        }

        Ok(Self {
            validators,
            threshold,
        })
    }

    /// Construye un `ValidatorSet` asignando IDs secuenciales `0..N-1` a una lista de claves públicas.
    pub fn from_pubkeys(pubkeys: &[[u8; 32]], threshold: u8) -> Result<Self, QuorumError> {
        let validators = pubkeys
            .iter()
            .enumerate()
            .map(|(idx, pk)| ValidatorInfo {
                id: idx as u8,
                pubkey: *pk,
            })
            .collect();
        Self::new(validators, threshold)
    }

    /// Busca un validador por su `id`.
    pub fn get(&self, id: u8) -> Option<&ValidatorInfo> {
        self.validators.iter().find(|v| v.id == id)
    }

    /// Busca un validador por su clave pública ed25519.
    pub fn find_by_pubkey(&self, pubkey: &[u8; 32]) -> Option<&ValidatorInfo> {
        self.validators.iter().find(|v| &v.pubkey == pubkey)
    }

    /// Parsea un archivo de configuración de validadores en formato texto:
    /// ```text
    /// # Comentarios ignorados
    /// threshold=2
    /// 0=<64 hex chars pubkey>
    /// 1=<64 hex chars pubkey>
    /// ```
    pub fn parse_config(text: &str) -> Result<Self, QuorumError> {
        let mut threshold: Option<u8> = None;
        let mut validators = Vec::new();

        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                return Err(QuorumError::MalformedEncoding);
            };
            let key = k.trim();
            let val = v.trim();
            if key == "threshold" {
                let m: u8 = val.parse().map_err(|_| QuorumError::MalformedEncoding)?;
                threshold = Some(m);
            } else {
                let id: u8 = key.parse().map_err(|_| QuorumError::MalformedEncoding)?;
                let pubkey = parse_hex_32(val).ok_or(QuorumError::InvalidValidatorPubkey(id))?;
                validators.push(ValidatorInfo { id, pubkey });
            }
        }

        let threshold = threshold.ok_or(QuorumError::InvalidThreshold {
            threshold: 0,
            num_validators: validators.len(),
        })?;
        Self::new(validators, threshold)
    }
}

/// Decodifica exactamente 64 caracteres hexadecimales a `[u8; 32]`.
pub fn parse_hex_32(hex: &str) -> Option<[u8; 32]> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
        let s = std::str::from_utf8(chunk).ok()?;
        out[i] = u8::from_str_radix(s, 16).ok()?;
    }
    Some(out)
}

/// Firma individual de un validador dentro de un `QuorumCertificate`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatorSignature {
    pub validator_id: u8,
    pub signature: [u8; 64],
}

/// Certificado de Quórum ($M$-de-$N$ firmas ed25519) sobre un `state_root` de época.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuorumCertificate {
    pub chain_id: u8,
    pub epoch: u64,
    pub state_root: [u8; 32],
    pub signatures: Vec<ValidatorSignature>,
}

/// Construye el mensaje canónico de 56 bytes con separación de dominio para firmar/verificar un QC.
///
/// Formato: `b"LEDGERITA_QC_V1" || chain_id(1B) || epoch(8B BE) || state_root(32B)`.
pub fn qc_signing_message(chain_id: u8, epoch: u64, state_root: &[u8; 32]) -> [u8; QC_SIGNING_MSG_LEN] {
    let mut msg = [0u8; QC_SIGNING_MSG_LEN];
    let tag_len = QC_DOMAIN_TAG.len();
    msg[..tag_len].copy_from_slice(QC_DOMAIN_TAG);
    msg[tag_len] = chain_id;
    msg[tag_len + 1..tag_len + 9].copy_from_slice(&epoch.to_be_bytes());
    msg[tag_len + 9..].copy_from_slice(state_root);
    msg
}

/// Firma un voto de quórum para `(chain_id, epoch, state_root)` usando la clave privada del validador.
pub fn sign_qc_vote(
    signing_key: &SigningKey,
    chain_id: u8,
    epoch: u64,
    state_root: &[u8; 32],
    validator_id: u8,
) -> ValidatorSignature {
    let msg = qc_signing_message(chain_id, epoch, state_root);
    let sig = signing_key.sign(&msg);
    ValidatorSignature {
        validator_id,
        signature: sig.to_bytes(),
    }
}

/// Verifica una firma individual de voto contra el `ValidatorSet`.
pub fn verify_vote_signature(
    chain_id: u8,
    epoch: u64,
    state_root: &[u8; 32],
    vote: &ValidatorSignature,
    vset: &ValidatorSet,
) -> Result<(), QuorumError> {
    let validator = vset
        .get(vote.validator_id)
        .ok_or(QuorumError::UnknownValidator(vote.validator_id))?;
    let vk = VerifyingKey::from_bytes(&validator.pubkey)
        .map_err(|_| QuorumError::InvalidValidatorPubkey(vote.validator_id))?;
    let sig = Signature::from_bytes(&vote.signature);
    let msg = qc_signing_message(chain_id, epoch, state_root);
    vk.verify_strict(&msg, &sig)
        .map_err(|_| QuorumError::InvalidSignature(vote.validator_id))
}

/// Función pura de verificación de un `QuorumCertificate` contra el estado esperado y `ValidatorSet`.
pub fn verify_qc(
    qc: &QuorumCertificate,
    expected_chain_id: u8,
    expected_epoch: u64,
    expected_root: &[u8; 32],
    vset: &ValidatorSet,
) -> Result<(), QuorumError> {
    qc.verify(expected_chain_id, expected_epoch, expected_root, vset)
}

impl QuorumCertificate {
    /// Verifica estrictamente que el certificado pertenezca a la cadena, época y `state_root`
    /// esperados, sin validadores duplicados ni desconocidos, con firmas ed25519 estrictas y
    /// alcanzando el umbral `vset.threshold`.
    pub fn verify(
        &self,
        expected_chain_id: u8,
        expected_epoch: u64,
        expected_root: &[u8; 32],
        vset: &ValidatorSet,
    ) -> Result<(), QuorumError> {
        if self.chain_id != expected_chain_id {
            return Err(QuorumError::ChainIdMismatch {
                expected: expected_chain_id,
                got: self.chain_id,
            });
        }
        if self.epoch != expected_epoch {
            return Err(QuorumError::EpochMismatch {
                expected: expected_epoch,
                got: self.epoch,
            });
        }
        if &self.state_root != expected_root {
            return Err(QuorumError::StateRootMismatch);
        }
        if self.signatures.len() > MAX_QC_SIGNATURES {
            return Err(QuorumError::TooManySignatures(self.signatures.len()));
        }

        let mut seen = HashSet::with_capacity(self.signatures.len());
        let mut valid_count = 0usize;

        for sig in &self.signatures {
            if !seen.insert(sig.validator_id) {
                return Err(QuorumError::DuplicateValidator(sig.validator_id));
            }
            verify_vote_signature(self.chain_id, self.epoch, &self.state_root, sig, vset)?;
            valid_count += 1;
        }

        if valid_count < vset.threshold as usize {
            return Err(QuorumError::InsufficientQuorum {
                got: valid_count,
                required: vset.threshold,
            });
        }

        Ok(())
    }

    /// Serialización binaria compacta y determinista.
    ///
    /// Formato: `chain_id(1B) | epoch(8B BE) | state_root(32B) | num_sigs(1B) | [validator_id(1B) + sig(64B)]*`
    pub fn encode(&self) -> Result<Vec<u8>, QuorumError> {
        if self.signatures.len() > MAX_QC_SIGNATURES {
            return Err(QuorumError::TooManySignatures(self.signatures.len()));
        }
        let total_len = QC_HEADER_LEN + self.signatures.len() * QC_SIG_ENTRY_LEN;
        let mut out = Vec::with_capacity(total_len);
        out.push(self.chain_id);
        out.extend_from_slice(&self.epoch.to_be_bytes());
        out.extend_from_slice(&self.state_root);
        out.push(self.signatures.len() as u8);
        for sig in &self.signatures {
            out.push(sig.validator_id);
            out.extend_from_slice(&sig.signature);
        }
        Ok(out)
    }

    /// Decodifica un `QuorumCertificate` desde el inicio de `data` y devuelve `(qc, bytes_leidos)`.
    pub fn decode_prefix(data: &[u8]) -> Result<(Self, usize), QuorumError> {
        if data.len() < QC_HEADER_LEN {
            return Err(QuorumError::MalformedEncoding);
        }
        let chain_id = data[0];
        let epoch = u64::from_be_bytes(
            data[1..9]
                .try_into()
                .map_err(|_| QuorumError::MalformedEncoding)?,
        );
        let mut state_root = [0u8; 32];
        state_root.copy_from_slice(&data[9..41]);
        let num_sigs = data[41] as usize;

        if num_sigs > MAX_QC_SIGNATURES {
            return Err(QuorumError::TooManySignatures(num_sigs));
        }

        let needed = QC_HEADER_LEN + num_sigs * QC_SIG_ENTRY_LEN;
        if data.len() < needed {
            return Err(QuorumError::MalformedEncoding);
        }

        let mut signatures = Vec::with_capacity(num_sigs);
        let mut offset = QC_HEADER_LEN;
        for _ in 0..num_sigs {
            let validator_id = data[offset];
            let mut sig_bytes = [0u8; 64];
            sig_bytes.copy_from_slice(&data[offset + 1..offset + QC_SIG_ENTRY_LEN]);
            signatures.push(ValidatorSignature {
                validator_id,
                signature: sig_bytes,
            });
            offset += QC_SIG_ENTRY_LEN;
        }

        Ok((
            Self {
                chain_id,
                epoch,
                state_root,
                signatures,
            },
            needed,
        ))
    }

    /// Decodifica un buffer que contiene exactamente un `QuorumCertificate` (sin bytes sobrantes).
    pub fn decode(data: &[u8]) -> Result<Self, QuorumError> {
        let (qc, read) = Self::decode_prefix(data)?;
        if read != data.len() {
            return Err(QuorumError::MalformedEncoding);
        }
        Ok(qc)
    }
}

/// Payload P2P de un voto individual (`MSG_VOTE = 0x03`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VotePayload {
    pub chain_id: u8,
    pub epoch: u64,
    pub state_root: [u8; 32],
    pub vote: ValidatorSignature,
}

impl VotePayload {
    /// Serializa el voto a exactamente 106 bytes.
    pub fn encode(&self) -> [u8; VOTE_PAYLOAD_LEN] {
        let mut buf = [0u8; VOTE_PAYLOAD_LEN];
        buf[0] = self.chain_id;
        buf[1..9].copy_from_slice(&self.epoch.to_be_bytes());
        buf[9..41].copy_from_slice(&self.state_root);
        buf[41] = self.vote.validator_id;
        buf[42..106].copy_from_slice(&self.vote.signature);
        buf
    }

    /// Decodifica un payload `MSG_VOTE` de exactamente 106 bytes.
    pub fn decode(data: &[u8]) -> Result<Self, QuorumError> {
        if data.len() != VOTE_PAYLOAD_LEN {
            return Err(QuorumError::MalformedEncoding);
        }
        let chain_id = data[0];
        let epoch = u64::from_be_bytes(
            data[1..9]
                .try_into()
                .map_err(|_| QuorumError::MalformedEncoding)?,
        );
        let mut state_root = [0u8; 32];
        state_root.copy_from_slice(&data[9..41]);
        let validator_id = data[41];
        let mut signature = [0u8; 64];
        signature.copy_from_slice(&data[42..106]);

        Ok(Self {
            chain_id,
            epoch,
            state_root,
            vote: ValidatorSignature {
                validator_id,
                signature,
            },
        })
    }
}

/// Límite de entradas `(epoch, state_root)` retenidas en `VoteCollector` (anti-OOM).
const MAX_COLLECTOR_ENTRIES: usize = 64;

/// Colector en memoria de votos `MSG_VOTE` por `(epoch, state_root)`.
///
/// Valida cada voto contra `ValidatorSet` antes de almacenarlo y ensambla un
/// `QuorumCertificate` en cuanto se alcanza `vset.threshold`.
#[derive(Debug, Default)]
pub struct VoteCollector {
    votes: HashMap<(u64, [u8; 32]), Vec<ValidatorSignature>>,
}

impl VoteCollector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Valida e inserta un voto entrante.
    ///
    /// Retorna `Ok(Some(qc))` si con este voto se alcanza (o supera) el umbral `vset.threshold`,
    /// `Ok(None)` si el voto es válido pero aún falta quórum, o `Err(QuorumError)` si el voto
    /// es inválido (firma falsa, validador desconocido, `chain_id` distinto).
    pub fn add_vote(
        &mut self,
        payload: &VotePayload,
        expected_chain_id: u8,
        vset: &ValidatorSet,
    ) -> Result<Option<QuorumCertificate>, QuorumError> {
        if payload.chain_id != expected_chain_id {
            return Err(QuorumError::ChainIdMismatch {
                expected: expected_chain_id,
                got: payload.chain_id,
            });
        }

        verify_vote_signature(
            payload.chain_id,
            payload.epoch,
            &payload.state_root,
            &payload.vote,
            vset,
        )?;

        let key = (payload.epoch, payload.state_root);
        if !self.votes.contains_key(&key) && self.votes.len() >= MAX_COLLECTOR_ENTRIES {
            // Evictar la época más antigua para acotar memoria.
            if let Some(oldest_key) = self.votes.keys().min_by_key(|(ep, _)| *ep).cloned() {
                self.votes.remove(&oldest_key);
            }
        }

        let entry = self.votes.entry(key).or_default();
        if !entry
            .iter()
            .any(|v| v.validator_id == payload.vote.validator_id)
        {
            entry.push(payload.vote.clone());
        }

        if entry.len() >= vset.threshold as usize {
            Ok(Some(QuorumCertificate {
                chain_id: expected_chain_id,
                epoch: payload.epoch,
                state_root: payload.state_root,
                signatures: entry.clone(),
            }))
        } else {
            Ok(None)
        }
    }

    /// Descarta votos de épocas anteriores a `min_epoch`.
    pub fn prune_below(&mut self, min_epoch: u64) {
        self.votes.retain(|(ep, _), _| *ep >= min_epoch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::NetworkId;

    fn test_keys_and_set(threshold: u8) -> (Vec<SigningKey>, ValidatorSet) {
        let keys = vec![
            SigningKey::from_bytes(&[1u8; 32]),
            SigningKey::from_bytes(&[2u8; 32]),
            SigningKey::from_bytes(&[3u8; 32]),
        ];
        let pubkeys: Vec<[u8; 32]> = keys.iter().map(|k| k.verifying_key().to_bytes()).collect();
        let vset = ValidatorSet::from_pubkeys(&pubkeys, threshold).expect("validator set válido");
        (keys, vset)
    }

    #[test]
    fn test_qc_valido_2_de_3() {
        let (keys, vset) = test_keys_and_set(2);
        let chain_id = NetworkId::Testnet.chain_id();
        let epoch = 1;
        let root = [0xAAu8; 32];

        let sig0 = sign_qc_vote(&keys[0], chain_id, epoch, &root, 0);
        let sig2 = sign_qc_vote(&keys[2], chain_id, epoch, &root, 2);

        let qc = QuorumCertificate {
            chain_id,
            epoch,
            state_root: root,
            signatures: vec![sig0, sig2],
        };

        assert_eq!(verify_qc(&qc, chain_id, epoch, &root, &vset), Ok(()));
    }

    #[test]
    fn test_qc_rechaza_umbral_insuficiente() {
        let (keys, vset) = test_keys_and_set(2);
        let chain_id = NetworkId::Testnet.chain_id();
        let epoch = 1;
        let root = [0xBBu8; 32];

        let sig0 = sign_qc_vote(&keys[0], chain_id, epoch, &root, 0);
        let qc = QuorumCertificate {
            chain_id,
            epoch,
            state_root: root,
            signatures: vec![sig0],
        };

        assert_eq!(
            verify_qc(&qc, chain_id, epoch, &root, &vset),
            Err(QuorumError::InsufficientQuorum {
                got: 1,
                required: 2
            })
        );
    }

    #[test]
    fn test_qc_rechaza_validador_duplicado() {
        let (keys, vset) = test_keys_and_set(2);
        let chain_id = NetworkId::Testnet.chain_id();
        let epoch = 2;
        let root = [0xCCu8; 32];

        let sig0 = sign_qc_vote(&keys[0], chain_id, epoch, &root, 0);
        let qc = QuorumCertificate {
            chain_id,
            epoch,
            state_root: root,
            signatures: vec![sig0.clone(), sig0],
        };

        assert_eq!(
            verify_qc(&qc, chain_id, epoch, &root, &vset),
            Err(QuorumError::DuplicateValidator(0))
        );
    }

    #[test]
    fn test_qc_rechaza_firma_alterada() {
        let (keys, vset) = test_keys_and_set(2);
        let chain_id = NetworkId::Testnet.chain_id();
        let epoch = 3;
        let root = [0xDDu8; 32];

        let sig0 = sign_qc_vote(&keys[0], chain_id, epoch, &root, 0);
        let mut sig1 = sign_qc_vote(&keys[1], chain_id, epoch, &root, 1);
        sig1.signature[0] ^= 0x01; // alteración de 1 bit

        let qc = QuorumCertificate {
            chain_id,
            epoch,
            state_root: root,
            signatures: vec![sig0, sig1],
        };

        assert_eq!(
            verify_qc(&qc, chain_id, epoch, &root, &vset),
            Err(QuorumError::InvalidSignature(1))
        );

        // También debe fallar si se altera el state_root esperado
        let mut altered_root = root;
        altered_root[31] ^= 0x01;
        let sig1_valid = sign_qc_vote(&keys[1], chain_id, epoch, &root, 1);
        let qc_valid = QuorumCertificate {
            chain_id,
            epoch,
            state_root: root,
            signatures: vec![
                sign_qc_vote(&keys[0], chain_id, epoch, &root, 0),
                sig1_valid,
            ],
        };
        assert_eq!(
            verify_qc(&qc_valid, chain_id, epoch, &altered_root, &vset),
            Err(QuorumError::StateRootMismatch)
        );
    }

    #[test]
    fn test_qc_rechaza_validador_desconocido() {
        let (keys, vset) = test_keys_and_set(2);
        let chain_id = NetworkId::Testnet.chain_id();
        let epoch = 4;
        let root = [0xEEu8; 32];

        let outsider = SigningKey::from_bytes(&[99u8; 32]);
        let sig0 = sign_qc_vote(&keys[0], chain_id, epoch, &root, 0);
        let sig_outsider = sign_qc_vote(&outsider, chain_id, epoch, &root, 9);

        let qc = QuorumCertificate {
            chain_id,
            epoch,
            state_root: root,
            signatures: vec![sig0, sig_outsider],
        };

        assert_eq!(
            verify_qc(&qc, chain_id, epoch, &root, &vset),
            Err(QuorumError::UnknownValidator(9))
        );
    }

    #[test]
    fn test_qc_rechaza_chain_id_distinto() {
        let (keys, vset) = test_keys_and_set(2);
        let testnet_id = NetworkId::Testnet.chain_id();
        let mainnet_id = NetworkId::Mainnet.chain_id();
        let epoch = 5;
        let root = [0x11u8; 32];

        let qc = QuorumCertificate {
            chain_id: testnet_id,
            epoch,
            state_root: root,
            signatures: vec![
                sign_qc_vote(&keys[0], testnet_id, epoch, &root, 0),
                sign_qc_vote(&keys[1], testnet_id, epoch, &root, 1),
            ],
        };

        assert_eq!(
            verify_qc(&qc, mainnet_id, epoch, &root, &vset),
            Err(QuorumError::ChainIdMismatch {
                expected: mainnet_id,
                got: testnet_id
            })
        );
    }

    #[test]
    fn test_qc_encode_decode_roundtrip() {
        let (keys, vset) = test_keys_and_set(2);
        let chain_id = NetworkId::Mainnet.chain_id();
        let epoch = 42;
        let root = [0x55u8; 32];

        let qc = QuorumCertificate {
            chain_id,
            epoch,
            state_root: root,
            signatures: vec![
                sign_qc_vote(&keys[0], chain_id, epoch, &root, 0),
                sign_qc_vote(&keys[2], chain_id, epoch, &root, 2),
            ],
        };

        let encoded = qc.encode().expect("encode ok");
        let decoded = QuorumCertificate::decode(&encoded).expect("decode ok");
        assert_eq!(decoded, qc);
        assert_eq!(verify_qc(&decoded, chain_id, epoch, &root, &vset), Ok(()));
    }

    #[test]
    fn test_qc_decode_rechaza_truncado_o_exceso_firmas() {
        assert_eq!(
            QuorumCertificate::decode(&[0u8; 10]),
            Err(QuorumError::MalformedEncoding)
        );

        let mut header_too_many = vec![0u8; QC_HEADER_LEN];
        header_too_many[41] = (MAX_QC_SIGNATURES + 1) as u8;
        assert_eq!(
            QuorumCertificate::decode(&header_too_many),
            Err(QuorumError::TooManySignatures(MAX_QC_SIGNATURES + 1))
        );
    }

    #[test]
    fn test_vote_payload_roundtrip() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let chain_id = NetworkId::Testnet.chain_id();
        let epoch = 9;
        let root = [0x77u8; 32];
        let payload = VotePayload {
            chain_id,
            epoch,
            state_root: root,
            vote: sign_qc_vote(&key, chain_id, epoch, &root, 1),
        };
        let bytes = payload.encode();
        let decoded = VotePayload::decode(&bytes).expect("vote decode ok");
        assert_eq!(decoded, payload);
    }
}
