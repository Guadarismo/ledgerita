use std::collections::BTreeMap;
use crate::crypto::Address;
use crate::transaction::Transaction;
use sha2::{Sha256, Digest};

/// The Global Ledger (Libro Mayor) mapping Addresses to their LIT balances.
#[derive(Debug, Clone)]
pub struct State {
    // BTreeMap guarantees deterministic ordering, crucial for consistent State Roots
    pub balances: BTreeMap<Address, u64>,
}

impl State {
    pub fn new() -> Self {
        State {
            balances: BTreeMap::new(),
        }
    }

    /// Mints initial balances (e.g., from the Genesis Treasury to validators)
    pub fn mint(&mut self, account: Address, amount: u64) {
        *self.balances.entry(account).or_insert(0) += amount;
    }

    /// Applies a transaction directly to the State.
    /// Returns an error if the transaction is invalid or funds are insufficient.
    /// This represents the mathematical execution of the Consensus.
    pub fn apply_transaction(&mut self, tx: &Transaction) -> Result<(), &'static str> {
        // 1. Proof of Formula (Signature validation)
        if !tx.is_signature_valid() {
            return Err("Invalid transaction signature!");
        }

        let total_cost = tx.amount.checked_add(tx.fee).ok_or("Overflow calculating total cost")?;

        // 2. Fund availability check
        let sender_balance = *self.balances.get(&tx.sender).unwrap_or(&0);
        if sender_balance < total_cost {
            return Err("Insufficient funds");
        }

        // 3. State execution (Deterministic math)
        self.balances.insert(tx.sender.clone(), sender_balance - total_cost);
        
        // Receiver gets the amount
        let receiver_balance = *self.balances.get(&tx.receiver).unwrap_or(&0);
        self.balances.insert(tx.receiver.clone(), receiver_balance + tx.amount);

        // (Fee logic is normally handled by allocating to the Block Leader. 
        // For now, fees are just deducted from sender and burned/withheld)

        Ok(())
    }

    /// Monitor Interno: Calcula el suministro total circulante sumando todos los saldos.
    /// Vital para asegurar matemáticamente que jamás se superen los 21,000,000 LIT.
    pub fn total_supply(&self) -> u64 {
        self.balances.values().sum()
    }

    /// Hidro-Checkpointing: Generates the Sequential Hash of the entire State
    /// Any altered bit in any account changes this root Hash entirely.
    pub fn generate_state_root(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();

        // Since it's a BTreeMap, iteration is always in the exact same sorted order (Alphabetical byte order).
        for (address, balance) in &self.balances {
            hasher.update(address.key.as_bytes());
            hasher.update(balance.to_be_bytes());
        }

        hasher.finalize().into()
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
        // Give sender some initial funds
        state.mint(sender_address.clone(), 5000);

        let mut tx = Transaction::new(sender_address.clone(), receiver_address.clone(), 1000, 10, 1);
        let msg = tx.to_bytes();
        tx.sign(sender_key.sign(&msg));

        assert!(state.apply_transaction(&tx).is_ok());
        
        // Assert balances
        assert_eq!(*state.balances.get(&sender_address).unwrap(), 5000 - 1010);
        assert_eq!(*state.balances.get(&receiver_address).unwrap(), 1000);
    }

    #[test]
    fn test_insufficient_funds() {
        let sender_key = SigningKey::from_bytes(&[1u8; 32]);
        let receiver_key = SigningKey::from_bytes(&[2u8; 32]);
        let sender_address = Address::new(sender_key.verifying_key());
        let receiver_address = Address::new(receiver_key.verifying_key());

        let mut state = State::new();
        state.mint(sender_address.clone(), 500); // Only 500 LIT

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

        // Simulate identical state being reconstructed across different nodes
        let mut state2 = State::new();
        state2.mint(Address::new(key2.verifying_key()), 200);
        state2.mint(Address::new(key1.verifying_key()), 100); // Inserted backwards

        let root2 = state2.generate_state_root();

        // BTreeMap guarantees same mathematical iteration order, so Hashes must match
        assert_eq!(root1, root2);

        // Mutating a single balance changes the root completely
        state2.mint(Address::new(key1.verifying_key()), 1);
        assert_ne!(root1, state2.generate_state_root());
    }
}
