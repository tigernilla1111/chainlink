use std::{hash::Hash, ops::Deref};

use crate::crypto::PublicKeyBytes;

const SHA512_OUTPUT_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Hash)]
pub struct HashBytes(#[serde(with = "serde_bytes")] [u8; SHA512_OUTPUT_BYTES]);
impl HashBytes {
    pub fn new(bytes: [u8; SHA512_OUTPUT_BYTES]) -> Self {
        HashBytes(bytes)
    }
}
impl From<[u8; SHA512_OUTPUT_BYTES]> for HashBytes {
    fn from(value: [u8; SHA512_OUTPUT_BYTES]) -> Self {
        HashBytes(value)
    }
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize, Debug)]
pub struct SignatureBytes(#[serde(with = "serde_bytes")] [u8; ed25519_dalek::SIGNATURE_LENGTH]);
impl SignatureBytes {
    pub fn new(bytes: [u8; ed25519_dalek::SIGNATURE_LENGTH]) -> SignatureBytes {
        SignatureBytes(bytes)
    }
}
impl Deref for SignatureBytes {
    type Target = [u8; ed25519_dalek::SIGNATURE_LENGTH];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
/// A set of `LedgerItem`s will be within each block
/// When a block is discovered, all items will get assigned unique IDs before being stored
#[derive(serde::Serialize, serde::Deserialize)]
pub enum LedgerUpdateType {
    Post {
        content: String,
    },
    Comment {
        post_id: HashBytes,
        parent_id: Option<HashBytes>,
        content: String,
    },
}
#[derive(serde::Serialize, serde::Deserialize)]
pub struct LedgerUpdate {
    author: PublicKeyBytes,
    data: LedgerUpdateType,
}
