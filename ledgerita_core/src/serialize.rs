use crate::crypto::Address;
use crate::error::SerializationError;
use crate::transaction::Transaction;
use ed25519_dalek::{Signature, VerifyingKey};

pub fn encode_address(addr: &Address) -> [u8; 32] {
    let mut buf = [0u8; 32];
    buf.copy_from_slice(addr.key.as_bytes());
    buf
}

pub fn decode_address(bytes: &[u8]) -> Result<Address, SerializationError> {
    if bytes.len() != 32 {
        return Err(SerializationError::InvalidData("address must be 32 bytes"));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(bytes);
    let key = VerifyingKey::from_bytes(&arr)
        .map_err(|_| SerializationError::InvalidData("invalid ed25519 public key"))?;
    Ok(Address::new(key))
}

pub fn encode_u64(val: u64) -> [u8; 8] {
    val.to_be_bytes()
}

pub fn decode_u64(bytes: &[u8]) -> Result<u64, SerializationError> {
    if bytes.len() != 8 {
        return Err(SerializationError::InvalidData("u64 value must be 8 bytes"));
    }
    let mut arr = [0u8; 8];
    arr.copy_from_slice(bytes);
    Ok(u64::from_be_bytes(arr))
}

pub fn encode_transaction(tx: &Transaction) -> Vec<u8> {
    let mut buf = Vec::with_capacity(32 + 32 + 8 + 8 + 8 + 64);
    buf.extend_from_slice(encode_address(&tx.sender).as_ref());
    buf.extend_from_slice(encode_address(&tx.receiver).as_ref());
    buf.extend_from_slice(&tx.amount.to_be_bytes());
    buf.extend_from_slice(&tx.fee.to_be_bytes());
    buf.extend_from_slice(&tx.nonce.to_be_bytes());
    match &tx.signature {
        Some(sig) => buf.extend_from_slice(&sig.to_bytes()),
        None => buf.extend_from_slice(&[0u8; 64]),
    }
    buf
}

pub fn decode_transaction(bytes: &[u8]) -> Result<Transaction, SerializationError> {
    if bytes.len() < 32 + 32 + 8 + 8 + 8 {
        return Err(SerializationError::InvalidData("transaction too short"));
    }
    let sender = decode_address(&bytes[0..32])?;
    let receiver = decode_address(&bytes[32..64])?;
    let amount = decode_u64(&bytes[64..72])?;
    let fee = decode_u64(&bytes[72..80])?;
    let nonce = decode_u64(&bytes[80..88])?;

    let signature = if bytes.len() >= 152 {
        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(&bytes[88..152]);
        if sig_bytes.iter().any(|&b| b != 0) {
            Some(Signature::from_bytes(&sig_bytes))
        } else {
            None
        }
    } else {
        None
    };

    let mut tx = Transaction::new(sender, receiver, amount, fee, nonce);
    if let Some(sig) = signature {
        tx.sign(sig);
    }
    Ok(tx)
}

pub fn hash_tx(tx: &Transaction) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(encode_transaction(tx));
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn test_address_roundtrip() {
        let sk = SigningKey::from_bytes(&[0xab; 32]);
        let addr = Address::from_signing_key(&sk);
        let encoded = encode_address(&addr);
        let decoded = decode_address(&encoded).unwrap();
        assert_eq!(addr.to_hex(), decoded.to_hex());
    }

    #[test]
    fn test_u64_roundtrip() {
        let val = 123456789012345u64;
        let encoded = encode_u64(val);
        let decoded = decode_u64(&encoded).unwrap();
        assert_eq!(val, decoded);
    }

    #[test]
    fn test_transaction_roundtrip_with_sig() {
        let sk = SigningKey::from_bytes(&[1u8; 32]);
        let rk = SigningKey::from_bytes(&[2u8; 32]);
        let sender = Address::from_signing_key(&sk);
        let receiver = Address::from_signing_key(&rk);

        let mut tx = Transaction::new(sender, receiver, 1000, 10, 1);
        let msg = tx.to_bytes();
        tx.sign(sk.sign(&msg));

        let encoded = encode_transaction(&tx);
        let decoded = decode_transaction(&encoded).unwrap();
        assert!(decoded.is_signature_valid());
        assert_eq!(tx.sender.to_hex(), decoded.sender.to_hex());
        assert_eq!(tx.amount, decoded.amount);
        assert_eq!(tx.fee, decoded.fee);
        assert_eq!(tx.nonce, decoded.nonce);
    }

    #[test]
    fn test_transaction_roundtrip_no_sig() {
        let sk = SigningKey::from_bytes(&[1u8; 32]);
        let rk = SigningKey::from_bytes(&[2u8; 32]);
        let sender = Address::from_signing_key(&sk);
        let receiver = Address::from_signing_key(&rk);

        let tx = Transaction::new(sender, receiver, 500, 5, 0);
        let encoded = encode_transaction(&tx);
        let decoded = decode_transaction(&encoded).unwrap();
        assert!(decoded.signature.is_none());
        assert!(!decoded.is_signature_valid());
    }

    #[test]
    fn test_hash_uniqueness() {
        let sk = SigningKey::from_bytes(&[1u8; 32]);
        let rk = SigningKey::from_bytes(&[2u8; 32]);
        let sender = Address::from_signing_key(&sk);
        let receiver = Address::from_signing_key(&rk);

        let tx1 = Transaction::new(sender.clone(), receiver.clone(), 100, 1, 1);
        let tx2 = Transaction::new(sender, receiver, 200, 1, 1);
        assert_ne!(hash_tx(&tx1), hash_tx(&tx2));
    }
}
