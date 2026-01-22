use sha2::{Digest, Sha512};

use crate::crypto::HashBytes;

struct PostId(HashBytes);
struct CommentId(HashBytes);

/// A set of `LedgerItem`s will be within each block
/// When a block is discovered, all items will get assigned unique IDs before being stored
pub enum LedgerItem {
    Post {
        author: crate::crypto::PublicKeyBytes,
        content: String,
    },
    Comment {
        author: crate::crypto::PublicKeyBytes,
        parent_id: Option<CommentId>,
        content: String,
    },
}

struct SignedLedgerItem {
    signature: crate::crypto::SignatureBytes,
}
