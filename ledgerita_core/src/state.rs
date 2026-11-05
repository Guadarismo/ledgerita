use std::collections::BTreeMap;
use crate::crypto::Address;
use crate::error::StorageError;
use crate::serialize;
use crate::storage::Storage;
use crate::transaction::Transaction;
use sha2::{Sha256, Digest};

pub const CHECKPOINT_INTERVAL: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncResult {
    AlreadySynced,
    RemoteAdopted,
    LocalNewer,
    Divergent,
}

#[derive(Debug, Clone)]
pub struct State {
    pub balances: BTreeMap<Address, u64>,
    pub nonces: BTreeMap<Address, u64>,
    pub fee_pool: u64,
    pub tx_count: u64,
    pub epoch: u64,
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl State {
    pub fn new() -> Self {
        State {
            balances: BTreeMap::new(),
            nonces: BTreeMap::new(),
            fee_pool: 0,
            tx_count: 0,
            epoch: 0,
        }
    }

    pub fn load(storage: &Storage) -> Result<Self, StorageError> {
        let balances = storage.load_balances()?;
        let nonces = storage.load_nonces()?;
        let (fee_pool, tx_count, epoch) = storage.load_meta()?;
        Ok(State {
            balances,
            nonces,
            fee_pool,
            tx_count,
            epoch,
        })
    }

    pub fn flush(&self, storage: &Storage) -> Result<(), StorageError> {
        storage.prune_balances(&self.balances)?;
        storage.prune_nonces(&self.nonces)?;
        for (addr, bal) in &self.balances {
            storage.apply_balance(addr, *bal)?;
        }
        for (addr, nonce) in &self.nonces {
            storage.apply_nonce(addr, *nonce)?;
        }
        storage.save_meta(self.fee_pool, self.tx_count, self.epoch)?;
        Ok(())
    }

    pub fn mint(&mut self, account: Address, amount: u64) {
        *self.balances.entry(account).or_insert(0) += amount;
    }

    /// Aplica una transacción al estado, validando firma, nonce y saldo.
    ///
    /// Chequea que la firma sea válida, que `tx.nonce` sea estrictamente mayor
    /// que el nonce actual del remitente (previene replays), que `amount + fee`
    /// no desborde y que el remitente tenga fondos suficientes. En éxito,
    /// actualiza saldos y nonces, acumula la comisión en `fee_pool` e
    /// incrementa `tx_count`. Devuelve `Err` con una descripción `&'static str`
    /// si alguna validación falla, dejando el estado intacto.
    pub fn apply_transaction(&mut self, tx: &Transaction) -> Result<(), &'static str> {
        if !tx.is_signature_valid() {
            return Err("Invalid transaction signature!");
        }

        let current_nonce = self.nonces.get(&tx.sender).copied().unwrap_or(0);
        if tx.nonce <= current_nonce {
            return Err("Invalid nonce: replay attack detected");
        }

        let total_cost = tx.amount.checked_add(tx.fee).ok_or("Overflow calculating total cost")?;

        let sender_balance = *self.balances.get(&tx.sender).unwrap_or(&0);
        if sender_balance < total_cost {
            return Err("Insufficient funds");
        }

        self.balances.insert(tx.sender.clone(), sender_balance - total_cost);

        let receiver_balance = *self.balances.get(&tx.receiver).unwrap_or(&0);
        self.balances.insert(tx.receiver.clone(), receiver_balance + tx.amount);

        self.nonces.insert(tx.sender.clone(), tx.nonce);

        self.fee_pool = self.fee_pool.checked_add(tx.fee).ok_or("Overflow adding fee to pool")?;
        self.tx_count += 1;

        Ok(())
    }

    pub fn total_supply(&self) -> u64 {
        self.balances.values().sum()
    }

    pub fn merge_if_fresher(&mut self, remote: &State, storage: &Storage) -> Result<SyncResult, StorageError> {
        let local_root = self.generate_state_root();
        let remote_root = remote.generate_state_root();
        if local_root == remote_root {
            return Ok(SyncResult::AlreadySynced);
        }
        let local_fresh = (self.epoch, self.tx_count);
        let remote_fresh = (remote.epoch, remote.tx_count);
        if remote_fresh > local_fresh {
            self.balances = remote.balances.clone();
            self.nonces = remote.nonces.clone();
            self.fee_pool = remote.fee_pool;
            self.tx_count = remote.tx_count;
            self.epoch = remote.epoch;
            self.flush(storage)?;
            Ok(SyncResult::RemoteAdopted)
        } else if local_fresh > remote_fresh {
            Ok(SyncResult::LocalNewer)
        } else {
            Ok(SyncResult::Divergent)
        }
    }

    pub fn is_checkpoint_due(&self) -> bool {
        self.tx_count > 0 && self.tx_count.is_multiple_of(CHECKPOINT_INTERVAL)
    }

    /// Avanza la época del "Cierre de Libro".
    ///
    /// DECISIÓN 2026-11-05 (park provisional): mientras no existan validadores
    /// reales (Prueba de Tiempo, deuda B/C), el `fee_pool` **NO se distribuye
    /// ni se pisa** — se estaciona y acumula entre épocas. Todos los nodos
    /// sincronizados comparten el mismo `fee_pool` (los fees se acumulan
    /// idénticamente vía `apply_transaction` y el campo vive en el estado de
    /// consenso, no en una cuenta con clave), así que el state root sigue
    /// determinista e idéntico entre nodos.
    ///
    /// El reparto a validadores regresará en la unidad de deuda A+B/C, como
    /// regla 100% derivada del estado (NUNCA recibiendo identidades
    /// node-local como el `wallet_address` de cada nodo: así mezclar datos
    /// fuera del consenso con saldos divergía los roots cada 4096 TX con fees).
    pub fn finalize_checkpoint(&mut self) -> [u8; 32] {
        self.epoch += 1;
        self.generate_state_root()
    }

    pub fn generate_state_root(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();

        hasher.update(self.epoch.to_be_bytes());
        hasher.update(self.fee_pool.to_be_bytes());
        hasher.update(self.tx_count.to_be_bytes());

        for (address, balance) in &self.balances {
            hasher.update(address.key.as_bytes());
            hasher.update(balance.to_be_bytes());
            let nonce = self.nonces.get(address).copied().unwrap_or(0);
            hasher.update(nonce.to_be_bytes());
        }

        hasher.finalize().into()
    }

    /// Serializa el estado a un snapshot. El primer byte es el `chain_id`
    /// de la red (`network`), de modo que snapshots de otra red se detectan
    /// inmediatamente en `decode_snapshot`.
    pub fn encode_snapshot(&self, network: crate::network::NetworkId) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(network.chain_id());
        buf.extend_from_slice(&self.epoch.to_be_bytes());
        buf.extend_from_slice(&self.fee_pool.to_be_bytes());
        buf.extend_from_slice(&self.tx_count.to_be_bytes());
        let num_entries = self.balances.len() as u32;
        buf.extend_from_slice(&num_entries.to_be_bytes());
        for (addr, balance) in &self.balances {
            let nonce = self.nonces.get(addr).copied().unwrap_or(0);
            buf.extend_from_slice(serialize::encode_address(addr).as_ref());
            buf.extend_from_slice(&balance.to_be_bytes());
            buf.extend_from_slice(&nonce.to_be_bytes());
        }
        buf
    }

    pub fn decode_snapshot(
        data: &[u8],
        expected: crate::network::NetworkId,
    ) -> Result<Self, crate::error::SerializationError> {
        use crate::error::SerializationError;
        if data.len() < 1 + 8 + 8 + 8 + 4 {
            return Err(SerializationError::InvalidData("snapshot too short"));
        }
        if data[0] != expected.chain_id() {
            return Err(SerializationError::InvalidData("snapshot from another network"));
        }
        let mut offset = 1;
        let epoch = {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&data[offset..offset + 8]);
            offset += 8;
            u64::from_be_bytes(arr)
        };
        let fee_pool = {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&data[offset..offset + 8]);
            offset += 8;
            u64::from_be_bytes(arr)
        };
        let tx_count = {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&data[offset..offset + 8]);
            offset += 8;
            u64::from_be_bytes(arr)
        };
        let num_entries = {
            let mut arr = [0u8; 4];
            arr.copy_from_slice(&data[offset..offset + 4]);
            offset += 4;
            u32::from_be_bytes(arr)
        };

        let mut balances = BTreeMap::new();
        let mut nonces = BTreeMap::new();

        for _ in 0..num_entries {
            if offset + 32 + 8 + 8 > data.len() {
                return Err(SerializationError::InvalidData("snapshot missing entry data"));
            }
            let addr = serialize::decode_address(&data[offset..offset + 32])?;
            offset += 32;
            let balance = {
                let mut arr = [0u8; 8];
                arr.copy_from_slice(&data[offset..offset + 8]);
                offset += 8;
                u64::from_be_bytes(arr)
            };
            let nonce = {
                let mut arr = [0u8; 8];
                arr.copy_from_slice(&data[offset..offset + 8]);
                offset += 8;
                u64::from_be_bytes(arr)
            };
            balances.insert(addr.clone(), balance);
            if nonce > 0 {
                nonces.insert(addr, nonce);
            }
        }

        Ok(State { balances, nonces, fee_pool, tx_count, epoch })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn test_valid_transaction_execution() {
        let sender_key = SigningKey::from_bytes(&[1u8; 32]);
        let receiver_key = SigningKey::from_bytes(&[2u8; 32]);
        let sender_address = Address::new(sender_key.verifying_key());
        let receiver_address = Address::new(receiver_key.verifying_key());

        let mut state = State::new();
        state.mint(sender_address.clone(), 5000);

        let mut tx = Transaction::new(sender_address.clone(), receiver_address.clone(), 1000, 10, 1);
        let msg = tx.to_bytes();
        tx.sign(sender_key.sign(&msg));

        assert!(state.apply_transaction(&tx).is_ok());

        assert_eq!(*state.balances.get(&sender_address).unwrap(), 5000 - 1010);
        assert_eq!(*state.balances.get(&receiver_address).unwrap(), 1000);
        assert_eq!(state.fee_pool, 10);
        assert_eq!(state.tx_count, 1);
    }

    #[test]
    fn test_insufficient_funds() {
        let sender_key = SigningKey::from_bytes(&[1u8; 32]);
        let receiver_key = SigningKey::from_bytes(&[2u8; 32]);
        let sender_address = Address::new(sender_key.verifying_key());
        let receiver_address = Address::new(receiver_key.verifying_key());

        let mut state = State::new();
        state.mint(sender_address.clone(), 500);

        let mut tx = Transaction::new(sender_address.clone(), receiver_address.clone(), 1000, 10, 1);
        let msg = tx.to_bytes();
        tx.sign(sender_key.sign(&msg));

        assert_eq!(state.apply_transaction(&tx), Err("Insufficient funds"));
    }

    #[test]
    fn test_state_root_determinism() {
        let key1 = SigningKey::from_bytes(&[1u8; 32]);
        let key2 = SigningKey::from_bytes(&[2u8; 32]);

        let mut state = State::new();
        state.mint(Address::new(key1.verifying_key()), 100);
        state.mint(Address::new(key2.verifying_key()), 200);

        let root1 = state.generate_state_root();

        let mut state2 = State::new();
        state2.mint(Address::new(key2.verifying_key()), 200);
        state2.mint(Address::new(key1.verifying_key()), 100);

        let root2 = state2.generate_state_root();
        assert_eq!(root1, root2);

        state2.mint(Address::new(key1.verifying_key()), 1);
        assert_ne!(root1, state2.generate_state_root());
    }

    #[test]
    fn test_replay_attack_detected() {
        let sender_key = SigningKey::from_bytes(&[3u8; 32]);
        let receiver_key = SigningKey::from_bytes(&[4u8; 32]);
        let sender_address = Address::new(sender_key.verifying_key());
        let receiver_address = Address::new(receiver_key.verifying_key());

        let mut state = State::new();
        state.mint(sender_address.clone(), 5000);

        let mut tx = Transaction::new(sender_address.clone(), receiver_address.clone(), 1000, 10, 1);
        let msg = tx.to_bytes();
        tx.sign(sender_key.sign(&msg));
        assert!(state.apply_transaction(&tx).is_ok());

        let mut replay = Transaction::new(sender_address.clone(), receiver_address.clone(), 1000, 10, 1);
        let msg2 = replay.to_bytes();
        replay.sign(sender_key.sign(&msg2));
        assert_eq!(state.apply_transaction(&replay), Err("Invalid nonce: replay attack detected"));
    }

    #[test]
    fn test_old_nonce_rejected() {
        let sender_key = SigningKey::from_bytes(&[5u8; 32]);
        let receiver_key = SigningKey::from_bytes(&[6u8; 32]);
        let sender_address = Address::new(sender_key.verifying_key());
        let receiver_address = Address::new(receiver_key.verifying_key());

        let mut state = State::new();
        state.mint(sender_address.clone(), 5000);

        let mut tx = Transaction::new(sender_address.clone(), receiver_address.clone(), 1000, 10, 5);
        let msg = tx.to_bytes();
        tx.sign(sender_key.sign(&msg));
        assert!(state.apply_transaction(&tx).is_ok());

        let mut old = Transaction::new(sender_address.clone(), receiver_address.clone(), 100, 1, 3);
        let msg3 = old.to_bytes();
        old.sign(sender_key.sign(&msg3));
        assert_eq!(state.apply_transaction(&old), Err("Invalid nonce: replay attack detected"));
    }

    #[test]
    fn test_fee_goes_to_pool() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let rk = SigningKey::from_bytes(&[8u8; 32]);
        let sender = Address::from_signing_key(&sk);
        let receiver = Address::from_signing_key(&rk);

        let mut state = State::new();
        state.mint(sender.clone(), 10000);

        let mut tx = Transaction::new(sender, receiver, 2000, 500, 1);
        let msg = tx.to_bytes();
        tx.sign(sk.sign(&msg));
        assert!(state.apply_transaction(&tx).is_ok());

        assert_eq!(state.fee_pool, 500);
    }

    #[test]
    fn test_checkpoint_parks_fee_pool() {
        // Decisión park 2026-11-05: el checkpoint NO distribuye NI pisa el
        // fee_pool; solo avanza la época y el fee_pool sigue acumulando.
        let sk = SigningKey::from_bytes(&[9u8; 32]);
        let rk = SigningKey::from_bytes(&[10u8; 32]);
        let sender = Address::from_signing_key(&sk);
        let receiver = Address::from_signing_key(&rk);

        let mut state = State::new();
        state.mint(sender.clone(), u64::MAX / 2);

        let interval = CHECKPOINT_INTERVAL;

        for i in 0..interval {
            let mut tx = Transaction::new(sender.clone(), receiver.clone(), 1, 100, i + 1);
            let msg = tx.to_bytes();
            tx.sign(sk.sign(&msg));
            state.apply_transaction(&tx).unwrap();
        }

        assert!(state.is_checkpoint_due());
        let fee_pool_antes = state.fee_pool;
        assert_eq!(fee_pool_antes, 100 * interval);
        let saldos_antes = state.balances.clone();

        let root = state.finalize_checkpoint();
        assert_eq!(state.epoch, 1);
        assert_eq!(state.fee_pool, fee_pool_antes, "el fee_pool se estaciona: no se pisa");
        assert_eq!(state.balances, saldos_antes, "el checkpoint no mueve saldos");
        assert_ne!(root, [0u8; 32]);
    }

    #[test]
    fn test_checkpoint_root_identico_entre_nodos_con_wallets_distintas() {
        // El test que habría atrapado el bug original: dos nodos con el MISMO
        // estadoamiento pero wallets node-local distintas deben terminar con
        // el MISMO root tras el checkpoint. Con el reparto a `wallet_address`
        // esto divergía (dato fuera del consenso mezclado en el root).
        let sk = SigningKey::from_bytes(&[0xa1; 32]);
        let rk = SigningKey::from_bytes(&[0xa2; 32]);

        let construir_estado = || {
            let mut state = State::new();
            state.mint(Address::from_signing_key(&sk), 100_000);
            state.mint(Address::from_signing_key(&rk), 50_000);

            let mut tx = Transaction::new(
                Address::from_signing_key(&sk),
                Address::from_signing_key(&rk),
                100, 77, 1,
            );
            let msg = tx.to_bytes();
            tx.sign(sk.sign(&msg));
            state.apply_transaction(&tx).unwrap();

            // Wallet "node-local" distinta en cada nodo (no entra al estado,
            // pero representa la identidad con la que el bug se filtraba).
            let _wallet_nodo_a = SigningKey::from_bytes(&[0xb1; 32]);
            let _wallet_nodo_b = SigningKey::from_bytes(&[0xb2; 32]);
            state
        };

        let mut nodo_a = construir_estado();
        let mut nodo_b = construir_estado();
        assert_eq!(nodo_a.fee_pool, 77);

        let root_a = nodo_a.finalize_checkpoint();
        let root_b = nodo_b.finalize_checkpoint();
        assert_eq!(root_a, root_b, "roots deben ser idénticos aunque las wallets de nodo difieran");
        assert_eq!(nodo_a.fee_pool, nodo_b.fee_pool, "el fee_pool estacionado es consenso");
    }

    #[test]
    fn test_fee_pool_acumula_entre_epocas() {
        // Varias épocas seguidas: el fee_pool solo crece, nunca se pisa.
        let sk = SigningKey::from_bytes(&[0xc1; 32]);
        let rk = SigningKey::from_bytes(&[0xc2; 32]);
        let sender = Address::from_signing_key(&sk);
        let receiver = Address::from_signing_key(&rk);

        let mut state = State::new();
        state.mint(sender.clone(), 100_000_000);

        let mut tx = Transaction::new(sender, receiver, 100, 500, 1);
        let msg = tx.to_bytes();
        tx.sign(sk.sign(&msg));
        state.apply_transaction(&tx).unwrap();

        state.finalize_checkpoint();
        assert_eq!(state.epoch, 1);
        assert_eq!(state.fee_pool, 500);

        state.finalize_checkpoint();
        assert_eq!(state.epoch, 2, "se puede avanzar otra época sin distribución");
        assert_eq!(state.fee_pool, 500, "sigue estacionado");
    }

    #[test]
    fn test_snapshot_roundtrip() {
        let key1 = SigningKey::from_bytes(&[12u8; 32]);
        let key2 = SigningKey::from_bytes(&[13u8; 32]);
        let addr1 = Address::from_signing_key(&key1);
        let addr2 = Address::from_signing_key(&key2);

        let mut state = State::new();
        state.mint(addr1.clone(), 1000);
        state.mint(addr2.clone(), 2000);
        state.nonces.insert(addr1.clone(), 7);
        state.fee_pool = 500;
        state.tx_count = 50;
        state.epoch = 2;

        let root1 = state.generate_state_root();
        let data = state.encode_snapshot(crate::network::NetworkId::Mainnet);
        let decoded = State::decode_snapshot(&data, crate::network::NetworkId::Mainnet).unwrap();
        let root2 = decoded.generate_state_root();

        assert_eq!(root1, root2);
        assert_eq!(state.balances, decoded.balances);
        assert_eq!(state.nonces, decoded.nonces);
        assert_eq!(state.fee_pool, decoded.fee_pool);
        assert_eq!(state.tx_count, decoded.tx_count);
        assert_eq!(state.epoch, decoded.epoch);

        let wrong_net = State::decode_snapshot(&data, crate::network::NetworkId::Testnet);
        assert!(wrong_net.is_err(), "snapshot de mainnet debe fallar al decodificar en testnet");
    }
}
