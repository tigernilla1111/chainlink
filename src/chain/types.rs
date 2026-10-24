//! IDs. All 32 bytes (design §3).

use serde::{Deserialize, Serialize};

/// A SHA-256 digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Hash(pub [u8; 32]);

/// SHA-256 of the serialized header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BlockId(pub Hash);

/// SHA-256 of the bytes the author signed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TxId(pub Hash);

/// The `TxId` of the tx that created the post or comment.
pub type PostId = TxId;
pub type CommentId = TxId;

/// The author's Ed25519 public key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountId(pub [u8; 32]);

pub type Signature = ed25519_dalek::Signature;
