use std::hash::Hash;

use crate::types::{HashBytes, PublicKeyBytes, SignatureBytes};
use ed25519_dalek::Verifier;
use ed25519_dalek::ed25519::signature::Signer;
use rand_core::OsRng;
use serde_bytes;
use sha2::{Digest, Sha512};
use thiserror;

#[derive(thiserror::Error, Debug)]
pub enum CryptoError {
    #[error("invalid signature from {0:?}")]
    InvalidSignature(PublicKeyBytes),

    #[error("invalid public key bytes {0:?}")]
    InvalidPublicKey(PublicKeyBytes),

    #[error("serialization failed")]
    Serialization,
}

impl From<SignatureBytes> for ed25519_dalek::Signature {
    fn from(value: SignatureBytes) -> Self {
        ed25519_dalek::Signature::from_bytes(&value)
    }
}
impl TryFrom<PublicKeyBytes> for ed25519_dalek::VerifyingKey {
    type Error = CryptoError;

    fn try_from(value: PublicKeyBytes) -> Result<Self, Self::Error> {
        ed25519_dalek::VerifyingKey::from_bytes(&value.0)
            .map_err(|_| CryptoError::InvalidPublicKey(value))
    }
}

pub struct PrivateKey(ed25519_dalek::SigningKey);

impl PrivateKey {
    pub fn new() -> Self {
        let mut rng = OsRng;
        Self(ed25519_dalek::SigningKey::generate(&mut rng))
    }

    pub fn public_key(&self) -> PublicKeyBytes {
        PublicKeyBytes(self.0.verifying_key().to_bytes())
    }

    pub fn sign_msg(&self, data: &Vec<u8>) -> Result<(SignatureBytes, HashBytes), CryptoError> {
        let hasher = msg_hasher(data);
        let sig = self
            .0
            .sign_prehashed(hasher.clone(), None)
            .map_err(|_| CryptoError::Serialization)?;

        Ok((
            SignatureBytes::new(sig.to_bytes()),
            HashBytes::new(hasher.finalize().into()),
        ))
    }
}

pub fn verify_msg(
    sig: &SignatureBytes,
    data: &Vec<u8>,
    sender: PublicKeyBytes,
) -> Result<HashBytes, CryptoError> {
    let hasher = msg_hasher(data);
    let pubkey = ed25519_dalek::VerifyingKey::try_from(sender)?;
    let sig: ed25519_dalek::Signature = (*sig).into();
    pubkey
        .verify_prehashed(hasher.clone(), None, &sig)
        .map_err(|_| CryptoError::InvalidSignature(sender))?;

    Ok(HashBytes::new(hasher.finalize().into()))
}

fn msg_hasher(data: &Vec<u8>) -> Sha512 {
    let mut h = Sha512::new();
    h.update(data);
    h
}
