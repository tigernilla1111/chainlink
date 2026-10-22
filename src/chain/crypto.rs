//! SHA-256 and Ed25519 verification (design §2). Signing lives with the app.

use ed25519_dalek::VerifyingKey;
use sha2::{Digest, Sha256};

use crate::chain::types::{AccountId, Hash, Signature};

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
#[error("invalid signature from {0:?}")]
pub struct InvalidSignature(pub AccountId);

pub fn sha256(data: &[u8]) -> Hash {
    Hash(Sha256::digest(data).into())
}

/// Plain Ed25519, verified strictly: a small-order key or `R`, or a non-canonical `S`,
/// is rejected, so every node accepts exactly the same signatures.
pub fn verify(author: AccountId, msg: &[u8], sig: &Signature) -> Result<(), InvalidSignature> {
    VerifyingKey::from_bytes(&author.0)
        .and_then(|key| key.verify_strict(msg, sig))
        .map_err(|_| InvalidSignature(author))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey, Verifier};

    #[test]
    fn sha256_known_vector() {
        let hex: String = sha256(b"abc")
            .0
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sign_verify_round_trip() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let author = AccountId(key.verifying_key().to_bytes());
        let sig = key.sign(b"hello");
        assert_eq!(verify(author, b"hello", &sig), Ok(()));
        assert_eq!(
            verify(author, b"hellp", &sig),
            Err(InvalidSignature(author))
        );

        let other = AccountId(SigningKey::from_bytes(&[8; 32]).verifying_key().to_bytes());
        assert_eq!(verify(other, b"hello", &sig), Err(InvalidSignature(other)));
    }

    #[test]
    fn strict_rejects_small_order_key() {
        let mut identity = [0; 32];
        identity[0] = 1;
        let author = AccountId(identity);
        let mut sig_bytes = [0; 64];
        sig_bytes[..32].copy_from_slice(&identity);
        let sig = Signature::from_bytes(&sig_bytes);

        let key = VerifyingKey::from_bytes(&identity).unwrap();
        assert!(key.verify(b"any message", &sig).is_ok());
        assert_eq!(
            verify(author, b"any message", &sig),
            Err(InvalidSignature(author))
        );
    }
}
