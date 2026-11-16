use std::collections::BTreeMap;
use std::path::Path;

use crate::crypto::Address;
use crate::error::{SerializationError, StorageError};
use crate::serialize::{decode_address, decode_u64, encode_address, encode_u64};

pub struct Storage {
    db: sled::Db,
    balances: sled::Tree,
    nonces: sled::Tree,
    meta: sled::Tree,
    faucet: sled::Tree,
    /// Reputación node-local por peer (unidad C, "Prueba de Tiempo").
    /// NUNCA forma parte del snapshot de consenso ni del state root.
    peers: sled::Tree,
    /// Historial node-local de TXs aplicadas (GUI). Registro de evidencia:
    /// NUNCA entra al snapshot de consenso ni viaja por MSG_SYNC.
    historia: sled::Tree,
    /// Directorio node-local de direcciones frecuentes (GUI, estructura
    /// punto 7 del checklist). NUNCA entra al snapshot de consenso.
    contactos: sled::Tree,
    /// GOTE (Checkpoint 21): participación de la época (TXs enviadas por
    /// cuenta HASTA el próximo Cierre). SÍ es consenso (entra al root y
    /// al snapshot v3) pero se transición/vacía a cada Cierre — la
    /// persistimos como el resto del estado.
    particip: sled::Tree,
}

impl Storage {
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        let db = sled::open(path)?;
        let balances = db.open_tree("balances")?;
        let nonces = db.open_tree("nonces")?;
        let meta = db.open_tree("meta")?;
        let faucet = db.open_tree("faucet")?;
        let peers = db.open_tree("peers")?;
        let historia = db.open_tree("historia")?;
        let contactos = db.open_tree("contactos")?;
        let particip = db.open_tree("particip")?;
        Ok(Storage {
            db,
            balances,
            nonces,
            meta,
            faucet,
            peers,
            historia,
            contactos,
            particip,
        })
    }

    /// Carga todos los registros node-local por peer (unit C). La clave es
    /// el PeerId en su forma multibase ASCII (`PeerId::to_string`).
    pub fn load_peers(
        &self,
    ) -> Result<std::collections::HashMap<String, crate::defense::PeerLocal>, StorageError> {
        let mut out = std::collections::HashMap::new();
        for result in self.peers.iter() {
            let (key, value) = result?;
            let pid = String::from_utf8_lossy(&key).into_owned();
            if let Some(rec) = decode_peer_local(&value) {
                out.insert(pid, rec);
            }
        }
        Ok(out)
    }

    /// Persiste (upsert) el registro node-local de un peer.
    pub fn save_peer(
        &self,
        peer_id: &str,
        rec: &crate::defense::PeerLocal,
    ) -> Result<(), StorageError> {
        let value = encode_peer_local(rec);
        self.peers
            .insert(peer_id.as_bytes().to_vec(), value.as_slice())?;
        Ok(())
    }

    /// Registra una TX aplicada (historial node-local de la GUI). La clave
    /// es el hash de la TX (idempotente: la misma TX no se duplica).
    pub fn save_tx_record(
        &self,
        tx_hash: &[u8; 32],
        rec: &TxHistoryRecord,
    ) -> Result<(), StorageError> {
        let value = encode_tx_history(rec);
        self.historia.insert(tx_hash.as_slice(), value.as_slice())?;
        Ok(())
    }

    /// Últimas `limit` TXs aplicadas, ordenadas cronológicamente del MÁS nuevo
    /// al más viejo (`ts_ms` descendente). El hash de cada una viaja en `rec.tx_hash`.
    pub fn load_tx_history(&self, limit: usize) -> Result<Vec<TxHistoryRecord>, StorageError> {
        let mut out = Vec::new();
        for result in self.historia.iter() {
            let (key, value) = result?;
            if let Some(mut rec) = decode_tx_history(&value) {
                if key.len() == 32 {
                    let mut hash = [0u8; 32];
                    hash.copy_from_slice(&key);
                    rec.tx_hash = hash;
                    out.push(rec);
                }
            }
        }
        out.sort_by(|a, b| {
            b.ts_ms
                .cmp(&a.ts_ms)
                .then_with(|| b.nonce.cmp(&a.nonce))
                .then_with(|| b.tx_hash.cmp(&a.tx_hash))
        });
        out.truncate(limit);
        Ok(out)
    }

    // ── Directorio node-local de direcciones frecuentes (GUI, punto 7) ──

    /// Todos los contactos guardados, ordenados por dirección. La clave es
    /// la dirección (32B): guardar la misma dirección con otra etiqueta es
    /// un upsert (idempotente). Registros corruptos se saltan sin panic.
    pub fn load_contactos(&self) -> Result<Vec<(Address, String)>, StorageError> {
        let mut out = Vec::new();
        for result in self.contactos.iter() {
            let (key, value) = result?;
            if key.len() != 32 {
                continue;
            }
            if let (Ok(addr), Some(label)) = (decode_address(&key), decode_contacto_label(&value)) {
                out.push((addr, label));
            }
        }
        out.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
        Ok(out)
    }

    /// Guarda (upsert) un contacto: dirección + etiqueta visible. La
    /// etiqueta se recorta a `CONTACTO_LABEL_MAX` caracteres y `<= 4 * CONTACTO_LABEL_MAX`
    /// bytes UTF-8 válidos antes de persistir.
    pub fn save_contacto(&self, addr: &Address, label: &str) -> Result<(), StorageError> {
        let etiqueta = truncate_contacto_label(label);
        if etiqueta.is_empty() {
            return Err(StorageError::Serialization(
                SerializationError::InvalidData("etiqueta de contacto vacía"),
            ));
        }
        let value = encode_contacto_label(&etiqueta);
        self.contactos
            .insert(encode_address(addr).as_ref(), value.as_slice())?;
        Ok(())
    }

    /// Elimina un contacto. Devuelve si existía (idempotente para la GUI).
    pub fn delete_contacto(&self, addr: &Address) -> Result<bool, StorageError> {
        self.contactos
            .remove(encode_address(addr).as_ref())
            .map(|opt| opt.is_some())
            .map_err(StorageError::from)
    }

    pub fn load_balances(&self) -> Result<BTreeMap<Address, u64>, StorageError> {
        let mut map = BTreeMap::new();
        for result in self.balances.iter() {
            let (key, value) = result?;
            let addr = decode_address(&key)?;
            let bal = decode_u64(&value)?;
            map.insert(addr, bal);
        }
        Ok(map)
    }

    pub fn load_nonces(&self) -> Result<BTreeMap<Address, u64>, StorageError> {
        let mut map = BTreeMap::new();
        for result in self.nonces.iter() {
            let (key, value) = result?;
            let addr = decode_address(&key)?;
            let nonce = decode_u64(&value)?;
            map.insert(addr, nonce);
        }
        Ok(map)
    }

    /// GOTE (Checkpoint 21): participación de la época persistida.
    pub fn load_particip(&self) -> Result<BTreeMap<Address, u64>, StorageError> {
        let mut map = BTreeMap::new();
        for result in self.particip.iter() {
            let (key, value) = result?;
            let addr = decode_address(&key)?;
            let cuenta = decode_u64(&value)?;
            map.insert(addr, cuenta);
        }
        Ok(map)
    }

    pub fn apply_particip(&self, addr: &Address, cuenta: u64) -> Result<(), StorageError> {
        let key = encode_address(addr);
        let value = encode_u64(cuenta);
        self.particip.insert(key.as_ref(), value.as_ref())?;
        Ok(())
    }

    pub fn prune_particip(&self, active: &BTreeMap<Address, u64>) -> Result<(), StorageError> {
        prune_tree(&self.particip, active)
    }

    /// GOTE (D7): emisión histórica del tesoro en la 5ª clave de meta.
    /// `None` = base de era ANTERIOR (Checkpoint 19, sin la clave): el
    /// caller (State::load) decide la seed de migración (constitución B:
    /// supply congelada al redeploy). No hay default 0 aquí ADEMÁS, para
    /// que el caller distinga "emisión 0 real" de "clave ausente" — el
    /// re-seedeo doble desincronizaría nodos.
    pub fn load_emitido_total(&self) -> Result<Option<u64>, StorageError> {
        Ok(self
            .meta
            .get("emitido_total")?
            .map(|v| decode_u64(&v))
            .transpose()?)
    }

    pub fn save_emitido_total(&self, emitido: u64) -> Result<(), StorageError> {
        self.meta
            .insert("emitido_total", encode_u64(emitido).as_ref())?;
        Ok(())
    }

    pub fn load_meta(&self) -> Result<(u64, u64, u64, u64), StorageError> {
        let fee_pool = self
            .meta
            .get("fee_pool")?
            .map(|v| decode_u64(&v))
            .unwrap_or(Ok(0))?;
        let tx_count = self
            .meta
            .get("tx_count")?
            .map(|v| decode_u64(&v))
            .unwrap_or(Ok(0))?;
        let epoch = self
            .meta
            .get("epoch")?
            .map(|v| decode_u64(&v))
            .unwrap_or(Ok(0))?;
        // Deuda 3b (diseño aprobado): fees quemados en los Cierres. Los
        // estados de era ANTERIOR no tienen la clave → default 0 (migración
        // natural: el primer flush persiste el formato de 4 claves).
        let fees_quemados = self
            .meta
            .get("fees_quemados")?
            .map(|v| decode_u64(&v))
            .unwrap_or(Ok(0))?;
        Ok((fee_pool, tx_count, epoch, fees_quemados))
    }

    pub fn save_meta(
        &self,
        fee_pool: u64,
        tx_count: u64,
        epoch: u64,
        fees_quemados: u64,
    ) -> Result<(), StorageError> {
        self.meta
            .insert("fee_pool", encode_u64(fee_pool).as_ref())?;
        self.meta
            .insert("tx_count", encode_u64(tx_count).as_ref())?;
        self.meta.insert("epoch", encode_u64(epoch).as_ref())?;
        self.meta
            .insert("fees_quemados", encode_u64(fees_quemados).as_ref())?;
        Ok(())
    }

    pub fn apply_balance(&self, addr: &Address, val: u64) -> Result<(), StorageError> {
        let key = encode_address(addr);
        let value = encode_u64(val);
        self.balances.insert(key.as_ref(), value.as_ref())?;
        Ok(())
    }

    pub fn apply_nonce(&self, addr: &Address, val: u64) -> Result<(), StorageError> {
        let key = encode_address(addr);
        let value = encode_u64(val);
        self.nonces.insert(key.as_ref(), value.as_ref())?;
        Ok(())
    }

    pub fn prune_balances(&self, active: &BTreeMap<Address, u64>) -> Result<(), StorageError> {
        prune_tree(&self.balances, active)
    }

    pub fn prune_nonces(&self, active: &BTreeMap<Address, u64>) -> Result<(), StorageError> {
        prune_tree(&self.nonces, active)
    }

    /// Persiste todo el estado de consenso (`balances`, `nonces`, `particip`, `meta`)
    /// en una sola transacción multi-árbol atómica de `sled` y vacía buffers a disco.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_state(
        &self,
        balances: &BTreeMap<Address, u64>,
        nonces: &BTreeMap<Address, u64>,
        particip: &BTreeMap<Address, u64>,
        fee_pool: u64,
        tx_count: u64,
        epoch: u64,
        fees_quemados: u64,
        emitido_total: u64,
    ) -> Result<(), StorageError> {
        self.commit_state_inner(
            balances,
            nonces,
            particip,
            fee_pool,
            tx_count,
            epoch,
            fees_quemados,
            emitido_total,
            None,
        )
    }

    /// Variante de `commit_state` (Checkpoint 49) que persiste además de forma
    /// atómica los metadatos BFT intra-época (`latest_qc`, `last_checkpoint_root`,
    /// `epoch_txs`) y poda `"epoch_txs"` de `sled` cuando está vacío.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_state_with_quorum(
        &self,
        balances: &BTreeMap<Address, u64>,
        nonces: &BTreeMap<Address, u64>,
        particip: &BTreeMap<Address, u64>,
        fee_pool: u64,
        tx_count: u64,
        epoch: u64,
        fees_quemados: u64,
        emitido_total: u64,
        latest_qc: Option<&crate::quorum::QuorumCertificate>,
        last_checkpoint_root: Option<&[u8; 32]>,
        epoch_txs_bytes: &[u8],
    ) -> Result<(), StorageError> {
        let qc_encoded = match latest_qc {
            Some(qc) => Some(qc.encode().map_err(|_| {
                StorageError::Serialization(SerializationError::InvalidData(
                    "invalid QuorumCertificate while encoding for storage",
                ))
            })?),
            None => None,
        };
        self.commit_state_inner(
            balances,
            nonces,
            particip,
            fee_pool,
            tx_count,
            epoch,
            fees_quemados,
            emitido_total,
            Some((qc_encoded.as_deref(), last_checkpoint_root, epoch_txs_bytes)),
        )
    }

    pub fn load_latest_qc(
        &self,
    ) -> Result<Option<crate::quorum::QuorumCertificate>, StorageError> {
        match self.meta.get("latest_qc")? {
            Some(v) => crate::quorum::QuorumCertificate::decode(&v)
                .map(Some)
                .map_err(|_| {
                    StorageError::Serialization(SerializationError::InvalidData(
                        "corrupt latest_qc in meta",
                    ))
                }),
            None => Ok(None),
        }
    }

    pub fn load_last_checkpoint_root(&self) -> Result<Option<[u8; 32]>, StorageError> {
        match self.meta.get("last_checkpoint_root")? {
            Some(v) => {
                if v.len() != 32 {
                    return Err(StorageError::Serialization(SerializationError::InvalidData(
                        "corrupt last_checkpoint_root length in meta",
                    )));
                }
                let mut root = [0u8; 32];
                root.copy_from_slice(&v);
                Ok(Some(root))
            }
            None => Ok(None),
        }
    }

    pub fn load_epoch_txs(&self) -> Result<Vec<u8>, StorageError> {
        Ok(self
            .meta
            .get("epoch_txs")?
            .map(|v| v.to_vec())
            .unwrap_or_default())
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_state_inner(
        &self,
        balances: &BTreeMap<Address, u64>,
        nonces: &BTreeMap<Address, u64>,
        particip: &BTreeMap<Address, u64>,
        fee_pool: u64,
        tx_count: u64,
        epoch: u64,
        fees_quemados: u64,
        emitido_total: u64,
        quorum_meta: Option<(Option<&[u8]>, Option<&[u8; 32]>, &[u8])>,
    ) -> Result<(), StorageError> {
        use sled::Transactional;

        let bal_batch = build_u64_map_batch(&self.balances, balances);
        let non_batch = build_u64_map_batch(&self.nonces, nonces);
        let par_batch = build_u64_map_batch(&self.particip, particip);

        let mut meta_batch = sled::Batch::default();
        meta_batch.insert("fee_pool", encode_u64(fee_pool).as_ref());
        meta_batch.insert("tx_count", encode_u64(tx_count).as_ref());
        meta_batch.insert("epoch", encode_u64(epoch).as_ref());
        meta_batch.insert("fees_quemados", encode_u64(fees_quemados).as_ref());
        meta_batch.insert("emitido_total", encode_u64(emitido_total).as_ref());
        if let Some((qc_bytes, lcr, epoch_txs_bytes)) = quorum_meta {
            match qc_bytes {
                Some(qb) => meta_batch.insert("latest_qc", qb),
                None => meta_batch.remove("latest_qc"),
            }
            match lcr {
                Some(root) => meta_batch.insert("last_checkpoint_root", root.as_slice()),
                None => meta_batch.remove("last_checkpoint_root"),
            }
            if !epoch_txs_bytes.is_empty() {
                meta_batch.insert("epoch_txs", epoch_txs_bytes);
            } else {
                meta_batch.remove("epoch_txs");
            }
        }

        (&self.balances, &self.nonces, &self.particip, &self.meta)
            .transaction(|(t_bal, t_non, t_par, t_meta)| {
                t_bal.apply_batch(&bal_batch)?;
                t_non.apply_batch(&non_batch)?;
                t_par.apply_batch(&par_batch)?;
                t_meta.apply_batch(&meta_batch)?;
                Ok::<(), sled::transaction::ConflictableTransactionError<sled::Error>>(())
            })
            .map_err(|e| match e {
                sled::transaction::TransactionError::Abort(err)
                | sled::transaction::TransactionError::Storage(err) => StorageError::Db(err),
            })?;

        self.db.flush()?;
        Ok(())
    }

    /// Total de LIT que el faucet ha entregado a `addr` (node-local, no consenso).
    pub fn faucet_given(&self, addr: &Address) -> Result<u64, StorageError> {
        Ok(self
            .faucet
            .get(encode_address(addr).as_ref())?
            .map(|v| decode_u64(&v))
            .transpose()?
            .unwrap_or(0))
    }

    /// Suma `amount` al total entregado por el faucet a `addr` y lo persiste.
    pub fn add_faucet_given(&self, addr: &Address, amount: u64) -> Result<u64, StorageError> {
        let prev = self.faucet_given(addr)?;
        let new = prev.checked_add(amount).ok_or_else(|| {
            StorageError::Serialization(SerializationError::InvalidData("faucet given overflow"))
        })?;
        self.faucet
            .insert(encode_address(addr).as_ref(), encode_u64(new).as_ref())?;
        Ok(new)
    }

    pub fn flush(&self) -> Result<(), StorageError> {
        self.db.flush()?;
        Ok(())
    }
}

fn collect_stale_keys(tree: &sled::Tree, active: &BTreeMap<Address, u64>) -> Vec<Vec<u8>> {
    tree.iter()
        .filter_map(|result| {
            result.ok().and_then(|(key, _)| {
                let addr = decode_address(&key).ok()?;
                if !active.contains_key(&addr) {
                    Some(key.to_vec())
                } else {
                    None
                }
            })
        })
        .collect()
}

fn build_u64_map_batch(tree: &sled::Tree, active: &BTreeMap<Address, u64>) -> sled::Batch {
    let mut batch = sled::Batch::default();
    for key in collect_stale_keys(tree, active) {
        batch.remove(key);
    }
    for (addr, val) in active {
        batch.insert(encode_address(addr).as_ref(), encode_u64(*val).as_ref());
    }
    batch
}

fn prune_tree(tree: &sled::Tree, active: &BTreeMap<Address, u64>) -> Result<(), StorageError> {
    for key in collect_stale_keys(tree, active) {
        tree.remove(key)?;
    }
    Ok(())
}

/// Registro node-local de una TX aplicada (historial para la GUI).
/// `kind`: 0 = TX normal de la red, 1 = mint del faucet. El hash va en la
/// CLAVE del registro (`tx_hash`, rellenado por `load_tx_history`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxHistoryRecord {
    pub tx_hash: [u8; 32],
    pub from: Address,
    pub to: Address,
    pub amount: u64,
    pub fee: u64,
    pub nonce: u64,
    /// Wall clock (UNIX millis) del momento en que la TX quedó aplicada.
    pub ts_ms: u64,
    pub kind: u64,
}

/// Codificación estable: TAG (slot 0) + tipo + amount + fee + nonce + ts
/// (6×u64 big-endian) + 32 bytes remitente + 32 bytes receptor = 112 bytes.
fn encode_tx_history(rec: &TxHistoryRecord) -> Vec<u8> {
    let mut buf = Vec::with_capacity(6 * 8 + 64);
    buf.extend_from_slice(&0u64.to_be_bytes()); // versión del formato
    buf.extend_from_slice(&rec.kind.to_be_bytes());
    buf.extend_from_slice(&rec.amount.to_be_bytes());
    buf.extend_from_slice(&rec.fee.to_be_bytes());
    buf.extend_from_slice(&rec.nonce.to_be_bytes());
    buf.extend_from_slice(&rec.ts_ms.to_be_bytes());
    buf.extend_from_slice(&encode_address(&rec.from));
    buf.extend_from_slice(&encode_address(&rec.to));
    buf
}

fn decode_tx_history(data: &[u8]) -> Option<TxHistoryRecord> {
    if data.len() != 6 * 8 + 64 {
        return None; // formato desconocido: la entrada se pierde, no es de aquí
    }
    let mut u64s = [0u64; 6];
    for (i, slot) in u64s.iter_mut().enumerate() {
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&data[i * 8..(i + 1) * 8]);
        *slot = u64::from_be_bytes(arr);
    }
    let from = decode_address(&data[48..80]).ok()?;
    let to = decode_address(&data[80..112]).ok()?;
    Some(TxHistoryRecord {
        tx_hash: [0u8; 32], // lo rellena load_tx_history con la clave
        from,
        to,
        amount: u64s[2],
        fee: u64s[3],
        nonce: u64s[4],
        ts_ms: u64s[5],
        kind: u64s[1],
    })
}

// ── Contactos: clave = dirección (32B); valor = TAG + etiqueta UTF-8 ──

/// Tope de caracteres visibles por etiqueta.
pub const CONTACTO_LABEL_MAX: usize = 48;
/// Tope de contactos (directorio visible nunca fuera de control).
pub const CONTACTO_MAX: usize = 50;

/// Recorta una etiqueta de contacto manteniendo límites de caracteres y bytes UTF-8
/// compatibles con `decode_contacto_label` (`<= 4 * CONTACTO_LABEL_MAX` bytes).
pub fn truncate_contacto_label(label: &str) -> String {
    let trimmed = label.trim();
    let max_bytes = 4 * CONTACTO_LABEL_MAX;
    let mut out = String::new();
    for ch in trimmed.chars().take(CONTACTO_LABEL_MAX) {
        if out.len() + ch.len_utf8() > max_bytes {
            break;
        }
        out.push(ch);
    }
    out.trim_end().to_string()
}

fn encode_contacto_label(label: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + CONTACTO_LABEL_MAX);
    buf.push(0u8); // TAG de versión del formato
    buf.extend_from_slice(label.as_bytes());
    buf
}

/// `None` si el formato es desconocido (el registro se pierde, no panic).
fn decode_contacto_label(data: &[u8]) -> Option<String> {
    if data.is_empty() || data[0] != 0 {
        return None;
    }
    let label = std::str::from_utf8(&data[1..]).ok()?;
    let trimmed = label.trim();
    if trimmed.is_empty() || trimmed.len() > 4 * CONTACTO_LABEL_MAX {
        return None;
    }
    Some(trimmed.to_string())
}

/// Codificación del registro `PeerLocal` node-local: 7×u64 big-endian
/// (el primero es TAG de versión de formato, hoy siempre 0). Formato
/// estable: los nodos deben releer sus registros sin migración.
fn encode_peer_local(rec: &crate::defense::PeerLocal) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8 * 7);
    buf.extend_from_slice(&0u64.to_be_bytes()); // versión del formato
    buf.extend_from_slice(&rec.primera_conexion_ms.to_be_bytes());
    buf.extend_from_slice(&rec.pings_ok.to_be_bytes());
    buf.extend_from_slice(&rec.pings_fallo.to_be_bytes());
    buf.extend_from_slice(&rec.tx_aceptadas.to_be_bytes());
    buf.extend_from_slice(&rec.snapshots_servidos.to_be_bytes());
    buf.extend_from_slice(&rec.strikes_totales.to_be_bytes());
    buf
}

fn decode_peer_local(data: &[u8]) -> Option<crate::defense::PeerLocal> {
    if data.len() != 8 * 7 {
        return None; // formato desconocido: el registro se pierde, no se pisa con líos
    }
    let mut u64s = [0u64; 7];
    for (i, slot) in u64s.iter_mut().enumerate() {
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&data[i * 8..(i + 1) * 8]);
        *slot = u64::from_be_bytes(arr);
    }
    Some(crate::defense::PeerLocal {
        primera_conexion_ms: u64s[1],
        pings_ok: u64s[2],
        pings_fallo: u64s[3],
        tx_aceptadas: u64s[4],
        snapshots_servidos: u64s[5],
        strikes_totales: u64s[6],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    use std::sync::atomic::{AtomicU64, Ordering};
    static TEST_CTR: AtomicU64 = AtomicU64::new(0);

    fn test_dir() -> std::path::PathBuf {
        let id = TEST_CTR.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "ledgerita_storage_test_{}_{}",
            std::process::id(),
            id
        ))
    }

    #[test]
    fn test_balance_write_read() {
        let dir = test_dir();
        let _ = std::fs::remove_dir_all(&dir);

        let sk = SigningKey::from_bytes(&[1u8; 32]);
        let addr = Address::from_signing_key(&sk);

        {
            let storage = Storage::open(&dir).unwrap();
            storage.apply_balance(&addr, 15000).unwrap();
            storage.flush().unwrap();
        }

        {
            let storage = Storage::open(&dir).unwrap();
            let balances = storage.load_balances().unwrap();
            assert_eq!(balances.get(&addr), Some(&15000));
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_nonce_write_read() {
        let dir = test_dir();
        let _ = std::fs::remove_dir_all(&dir);

        let sk = SigningKey::from_bytes(&[2u8; 32]);
        let addr = Address::from_signing_key(&sk);

        {
            let storage = Storage::open(&dir).unwrap();
            storage.apply_nonce(&addr, 42).unwrap();
            storage.flush().unwrap();
        }

        {
            let storage = Storage::open(&dir).unwrap();
            let nonces = storage.load_nonces().unwrap();
            assert_eq!(nonces.get(&addr), Some(&42));
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_state_full_roundtrip() {
        let dir = test_dir();
        let _ = std::fs::remove_dir_all(&dir);

        let sk1 = SigningKey::from_bytes(&[3u8; 32]);
        let sk2 = SigningKey::from_bytes(&[4u8; 32]);
        let sk3 = SigningKey::from_bytes(&[5u8; 32]);
        let addr1 = Address::from_signing_key(&sk1);
        let addr2 = Address::from_signing_key(&sk2);
        let addr3 = Address::from_signing_key(&sk3);

        {
            let storage = Storage::open(&dir).unwrap();
            storage.apply_balance(&addr1, 100).unwrap();
            storage.apply_balance(&addr2, 200).unwrap();
            storage.apply_balance(&addr3, 300).unwrap();
            storage.apply_nonce(&addr1, 5).unwrap();
            storage.apply_nonce(&addr3, 10).unwrap();
            storage.flush().unwrap();
        }

        {
            let storage = Storage::open(&dir).unwrap();
            let balances = storage.load_balances().unwrap();
            let nonces = storage.load_nonces().unwrap();
            assert_eq!(balances.len(), 3);
            assert_eq!(*balances.get(&addr1).unwrap(), 100);
            assert_eq!(*balances.get(&addr2).unwrap(), 200);
            assert_eq!(*balances.get(&addr3).unwrap(), 300);
            assert_eq!(*nonces.get(&addr1).unwrap(), 5);
            assert_eq!(nonces.get(&addr2), None);
            assert_eq!(*nonces.get(&addr3).unwrap(), 10);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_flush_persistence() {
        let dir = test_dir();
        let _ = std::fs::remove_dir_all(&dir);

        let sk = SigningKey::from_bytes(&[6u8; 32]);
        let addr = Address::from_signing_key(&sk);

        let storage = Storage::open(&dir).unwrap();
        storage.apply_balance(&addr, 999).unwrap();
        storage.flush().unwrap();
        drop(storage);

        let storage2 = Storage::open(&dir).unwrap();
        let balances = storage2.load_balances().unwrap();
        assert_eq!(*balances.get(&addr).unwrap(), 999);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_prune_removes_stale_balances() {
        let dir = test_dir();
        let _ = std::fs::remove_dir_all(&dir);

        let sk1 = SigningKey::from_bytes(&[7u8; 32]);
        let sk2 = SigningKey::from_bytes(&[8u8; 32]);
        let addr1 = Address::from_signing_key(&sk1);
        let addr2 = Address::from_signing_key(&sk2);

        let storage = Storage::open(&dir).unwrap();
        storage.apply_balance(&addr1, 100).unwrap();
        storage.apply_balance(&addr2, 200).unwrap();
        storage.flush().unwrap();
        assert_eq!(storage.load_balances().unwrap().len(), 2);

        let mut active = BTreeMap::new();
        active.insert(addr1.clone(), 300);
        storage.prune_balances(&active).unwrap();
        storage.apply_balance(&addr1, 300).unwrap();
        storage.flush().unwrap();

        let balances = storage.load_balances().unwrap();
        assert_eq!(balances.len(), 1, "addr2 debería haber sido podado");
        assert_eq!(*balances.get(&addr1).unwrap(), 300);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_prune_removes_stale_nonces() {
        let dir = test_dir();
        let _ = std::fs::remove_dir_all(&dir);

        let sk1 = SigningKey::from_bytes(&[9u8; 32]);
        let sk2 = SigningKey::from_bytes(&[10u8; 32]);
        let addr1 = Address::from_signing_key(&sk1);
        let addr2 = Address::from_signing_key(&sk2);

        let storage = Storage::open(&dir).unwrap();
        storage.apply_nonce(&addr1, 5).unwrap();
        storage.apply_nonce(&addr2, 10).unwrap();
        storage.flush().unwrap();
        assert_eq!(storage.load_nonces().unwrap().len(), 2);

        let mut active = BTreeMap::new();
        active.insert(addr1.clone(), 6);
        storage.prune_nonces(&active).unwrap();
        storage.apply_nonce(&addr1, 6).unwrap();
        storage.flush().unwrap();

        let nonces = storage.load_nonces().unwrap();
        assert_eq!(nonces.len(), 1, "addr2 nonce debería haber sido podado");
        assert_eq!(*nonces.get(&addr1).unwrap(), 6);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_peers_node_local_roundtrip() {
        // Unidad C: la reputación node-local por peer sobrevive al reinicio
        // del nodo (save → reopen → load).
        let dir = test_dir();
        let _ = std::fs::remove_dir_all(&dir);

        let pid1 = "12D3KooWGtestpeeruno00000000000000000000000000";
        let rec1 = crate::defense::PeerLocal {
            primera_conexion_ms: 1_700_000_000_000,
            pings_ok: 33,
            pings_fallo: 1,
            tx_aceptadas: 12,
            snapshots_servidos: 7,
            strikes_totales: 2,
        };
        let pid2 = "12D3KooWGtestpeerdos000000000000000000000000000";
        let rec2 = crate::defense::PeerLocal::default();

        {
            let storage = Storage::open(&dir).unwrap();
            storage.save_peer(pid1, &rec1).unwrap();
            storage.save_peer(pid2, &rec2).unwrap();
            storage.flush().unwrap();
        }

        {
            // "Reinicio" del nodo: nueva apertura del mismo tree.
            let storage = Storage::open(&dir).unwrap();
            let peers = storage.load_peers().unwrap();
            assert_eq!(peers.len(), 2, "ambos registros deben sobrevivir");
            assert_eq!(
                peers.get(pid1),
                Some(&rec1),
                "rec1 idéntico tras el roundtrip"
            );
            assert_eq!(peers.get(pid2), Some(&rec2));
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_decode_peer_local_formato_desconocido() {
        // Un registro corrupto/de otro formato se pierde calladamente:
        // load_peers nunca debe panic ni pisar con líos.
        assert!(decode_peer_local(&[]).is_none());
        assert!(decode_peer_local(&[0u8; 8 * 6]).is_none()); // corto
        assert!(decode_peer_local(&[0u8; 8 * 8]).is_none()); // largo

        // Un registro VÁLIDO con chunks de longitud exacta sí decodifica.
        // Slots: [0]=versión, [1]=primera_conexion_ms, [2]=pings_ok,
        // [3]=pings_fallo, [4]=tx_aceptadas, [5]=snapshots, [6]=strikes.
        let mut data = Vec::new();
        data.extend_from_slice(&0u64.to_be_bytes()); // versión
        for n in 1..=6u64 {
            data.extend_from_slice(&n.to_be_bytes());
        }
        let rec = decode_peer_local(&data).unwrap();
        assert_eq!(rec.primera_conexion_ms, 1);
        assert_eq!(rec.pings_ok, 2);
        assert_eq!(rec.strikes_totales, 6);
    }

    #[test]
    fn test_tx_history_roundtrip() {
        // Historial node-local de TXs: save → reopen → load (más nuevo
        // primero y con el hash de la clave en el registro).
        let dir = test_dir();
        let _ = std::fs::remove_dir_all(&dir);
        let sk1 = SigningKey::from_bytes(&[0xa1u8; 32]);
        let sk2 = SigningKey::from_bytes(&[0xa2u8; 32]);
        let addr1 = Address::from_signing_key(&sk1);
        let addr2 = Address::from_signing_key(&sk2);

        let rec1 = TxHistoryRecord {
            tx_hash: [7u8; 32],
            from: addr1.clone(),
            to: addr2.clone(),
            amount: 500_000,
            fee: 10,
            nonce: 3,
            ts_ms: 1_760_000_000_000,
            kind: 0,
        };
        let rec2 = TxHistoryRecord {
            tx_hash: [9u8; 32],
            from: addr2.clone(),
            to: addr1.clone(),
            amount: 25_000,
            fee: 7,
            nonce: 1,
            ts_ms: 1_760_000_900_000,
            kind: 1, // faucet
        };

        {
            let storage = Storage::open(&dir).unwrap();
            storage.save_tx_record(&rec1.tx_hash, &rec1).unwrap();
            storage.save_tx_record(&rec2.tx_hash, &rec2).unwrap();
            storage.flush().unwrap();
        }

        {
            let storage = Storage::open(&dir).unwrap();
            let hist = storage.load_tx_history(10).unwrap();
            assert_eq!(hist.len(), 2, "dos TXs en el historial");
            let newest = &hist[0];
            assert_eq!(newest.tx_hash, rec2.tx_hash, "la más nueva primero");
            assert_eq!(newest.from, addr2);
            assert_eq!(newest.to, addr1);
            assert_eq!(newest.amount, 25_000);
            assert_eq!(newest.fee, 7);
            assert_eq!(newest.nonce, 1);
            assert_eq!(newest.ts_ms, 1_760_000_900_000);
            assert_eq!(newest.kind, 1);
            let oldest = &hist[1];
            assert_eq!(oldest.tx_hash, rec1.tx_hash);
            assert_eq!(oldest.from, addr1);
            assert_eq!(oldest.to, addr2);
            assert_eq!(oldest.amount, 500_000);
            assert_eq!(oldest.kind, 0);
        }

        // Idempotente: la misma TX re-registrada no duplica entrada.
        {
            let storage = Storage::open(&dir).unwrap();
            storage.save_tx_record(&rec1.tx_hash, &rec1).unwrap();
            assert_eq!(storage.load_tx_history(10).unwrap().len(), 2);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_tx_history_limite_y_basura() {
        // El límite corta al MÁS nuevo y los registros corruptos se saltan
        // sin panic ni contagio.
        let dir = test_dir();
        let _ = std::fs::remove_dir_all(&dir);
        let sk = SigningKey::from_bytes(&[0xb1u8; 32]);
        let addr = Address::from_signing_key(&sk);

        {
            let storage = Storage::open(&dir).unwrap();
            for i in 0..5u64 {
                let rec = TxHistoryRecord {
                    tx_hash: [i as u8; 32],
                    from: addr.clone(),
                    to: addr.clone(),
                    amount: i,
                    fee: 0,
                    nonce: i,
                    ts_ms: 1_000 + i,
                    kind: 0,
                };
                storage.save_tx_record(&rec.tx_hash, &rec).unwrap();
            }
            // Un registro corrupto de longitud desconocida.
            storage
                .historia
                .insert([42u8; 32].as_slice(), b"basura".as_slice())
                .unwrap();
            storage.flush().unwrap();
        }

        {
            let storage = Storage::open(&dir).unwrap();
            let hist = storage.load_tx_history(3).unwrap();
            assert_eq!(hist.len(), 3, "límite respetado");
            assert_eq!(hist[0].tx_hash, [4u8; 32], "el más nuevo primero");
            assert_eq!(hist[2].tx_hash, [2u8; 32]);
            // Con límite mayor, la basura no cuenta (5 legítimas).
            let hist = storage.load_tx_history(50).unwrap();
            assert_eq!(hist.len(), 5, "la entrada corrupta no aparece");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_contactos_roundtrip_upsert_delete_y_basura() {
        // Directorio de direcciones frecuentes: upsert por dirección,
        // etiquetas UTF-8, registros corruptos se saltan y el delete
        // reporta si existía.
        let dir = test_dir();
        let _ = std::fs::remove_dir_all(&dir);
        let a1 = Address::from_signing_key(&SigningKey::from_bytes(&[0xd1u8; 32]));
        let a2 = Address::from_signing_key(&SigningKey::from_bytes(&[0xd2u8; 32]));

        {
            let storage = Storage::open(&dir).unwrap();
            storage.save_contacto(&a1, "kettle02 TestNet").unwrap();
            storage.save_contacto(&a2, "kettle03 seed").unwrap();
            assert_eq!(storage.load_contactos().unwrap().len(), 2);
            // Upsert: misma dirección, etiqueta nueva.
            storage
                .save_contacto(&a2, "seed TestNet (kettle03)")
                .unwrap();
            assert_eq!(storage.load_contactos().unwrap().len(), 2);
            // Basura: clave corta y clave válida con valor sin TAG.
            storage
                .contactos
                .insert(b"xyz".as_slice(), b"junk")
                .unwrap();
            storage
                .contactos
                .insert([9u8; 32].as_slice(), b"sin TAG".as_slice())
                .unwrap();
            storage.flush().unwrap();
        }
        {
            let storage = Storage::open(&dir).unwrap();
            let contactos = storage.load_contactos().unwrap();
            assert_eq!(
                contactos.len(),
                2,
                "la basura no entra al directorio ni corrompe el resto"
            );
            let (a2_rec, label) = contactos
                .iter()
                .find(|(a, _)| *a == a2)
                .cloned()
                .expect("el contacto con upsert sobrevive al reopen");
            assert_eq!(a2_rec, a2);
            assert_eq!(label, "seed TestNet (kettle03)");
            // Orden alfabético por etiqueta.
            assert!(contactos[0].1 <= contactos[1].1, "ordenado por etiqueta");
            // Delete: la primera vez informa que existía, la segunda que ya no.
            assert!(storage.delete_contacto(&a1).unwrap());
            assert!(!storage.delete_contacto(&a1).unwrap());
            assert_eq!(storage.load_contactos().unwrap().len(), 1);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_ckpt55_tx_history_orden_cronologico_y_contacto_largo_utf8() {
        let dir = test_dir();
        let _ = std::fs::remove_dir_all(&dir);
        let a1 = Address::from_signing_key(&SigningKey::from_bytes(&[0xd3u8; 32]));
        let storage = Storage::open(&dir).unwrap();

        // Hashes en orden opuesto a timestamps: [0xff;32] antiguo, [0x01;32] más nuevo
        storage
            .save_tx_record(
                &[0xffu8; 32],
                &TxHistoryRecord {
                    tx_hash: [0xffu8; 32],
                    from: a1.clone(),
                    to: a1.clone(),
                    amount: 10,
                    fee: 1,
                    nonce: 1,
                    ts_ms: 1_000,
                    kind: 0,
                },
            )
            .unwrap();
        storage
            .save_tx_record(
                &[0x01u8; 32],
                &TxHistoryRecord {
                    tx_hash: [0x01u8; 32],
                    from: a1.clone(),
                    to: a1.clone(),
                    amount: 20,
                    fee: 1,
                    nonce: 2,
                    ts_ms: 5_000,
                    kind: 0,
                },
            )
            .unwrap();
        let hist = storage.load_tx_history(10).unwrap();
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0].tx_hash, [0x01u8; 32], "el más reciente por ts_ms va primero");
        assert_eq!(hist[1].tx_hash, [0xffu8; 32]);

        // Etiqueta larga UTF-8 (> 192 bytes) se recorta limpio y recarga sin perderse
        let larga = "🚀".repeat(80);
        storage.save_contacto(&a1, &larga).unwrap();
        let contactos = storage.load_contactos().unwrap();
        assert_eq!(contactos.len(), 1);
        assert!(contactos[0].1.chars().count() <= CONTACTO_LABEL_MAX);
        assert!(contactos[0].1.len() <= 4 * CONTACTO_LABEL_MAX);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_meta_migra_estado_de_era_vieja() {
        // Deuda 3b: un estado llevado por binario ANTERIOR tiene meta de 3
        // claves (sin fees_quemados). El binario nuevo lo debe cargar con
        // quemados=0 (migración natural, sin corromper nada).
        let dir = test_dir();
        let _ = std::fs::remove_dir_all(&dir);
        {
            let storage = Storage::open(&dir).unwrap();
            // Escritura "era vieja": solo las 3 claves clásicas.
            storage
                .meta
                .insert("fee_pool".as_bytes(), encode_u64(777).as_ref())
                .unwrap();
            storage
                .meta
                .insert("tx_count".as_bytes(), encode_u64(8192).as_ref())
                .unwrap();
            storage
                .meta
                .insert("epoch".as_bytes(), encode_u64(2).as_ref())
                .unwrap();
            storage.flush().unwrap();
        }
        {
            let storage = Storage::open(&dir).unwrap();
            let (pool, tx, epoch, quemados) = storage.load_meta().unwrap();
            assert_eq!((pool, tx, epoch), (777, 8192, 2));
            assert_eq!(quemados, 0, "era sin burn → default 0");
            // Y el save nuevo persiste la 4ª clave sin pisar nada.
            storage.save_meta(778, 8193, 2, 500).unwrap();
            storage.flush().unwrap();
        }
        {
            let storage = Storage::open(&dir).unwrap();
            assert_eq!(storage.load_meta().unwrap(), (778, 8193, 2, 500));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_commit_state_atomic_and_prune() {
        let dir = test_dir();
        let _ = std::fs::remove_dir_all(&dir);

        let a1 = Address::from_signing_key(&SigningKey::from_bytes(&[0x51; 32]));
        let a2 = Address::from_signing_key(&SigningKey::from_bytes(&[0x52; 32]));
        let a3 = Address::from_signing_key(&SigningKey::from_bytes(&[0x53; 32]));

        let storage = Storage::open(&dir).unwrap();
        let mut bals = BTreeMap::new();
        bals.insert(a1.clone(), 100);
        bals.insert(a2.clone(), 200);
        let mut nonces = BTreeMap::new();
        nonces.insert(a1.clone(), 3);
        let mut particip = BTreeMap::new();
        particip.insert(a1.clone(), 1);
        storage
            .commit_state(&bals, &nonces, &particip, 10, 5, 1, 20, 500_000)
            .unwrap();

        // Segunda escritura atómica que poda a2 y actualiza meta
        bals.remove(&a2);
        bals.insert(a3.clone(), 300);
        nonces.insert(a3.clone(), 1);
        particip.clear();
        storage
            .commit_state(&bals, &nonces, &particip, 15, 6, 1, 20, 500_000)
            .unwrap();
        drop(storage);

        let reopened = Storage::open(&dir).unwrap();
        let b2 = reopened.load_balances().unwrap();
        assert_eq!(b2.len(), 2);
        assert_eq!(b2.get(&a1), Some(&100));
        assert!(!b2.contains_key(&a2));
        assert_eq!(b2.get(&a3), Some(&300));
        assert_eq!(reopened.load_nonces().unwrap().get(&a3), Some(&1));
        assert!(reopened.load_particip().unwrap().is_empty());
        assert_eq!(reopened.load_meta().unwrap(), (15, 6, 1, 20));
        assert_eq!(reopened.load_emitido_total().unwrap(), Some(500_000));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_persist_quorum_y_epoch_txs_con_poda() {
        let dir = test_dir();
        let _ = std::fs::remove_dir_all(&dir);

        let sk_v0 = SigningKey::from_bytes(&[0x61; 32]);
        let root1 = [0x11u8; 32];
        let root2 = [0x22u8; 32];
        let qc = crate::quorum::QuorumCertificate {
            chain_id: 2,
            epoch: 1,
            state_root: root1,
            signatures: vec![crate::quorum::sign_qc_vote(&sk_v0, 2, 1, &root1, 0)],
        };
        let journal = vec![0xDEu8, 0xAD, 0xBE, 0xEF];

        {
            let storage = Storage::open(&dir).unwrap();
            let bals = BTreeMap::new();
            let nonces = BTreeMap::new();
            let particip = BTreeMap::new();
            storage
                .commit_state_with_quorum(
                    &bals,
                    &nonces,
                    &particip,
                    10,
                    4097,
                    1,
                    0,
                    500_000,
                    Some(&qc),
                    Some(&root1),
                    &journal,
                )
                .unwrap();
        }

        {
            let reopened = Storage::open(&dir).unwrap();
            assert_eq!(reopened.load_latest_qc().unwrap(), Some(qc));
            assert_eq!(reopened.load_last_checkpoint_root().unwrap(), Some(root1));
            assert_eq!(reopened.load_epoch_txs().unwrap(), journal);

            // Cierre de la siguiente época: poda `"epoch_txs"` y reemplaza el ancla.
            let bals = BTreeMap::new();
            let nonces = BTreeMap::new();
            let particip = BTreeMap::new();
            reopened
                .commit_state_with_quorum(
                    &bals,
                    &nonces,
                    &particip,
                    0,
                    8192,
                    2,
                    10,
                    1_000_000,
                    None,
                    Some(&root2),
                    &[],
                )
                .unwrap();
        }

        let reopened2 = Storage::open(&dir).unwrap();
        assert_eq!(reopened2.load_latest_qc().unwrap(), None);
        assert_eq!(reopened2.load_last_checkpoint_root().unwrap(), Some(root2));
        assert!(reopened2.load_epoch_txs().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
