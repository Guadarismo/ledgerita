use crate::crypto::Address;
use crate::error::SerializationError;
use crate::transaction::Transaction;
use ed25519_dalek::Signature;

pub fn encode_address(addr: &Address) -> [u8; 32] {
    *addr.as_bytes()
}

pub fn decode_address(bytes: &[u8]) -> Result<Address, SerializationError> {
    if bytes.len() != 32 {
        return Err(SerializationError::InvalidData("address must be 32 bytes"));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(bytes);
    Ok(Address::from_hash_bytes(arr))
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
    let cap = 32 + 32 + 8 + 8 + 8 + 64 + 1 + if tx.spender_pubkey.is_some() { 32 } else { 0 };
    let mut buf = Vec::with_capacity(cap);
    buf.extend_from_slice(encode_address(&tx.sender).as_ref());
    buf.extend_from_slice(encode_address(&tx.receiver).as_ref());
    buf.extend_from_slice(&tx.amount.to_be_bytes());
    buf.extend_from_slice(&tx.fee.to_be_bytes());
    buf.extend_from_slice(&tx.nonce.to_be_bytes());
    match &tx.signature {
        Some(sig) => buf.extend_from_slice(&sig.to_bytes()),
        None => buf.extend_from_slice(&[0u8; 64]),
    }
    if let Some(pk) = &tx.spender_pubkey {
        buf.extend_from_slice(pk);
    }
    buf.push(tx.chain_id);
    buf
}

pub fn decode_transaction(bytes: &[u8]) -> Result<Transaction, SerializationError> {
    const TX_V4_D1_LEN: usize = 153;
    const TX_V4_D2_LEN: usize = 185;
    if bytes.len() != TX_V4_D1_LEN && bytes.len() != TX_V4_D2_LEN {
        return Err(SerializationError::InvalidData(
            "non-canonical transaction length or trailing bytes",
        ));
    }
    let sender = decode_address(&bytes[0..32])?;
    let receiver = decode_address(&bytes[32..64])?;
    let amount = decode_u64(&bytes[64..72])?;
    let fee = decode_u64(&bytes[72..80])?;
    let nonce = decode_u64(&bytes[80..88])?;

    let mut sig_bytes = [0u8; 64];
    sig_bytes.copy_from_slice(&bytes[88..152]);
    let signature = if sig_bytes.iter().any(|&b| b != 0) {
        Some(Signature::from_bytes(&sig_bytes))
    } else {
        None
    };

    // TX v4: D1 = 153 B (termina con chain_id), D2 = 185 B (spender_pubkey 32 B + chain_id).
    let (spender_pubkey, chain_id) = if bytes.len() == TX_V4_D2_LEN {
        let mut pk = [0u8; 32];
        pk.copy_from_slice(&bytes[152..184]);
        (Some(pk), bytes[184])
    } else {
        (None, bytes[152])
    };

    let mut tx = Transaction::new(sender, receiver, amount, fee, nonce).with_chain_id(chain_id);
    tx.spender_pubkey = spender_pubkey;
    if let Some(sig) = signature {
        tx.sign(sig);
    }
    Ok(tx)
}

pub fn hash_tx(tx: &Transaction) -> [u8; 32] {
    // Deuda 9a (Checkpoint 30): el algoritmo vive en hash.rs (única
    // puerta); esta función se conserva por compatibilidad de API.
    crate::hash::hash_tx(tx)
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
        assert_eq!(decoded.spender_pubkey, None);
    }

    #[test]
    fn test_d2_transaction_roundtrip_v3() {
        let sk = SigningKey::from_bytes(&[31u8; 32]);
        let rk = SigningKey::from_bytes(&[32u8; 32]);
        let receiver_d2 = Address::from_signing_key_d2(&rk);

        let mut tx = Transaction::new_d2(&sk.verifying_key(), receiver_d2, 2500, 10, 1);
        let msg = tx.to_bytes();
        tx.sign(sk.sign(&msg));

        let encoded = encode_transaction(&tx);
        assert_eq!(encoded.len(), 185, "TX v4 D2 debe medir 152 + 32 + 1 = 185 bytes");
        let decoded = decode_transaction(&encoded).unwrap();
        assert_eq!(decoded.spender_pubkey, Some(sk.verifying_key().to_bytes()));
        assert!(decoded.is_signature_valid());
    }

    #[test]
    fn test_d2_address_opaque_hash_no_curve_point() {
        let sk = SigningKey::from_bytes(&[33u8; 32]);
        let addr_d2 = Address::from_signing_key_d2(&sk);
        let enc = encode_address(&addr_d2);
        let dec = decode_address(&enc).unwrap();
        assert_eq!(addr_d2, dec);
        assert_eq!(Address::from_hex(&addr_d2.to_hex()), Some(addr_d2));
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

    #[test]
    fn test_decode_transaction_rechaza_longitudes_no_canonicas() {
        let sk = SigningKey::from_bytes(&[1u8; 32]);
        let rk = SigningKey::from_bytes(&[2u8; 32]);
        let mut tx = Transaction::new(
            Address::from_signing_key(&sk),
            Address::from_signing_key(&rk),
            100,
            1,
            1,
        );
        let msg = tx.to_bytes();
        tx.sign(sk.sign(&msg));
        let canon = encode_transaction(&tx);
        assert_eq!(canon.len(), 153);

        // 152 B sin chain_id, 154 B con trailing byte, 88 B truncada
        assert!(decode_transaction(&canon[..88]).is_err());
        assert!(decode_transaction(&canon[..152]).is_err());
        let mut with_trailing = canon.clone();
        with_trailing.push(0x99);
        assert!(decode_transaction(&with_trailing).is_err());
    }
}
