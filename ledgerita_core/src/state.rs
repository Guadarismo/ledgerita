use crate::crypto::Address;
use crate::error::StorageError;
use crate::faucet::HARD_CAP_SAT;
use crate::serialize;
use crate::storage::Storage;
use crate::transaction::Transaction;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const CHECKPOINT_INTERVAL: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncResult {
    AlreadySynced,
    RemoteAdopted,
    LocalNewer,
    Divergent,
    /// El snapshot calificaba como "más fresco" pero violó una invariante
    /// constitucional (gate de la unidad B+C+G): NO se adopta y el caller
    /// debe registrar strike grave contra el peer (design §1/§2).
    RejectedPorGate(GateReason),
}

/// Motivo de rechazo del gate de invariantes de snapshots
/// (`DISENO_DEFENSA_RED.md` §1). Cada variante es diagnosticable en el WARN
/// de red y contable como strike grave (`StrikeKind::SnapshotInvalido`, §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateReason {
    /// g1: el supply del snapshot excede el hard cap de 21M LIT.
    SupplyExcedeHardCap,
    /// g2: (epoch, tx_count) matemáticamente imposible (checkpoint cada 4096 TX).
    EpochIncoherente,
    /// g3: un nonce del snapshot retrocede respecto al estado local (reescritura de historia).
    NonceRetrocedido,
    /// g5: más cuentas nuevas que TX disponibles para crearlas (cuentas "gratis" falsas).
    CuentasImplausibles,
    /// g4' (Deuda 3b): fees_totales (pool+quemados) del snapshot MÁS fresco
    /// menores que los del local — los fees solo se mueven a pool/burn y
    /// jamás se crean ni desaparecen de la nada (bajo burn, el pool SOLO
    /// retrocede en el Cierre; el PAR es monotónico).
    FeesTotalesRetrocedidos,
}

#[derive(Debug, Clone)]
pub struct State {
    pub balances: BTreeMap<Address, u64>,
    pub nonces: BTreeMap<Address, u64>,
    pub fee_pool: u64,
    /// Total de fees DESTRUIDOS en los Cierres de Libro (deuda 3b, burn de
    /// fees decisión 2026-11-07, deflación). Vive en consenso: entra al
    /// state root y al snapshot.
    pub fees_quemados: u64,
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
            fees_quemados: 0,
            tx_count: 0,
            epoch: 0,
        }
    }

    pub fn load(storage: &Storage) -> Result<Self, StorageError> {
        let balances = storage.load_balances()?;
        let nonces = storage.load_nonces()?;
        let (fee_pool, tx_count, epoch, fees_quemados) = storage.load_meta()?;
        Ok(State {
            balances,
            nonces,
            fee_pool,
            fees_quemados,
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
        storage.save_meta(self.fee_pool, self.tx_count, self.epoch, self.fees_quemados)?;
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

        let total_cost = tx
            .amount
            .checked_add(tx.fee)
            .ok_or("Overflow calculating total cost")?;

        let sender_balance = *self.balances.get(&tx.sender).unwrap_or(&0);
        if sender_balance < total_cost {
            return Err("Insufficient funds");
        }

        self.balances
            .insert(tx.sender.clone(), sender_balance - total_cost);

        let receiver_balance = *self.balances.get(&tx.receiver).unwrap_or(&0);
        self.balances
            .insert(tx.receiver.clone(), receiver_balance + tx.amount);

        self.nonces.insert(tx.sender.clone(), tx.nonce);

        self.fee_pool = self
            .fee_pool
            .checked_add(tx.fee)
            .ok_or("Overflow adding fee to pool")?;
        self.tx_count += 1;

        Ok(())
    }

    pub fn total_supply(&self) -> u64 {
        self.balances.values().sum()
    }

    /// Gate de invariantes para snapshots entrantes (unidad B+C+G, Prioridad 1).
    ///
    /// Un snapshot que califique como "más fresco" se valida por COMPLETO
    /// antes de adoptar: si viola una invariante constitucional se rechaza
    /// SIN tocar el estado local (no se adopta, no se reenvía, el estado
    /// queda exactamente como estaba). Todas las reglas son funciones puras
    /// de (local, remote): cero aleatoriedad, cero dato node-local — la
    /// lección del hallazgo A es ley.
    ///
    /// Refinamiento sobre el borrador del diseño: `merge_if_fresher` corre el
    /// gate SOLO en la rama de adopción (snapshot más fresco que el local).
    /// Un snapshot atrasado pero honesto (nonces/supply menores que el local)
    /// no es una mentira: no se adopta por frescura y castigarlo con strikes
    /// banearía peers legítimos rezagados. En la rama `Divergent` (misma
    /// frescura, distinto root) tampoco corre el gate: la divergencia es el
    /// problema de consenso aparte (se diagnostica con su propio WARN).
    /// Deuda 3b: `network` es explícito (el gate g5 formalizó el allowance
    /// de génesis: MainNet 0 / TestNet 1 — el gate ya no adivina).
    pub fn validate_incoming_snapshot(
        &self,
        network: crate::network::NetworkId,
        remote: &State,
    ) -> Result<(), GateReason> {
        // g1 — hard cap absoluto (21M LIT, constitucional). Suma CHECKEADA:
        // un mentiroso con balances del orden de u64::MAX no debe colgar
        // el nodo con overflow.
        let mut supply: u64 = 0;
        for bal in remote.balances.values() {
            supply = match supply.checked_add(*bal) {
                Some(s) => s,
                None => return Err(GateReason::SupplyExcedeHardCap),
            };
        }
        if supply > HARD_CAP_SAT {
            return Err(GateReason::SupplyExcedeHardCap);
        }

        // g2 — coherencia epoch/TX: el epoch SOLO avanza con un checkpoint
        // (cada 4096 TX). Cota superior INCLUSIVA porque tx_count == (epoch+1)
        // * 4096 es la ventana transitoria "checkpoint due pero aún no
        // finalizado" (finalize_checkpoint corre justo después de aplicar);
        // más allá de eso es matemáticamente mentiroso.
        let lower = remote
            .epoch
            .checked_mul(CHECKPOINT_INTERVAL)
            .ok_or(GateReason::EpochIncoherente)?;
        let upper = lower.saturating_add(CHECKPOINT_INTERVAL);
        if remote.tx_count < lower || remote.tx_count > upper {
            return Err(GateReason::EpochIncoherente);
        }

        // g3 — nonces monótonicos: solo crecen con las TXs. Un snapshot "más
        // fresco" que conozca un nonce MENOR que el estado local reescribió
        // la historia. (Solo se mira la intersección de direcciones: una
        // cuenta nueva para el local no tiene nonce anterior que violar.)
        for (addr, remote_nonce) in &remote.nonces {
            if let Some(local_nonce) = self.nonces.get(addr) {
                if *remote_nonce < *local_nonce {
                    return Err(GateReason::NonceRetrocedido);
                }
            }
        }

        // g4' — fees_totales monotónicos bajo burn (Deuda 3b, reemplaza el
        // park provisional del Checkpoint 5): los fees solo tienen dos
        // destinos (pool vivo o quemado en el Cierre de Libro) y jamás se
        // crean de la nada, así que el PAR (pool + quemados) es monotónico
        // no-decreciente con las TX — el burn conserva la suma (mueve pool
        // → quemados) y las TX nuevas solo la suben. Un snapshot MÁS fresco
        // con par menor es mentira.
        let fees_totales_remoto = remote.fee_pool.saturating_add(remote.fees_quemados);
        let fees_totales_local = self.fee_pool.saturating_add(self.fees_quemados);
        // Sanity de hard cap: TODO fee nace de supply existente ≤ 21M; un
        // pool o quemados mayores son contablemente imposibles (evita que un
        // mentiroso con u64 MAX colgue a la venta por overflow).
        if remote.fee_pool > HARD_CAP_SAT
            || remote.fees_quemados > HARD_CAP_SAT
            || fees_totales_remoto > HARD_CAP_SAT
        {
            return Err(GateReason::FeesTotalesRetrocedidos);
        }
        if self.tx_count > 0 && fees_totales_remoto < fees_totales_local {
            return Err(GateReason::FeesTotalesRetrocedidos);
        }

        // g5 — cuentas plausibles: toda cuenta del snapshot desconocida para
        // el estado local debió nacer de al menos una TX/mint del propio
        // snapshot, MÁS el allow de cuentas GÉNESIS de la red (Deuda 3b:
        // formalizada en lugar del "+1" mágico — genesis::GENESIS_ACCOUNTS:
        // MainNet 0, TestNet 1 [el tesoro del faucet no nace de una TX]).
        // Un mentiroso con muchas cuentas falsas no puede "fabricarlas
        // gratis" sin inflar tx_count — y eso lo caza g2.
        let nuevas_cuentas = remote
            .balances
            .keys()
            .filter(|a| !self.balances.contains_key(*a))
            .count() as u64;
        let costo_disponible = remote
            .tx_count
            .saturating_add(self.balances.len() as u64)
            .saturating_add(crate::genesis::genesis_accounts(network));
        if nuevas_cuentas > costo_disponible {
            return Err(GateReason::CuentasImplausibles);
        }

        Ok(())
    }

    pub fn merge_if_fresher(
        &mut self,
        network: crate::network::NetworkId,
        remote: &State,
        storage: &Storage,
    ) -> Result<SyncResult, StorageError> {
        let local_root = self.generate_state_root();
        let remote_root = remote.generate_state_root();
        if local_root == remote_root {
            return Ok(SyncResult::AlreadySynced);
        }
        let local_fresh = (self.epoch, self.tx_count);
        let remote_fresh = (remote.epoch, remote.tx_count);
        if remote_fresh > local_fresh {
            // GATE (unidad B+C+G): ni siquiera se compara el resto — un
            // snapshot que viola una invariante NO se adopta y el resultado
            // le informa al caller (strike grave contra el peer, §2).
            if let Err(gate) = self.validate_incoming_snapshot(network, remote) {
                return Ok(SyncResult::RejectedPorGate(gate));
            }
            self.balances = remote.balances.clone();
            self.nonces = remote.nonces.clone();
            self.fee_pool = remote.fee_pool;
            self.fees_quemados = remote.fees_quemados;
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
    /// DECISIÓN 2026-11-07 (Deuda 3b, diseño aprobado — reemplaza el park
    /// provisional del Checkpoint 5): en cada Cierre el `fee_pool` se
    /// QUEMA — pasa íntegro a `fees_quemados` y sale del ledger para
    /// siempre (deflación; las monedas nunca vuelven). Es 100%
    /// estado-derived: cualquier nodo con el mismo estado deriva el mismo
    /// burn, determinista y auditable.
    ///
    /// La emisión (micro-goteo del tesoro virtual) NO ocurre aquí aún:
    /// empieza cuando aterrice el módulo de reparto a validadores, y sin
    /// validadores elegibles esa época no se emite nada y no se pierde el
    /// turno (el tesoro disponible SIEMPRE se deriva del estado:
    /// HARD_CAP_SAT − emitido_total − total_supply()).
    pub fn finalize_checkpoint(&mut self) -> [u8; 32] {
        // Burn de fees (deflación, decisión 2026-11-07): pool → quemados.
        // Invariantes exactas: pool_before + quemados_before == quemados_after
        // y pool_after == 0.
        let pool = self.fee_pool;
        self.fees_quemados = self.fees_quemados.saturating_add(pool);
        self.fee_pool = 0;

        self.epoch += 1;
        self.generate_state_root()
    }

    pub fn generate_state_root(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();

        hasher.update(self.epoch.to_be_bytes());
        hasher.update(self.fee_pool.to_be_bytes());
        // Deuda 3b: los fees quemados entran al root (consenso completo del
        // destino de los fees: pool vivo o destruido, nada ambiguo).
        hasher.update(self.fees_quemados.to_be_bytes());
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
        // Deuda 3b (burn de fees): el campo entra al cable. ATOMICO con el
        // redeploy: binarios mezclados NO se entienden (length-check abajo
        // rechaza el formato viejo — el deploy.ps1 actualiza todos juntos).
        buf.extend_from_slice(&self.fees_quemados.to_be_bytes());
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
        // Formato v2 (burn de fees): 1 + epoch + fee_pool + fees_quemados +
        // tx_count (4×u64) + num_entries(u32) + entradas de 72 bytes.
        if data.len() < 1 + 8 * 4 + 4 {
            return Err(SerializationError::InvalidData("snapshot too short"));
        }
        if data[0] != expected.chain_id() {
            return Err(SerializationError::InvalidData(
                "snapshot from another network",
            ));
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
        let fees_quemados = {
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
                return Err(SerializationError::InvalidData(
                    "snapshot missing entry data",
                ));
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

        Ok(State {
            balances,
            nonces,
            fee_pool,
            fees_quemados,
            tx_count,
            epoch,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::NetworkId;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn test_valid_transaction_execution() {
        let sender_key = SigningKey::from_bytes(&[1u8; 32]);
        let receiver_key = SigningKey::from_bytes(&[2u8; 32]);
        let sender_address = Address::new(sender_key.verifying_key());
        let receiver_address = Address::new(receiver_key.verifying_key());

        let mut state = State::new();
        state.mint(sender_address.clone(), 5000);

        let mut tx = Transaction::new(
            sender_address.clone(),
            receiver_address.clone(),
            1000,
            10,
            1,
        );
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

        let mut tx = Transaction::new(
            sender_address.clone(),
            receiver_address.clone(),
            1000,
            10,
            1,
        );
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

        let mut tx = Transaction::new(
            sender_address.clone(),
            receiver_address.clone(),
            1000,
            10,
            1,
        );
        let msg = tx.to_bytes();
        tx.sign(sender_key.sign(&msg));
        assert!(state.apply_transaction(&tx).is_ok());

        let mut replay = Transaction::new(
            sender_address.clone(),
            receiver_address.clone(),
            1000,
            10,
            1,
        );
        let msg2 = replay.to_bytes();
        replay.sign(sender_key.sign(&msg2));
        assert_eq!(
            state.apply_transaction(&replay),
            Err("Invalid nonce: replay attack detected")
        );
    }

    #[test]
    fn test_old_nonce_rejected() {
        let sender_key = SigningKey::from_bytes(&[5u8; 32]);
        let receiver_key = SigningKey::from_bytes(&[6u8; 32]);
        let sender_address = Address::new(sender_key.verifying_key());
        let receiver_address = Address::new(receiver_key.verifying_key());

        let mut state = State::new();
        state.mint(sender_address.clone(), 5000);

        let mut tx = Transaction::new(
            sender_address.clone(),
            receiver_address.clone(),
            1000,
            10,
            5,
        );
        let msg = tx.to_bytes();
        tx.sign(sender_key.sign(&msg));
        assert!(state.apply_transaction(&tx).is_ok());

        let mut old = Transaction::new(sender_address.clone(), receiver_address.clone(), 100, 1, 3);
        let msg3 = old.to_bytes();
        old.sign(sender_key.sign(&msg3));
        assert_eq!(
            state.apply_transaction(&old),
            Err("Invalid nonce: replay attack detected")
        );
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
    fn test_checkpoint_quema_fee_pool() {
        // Deuda 3b (burn, 2026-11-07): el Cierre QUEMA el fee_pool — pasa
        // íntegro a fees_quemados, el pool queda en 0 y las monedas jamás
        // vuelven. Invariancia exacta del traslado.
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
        assert_eq!(state.fee_pool, 100 * interval);
        assert_eq!(state.fees_quemados, 0);
        let saldos_antes = state.balances.clone();

        let root = state.finalize_checkpoint();
        assert_eq!(state.epoch, 1);
        assert_eq!(
            state.fee_pool, 0,
            "el pool queda vacío tras el burn del Cierre"
        );
        assert_eq!(
            state.fees_quemados,
            100 * interval,
            "todo lo quemado quedó contado (auditable)"
        );
        assert_eq!(
            state.balances, saldos_antes,
            "el Cierre quema fees, nunca saldos"
        );
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
                100,
                77,
                1,
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
        assert_eq!(
            root_a, root_b,
            "roots deben ser idénticos aunque las wallets de nodo difieran"
        );
        assert_eq!(
            nodo_a.fee_pool, nodo_b.fee_pool,
            "el post-burn del pool es consenso"
        );
        assert_eq!(
            nodo_a.fees_quemados, nodo_b.fees_quemados,
            "el quemado también es consenso (mismo burn derivado)"
        );
    }

    #[test]
    fn test_burn_en_cada_cierre_libro() {
        // Deuda 3b: múltiples Cierres seguidos — el burn corre EN TODOS,
        // fees_quemados acumula y el fee_pool arranca época en 0.
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
        assert_eq!(state.fee_pool, 0, "época nueva arranca sin pool (burn)");

        // Fees de la época 1: también se queman al cerrarla.
        state.tx_count = CHECKPOINT_INTERVAL - 1;
        let mut tx2 = Transaction::new(
            Address::from_signing_key(&sk),
            Address::from_signing_key(&rk),
            100,
            999,
            2,
        );
        let msg = tx2.to_bytes();
        tx2.sign(sk.sign(&msg));
        state.apply_transaction(&tx2).unwrap();
        assert_eq!(state.fee_pool, 999);

        state.finalize_checkpoint();
        assert_eq!(state.epoch, 2, "segunda época por burn");
        assert_eq!(state.fee_pool, 0);
        assert_eq!(
            state.fees_quemados,
            500 + 999,
            "el acumulado de dos Cierres es la suma exacta"
        );
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
        assert!(
            wrong_net.is_err(),
            "snapshot de mainnet debe fallar al decodificar en testnet"
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // Gate de invariantes para snapshots entrantes (unidad B+C+G, G).
    // Sólo corre en la rama de ADOPCIÓN de merge_if_fresher (snapshot más
    // fresco): ver doc del método y DISENO_DEFENSA_RED.md §1.
    // ─────────────────────────────────────────────────────────────────────

    use ed25519_dalek::SigningKey as SK;

    static GATE_CTR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn gate_storage(tag: &str) -> (Storage, std::path::PathBuf) {
        let id = GATE_CTR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "ledgerita_gate_test_{}_{}_{}",
            std::process::id(),
            tag,
            id
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (Storage::open(&dir).unwrap(), dir)
    }

    fn addr(seed: u8) -> Address {
        Address::from_signing_key(&SK::from_bytes(&[seed; 32]))
    }

    /// TestNet-like legítimo: cuenta de génesis (fee_pool/tesoro, NO cuenta
    /// como TX) + 2 goteos del faucet (cada uno cuesta 1 TX).
    fn testnet_legitimo() -> State {
        let mut s = State::new();
        s.mint(addr(0x01), 1_000_000_000_000); // génesis: gratis
        s.mint(addr(0x02), 1_000_000);
        s.mint(addr(0x03), 1_000_000);
        s.tx_count = 2; // solo los goteos cuentan
        s
    }

    /// Firma una TX 1:1 entre sender→receiver y la aplica al estado.
    fn aplicar_tx(
        state: &mut State,
        sender_sk: &SK,
        seed_receiver: u8,
        amount: u64,
        fee: u64,
        nonce: u64,
    ) {
        let sender = Address::from_signing_key(sender_sk);
        let receiver = addr(seed_receiver);
        let mut tx = Transaction::new(sender, receiver, amount, fee, nonce);
        let msg = tx.to_bytes();
        tx.sign(sender_sk.sign(&msg));
        state.apply_transaction(&tx).unwrap();
    }

    /// Desenvuelve el Result del merge para poder comparar con assert_eq
    /// (`StorageError` no es PartialEq; un error de storage aquí es panic).
    fn sync_result(r: Result<SyncResult, StorageError>) -> SyncResult {
        match r {
            Ok(x) => x,
            Err(e) => panic!("el merge falló con error de storage inesperado: {e:?}"),
        }
    }

    #[test]
    fn test_snapshot_mentiroso_supply_inflado_rejectado() {
        // g1: ningún snapshot legítimo puede superar los 21M LIT.
        let mut mentiroso = State::new();
        mentiroso.mint(addr(0x10), HARD_CAP_SAT + 1);
        mentiroso.tx_count = 1;

        let mut local = State::new();
        let (storage, dir) = gate_storage("g1");

        assert_eq!(
            sync_result(local.merge_if_fresher(NetworkId::Testnet, &mentiroso, &storage)),
            SyncResult::RejectedPorGate(GateReason::SupplyExcedeHardCap),
            "supply > hard cap debe ser rechazado por el gate"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_snapshot_supply_desbordado_rejectado() {
        // g1 con overflow: balances del orden de u64::MAX sumados deben
        // rechazarse SIN colgar el nodo (checked_add, no sum() panic).
        let mut mentiroso = State::new();
        mentiroso.mint(addr(0x11), u64::MAX / 2);
        mentiroso.mint(addr(0x12), u64::MAX / 2);
        mentiroso.tx_count = 1;

        let mut local = State::new();
        let (storage, dir) = gate_storage("g1b");

        assert_eq!(
            sync_result(local.merge_if_fresher(NetworkId::Testnet, &mentiroso, &storage)),
            SyncResult::RejectedPorGate(GateReason::SupplyExcedeHardCap),
            "overflow de la suma de balances debe rechazarse limpio"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_snapshot_epoch_mentiroso_rejectado() {
        // g2: el epoch solo avanza con checkpoints (cada 4096 TX);
        // epoch=3 con tx_count=100 es matemáticamente imposible.
        let mut mentiroso = State::new();
        mentiroso.mint(addr(0x13), 100);
        mentiroso.epoch = 3;
        mentiroso.tx_count = 100;

        let mut local = State::new();
        let (storage, dir) = gate_storage("g2");

        assert_eq!(
            sync_result(local.merge_if_fresher(NetworkId::Testnet, &mentiroso, &storage)),
            SyncResult::RejectedPorGate(GateReason::EpochIncoherente),
            "epoch por sobre sus TXs es mentira de checkpoint"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_snapshot_epoch_probado_aceptado() {
        // g2, lado bueno: ventana INCLUSIVA del checkpoint pendiente
        // (tx_count == (epoch+1)*4096, ventana transitoria real) es legítima.
        let mut legitimo = State::new();
        legitimo.mint(addr(0x14), 1_000);
        legitimo.epoch = 0;
        legitimo.tx_count = CHECKPOINT_INTERVAL; // checkpoint due, aún no finalizado

        let mut local = State::new();
        let (storage, dir) = gate_storage("g2b");

        assert_eq!(
            sync_result(local.merge_if_fresher(NetworkId::Testnet, &legitimo, &storage)),
            SyncResult::RemoteAdopted,
            "la ventana transitoria de checkpoint pendiente es legítima"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_snapshot_nonces_retrocedidos_rejectado() {
        // g3: los nonces solo crecen con las TXs; reescribirlos es historia falsa.
        let sender_sk = SK::from_bytes(&[0x20; 32]);
        let mut local = State::new();
        local.mint(Address::from_signing_key(&sender_sk), 10_000);
        aplicar_tx(&mut local, &sender_sk, 0x30, 100, 10, 5); // nonce del sender: 5

        let mut mentiroso = State::new();
        mentiroso.mint(Address::from_signing_key(&sender_sk), 10_000 - 50);
        mentiroso.mint(addr(0x31), 50);
        mentiroso
            .nonces
            .insert(Address::from_signing_key(&sender_sk), 3); // RETROCEDIDO
        mentiroso.fee_pool = 10;
        mentiroso.tx_count = 2;
        mentiroso.epoch = 0;

        let (storage, dir) = gate_storage("g3");
        assert_eq!(
            sync_result(local.merge_if_fresher(NetworkId::Testnet, &mentiroso, &storage)),
            SyncResult::RejectedPorGate(GateReason::NonceRetrocedido),
            "un nonce menor que el local en un snapshot más fresco es reescritura"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_snapshot_fees_totales_reducidos_rejectado() {
        // g4' (Deuda 3b, burn): fees_totales = pool + quemados es monotónico.
        // Un snapshot más fresco que "pierde" un satoshi de fee en el camino
        // es mentira (lo tomó pero no lo pagó ni lo quemó).
        let sender_sk = SK::from_bytes(&[0x40; 32]);
        let mut local = State::new();
        local.mint(Address::from_signing_key(&sender_sk), 10_000);
        aplicar_tx(&mut local, &sender_sk, 0x41, 100, 500, 1); // fees_totales local: 500

        let mut mentiroso = State::new();
        mentiroso.mint(Address::from_signing_key(&sender_sk), 9_900);
        mentiroso.mint(addr(0x41), 100);
        mentiroso.fee_pool = 499; // ¿¡dónde fue el otro satoshi de fee!? (sin quemados)
        mentiroso.tx_count = 2;
        mentiroso.epoch = 0;

        let (storage, dir) = gate_storage("g4_burn_mentira");
        assert_eq!(
            sync_result(local.merge_if_fresher(NetworkId::Testnet, &mentiroso, &storage)),
            SyncResult::RejectedPorGate(GateReason::FeesTotalesRetrocedidos),
            "bajo burn, fees_totales (pool+quemados) NUNCA retrocede de un snapshot más fresco"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_snapshot_con_burn_legitimo_aceptado() {
        // g4' (Deuda 3b, burn): pool que retrocede por el BURN del Cierre
        // es LEGÍTIMO — quemados compensa exactamente (fees_totales igual).
        let sender_sk = SK::from_bytes(&[0x44; 32]);
        let mut local = State::new();
        local.mint(Address::from_signing_key(&sender_sk), 10_000);
        aplicar_tx(&mut local, &sender_sk, 0x45, 100, 500, 1);
        local.fee_pool = 200; // quedaron 300 quemados de una era anterior
        local.fees_quemados = 300;

        let mut burn_nodo = State::new(); // el otro nodo ya clausuró la época
        burn_nodo.mint(Address::from_signing_key(&sender_sk), 9_900);
        burn_nodo.mint(addr(0x45), 100);
        burn_nodo.fee_pool = 0; // pool quemado en el Cierre...
        burn_nodo.fees_quemados = 500; // ...300 viejos + 200 de este pool
        burn_nodo.tx_count = CHECKPOINT_INTERVAL; // una época completa (g2 coherente)
        burn_nodo.epoch = 1;

        let (storage, dir) = gate_storage("g4_burn_legit");
        assert_eq!(
            sync_result(local.merge_if_fresher(NetworkId::Testnet, &burn_nodo, &storage)),
            SyncResult::RemoteAdopted,
            "el burn del Cierre respeta fees_totales: la adopción es honesta"
        );
        assert_eq!(local.fee_pool, 0);
        assert_eq!(local.fees_quemados, 500);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_snapshot_fees_quemados_gigantes_rejectado() {
        // g4' sanity de hard cap: pool/quemados mayores a 21M LIT son
        // contablemente imposibles (todo fee nace de supply existente).
        let mut local = State::new();
        local.tx_count = 1; // gates activos
        let mut mentiroso = State::new();
        mentiroso.tx_count = 4096; // fresco
        mentiroso.epoch = 0;
        mentiroso.fee_pool = HARD_CAP_SAT + 1; // contrabando contable

        let (storage, dir) = gate_storage("g4_gigante");
        let res = local.merge_if_fresher(NetworkId::Testnet, &mentiroso, &storage);
        assert!(
            matches!(
                res,
                Err(_)
                    | Ok(SyncResult::RejectedPorGate(
                        GateReason::FeesTotalesRetrocedidos
                    ))
            ),
            "un pool gigante no puede pasar el gate ni desbordar el nodo"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_snapshot_cuentas_gratis_rejectado() {
        // g5 (débil): cuentas nuevas mayores a las TX disponibles para
        // crearlas; el mentiroso no puede fabricar cuentas gratis.
        let mut mentiroso = State::new();
        for n in 0..10u8 {
            mentiroso.mint(addr(0x50 + n), 1);
        }
        mentiroso.tx_count = 3; // 9 cuentas nuevas con solo 3 TX + la cuenta local del mentiroso

        let mut local = State::new();
        local.mint(addr(0x50), 1_000); // comparte UNA dirección con el mentiroso
        let (storage, dir) = gate_storage("g5");

        assert_eq!(
            sync_result(local.merge_if_fresher(NetworkId::Testnet, &mentiroso, &storage)),
            SyncResult::RejectedPorGate(GateReason::CuentasImplausibles),
            "cuentas sin TX de nacimiento (más allá de la holgura génesis) son implausibles"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_snapshot_legitimo_aceptado() {
        // Regresión: génesis compartido + goteos de faucet (1 TX c/u) y
        // TXs con fees pasan el gate limpio y se adoptan.
        let legitimo = testnet_legitimo();
        let mut local = State::new(); // nodo recién nacido sin génesis aplicado
        let (storage, dir) = gate_storage("ok");

        assert_eq!(
            sync_result(local.merge_if_fresher(NetworkId::Testnet, &legitimo, &storage)),
            SyncResult::RemoteAdopted,
            "génesis + faucets legítimos pasan el gate"
        );
        assert_eq!(local.tx_count, 2);
        assert_eq!(local.balances, legitimo.balances);

        // Y una TX real con fee por encima sigue siendo adopción honesta.
        let sender_sk = SK::from_bytes(&[0x02; 32]); // cuenta con fondos del faucet
        let mut remoto2 = legitimo.clone();
        aplicar_tx(&mut remoto2, &sender_sk, 0x60, 500, 7, 1);
        assert_eq!(
            sync_result(local.merge_if_fresher(NetworkId::Testnet, &remoto2, &storage)),
            SyncResult::RemoteAdopted,
            "la TX legítima con fee trae ether... digo, fee al pool monotónico"
        );
        assert_eq!(local.fee_pool, 7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_gate_no_toca_estado_local_al_rechazar() {
        // La promesa central: el rechazo NO muta el estado local ni lo flush.
        let sender_sk = SK::from_bytes(&[0x70; 32]);
        let mut local = State::new();
        local.mint(Address::from_signing_key(&sender_sk), 10_000);
        aplicar_tx(&mut local, &sender_sk, 0x71, 100, 10, 1);
        local.epoch = 1; // ya checkpointeado en este nodo

        let root_antes = local.generate_state_root();
        let (balances_antes, nonces_antes) = (local.balances.clone(), local.nonces.clone());
        let (pool_antes, tx_antes, epoch_antes) = (local.fee_pool, local.tx_count, local.epoch);

        let mut mentiroso = State::new();
        mentiroso.mint(addr(0x72), HARD_CAP_SAT + 42);
        mentiroso.epoch = 1; // fresher que el local (1,1): (1,99) > (1,1)
        mentiroso.tx_count = 99;

        let (storage, dir) = gate_storage("untouched");
        assert!(matches!(
            sync_result(local.merge_if_fresher(NetworkId::Testnet, &mentiroso, &storage)),
            SyncResult::RejectedPorGate(_)
        ));

        assert_eq!(local.generate_state_root(), root_antes, "root intacto");
        assert_eq!(local.balances, balances_antes, "balances intactos");
        assert_eq!(local.nonces, nonces_antes, "nonces intactos");
        assert_eq!(
            (local.fee_pool, local.tx_count, local.epoch),
            (pool_antes, tx_antes, epoch_antes)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
