use crate::crypto::Address;
use crate::error::StorageError;
use crate::faucet::HARD_CAP_SAT;
use crate::serialize;
use crate::storage::Storage;
use crate::transaction::Transaction;
use sha2::Digest;
use std::collections::BTreeMap;

pub const CHECKPOINT_INTERVAL: u64 = 4096;

/// GOTE (diseño aprobado 2026-11-08, D5 del tesoro): goteo PLANO del
/// tesoro virtual — 500 LIT por época mientras haya tesoro disponible
/// (se recorta el último goteo). Constitución B (ever-minted): lo
/// minteado del tesoro JAMÁS vuelve a nacer tras el burn de fees.
pub const GOTEO_POR_EPOCA_SAT: u64 = 500 * 1_000_000;

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

/// Registro inverso node-local de la última transacción aplicada (`apply_transaction`),
/// usado por la Regla de Elección de Bifurcación (Checkpoint 42) para revertir una TX
/// perdedora en un empate `(epoch, tx_count)`. No entra al `state_root` ni al snapshot.
#[derive(Debug, Clone)]
pub struct TxUndo {
    pub tx: Transaction,
    pub prev_sender_balance: u64,
    pub prev_receiver_balance: Option<u64>,
    pub prev_sender_nonce: Option<u64>,
    pub prev_sender_particip: Option<u64>,
}

/// Resultado de la resolución determinista de bifurcación cuando `(epoch, tx_count)`
/// son iguales pero `local_root != remote_root` (Checkpoint 42).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkResolution {
    NotDivergent(SyncResult),
    /// `local_root < remote_root`: el estado local gana el desempate numérico/léxico.
    KeptLocal,
    /// `remote_root < local_root`: el nodo local retrocede su TX perdedora (si la hay),
    /// adopta el estado remoto y reintenta aplicar la TX si sigue siendo válida.
    AdoptedRemote {
        rolled_back_tx: Option<Transaction>,
        reapplied: bool,
    },
    /// El candidato remoto con menor root violó una compuerta de consenso (`g1`–`g7`).
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
    /// g6 (GOTE, Checkpoint 21): la emisión del tesoro es ilegible —
    /// `emitido_total` excede el techo 21M ever-minted (D7) o RETROCEDE
    /// respecto al local en un snapshot más fresco (el tesoro solo emite;
    /// jamás des-emite).
    GoteoIncoherente,
    /// g7a (BFT Ligero, Checkpoint 40): el snapshot remoto avanza de época
    /// (`epoch > 0`) sin adjuntar un `QuorumCertificate`.
    FaltaCertificadoQuorum,
    /// g7b/g7c (BFT Ligero, Checkpoint 40): el `QuorumCertificate` adjunto
    /// no coincide con `(chain_id, epoch, state_root)` o falla la verificación
    /// criptográfica contra el `ValidatorSet`.
    CertificadoQuorumInvalido,
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
    /// GOTE (D7 constitución B): LIT ya MINTED del tesoro virtual por el
    /// goteo histórico (+ seed de migración = supply congelada al
    /// redeploy). Consenso: entra al root y al snapshot (v3). El techo
    /// 21M es EVER-MINTEADO: `emitido_total ≤ HARD_CAP_SAT` siempre,
    /// lo quemado jamás vuelve a nacer.
    pub emitido_total: u64,
    /// GOTE (D6, R1 equitativa): TXs enviadas por cuenta ESTA época.
    /// Elegible del goteo = contar ≥ 1. Es función del orden de TXs (que
    /// ya es consenso) ⇒ entra AL ROOT OBLIGATORIO (lo node-local jamás
    /// paga el Cierre: lección del Checkpoint 5/13). Se VACÍA en cada
    /// Cierre de Libro.
    pub participacion_epoch: BTreeMap<Address, u64>,
    /// BFT Ligero (Checkpoint 40): último Certificado de Quórum que avala
    /// el estado de la época actual (`epoch > 0`). Viaja en Snapshot v5 y
    /// no altera `generate_state_root()` (el QC firma el root, no al revés).
    pub latest_qc: Option<crate::quorum::QuorumCertificate>,
    /// Registro inverso node-local de la última TX aplicada (Checkpoint 42).
    pub last_undo: Option<TxUndo>,
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
            emitido_total: 0,
            participacion_epoch: BTreeMap::new(),
            latest_qc: None,
            last_undo: None,
        }
    }

    pub fn load(storage: &Storage) -> Result<Self, StorageError> {
        let balances = storage.load_balances()?;
        let nonces = storage.load_nonces()?;
        let (fee_pool, tx_count, epoch, fees_quemados) = storage.load_meta()?;
        // GOTE (D7, constitución B ever-minted): migración. Si la 5ª clave
        // `emitido_total` no existe (era del Checkpoint 19), el nodo se
        // SEEDea UNA VEZ con su total_supply() congelado: "todo LIT que ya
        // existe ya consumió su parte del tesoro". Los 4 nodos de
        // producción migran en la MISMA ventana atómica ⇒ misma seed ⇒
        // mismo estado ⇒ misma emisión. Al boot se persiste
        // INMEDIATAMENTE (reload posterior NO re-seedea con una supply ya
        // movida: eso desincronizaría).
        let emitido_total = match storage.load_emitido_total()? {
            Some(e) => e,
            None => {
                let seed = balances.values().sum();
                storage.save_emitido_total(seed)?;
                seed
            }
        };
        let participacion_epoch = storage.load_particip()?;
        Ok(State {
            balances,
            nonces,
            fee_pool,
            fees_quemados,
            tx_count,
            epoch,
            emitido_total,
            participacion_epoch,
            latest_qc: None,
            last_undo: None,
        })
    }

    pub fn flush(&self, storage: &Storage) -> Result<(), StorageError> {
        storage.prune_balances(&self.balances)?;
        storage.prune_nonces(&self.nonces)?;
        storage.prune_particip(&self.participacion_epoch)?;
        for (addr, bal) in &self.balances {
            storage.apply_balance(addr, *bal)?;
        }
        for (addr, nonce) in &self.nonces {
            storage.apply_nonce(addr, *nonce)?;
        }
        for (addr, cuenta) in &self.participacion_epoch {
            storage.apply_particip(addr, *cuenta)?;
        }
        storage.save_meta(self.fee_pool, self.tx_count, self.epoch, self.fees_quemados)?;
        storage.save_emitido_total(self.emitido_total)?;
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

        let prev_receiver_balance = if tx.receiver != tx.sender {
            self.balances.get(&tx.receiver).copied()
        } else {
            Some(sender_balance)
        };
        let prev_sender_nonce = self.nonces.get(&tx.sender).copied();
        let prev_sender_particip = self.participacion_epoch.get(&tx.sender).copied();

        let new_fee_pool = self
            .fee_pool
            .checked_add(tx.fee)
            .ok_or("Overflow adding fee to pool")?;

        self.balances
            .insert(tx.sender.clone(), sender_balance - total_cost);

        let receiver_balance = *self.balances.get(&tx.receiver).unwrap_or(&0);
        self.balances
            .insert(tx.receiver.clone(), receiver_balance + tx.amount);

        self.nonces.insert(tx.sender.clone(), tx.nonce);

        self.fee_pool = new_fee_pool;
        self.tx_count += 1;

        // GOTE (D6): el remitente participó en esta época. La secuencia de
        // TX ya es de consenso (se aplica idéntica en todos los nodos),
        // así que este mapa queda determinista y entra al state root.
        // Elegible del goteo en el Cierre = contador ≥ 1 (R1 equitativa:
        // el CONTEO registra, pero el peso del reparto NO es el conteo).
        *self
            .participacion_epoch
            .entry(tx.sender.clone())
            .or_insert(0) += 1;

        self.last_undo = Some(TxUndo {
            tx: tx.clone(),
            prev_sender_balance: sender_balance,
            prev_receiver_balance,
            prev_sender_nonce,
            prev_sender_particip,
        });

        Ok(())
    }

    /// Revierte la última transacción aplicada localmente mediante su `TxUndo`
    /// (Checkpoint 42) y la devuelve para poder re-encolarla o reintentarla.
    pub fn rollback_last_tx(&mut self) -> Option<Transaction> {
        let undo = self.last_undo.take()?;
        let tx = undo.tx;

        if tx.receiver != tx.sender {
            match undo.prev_receiver_balance {
                Some(bal) => {
                    self.balances.insert(tx.receiver.clone(), bal);
                }
                None => {
                    self.balances.remove(&tx.receiver);
                }
            }
        }
        self.balances
            .insert(tx.sender.clone(), undo.prev_sender_balance);

        match undo.prev_sender_nonce {
            Some(n) => {
                self.nonces.insert(tx.sender.clone(), n);
            }
            None => {
                self.nonces.remove(&tx.sender);
            }
        }

        match undo.prev_sender_particip {
            Some(p) => {
                self.participacion_epoch.insert(tx.sender.clone(), p);
            }
            None => {
                self.participacion_epoch.remove(&tx.sender);
            }
        }

        self.fee_pool = self.fee_pool.saturating_sub(tx.fee);
        self.tx_count = self.tx_count.saturating_sub(1);

        Some(tx)
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
        self.validate_incoming_snapshot_with_quorum(network, remote, None)
    }

    /// Versión completa del gate `g1`–`g7` con verificación BFT ligera (`g7`)
    /// cuando `validator_set` está activo.
    pub fn validate_incoming_snapshot_with_quorum(
        &self,
        network: crate::network::NetworkId,
        remote: &State,
        validator_set: Option<&crate::quorum::ValidatorSet>,
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

        // g6 — coherencia del goteo del tesoro (GOTE, Checkpoint 21;
        // DISENO_GOTE.md §6). El tesoro virtual SOLO EMITE:
        // a) techo constitucional D7 (ever-minted): emitido_total ≤ 21M,
        //    jamás hay "más emitido que moneda total" (cota mensurable);
        // b) monotonía: un snapshot MÁS fresco con emitido MENOR que el
        //    local es mentira — el tesoro jamás des-emite (el único
        //    movimiento legal es crecer en cada Cierre con elegibles).
        // c) cota RELATIVA por época (versión fina, Checkpoint 23): solo
        //    la rama de adopción (remote (epoch,tx) > local, garantizado
        //    por merge_if_fresher) llega acá — y cada época que el remoto
        //    adelante al local pudo emitir A LO SUMO GOTEO_POR_EPOCA_SAT
        //    (el goteo dispara únicamente en el Cierre, monto
        //    min(500 LIT, disponible) ≤ 500):
        //      remote.emitido ≤ local.emitido + 500×(remote.epoch − local.epoch)
        //    Forma sin seed por red (la nota del diseño pedía un "seed
        //    firmado por red" que jamás se definió — la RELATIVA lo evita
        //    usando el punto de anclaje del estado local, que ya pasó su
        //    propio gate en su momento). Épocas iguales ⇒ delta 0 ⇒ la
        //    emisión no puede variar dentro de una época (el Cierre
        //    AVANZA la época): combinada con b) obliga a la IGUALDAD
        //    exacta — caza de paso al mentiroso que mintea sin pasar por
        //    un Cierre; y en el límite, un remoto N épocas adelante con
        //    crecimiento exactamente 500×N (lo máximo legal) pasa justo
        //    por el ≤. El saturating del multiplicador colapsa una época
        //    remota astronómica (u64) a la cota máxima — ahí queda de
        //    guardia el techo absoluto del inciso a).
        //    ({delta} épocas × 500 LIT es la cota EXACTA del diseño: sin
        //    elegibles la época no emite, y el goteo nunca supera 500.)
        let delta_epocas = remote.epoch.saturating_sub(self.epoch);
        let cota_goteo = self
            .emitido_total
            .saturating_add(GOTEO_POR_EPOCA_SAT.saturating_mul(delta_epocas));
        if remote.emitido_total > HARD_CAP_SAT
            || remote.emitido_total < self.emitido_total
            || remote.emitido_total > cota_goteo
        {
            return Err(GateReason::GoteoIncoherente);
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

        // g7 — Certificado de Quórum (BFT Ligero, Checkpoint 40):
        // Todo avance a época >= 1 requiere un `QuorumCertificate` válido
        // firmado por al menos M-de-N validadores sobre el `state_root` de `remote`.
        if let Some(vset) = validator_set
            && (remote.epoch > 0 || remote.epoch > self.epoch)
        {
            let qc = remote
                .latest_qc
                .as_ref()
                .ok_or(GateReason::FaltaCertificadoQuorum)?;
            let remote_root = remote.generate_state_root();
            qc.verify(network.chain_id(), remote.epoch, &remote_root, vset)
                .map_err(|_| GateReason::CertificadoQuorumInvalido)?;
        }

        Ok(())
    }

    pub fn merge_if_fresher(
        &mut self,
        network: crate::network::NetworkId,
        remote: &State,
        storage: &Storage,
    ) -> Result<SyncResult, StorageError> {
        self.merge_if_fresher_with_quorum(network, remote, storage, None)
    }

    pub fn merge_if_fresher_with_quorum(
        &mut self,
        network: crate::network::NetworkId,
        remote: &State,
        storage: &Storage,
        validator_set: Option<&crate::quorum::ValidatorSet>,
    ) -> Result<SyncResult, StorageError> {
        let local_root = self.generate_state_root();
        let remote_root = remote.generate_state_root();
        if local_root == remote_root {
            return Ok(SyncResult::AlreadySynced);
        }
        let local_fresh = (self.epoch, self.tx_count);
        let remote_fresh = (remote.epoch, remote.tx_count);
        if remote_fresh > local_fresh {
            // GATE (unidad B+C+G + g7 BFT): ni siquiera se compara el resto — un
            // snapshot que viola una invariante NO se adopta y el resultado
            // le informa al caller (strike grave contra el peer, §2).
            if let Err(gate) =
                self.validate_incoming_snapshot_with_quorum(network, remote, validator_set)
            {
                return Ok(SyncResult::RejectedPorGate(gate));
            }
            self.balances = remote.balances.clone();
            self.nonces = remote.nonces.clone();
            self.fee_pool = remote.fee_pool;
            self.fees_quemados = remote.fees_quemados;
            self.tx_count = remote.tx_count;
            self.epoch = remote.epoch;
            // GOTE (Checkpoint 21): la emisión y la participación de la
            // época viajan en el snapshot v3 (con raíz del remoto, ya
            // pasada por g6);
            // adoptarlas es parte de la ÚNICA definición del estado.
            self.emitido_total = remote.emitido_total;
            self.participacion_epoch = remote.participacion_epoch.clone();
            self.latest_qc = remote.latest_qc.clone();
            self.last_undo = None;
            self.flush(storage)?;
            Ok(SyncResult::RemoteAdopted)
        } else if local_fresh > remote_fresh {
            Ok(SyncResult::LocalNewer)
        } else {
            Ok(SyncResult::Divergent)
        }
    }

    /// Regla determinista de desempate ante bifurcaciones (`epoch`, `tx_count` idénticos
    /// pero `local_root != remote_root`) — Checkpoint 42:
    /// - **Criterio A:** prevalece el `state_root` lexicográfica/numéricamente menor.
    /// - **Criterio B:** si `remote_root < local_root` (y pasa las compuertas `g1`–`g7`),
    ///   el nodo perdedor revierte su última TX (`rollback_last_tx`), adopta el estado
    ///   remoto y reintenta aplicar la TX revertida si sigue siendo válida.
    pub fn resolve_fork_if_divergent(
        &mut self,
        network: crate::network::NetworkId,
        remote: &State,
        storage: &Storage,
        validator_set: Option<&crate::quorum::ValidatorSet>,
    ) -> Result<ForkResolution, StorageError> {
        let local_root = self.generate_state_root();
        let remote_root = remote.generate_state_root();
        if local_root == remote_root {
            return Ok(ForkResolution::NotDivergent(SyncResult::AlreadySynced));
        }
        let local_fresh = (self.epoch, self.tx_count);
        let remote_fresh = (remote.epoch, remote.tx_count);
        if remote_fresh != local_fresh {
            let sync = self.merge_if_fresher_with_quorum(network, remote, storage, validator_set)?;
            return Ok(ForkResolution::NotDivergent(sync));
        }

        // Criterio A: desempate determinista por menor state_root.
        if local_root < remote_root {
            return Ok(ForkResolution::KeptLocal);
        }

        // Validar compuertas g1-g7 antes de adoptar un estado empatado.
        if let Err(gate) =
            self.validate_incoming_snapshot_with_quorum(network, remote, validator_set)
        {
            return Ok(ForkResolution::RejectedPorGate(gate));
        }

        // Criterio B: rollback de la TX perdedora, adopción del ganador y reintento.
        let rolled_back_tx = self.rollback_last_tx();
        self.balances = remote.balances.clone();
        self.nonces = remote.nonces.clone();
        self.fee_pool = remote.fee_pool;
        self.fees_quemados = remote.fees_quemados;
        self.tx_count = remote.tx_count;
        self.epoch = remote.epoch;
        self.emitido_total = remote.emitido_total;
        self.participacion_epoch = remote.participacion_epoch.clone();
        self.latest_qc = remote.latest_qc.clone();
        self.last_undo = None;

        let reapplied = if let Some(ref tx) = rolled_back_tx {
            self.apply_transaction(tx).is_ok()
        } else {
            false
        };

        self.flush(storage)?;
        Ok(ForkResolution::AdoptedRemote {
            rolled_back_tx,
            reapplied,
        })
    }

    pub fn is_checkpoint_due(&self) -> bool {
        self.tx_count > 0 && self.tx_count.is_multiple_of(CHECKPOINT_INTERVAL)
    }

    /// LIT todavía dentro del tesoro virtual (GOTE, D7 constitución B):
    /// `HARD_CAP_SAT − emitido_total` — el techo 21M es EVER-MINTEADO,
    /// los fees quemados NO reabren tesoro (lo muerto no renace).
    pub fn tesoro_disponible(&self) -> u64 {
        HARD_CAP_SAT.saturating_sub(self.emitido_total)
    }

    /// Avanza la época del "Cierre de Libro" (GOTE, Checkpoint 21 — diseño
    /// aprobado 2026-11-08, D6–D10 firmadas en `DISENO_GOTE.md` §11).
    ///
    /// TODO es UNA función pura del estado: cualquier nodo con el estado
    /// deriva byte a byte el mismo Cierre (ley anti-node-local, hallazgo A).
    ///
    /// Orden exacto:
    /// 1. Elegibles (D6/R1): cuentas con `participacion_epoch ≥ 1`
    ///    (enviaron al menos una TX esta época; todas pesan 1).
    /// 2. Goteo plano (D5 tesoro + D9): si hay tesoro disponible, se reparte
    ///    `min(GOTEO_POR_EPOCA_SAT, disponible)` en PARTES IGUALES con
    ///    división entera; MINT directo en cuentas (el tesoro NO es cuenta,
    ///    nadie lo firma). `emitido_total` solo sube lo EFECTIVAMENTE
    ///    repartido: el dust queda dentro del tesoro para futuros goteos
    ///    (D9 — nunca poza muerta: sigue siendo emisión pendiente).
    /// 3. Fees (D5/Q2):
    ///    - Con elegibles y goteo vivo (era transitoria del burn del 3b):
    ///      pool → 100% quemados (Checkpoint 19).
    ///    - Con elegibles y goteo agotado (era post-goteo): 50/50 estado-
    ///      derived — mitad repartida en partes iguales entre elegibles,
    ///      dust de esa mitad → carry del pool (poza muerta prohibida),
    ///      resto → quemados. Contabilidad exacta: quemado + distribuido
    ///      + carry == pool (nada se crea ni desaparece).
    ///    - SIN elegibles: el tesoro NO pierde el turno y el pool queda
    ///      ESTACIONADO (carry completo, sin burn — jamás se quema sin
    ///      destino legal).
    /// 4. `participacion_epoch` se VACÍA SIEMPRE (la época nueva nace en
    ///    blanco) y `epoch += 1`.
    pub fn finalize_checkpoint(&mut self) -> [u8; 32] {
        let elegibles: Vec<&Address> = self
            .participacion_epoch
            .iter()
            .filter(|(_, cuenta)| **cuenta >= 1)
            .map(|(addr, _)| addr)
            .collect();

        if !elegibles.is_empty() {
            let n = elegibles.len() as u64;
            let disponible = self.tesoro_disponible();

            if disponible > 0 {
                // ERA DE GOTEO: emisión + burn transitorio del 3b.
                let goteo_piso = GOTEO_POR_EPOCA_SAT.min(disponible);
                // División entera SIN mínimo 1: si alguna vez hubiese más
                // elegibles que satoshis de goteo, mintear parte = 1 a
                // todos desbordaría el DISPONIBLE y rompería la
                // constitución del techo (D7). El dust queda en el tesoro.
                let parte = goteo_piso / n;
                if parte > 0 {
                    let repartido = parte
                        .checked_mul(n)
                        .expect("goteo: parte*n ≤ goteo_piso ≤ disponible ≤ u64::MAX");
                    for addr in elegibles.clone() {
                        *self.balances.entry(addr.clone()).or_insert(0) += parte;
                    }
                    self.emitido_total = self
                        .emitido_total
                        .checked_add(repartido)
                        .expect("goteo: emitido_total + repartido ≤ HARD_CAP_SAT");
                }
                let pool = self.fee_pool;
                self.fees_quemados = self.fees_quemados.saturating_add(pool);
                self.fee_pool = 0;
            } else {
                // ERA POST-GOTEAR: split 50/50 de fees (D5/Q2).
                let pool = self.fee_pool;
                let mitad = pool / 2;
                let dist = mitad / n;
                let distribuido = dist
                    .checked_mul(n)
                    .expect("dist*n ≤ mitad ≤ pool ≤ u64::MAX");
                if dist > 0 {
                    for addr in elegibles {
                        *self.balances.entry(addr.clone()).or_insert(0) += dist;
                    }
                }
                // Dust de la mitad distribuida → carry del pool (D5: el
                // destino "pool" es legal). Quemado + distribuido + carry
                // == pool, exacto (g4' monotónico sigue en pie).
                let carried = mitad - distribuido;
                let quemado = pool - mitad - carried;
                self.fees_quemados = self.fees_quemados.saturating_add(quemado);
                self.fee_pool = carried;
            }
        }
        // Sin elegibles: cero emisión, tesoro sin perder turno (D firmada)
        // y fee_pool estacionado intacto (carry, Q2 firmada — jamás se
        // quema sin destino: la época sin participantes no quema).

        self.participacion_epoch.clear();
        self.epoch += 1;
        self.last_undo = None;
        self.generate_state_root()
    }

    pub fn generate_state_root(&self) -> [u8; 32] {
        // Deuda 9a (Checkpoint 30): el algoritmo vive en hash.rs (única
        // puerta del swap futuro). Hoy: SHA-256 era 1, byte a byte
        // idéntico a la implementación directa de toda la vida.
        let mut hasher = crate::hash::new_state_hasher();

        hasher.update(self.epoch.to_be_bytes());
        hasher.update(self.fee_pool.to_be_bytes());
        // Deuda 3b: los fees quemados entran al root (consenso completo del
        // destino de los fees: pool vivo o destruido, nada ambiguo).
        hasher.update(self.fees_quemados.to_be_bytes());
        hasher.update(self.tx_count.to_be_bytes());
        // GOTE (Checkpoint 21): la emisión del tesoro es consenso (D7) y la
        // participación también (la secuencia de TX lo es) — ambos entran
        // al root; omitirlos ocultaría divergencias el día que un nodo se
        // permute con otra participación (lección del hallazgo A).
        hasher.update(self.emitido_total.to_be_bytes());

        for (address, balance) in &self.balances {
            hasher.update(address.as_bytes());
            hasher.update(balance.to_be_bytes());
            let nonce = self.nonces.get(address).copied().unwrap_or(0);
            hasher.update(nonce.to_be_bytes());
        }

        // BTreeMap iterado en orden de claves: determinista entre nodos.
        for (address, cuenta) in &self.participacion_epoch {
            hasher.update(address.as_bytes());
            hasher.update(cuenta.to_be_bytes());
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
        // GOTE (Checkpoint 21, snapshot v3): la emisión del tesoro viaja
        // (D7) y la participación de la época VIAJA TAMBIÉN — sin ella un
        // nodo que adopta no sabría quién es elegible en SU próximo
        // Cierre, y el goteo divergiría.
        buf.extend_from_slice(&self.emitido_total.to_be_bytes());
        let num_entries = self.balances.len() as u32;
        buf.extend_from_slice(&num_entries.to_be_bytes());
        for (addr, balance) in &self.balances {
            let nonce = self.nonces.get(addr).copied().unwrap_or(0);
            buf.extend_from_slice(serialize::encode_address(addr).as_ref());
            buf.extend_from_slice(&balance.to_be_bytes());
            buf.extend_from_slice(&nonce.to_be_bytes());
        }
        // GOTE (v3): entradas de participación al final del cable.
        let num_particip = self.participacion_epoch.len() as u32;
        buf.extend_from_slice(&num_particip.to_be_bytes());
        for (addr, cuenta) in &self.participacion_epoch {
            buf.extend_from_slice(serialize::encode_address(addr).as_ref());
            buf.extend_from_slice(&cuenta.to_be_bytes());
        }
        // Snapshot v4 (deuda 9a, Checkpoint 30): la ETIQUETA DEL ALGORITMO
        // DE HASH viaja como ÚLTIMO byte del cable. Los nodos de la era v3
        // la ignoran (su decode no exige bytes sobrantes) ⇒ transición SIN
        // ventana de incompatibilidad; los nodos v4 la leen y rechazan un
        // algoritmo que no conocen (un root de otro algoritmo no es
        // verificable acá — U7: pierden vivacidad, jamás corrompen).
        buf.push(crate::hash::HASH_ALGO_ACTUAL);
        // Snapshot v5 (Checkpoint 40): si existe QC, se adjunta `0x01 || qc.encode()`.
        if let Some(qc) = &self.latest_qc
            && let Ok(qc_bytes) = qc.encode()
        {
            buf.push(0x01);
            buf.extend_from_slice(&qc_bytes);
        }
        buf
    }

    pub fn decode_snapshot(
        data: &[u8],
        expected: crate::network::NetworkId,
    ) -> Result<Self, crate::error::SerializationError> {
        use crate::error::SerializationError;
        // Formato v3 (GOTE, Checkpoint 21): 1 + epoch + fee_pool +
        // fees_quemados + tx_count + emitido_total (5×u64) +
        // num_entries(u32) + entradas de 72 bytes + num_participaciones
        // (u32) + entradas de participación de 40 bytes (32 addr + 8 count).
        // ATOMICO: binarios de la era v2 NO entienden este cable (y al
        // revés) — el redeploy de la ventana actualiza a todos juntos.
        if data.len() < 1 + 8 * 5 + 4 {
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
        let emitido_total = {
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

        // GOTE (v3): entradas de participación (32 bytes addr + 8 bytes count).
        if offset + 4 > data.len() {
            return Err(SerializationError::InvalidData(
                "snapshot missing participation count",
            ));
        }
        let num_particip = {
            let mut arr = [0u8; 4];
            arr.copy_from_slice(&data[offset..offset + 4]);
            offset += 4;
            u32::from_be_bytes(arr)
        };
        let mut participacion_epoch = BTreeMap::new();
        for _ in 0..num_particip {
            if offset + 32 + 8 > data.len() {
                return Err(SerializationError::InvalidData(
                    "snapshot missing participation data",
                ));
            }
            let addr = serialize::decode_address(&data[offset..offset + 32])?;
            offset += 32;
            let cuenta = {
                let mut arr = [0u8; 8];
                arr.copy_from_slice(&data[offset..offset + 8]);
                offset += 8;
                u64::from_be_bytes(arr)
            };
            if cuenta == 0 {
                // Sin presencia no se serializa: un cero participativo en
                // el cable es formato corrupto (basura no pasa).
                return Err(SerializationError::InvalidData(
                    "snapshot participation entry with zero count",
                ));
            }
            participacion_epoch.insert(addr, cuenta);
        }

        // Snapshot v4 (deuda 9a, Checkpoint 30): etiqueta del algoritmo de
        // hash al final del cable. Formato v3 (sin el byte) = legado era 1:
        // se asume SHA-256 (el único algoritmo que existió — compatibilidad
        // total con nodos viejos durante la transición). Un byte de un
        // algoritmo DESCONOCIDO se rechaza: un root producido por otro
        // algoritmo no es verificable acá, y adoptar a ciegas sería fe.
        if offset < data.len() {
            let hash_algo = data[offset];
            if hash_algo != crate::hash::HASH_ALGO_ACTUAL {
                return Err(SerializationError::InvalidData(
                    "snapshot con algoritmo de hash desconocido",
                ));
            }
            offset += 1;
        }

        // Snapshot v5 (Checkpoint 40): QC opcional tras `hash_algo`.
        let latest_qc = if offset < data.len() {
            let has_qc = data[offset];
            offset += 1;
            match has_qc {
                0x00 => {
                    if offset != data.len() {
                        return Err(SerializationError::InvalidData(
                            "snapshot v5 bytes sobrantes tras flag QC=0",
                        ));
                    }
                    None
                }
                0x01 => {
                    let (qc, read) =
                        crate::quorum::QuorumCertificate::decode_prefix(&data[offset..]).map_err(
                            |_| {
                                SerializationError::InvalidData(
                                    "snapshot v5 QuorumCertificate corrupto o truncado",
                                )
                            },
                        )?;
                    offset += read;
                    if offset != data.len() {
                        return Err(SerializationError::InvalidData(
                            "snapshot v5 bytes sobrantes tras QuorumCertificate",
                        ));
                    }
                    Some(qc)
                }
                _ => {
                    return Err(SerializationError::InvalidData(
                        "snapshot v5 flag has_qc inválido",
                    ));
                }
            }
        } else {
            None
        };

        Ok(State {
            balances,
            nonces,
            fee_pool,
            fees_quemados,
            tx_count,
            epoch,
            emitido_total,
            participacion_epoch,
            latest_qc,
            last_undo: None,
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
        // Deuda 3b (burn, 2026-11-07) + GOTE (Checkpoint 21): el Cierre
        // QUEMA el fee_pool íntegro en la era de goteo (el burn es el
        // régimen transitorio hasta agotar el tesoro) Y emite el goteo
        // plano a los elegibles. El remitente con TX de la época es el
        // único elegible: captura el goteo COMPLETO (500 LIT, hay tesoro
        // de sobra). Invariancia del traslado de fees intacta.
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
        let saldo_sender_antes = state.balances[&sender];
        let saldo_receiver_antes = state.balances[&receiver];

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
        // El remitente participó ⇒ captura el goteo completo (único
        // elegible) — el Cierre SÍ toca saldos ahora, pero SOLO via
        // goteo/split consensuado. Los fees quemados jamás pagan nada.
        assert_eq!(
            state.balances[&sender],
            saldo_sender_antes + GOTEO_POR_EPOCA_SAT,
            "goteo exacto al único elegible"
        );
        assert_eq!(
            state.balances[&receiver], saldo_receiver_antes,
            "el receptor no envió TX: no es elegible"
        );
        assert_eq!(state.emitido_total, GOTEO_POR_EPOCA_SAT);
        assert_eq!(
            state.participacion_epoch.len(),
            0,
            "la época nueva nace en blanco"
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

    // =====================================================================
    // GOTE (Checkpoint 21) — tests bloqueantes del DISENO_GOTE.md §8.
    // =====================================================================

    #[test]
    fn test_goteo_exacto_3_elegibles_dust_en_tesoro() {
        // §8.1: 3 elegibles reparten 500 LIT en partes IGUALES con división
        // entera: 166,666,666 c/u; emitido_total solo sube lo repartido
        // (499,999,998) — el dust de 2 sat QUEDA en el tesoro (D9). Los
        // fees se queman 100% (era de goteo = burn transitorio del 3b).
        let (sk1, sk2, sk3) = (
            SigningKey::from_bytes(&[0x21; 32]),
            SigningKey::from_bytes(&[0x22; 32]),
            SigningKey::from_bytes(&[0x23; 32]),
        );
        let a1 = Address::from_signing_key(&sk1);
        let a2 = Address::from_signing_key(&sk2);
        let a3 = Address::from_signing_key(&sk3);

        let mut s = State::new();
        s.mint(a1.clone(), 10_000);
        s.mint(a2.clone(), 10_000);
        s.mint(a3.clone(), 10_000);

        aplicar_tx(&mut s, &sk1, 0x20, 1, 7, 1);
        aplicar_tx(&mut s, &sk2, 0x20, 1, 7, 1);
        aplicar_tx(&mut s, &sk3, 0x20, 1, 7, 1);
        assert_eq!(
            s.participacion_epoch.len(),
            3,
            "los tres remitentes cuentan"
        );
        let saldos_antes_del_cierre = s.balances.clone();

        s.finalize_checkpoint();

        let parte = GOTEO_POR_EPOCA_SAT / 3; // 166,666,666
        for (a, antes) in [
            (&a1, &saldos_antes_del_cierre),
            (&a2, &saldos_antes_del_cierre),
            (&a3, &saldos_antes_del_cierre),
        ] {
            assert_eq!(
                s.balances[&a],
                antes[&a] + parte,
                "cada elegible cobra EXACTAMENTE la misma parte"
            );
        }
        assert_eq!(s.emitido_total, parte * 3, "el dust de 2 sat no se emite");
        assert_eq!(
            GOTEO_POR_EPOCA_SAT - s.emitido_total,
            2,
            "el dust queda DISPONIBLE en el tesoro (D9), no se pierde"
        );
        assert_eq!(s.fee_pool, 0, "fue Quemadura 100% en era de goteo");
        assert_eq!(
            s.fees_quemados, 21,
            "los fees del pool auditable en quemados"
        );
        assert_eq!(s.epoch, 1);
        assert!(s.participacion_epoch.is_empty(), "nueva época en blanco");
    }

    #[test]
    fn test_goteo_sin_elegibles_no_emite_y_pool_carry() {
        // §8.2: sin participación esa época NO se emite nada (el tesoro
        // disponible es derivado: no pierde el turno) y el fee_pool queda
        // ESTACIONADO — jamás se quema sin destino legal (Q2 firmada).
        let mut s = State::new();
        s.mint(addr(0x30), 1_000);
        s.fee_pool = 50; // crafting directo: sin TX = sin participación
        let saldo_antes = s.balances[&addr(0x30)];

        s.finalize_checkpoint();

        assert_eq!(s.emitido_total, 0, "sin elegibles: cero emisión");
        assert_eq!(s.epoch, 1, "el epoch avanza igual (la época cerró)");
        assert_eq!(s.fee_pool, 50, "carry completo: no se quema sin destino");
        assert_eq!(s.fees_quemados, 0);
        assert_eq!(
            s.balances[&addr(0x30)],
            saldo_antes,
            "nadie cobra lo que no participó"
        );
    }

    #[test]
    fn test_goteo_recortado_tesoro_agotado() {
        // §8.3: el ÚLTIMO goteo se recorta EXACTO al disponible y el
        // emitido_total termina pisando el techo (constitución B: jamás
        // lo cruza). La época siguiente ya vive en la era post-goteo.
        let sk = SigningKey::from_bytes(&[0x31; 32]);
        let remanente: u64 = 1_000;

        let mut s = State::new();
        s.mint(Address::from_signing_key(&sk), 10_000);
        s.emitido_total = HARD_CAP_SAT - remanente; // seed: tesoro casi agotado
        aplicar_tx(&mut s, &sk, 0x32, 1, 5, 1);

        s.finalize_checkpoint();

        assert_eq!(
            s.emitido_total, HARD_CAP_SAT,
            "el goteo se recorta exacto: jamás cruza el techo"
        );
        assert_eq!(
            s.balances[&Address::from_signing_key(&sk)],
            10_000 - 6 + remanente,
            "único elegible cobra el remanente íntegro"
        );
        assert_eq!(s.fee_pool, 0);
        assert_eq!(s.tesoro_disponible(), 0, "era post-goteo desde aquí");
    }

    #[test]
    fn test_post_goteo_split_50_50_impar_dust_carry() {
        // §8.4: era post-goteo (tesoro en 0): con fee_pool IMPAR la mitad
        // se reparte en partes iguales, el dust de esa mitad queda de
        // carry en el pool (poza muerta prohibida) y el resto se quema.
        // Contabilidad EXACTA: quemado + distribuido + carry == pool.
        let (sk1, sk2, sk3) = (
            SigningKey::from_bytes(&[0x41; 32]),
            SigningKey::from_bytes(&[0x42; 32]),
            SigningKey::from_bytes(&[0x43; 32]),
        );
        let a1 = Address::from_signing_key(&sk1);
        let a2 = Address::from_signing_key(&sk2);
        let a3 = Address::from_signing_key(&sk3);

        let mut s = State::new();
        s.mint(a1.clone(), 10_000);
        s.mint(a2.clone(), 10_000);
        s.mint(a3.clone(), 10_000);
        s.emitido_total = HARD_CAP_SAT; // era post-goteo

        aplicar_tx(&mut s, &sk1, 0x44, 1, 99, 1); // fees 99,1,1 → pool 101
        aplicar_tx(&mut s, &sk2, 0x44, 1, 1, 1);
        aplicar_tx(&mut s, &sk3, 0x44, 1, 1, 1);
        assert_eq!(s.fee_pool, 101);
        let saldos_antes_del_cierre = s.balances.clone();

        s.finalize_checkpoint();

        // mitad=50, dist=50/3=16, distribuido=48, carry=2, quemado=49.
        for a in [&a1, &a2, &a3] {
            assert_eq!(
                s.balances[a],
                saldos_antes_del_cierre[a] + 16,
                "post-goteo: mitad distribuida en partes iguales"
            );
        }
        assert_eq!(s.emitido_total, HARD_CAP_SAT, "post-goteo NO re-emite");
        assert_eq!(s.fee_pool, 2, "el dust de la distribución queda de carry");
        assert_eq!(s.fees_quemados, 49, "lo demás del pool, quemado");
        assert_eq!(
            s.fee_pool + s.fees_quemados,
            51,
            "contabilidad exacta: 101 del pool = 49 burn + 2 carry + 50 distribuido (este sale del pool a los saldos)"
        );
    }

    #[test]
    fn test_participacion_se_limpia_y_solo_envio_cuenta() {
        // §8.5: la participación se VACÍA en cada Cierre (cada época nace
        // en blanco) y SOLO el envío otorga participación (R1: el receptor
        // de una TX no es elegible).
        let (sk1, sk2) = (
            SigningKey::from_bytes(&[0x51; 32]),
            SigningKey::from_bytes(&[0x52; 32]),
        );
        let a1 = Address::from_signing_key(&sk1);
        let a2 = Address::from_signing_key(&sk2);

        let mut s = State::new();
        s.mint(a1.clone(), 10_000);
        s.mint(a2.clone(), 10_000);

        aplicar_tx(&mut s, &sk1, 0x53, 1, 3, 1);
        let saldo_a1_tras_tx = s.balances[&a1];

        // Cierre de la época 1 (state epoch 0 → 1): a1 ES elegible aquí
        // y cobra su goteo.
        s.finalize_checkpoint();
        assert!(s.participacion_epoch.is_empty(), "la época nace en blanco");
        assert_eq!(s.epoch, 1);
        assert_eq!(
            s.balances[&a1],
            saldo_a1_tras_tx + GOTEO_POR_EPOCA_SAT,
            "a1 cobró el goteo de la época 1"
        );
        let saldo_a1_tras_goteo_1 = s.balances[&a1];

        // Época 2: SOLO sk2 envía → sk2 cobra el goteo completo; sk1 (que
        // ya cobró en la época 1 y no envió en la 2) NO cobra de nuevo.
        aplicar_tx(&mut s, &sk2, 0x53, 1, 3, 1);
        assert_eq!(s.participacion_epoch.get(&a2), Some(&1));
        assert!(!s.participacion_epoch.contains_key(&a1));

        let saldo_a2 = s.balances[&a2];
        s.finalize_checkpoint();
        assert_eq!(s.epoch, 2);
        assert_eq!(
            s.balances[&a2],
            saldo_a2 + GOTEO_POR_EPOCA_SAT,
            "solo el elegible de la época cobra"
        );
        assert_eq!(
            s.balances[&a1], saldo_a1_tras_goteo_1,
            "no reenvió en la época 2: no cobra en la 2"
        );
        assert_eq!(
            s.emitido_total,
            2 * GOTEO_POR_EPOCA_SAT,
            "1 goteo por Cierre"
        );
    }

    #[test]
    fn test_participacion_y_emitido_entrar_al_root() {
        // Ley de pureza del hallazgo A: si la emisión o la participación
        // cambiaran sin entrar al root, una divergencia quedaría oculta.
        // Dos estados "idénticos salvo" eso parked DEN root distinto.
        let mut s_base = State::new();
        s_base.mint(addr(0x60), 1_000);

        let mut con_particip = s_base.clone();
        con_particip.participacion_epoch.insert(addr(0x60), 3);
        assert_ne!(
            s_base.generate_state_root(),
            con_particip.generate_state_root(),
            "la participación obliga divergencia visible"
        );

        let mut con_emitido = s_base.clone();
        con_emitido.emitido_total = 7;
        assert_ne!(
            s_base.generate_state_root(),
            con_emitido.generate_state_root(),
            "la emisión obliga divergencia visible"
        );
    }

    #[test]
    fn test_snapshot_v3_roundtrip_con_participacion_y_emitido() {
        // §8.7: el snapshot v3 viaja con emitido_total + entradas de
        // participación; el roundtrip reproduce el estado byte a byte
        // (mismo root).
        let mut s = testnet_legitimo();
        let sk2 = SigningKey::from_bytes(&[0x02; 32]); // minted en el helper
        aplicar_tx(&mut s, &sk2, 0x03, 10, 3, 1);
        s.emitido_total = 7_000_000;
        // crafting de participación con conteo > 1 (vía 2 TXs)
        aplicar_tx(&mut s, &sk2, 0x03, 10, 3, 2);

        let data = s.encode_snapshot(NetworkId::Testnet);
        let d = State::decode_snapshot(&data, NetworkId::Testnet).unwrap();
        assert_eq!(d.emitido_total, 7_000_000);
        assert_eq!(d.participacion_epoch, s.participacion_epoch);
        assert_eq!(d.participacion_epoch.get(&addr(0x02)), Some(&2));
        assert_eq!(
            d.generate_state_root(),
            s.generate_state_root(),
            "roundtrip v3: mismo root"
        );

        // Roundtrip del roundtrip (idempotencia del formato).
        let d2 = State::decode_snapshot(&d.encode_snapshot(NetworkId::Testnet), NetworkId::Testnet)
            .unwrap();
        assert_eq!(d2.generate_state_root(), s.generate_state_root());
    }

    #[test]
    fn test_snapshot_v4_etiqueta_algoritmo_al_final() {
        // Deuda 9a (Checkpoint 30): el snapshot v4 viaja con el byte
        // HASH_ALGO_ACTUAL como ÚLTIMO byte; el roundtrip lo conserva y
        // el root es idéntico (la etiqueta no entra al root).
        let s = testnet_legitimo();
        let data = s.encode_snapshot(NetworkId::Testnet);
        assert_eq!(
            *data.last().unwrap(),
            crate::hash::HASH_ALGO_ACTUAL,
            "el último byte del cable es la etiqueta del algoritmo"
        );
        let d = State::decode_snapshot(&data, NetworkId::Testnet).unwrap();
        assert_eq!(d.generate_state_root(), s.generate_state_root());
    }

    #[test]
    fn test_snapshot_v3_legado_sin_etiqueta_se_acepta() {
        // Transición: un snapshot de la era v3 (sin el byte del final) es
        // legado del algoritmo era 1 — se acepta asumiendo SHA-256. Un
        // nodo viejo y uno nuevo se entienden SIN ventana de corte.
        let s = testnet_legitimo();
        let data_v4 = s.encode_snapshot(NetworkId::Testnet);
        let data_v3 = &data_v4[..data_v4.len() - 1]; // quitar la etiqueta
        let d = State::decode_snapshot(data_v3, NetworkId::Testnet).unwrap();
        assert_eq!(d.generate_state_root(), s.generate_state_root());
    }

    #[test]
    fn test_snapshot_algoritmo_desconocido_rejectado() {
        // U7: un root producido por un algoritmo que el nodo no conoce no
        // es verificable acá — se rechaza (basura no pasa; el nodo pierde
        // vivacidad con ese peer, jamás corrompe su estado).
        let s = testnet_legitimo();
        let mut data = s.encode_snapshot(NetworkId::Testnet);
        let n = data.len();
        data[n - 1] = 99; // algoritmo desconocido
        let err = State::decode_snapshot(&data, NetworkId::Testnet);
        assert!(
            err.is_err(),
            "un algoritmo de hash desconocido se rechaza"
        );
    }

    #[test]
    fn test_g6_emitido_retrocedido_rejectado() {
        // §8.6a: el tesoro solo EMITE — una snapshot MÁS fresco con
        // emitido_total menor que el local es mentira (g6).
        let mut local = State::new();
        local.emitido_total = 600_000_000;
        local.tx_count = 0;
        local.epoch = 0;

        let mut mentiroso = State::new();
        mentiroso.mint(addr(0x61), 1_000);
        mentiroso.epoch = 0;
        mentiroso.tx_count = 1;
        mentiroso.emitido_total = 500_000_000; // < local: des-emisión falsa
        mentiroso.participacion_epoch.insert(addr(0x61), 1);

        let (storage, dir) = gate_storage("g6a");
        assert_eq!(
            sync_result(local.merge_if_fresher(NetworkId::Testnet, &mentiroso, &storage)),
            SyncResult::RejectedPorGate(GateReason::GoteoIncoherente),
            "emitido retrocedido en snapshot fresco: REJECT"
        );
        assert_eq!(local.emitido_total, 600_000_000, "el gate no toca el local");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_g6_emitido_gigante_rejectado() {
        // §8.6b: emitido_total por sobre el techo 21M ever-minted (D7).
        let mut local = State::new();
        let mut mentiroso = State::new();
        mentiroso.mint(addr(0x62), 1_000);
        mentiroso.tx_count = 1;
        mentiroso.emitido_total = HARD_CAP_SAT + 1;
        mentiroso.participacion_epoch.insert(addr(0x62), 1);

        let (storage, dir) = gate_storage("g6b");
        assert_eq!(
            sync_result(local.merge_if_fresher(NetworkId::Testnet, &mentiroso, &storage)),
            SyncResult::RejectedPorGate(GateReason::GoteoIncoherente),
            "emitido > 21M jamás pasa el gate"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_g6_emitido_igual_o_mayor_adopta_con_participacion() {
        // §8.6, lado bueno: un envejecido LEGÍTIMO de exactamente UNA época
        // de goteo (delta 1 ⇒ cota local + 500 LIT, el máximo legal) se
        // adopta y la participación viaja con el estado. (Checkpoint 23:
        // el salto de emisión DENTRO de una misma época ya NO es legal —
        // el Cierre AVANZA la época — así que el remoto debe ir mínimo una
        // época adelante para poder emitir; tx_count ≥ epoch×4096 por g2.)
        let mut local = State::new();
        local.emitido_total = 500_000_000;

        let mut legitimo = State::new();
        legitimo.mint(addr(0x63), 1_000);
        legitimo.tx_count = CHECKPOINT_INTERVAL; // floor legal de la época 1
        legitimo.epoch = 1;
        legitimo.emitido_total = 1_000_000_000; // 500M + 500M (exacto)
        legitimo.participacion_epoch.insert(addr(0x63), 2);

        let (storage, dir) = gate_storage("g6c");
        assert_eq!(
            sync_result(local.merge_if_fresher(NetworkId::Testnet, &legitimo, &storage)),
            SyncResult::RemoteAdopted
        );
        assert_eq!(local.emitido_total, 1_000_000_000, "emitido adoptado");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_g6_cota_relativa_rechaza_inflacion_entre_epocas() {
        // Checkpoint 23 (cota relativa): 3 épocas adelante solo pueden
        // emitir a lo sumo 3×500 LIT; 3×500+1 sat de más es inflación que
        // ni el techo absoluto ni la monotonía cazan.
        let mut local = State::new();
        local.emitido_total = 500_000_000;

        let mut mentiroso = State::new();
        mentiroso.mint(addr(0x64), 1_000);
        mentiroso.tx_count = 3 * CHECKPOINT_INTERVAL; // floor legal de época 3
        mentiroso.epoch = 3;
        mentiroso.emitido_total = 500_000_000 + 3 * GOTEO_POR_EPOCA_SAT + 1;

        let (storage, dir) = gate_storage("g6d");
        assert_eq!(
            sync_result(local.merge_if_fresher(NetworkId::Testnet, &mentiroso, &storage)),
            SyncResult::RejectedPorGate(GateReason::GoteoIncoherente),
            "cota relativa: +1 sat sobre 3×500 es REJECT"
        );
        assert_eq!(local.emitido_total, 500_000_000, "el gate no toca el local");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_g6_cota_relativa_acepta_crecimiento_exacto_multiepoca() {
        // Lado bueno de la cota relativa: N épocas adelante con el
        // crecimiento EXACTO máximo legal 500×N pasa justo por el ≤.
        let mut local = State::new();
        local.emitido_total = 500_000_000;

        let mut legitimo = State::new();
        legitimo.mint(addr(0x65), 1_000);
        legitimo.tx_count = 2 * CHECKPOINT_INTERVAL;
        legitimo.epoch = 2;
        legitimo.emitido_total = 500_000_000 + 2 * GOTEO_POR_EPOCA_SAT; // ≤ exacto

        let (storage, dir) = gate_storage("g6e");
        assert_eq!(
            sync_result(local.merge_if_fresher(NetworkId::Testnet, &legitimo, &storage)),
            SyncResult::RemoteAdopted
        );
        assert_eq!(local.emitido_total, 1_500_000_000, "crecimiento 500×2 exacto adoptado");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_g6_misma_epoca_emitido_distinto_rechazado() {
        // Checkpoint 23: con la MISMA época la emisión debe ser IDENTICA —
        // el Cierre avanza la época, de modo que mintear sin pasar por un
        // Cierre es matemáticamente imposible (b) monotonía + cota con
        // delta 0 lo obligan a la igualdad exacta).
        let mut local = State::new();
        local.emitido_total = 500_000_000;
        local.epoch = 0;
        local.tx_count = 0;

        let mut mentiroso = State::new();
        mentiroso.mint(addr(0x66), 1_000);
        mentiroso.epoch = 0;
        mentiroso.tx_count = 1; // misma época, más TXs dentro de la ventana
        mentiroso.emitido_total = 500_000_001; // +1 sat suelto: sin Cierre
        mentiroso.participacion_epoch.insert(addr(0x66), 1);

        let (storage, dir) = gate_storage("g6f");
        assert_eq!(
            sync_result(local.merge_if_fresher(NetworkId::Testnet, &mentiroso, &storage)),
            SyncResult::RejectedPorGate(GateReason::GoteoIncoherente),
            "misma época ⇒ emitido debe ser IGUAL: REJECT"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_serie_de_cierres_jamas_pisa_el_techo() {
        // §8.9: los goteos acumulan con recorte EXACTO al final: la
        // emisión de la serie toca HARD_CAP y NUNCA lo cruza; al agotarse
        // el tesoro la época siguiente pasa automáticamente al split 50/50.
        let sk = SigningKey::from_bytes(&[0x70; 32]);
        let a1 = Address::from_signing_key(&sk);

        // seed: quedan 2,500,000,000 sat = 5 goteos exactos de 500 LIT.
        let mut s = State::new();
        s.mint(a1.clone(), 1_000_000_000);
        s.emitido_total = HARD_CAP_SAT - 2_500_000_000;

        for i in 1..=5u64 {
            aplicar_tx(&mut s, &sk, 0x71, 1, 2, i); // participation por época
            let saldo_antes = s.balances[&a1];
            s.finalize_checkpoint();
            assert_eq!(s.balances[&a1] - saldo_antes, 500_000_000, "época {i}");
            assert!(s.emitido_total <= HARD_CAP_SAT);
        }
        assert_eq!(s.emitido_total, HARD_CAP_SAT, "tesoro agotado en la 5");

        // Época 6: era post-goteo — SOLO split 50/50 (mitad de fee 2 → 1).
        aplicar_tx(&mut s, &sk, 0x71, 1, 2, 6);
        let saldo_antes = s.balances[&a1];
        s.finalize_checkpoint();
        assert_eq!(s.balances[&a1] - saldo_antes, 1, "post-goteo: mitad de 2");
        assert_eq!(s.emitido_total, HARD_CAP_SAT, "post-goteo no emite más");
        assert!(s.fees_quemados <= HARD_CAP_SAT, "sanity de cap en quemados");
        // Auditable: fees_totales solo sube y jamás excede el cap.
        assert!(s.fee_pool + s.fees_quemados <= HARD_CAP_SAT);
    }

    #[test]
    fn test_migracion_emitido_seed_con_supply_congelada() {
        // D7/§9: base de la era Checkpoint 19 (sin la 5ª clave): el load
        // seedea emitido_total = supply CONGELADA y la persiste de una —
        // para que un reload NO re-seedee con una supply ya movida.
        let (storage, dir) = gate_storage("goteo_seed");
        storage.apply_balance(&addr(0x80), 700).unwrap();
        storage.apply_balance(&addr(0x81), 300).unwrap();
        storage.save_meta(5, 2, 1, 42).unwrap();
        // SIN save_emitido_total: la base rinde "era anterior".

        let mut s = State::load(&storage).unwrap();
        assert_eq!(
            s.emitido_total, 1_000,
            "seed = supply congelada (700 + 300)"
        );
        assert_eq!(
            storage.load_emitido_total().unwrap(),
            Some(1_000),
            "la seed se persiste en el MISMO load (no se re-seedeo jamás)"
        );

        // La emisión sigue su vida: +500,500,000 sat sobre la seed →
        // persiste → reload lee 500,501,000 (no un re-seedeo de la supply
        // nueva, que daría 1,001,500,000).
        s.emitido_total += 500_500_000;
        s.flush(&storage).unwrap();
        let mut s2 = State::load(&storage).unwrap();
        assert_eq!(s2.emitido_total, 500_501_000, "reload sin re-seedeo");

        // Y la participación también sobrevive el flush/load (tree nuevo).
        s2.participacion_epoch.insert(addr(0x80), 5);
        s2.flush(&storage).unwrap();
        let s3 = State::load(&storage).unwrap();
        assert_eq!(s3.participacion_epoch.get(&addr(0x80)), Some(&5));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // Checkpoint 40: Gate g7 (QuorumCertificate) + Snapshot v5
    // -----------------------------------------------------------------------

    fn test_vset_2_de_3() -> (Vec<SigningKey>, crate::quorum::ValidatorSet) {
        let keys = vec![
            SigningKey::from_bytes(&[11u8; 32]),
            SigningKey::from_bytes(&[22u8; 32]),
            SigningKey::from_bytes(&[33u8; 32]),
        ];
        let pubkeys: Vec<[u8; 32]> = keys.iter().map(|k| k.verifying_key().to_bytes()).collect();
        let vset = crate::quorum::ValidatorSet::from_pubkeys(&pubkeys, 2).unwrap();
        (keys, vset)
    }

    fn firmar_qc_para_estado(
        state: &State,
        network: NetworkId,
        keys: &[SigningKey],
        ids: &[u8],
    ) -> crate::quorum::QuorumCertificate {
        let root = state.generate_state_root();
        let chain_id = network.chain_id();
        let sigs = ids
            .iter()
            .map(|&id| crate::quorum::sign_qc_vote(&keys[id as usize], chain_id, state.epoch, &root, id))
            .collect();
        crate::quorum::QuorumCertificate {
            chain_id,
            epoch: state.epoch,
            state_root: root,
            signatures: sigs,
        }
    }

    #[test]
    fn test_g7_rechaza_epoch_sin_qc() {
        let (_, vset) = test_vset_2_de_3();
        let mut local = State::new();
        let (storage, dir) = gate_storage("g7_no_qc");
        let mut remote = testnet_legitimo();
        remote.epoch = 1;
        remote.tx_count = 4096;
        remote.latest_qc = None;

        let res = local
            .merge_if_fresher_with_quorum(NetworkId::Testnet, &remote, &storage, Some(&vset));
        assert_eq!(
            sync_result(res),
            SyncResult::RejectedPorGate(GateReason::FaltaCertificadoQuorum)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_g7_rechaza_qc_root_distinto() {
        let (keys, vset) = test_vset_2_de_3();
        let mut local = State::new();
        let (storage, dir) = gate_storage("g7_bad_root");
        let mut remote = testnet_legitimo();
        remote.epoch = 1;
        remote.tx_count = 4096;
        let qc = firmar_qc_para_estado(&remote, NetworkId::Testnet, &keys, &[0, 1]);
        remote.latest_qc = Some(qc);

        // Atacante altera su propio balance después de que los validadores firmaron el QC
        *remote.balances.entry(addr(0xAA)).or_insert(0) += 1_000_000;

        let res = local
            .merge_if_fresher_with_quorum(NetworkId::Testnet, &remote, &storage, Some(&vset));
        assert_eq!(
            sync_result(res),
            SyncResult::RejectedPorGate(GateReason::CertificadoQuorumInvalido)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_g7_acepta_snapshot_con_qc_valido() {
        let (keys, vset) = test_vset_2_de_3();
        let mut local = State::new();
        let (storage, dir) = gate_storage("g7_ok");
        let mut remote = testnet_legitimo();
        remote.epoch = 1;
        remote.tx_count = 4096;
        remote.latest_qc = Some(firmar_qc_para_estado(&remote, NetworkId::Testnet, &keys, &[0, 2]));

        let encoded = remote.encode_snapshot(NetworkId::Testnet);
        let decoded = State::decode_snapshot(&encoded, NetworkId::Testnet).unwrap();
        assert_eq!(decoded.latest_qc, remote.latest_qc);

        let res = local
            .merge_if_fresher_with_quorum(NetworkId::Testnet, &decoded, &storage, Some(&vset));
        assert_eq!(sync_result(res), SyncResult::RemoteAdopted);
        assert_eq!(local.epoch, 1);
        assert_eq!(local.latest_qc, remote.latest_qc);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_decode_snapshot_v5_bounds() {
        let (keys, _) = test_vset_2_de_3();
        let mut s = testnet_legitimo();
        s.epoch = 1;
        s.tx_count = 4096;
        s.latest_qc = Some(firmar_qc_para_estado(&s, NetworkId::Testnet, &keys, &[0, 1]));

        let encoded = s.encode_snapshot(NetworkId::Testnet);
        // Truncar los últimos 10 bytes del QC
        let truncated = &encoded[..encoded.len() - 10];
        assert!(State::decode_snapshot(truncated, NetworkId::Testnet).is_err());

        // Flag has_qc inválido
        let mut bad_flag = s.encode_snapshot(NetworkId::Testnet);
        let flag_idx = testnet_legitimo().encode_snapshot(NetworkId::Testnet).len();
        bad_flag[flag_idx] = 0x99;
        assert!(State::decode_snapshot(&bad_flag, NetworkId::Testnet).is_err());
    }

    // -----------------------------------------------------------------------
    // Checkpoint 42: Fork Choice Rule (desempate determinista + rollback)
    // -----------------------------------------------------------------------

    #[test]
    fn test_rollback_last_tx_restaura_estado_exacto() {
        let sk_a = SK::from_bytes(&[41u8; 32]);
        let sk_b = SK::from_bytes(&[42u8; 32]);
        let addr_a = Address::from_signing_key(&sk_a);
        let addr_b = Address::from_signing_key(&sk_b);

        let mut s = State::new();
        s.mint(addr_a.clone(), 10_000);
        let root_antes = s.generate_state_root();

        aplicar_tx(&mut s, &sk_a, 0x42, 1_000, 10, 1);
        assert_ne!(s.generate_state_root(), root_antes);
        assert_eq!(s.tx_count, 1);
        assert_eq!(s.fee_pool, 10);

        let rolled = s.rollback_last_tx().expect("debe haber TxUndo");
        assert_eq!(rolled.amount, 1_000);
        assert_eq!(s.generate_state_root(), root_antes);
        assert_eq!(s.tx_count, 0);
        assert_eq!(s.fee_pool, 0);
        assert!(!s.balances.contains_key(&addr(0x42)));
        assert!(!s.nonces.contains_key(&addr_a));
        assert!(!s.participacion_epoch.contains_key(&addr_a));
        let _ = addr_b;
    }

    #[test]
    fn test_fork_choice_convergencia_txs_simultaneas_distintos_emisores() {
        let sk_a = SK::from_bytes(&[51u8; 32]);
        let sk_b = SK::from_bytes(&[52u8; 32]);
        let addr_a = Address::from_signing_key(&sk_a);
        let addr_b = Address::from_signing_key(&sk_b);

        let mut base = State::new();
        base.mint(addr_a.clone(), 50_000);
        base.mint(addr_b.clone(), 50_000);

        // Nodo 1 aplica TX de A; Nodo 2 aplica TX de B simultáneamente (tx_count == 1 en ambos)
        let mut nodo1 = base.clone();
        aplicar_tx(&mut nodo1, &sk_a, 0x61, 1_000, 5, 1);
        let tx_a = nodo1.last_undo.as_ref().unwrap().tx.clone();

        let mut nodo2 = base.clone();
        aplicar_tx(&mut nodo2, &sk_b, 0x62, 2_000, 5, 1);
        let tx_b = nodo2.last_undo.as_ref().unwrap().tx.clone();

        assert_ne!(nodo1.generate_state_root(), nodo2.generate_state_root());

        let (storage1, dir1) = gate_storage("fork_n1");
        let (storage2, dir2) = gate_storage("fork_n2");

        let nodo1_snapshot = nodo1.clone();
        let res1 = nodo1
            .resolve_fork_if_divergent(NetworkId::Testnet, &nodo2, &storage1, None)
            .unwrap();
        let res2 = nodo2
            .resolve_fork_if_divergent(NetworkId::Testnet, &nodo1_snapshot, &storage2, None)
            .unwrap();

        // Uno conserva su estado y luego recibe la TX del otro; el otro hace rollback + adopta + reaplica
        match (&res1, &res2) {
            (
                ForkResolution::AdoptedRemote {
                    rolled_back_tx: Some(rb),
                    reapplied: true,
                },
                ForkResolution::KeptLocal,
            ) => {
                assert_eq!(rb.amount, tx_a.amount);
                nodo2.apply_transaction(&tx_a).unwrap();
            }
            (
                ForkResolution::KeptLocal,
                ForkResolution::AdoptedRemote {
                    rolled_back_tx: Some(rb),
                    reapplied: true,
                },
            ) => {
                assert_eq!(rb.amount, tx_b.amount);
                nodo1.apply_transaction(&tx_b).unwrap();
            }
            other => panic!("resolución inesperada: {other:?}"),
        }

        assert_eq!(
            nodo1.generate_state_root(),
            nodo2.generate_state_root(),
            "ambos nodos deben converger a exactamente el mismo state root"
        );
        assert_eq!(nodo1.tx_count, 2);
        let _ = std::fs::remove_dir_all(&dir1);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn test_fork_choice_doble_gasto_mismo_emisor_converge_al_ganador() {
        let sk_a = SK::from_bytes(&[61u8; 32]);
        let addr_a = Address::from_signing_key(&sk_a);

        let mut base = State::new();
        base.mint(addr_a, 10_000);

        // Dos TXs contradictorias con el mismo nonce=1 desde el mismo emisor
        let mut nodo1 = base.clone();
        aplicar_tx(&mut nodo1, &sk_a, 0x71, 3_000, 5, 1);

        let mut nodo2 = base;
        aplicar_tx(&mut nodo2, &sk_a, 0x72, 4_000, 5, 1);

        let (storage1, dir1) = gate_storage("fork_dg1");
        let (storage2, dir2) = gate_storage("fork_dg2");

        let nodo1_snapshot = nodo1.clone();
        let res1 = nodo1
            .resolve_fork_if_divergent(NetworkId::Testnet, &nodo2, &storage1, None)
            .unwrap();
        let res2 = nodo2
            .resolve_fork_if_divergent(NetworkId::Testnet, &nodo1_snapshot, &storage2, None)
            .unwrap();

        // El perdedor hace rollback, adopta al ganador, y al intentar reaplicar su TX con nonce=1
        // falla limpiamente (`reapplied: false`), convergiendo ambos a tx_count=1 y mismo root.
        assert!(matches!(
            (&res1, &res2),
            (
                ForkResolution::AdoptedRemote {
                    reapplied: false,
                    ..
                },
                ForkResolution::KeptLocal
            ) | (
                ForkResolution::KeptLocal,
                ForkResolution::AdoptedRemote {
                    reapplied: false,
                    ..
                }
            )
        ));
        assert_eq!(nodo1.generate_state_root(), nodo2.generate_state_root());
        assert_eq!(nodo1.tx_count, 1);
        let _ = std::fs::remove_dir_all(&dir1);
        let _ = std::fs::remove_dir_all(&dir2);
    }
}
