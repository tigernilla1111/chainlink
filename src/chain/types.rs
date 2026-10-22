// TODO:
// - should only need a From or ::new for the Byte data types
use std::{hash::Hash, ops::Deref};

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

#[derive(serde::Deserialize, serde::Serialize, PartialEq, Eq, Hash, Clone, Copy, Debug)]
pub struct PublicKeyBytes(pub [u8; ed25519_dalek::PUBLIC_KEY_LENGTH]);

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize, Debug, Eq, PartialEq, Hash)]
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
#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub enum LedgerUpdateType {
    Post {
        content: String,
    },
    Comment {
        parent_post_id: HashBytes,
        parent_comment_id: Option<HashBytes>,
        content: String,
    },
}
#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct LedgerUpdate {
    pub author: PublicKeyBytes,
    pub data: LedgerUpdateType,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Eq, PartialEq, Hash)]
pub struct SignedLedgerUpdate {
    pub signature: SignatureBytes,
    /// Serialized [`LedgerUpdate`]. Stored as bytes to ensure the exact data
    /// that was signed is preserved without re-serialization discrepancies.
    pub canonical_bytes: Vec<u8>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Block {
    /// Serialized [`BlockHeader`]. Stored as bytes to ensure the block hash
    /// is always computed from the same byte representation, preventing hash
    /// malleability from different serializations of the same logical header.
    header: Vec<u8>,
    /// Vec of serialized [`SignedLedgerUpdate`]s. Stored as bytes to ensure
    /// the merkle root is computed from the exact transaction data as submitted,
    /// without re-serialization discrepancies between nodes.
    updates: Vec<Vec<u8>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct BlockHeader {
    prev_block_hash: HashBytes,
    merkle_root: HashBytes,
    nonce: u128,
}
