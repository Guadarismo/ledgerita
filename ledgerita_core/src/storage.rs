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
}

impl Storage {
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        let db = sled::open(path)?;
        let balances = db.open_tree("balances")?;
        let nonces = db.open_tree("nonces")?;
        let meta = db.open_tree("meta")?;
        let faucet = db.open_tree("faucet")?;
        let peers = db.open_tree("peers")?;
        Ok(Storage { db, balances, nonces, meta, faucet, peers })
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

    pub fn load_meta(&self) -> Result<(u64, u64, u64), StorageError> {
        let fee_pool = self.meta.get("fee_pool")?
            .map(|v| decode_u64(&v))
            .unwrap_or(Ok(0))?;
        let tx_count = self.meta.get("tx_count")?
            .map(|v| decode_u64(&v))
            .unwrap_or(Ok(0))?;
        let epoch = self.meta.get("epoch")?
            .map(|v| decode_u64(&v))
            .unwrap_or(Ok(0))?;
        Ok((fee_pool, tx_count, epoch))
    }

    pub fn save_meta(&self, fee_pool: u64, tx_count: u64, epoch: u64) -> Result<(), StorageError> {
        self.meta.insert("fee_pool", encode_u64(fee_pool).as_ref())?;
        self.meta.insert("tx_count", encode_u64(tx_count).as_ref())?;
        self.meta.insert("epoch", encode_u64(epoch).as_ref())?;
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
        let stale_keys: Vec<Vec<u8>> = self.balances.iter()
            .filter_map(|result| {
                result.ok().and_then(|(key, _)| {
                    let addr = decode_address(&key).ok()?;
                    if !active.contains_key(&addr) { Some(key.to_vec()) } else { None }
                })
            })
            .collect();
        for key in stale_keys {
            self.balances.remove(key)?;
        }
        Ok(())
    }

    pub fn prune_nonces(&self, active: &BTreeMap<Address, u64>) -> Result<(), StorageError> {
        let stale_keys: Vec<Vec<u8>> = self.nonces.iter()
            .filter_map(|result| {
                result.ok().and_then(|(key, _)| {
                    let addr = decode_address(&key).ok()?;
                    if !active.contains_key(&addr) { Some(key.to_vec()) } else { None }
                })
            })
            .collect();
        for key in stale_keys {
            self.nonces.remove(key)?;
        }
        Ok(())
    }

    /// Total de LIT que el faucet ha entregado a `addr` (node-local, no consenso).
    pub fn faucet_given(&self, addr: &Address) -> Result<u64, StorageError> {
        Ok(self.faucet
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
        self.faucet.insert(encode_address(addr).as_ref(), encode_u64(new).as_ref())?;
        Ok(new)
    }

    pub fn flush(&self) -> Result<(), StorageError> {
        self.db.flush()?;
        Ok(())
    }
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
        std::env::temp_dir().join(format!("ledgerita_storage_test_{}_{}", std::process::id(), id))
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
            assert_eq!(peers.get(pid1), Some(&rec1), "rec1 idéntico tras el roundtrip");
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
}
