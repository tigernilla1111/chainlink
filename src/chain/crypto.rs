use crate::chain::types::{AccountId, Signature};
use ed25519_dalek::Verifier;
use ed25519_dalek::ed25519::signature::Signer;
use rand_core::OsRng;
use sha2::{Digest, Sha512};

#[derive(thiserror::Error, Debug)]
pub enum CryptoError {
    #[error("invalid signature from {0:?}")]
    InvalidSignature(AccountId),

    #[error("invalid public key bytes {0:?}")]
    InvalidPublicKey(AccountId),

    #[error("serialization failed")]
    Serialization,
}

impl TryFrom<AccountId> for ed25519_dalek::VerifyingKey {
    type Error = CryptoError;

    fn try_from(value: AccountId) -> Result<Self, Self::Error> {
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

    pub fn public_key(&self) -> AccountId {
        AccountId(self.0.verifying_key().to_bytes())
    }

    pub fn sign_msg(&self, data: &[u8]) -> Result<Signature, CryptoError> {
        self.0
            .sign_prehashed(msg_hasher(data), None)
            .map_err(|_| CryptoError::Serialization)
    }
}

pub fn verify_msg(sig: &Signature, data: &[u8], sender: AccountId) -> Result<(), CryptoError> {
    let pubkey = ed25519_dalek::VerifyingKey::try_from(sender)?;
    pubkey
        .verify_prehashed(msg_hasher(data), None, sig)
        .map_err(|_| CryptoError::InvalidSignature(sender))
}

fn msg_hasher(data: &[u8]) -> Sha512 {
    let mut h = Sha512::new();
    h.update(data);
    h
}
