use std::hash::Hash;

use ed25519_dalek::Verifier;
use ed25519_dalek::ed25519::signature::Signer;
use rand_core::OsRng;
use serde_bytes;
use sha2::{Digest, Sha512};
use thiserror;

const SHA512_OUTPUT_BYTES: usize = 64;

#[derive(thiserror::Error, Debug)]
pub enum CryptoError {
    #[error("invalid signature from {0:?}")]
    InvalidSignature(PublicKeyBytes),

    #[error("invalid public key bytes {0:?}")]
    InvalidPublicKey(PublicKeyBytes),

    #[error("serialization failed")]
    Serialization,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Hash)]
pub struct HashBytes(#[serde(with = "serde_bytes")] [u8; SHA512_OUTPUT_BYTES]);

#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
pub struct SignatureBytes(#[serde(with = "serde_bytes")] [u8; ed25519_dalek::SIGNATURE_LENGTH]);

impl SignatureBytes {
    fn to_signature(&self) -> ed25519_dalek::Signature {
        ed25519_dalek::Signature::from_bytes(&self.0)
    }
}

#[derive(serde::Deserialize, serde::Serialize, PartialEq, Eq, Hash, Clone, Copy, Debug)]
pub struct PublicKeyBytes(pub [u8; ed25519_dalek::PUBLIC_KEY_LENGTH]);

impl PublicKeyBytes {
    fn to_verifying_key(self) -> Result<ed25519_dalek::VerifyingKey, CryptoError> {
        ed25519_dalek::VerifyingKey::from_bytes(&self.0)
            .map_err(|_| CryptoError::InvalidPublicKey(self))
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
            SignatureBytes(sig.to_bytes()),
            HashBytes(hasher.finalize().into()),
        ))
    }
}

pub fn verify_msg(
    sig: &SignatureBytes,
    data: &Vec<u8>,
    sender: PublicKeyBytes,
) -> Result<HashBytes, CryptoError> {
    let hasher = msg_hasher(data);

    let pubkey = sender.to_verifying_key()?;
    pubkey
        .verify_prehashed(hasher.clone(), None, &sig.to_signature())
        .map_err(|_| CryptoError::InvalidSignature(sender))?;

    Ok(HashBytes(hasher.finalize().into()))
}

fn msg_hasher(data: &Vec<u8>) -> Sha512 {
    let mut h = Sha512::new();
    h.update(data);
    h
}
