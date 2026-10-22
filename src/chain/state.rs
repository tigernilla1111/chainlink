//! Per-block state snapshots (design §3). Accounts follow the lean of open question 4:
//! no account records, the key is the identity, `seen_txs` blocks replays.

use std::sync::Arc;

use bytes::Bytes;
use imbl::{HashMap, HashSet, Vector};

use crate::chain::types::{AccountId, CommentId, PostId, TxId};

/// Cheap to clone: shares structure with the original.
#[derive(Clone)]
pub struct ChainState {
    pub height: u64,
    pub posts: HashMap<PostId, Arc<Post>>,
    pub comments: HashMap<CommentId, Arc<Comment>>,
    /// In posting order.
    pub threads: HashMap<PostId, Vector<CommentId>>,
    /// Txs that haven't expired yet.
    pub seen_txs: HashSet<TxId>,
}

pub struct Post {
    pub author: AccountId,
    pub height: u64,
    /// Empty once deleted.
    pub body: Bytes,
    pub deleted: bool,
}

pub struct Comment {
    pub author: AccountId,
    pub height: u64,
    pub post: PostId,
    pub reply_to: Option<CommentId>,
    /// Empty once deleted.
    pub body: Bytes,
    pub deleted: bool,
}
