//! Per-block state snapshots (design §3). Accounts follow the lean of open question 4:
//! no account records, the key is the identity, `seen_txs` blocks replays.

use std::sync::Arc;

use bytes::Bytes;
use imbl::{HashSet, OrdMap};

use crate::chain::types::{AccountId, CommentId, PostId, TxId};

/// Cheap to clone: shares structure with the original.
#[derive(Clone)]
pub struct ChainState {
    pub height: u64,
    pub posts: OrdMap<PostId, Arc<Post>>,
    pub comments: OrdMap<CommentId, Arc<Comment>>,
    /// Keyed by post and position in the thread, from 0, so a post's comments
    /// are one key range, in posting order.
    pub threads: OrdMap<(PostId, u32), CommentId>,
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
