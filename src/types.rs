use crate::crypto::{HashBytes, PublicKeyBytes, SignatureBytes};

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

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct SignedLedgerUpdateBytes {
    pub signature: SignatureBytes,
    pub canonical_bytes: Vec<u8>,
}
