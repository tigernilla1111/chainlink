//! Per-block state snapshots and `apply_block` (design §3, §4, §8). No account records: the key
//! is the identity, and `seen_txs` blocks replays.

use std::sync::Arc;

use bytes::Bytes;
use imbl::{HashSet, OrdMap};

use crate::chain::block::{Block, Tx, TxPayload};
use crate::chain::types::{AccountId, CommentId, PostId, TxId};
use crate::chain::{R, TX_TTL, genesis_block};

/// Cheap to clone: shares structure with the original.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainState {
    pub height: u64,
    pub posts: OrdMap<PostId, Arc<Post>>,
    pub comments: OrdMap<CommentId, Arc<Comment>>,
    /// Keyed by post and position in the thread, from 0, so a post's comments
    /// are one key range, in posting order.
    pub threads: OrdMap<(PostId, u32), CommentId>,
    /// Txs that haven't expired yet.
    pub seen_txs: HashSet<TxId>,
    /// The last `R + 1` blocks by height (every block while the chain is shorter), read when
    /// advancing (design §8). Keyed by height, so the expiring blocks are plain lookups.
    pub blocks: OrdMap<u64, Arc<Block>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Post {
    pub author: AccountId,
    pub height: u64,
    /// Empty once deleted.
    pub body: Bytes,
    pub deleted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub author: AccountId,
    pub height: u64,
    pub post: PostId,
    pub reply_to: Option<CommentId>,
    /// Empty once deleted.
    pub body: Bytes,
    pub deleted: bool,
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum TxError {
    #[error("tx {tx:?} is valid until {valid_until_height}, outside the window at height {height}")]
    OutOfWindow {
        tx: TxId,
        valid_until_height: u64,
        height: u64,
    },
    #[error("tx {0:?} was already included")]
    Replay(TxId),
    /// A reply's post isn't a post in the state, or a delete's target isn't a post or comment in
    /// it (never was, or aged out past `R`).
    #[error(
        "tx {tx:?} targets {target:?}, which isn't a post or comment it can target in the state"
    )]
    UnknownTarget { tx: TxId, target: TxId },
    #[error("tx {tx:?} replies to {reply_to:?}, which isn't in its post's thread")]
    ReplyToNotInThread { tx: TxId, reply_to: CommentId },
    #[error("tx {tx:?} deletes {target:?}, which is already deleted")]
    AlreadyDeleted { tx: TxId, target: TxId },
    #[error("tx {tx:?} deletes {target:?}, whose author it isn't")]
    NotAuthor { tx: TxId, target: TxId },
}

impl ChainState {
    /// Empty, at height 0, holding the genesis block.
    pub fn genesis() -> Self {
        Self {
            height: 0,
            posts: OrdMap::new(),
            comments: OrdMap::new(),
            threads: OrdMap::new(),
            seen_txs: HashSet::new(),
            blocks: OrdMap::unit(0, genesis_block()),
        }
    }

    /// The state one block later, at height `h`, before any tx (design §8): the `seen_txs`
    /// entries of block `h − TX_TTL − 1` go, and the posts of block `h − R − 1` go with their
    /// threads.
    fn advanced(&self) -> Self {
        let mut state = self.clone();
        state.height += 1;
        let h = state.height;
        if let Some(k) = h.checked_sub(TX_TTL + 1) {
            let expired = state
                .blocks
                .get(&k)
                .expect("the state keeps its last R + 1 blocks");
            for tx in &expired.txs {
                state.seen_txs.remove(&tx.id());
            }
        }
        if let Some(k) = h.checked_sub(R + 1) {
            let aged = state
                .blocks
                .remove(&k)
                .expect("the state keeps its last R + 1 blocks");
            for tx in &aged.txs {
                if matches!(tx.payload, TxPayload::CreatePost { .. }) {
                    state.drop_thread(tx.id());
                }
            }
        }
        state
    }

    /// Removes a post and every comment under it. A post's comments are one key range of
    /// `threads` (design §3), and no comment outlives its post, since a reply needs its post.
    fn drop_thread(&mut self, post: PostId) {
        self.posts.remove(&post);
        let thread: Vec<_> = self
            .threads
            .range((post, 0)..=(post, u32::MAX))
            .map(|(&key, &comment)| (key, comment))
            .collect();
        for (key, comment) in thread {
            self.threads.remove(&key);
            self.comments.remove(&comment);
        }
    }

    /// Applies one tx at this state's height (design §4). Everything is checked before
    /// anything is written, so a failed tx leaves the state as it was.
    fn apply_tx(&mut self, tx: &Tx) -> Result<(), TxError> {
        let id = tx.id();
        let height = self.height;
        if !tx.in_window(height) {
            return Err(TxError::OutOfWindow {
                tx: id,
                valid_until_height: tx.valid_until_height,
                height,
            });
        }
        if self.seen_txs.contains(&id) {
            return Err(TxError::Replay(id));
        }
        match &tx.payload {
            TxPayload::CreatePost { body } => {
                self.posts.insert(
                    id,
                    Arc::new(Post {
                        author: tx.author,
                        height,
                        body: body.clone(),
                        deleted: false,
                    }),
                );
            }
            TxPayload::CreateComment {
                post,
                reply_to,
                body,
            } => {
                if !self.posts.contains_key(post) {
                    return Err(TxError::UnknownTarget {
                        tx: id,
                        target: *post,
                    });
                }
                if let Some(reply_to) = reply_to
                    && self.comments.get(reply_to).is_none_or(|c| c.post != *post)
                {
                    return Err(TxError::ReplyToNotInThread {
                        tx: id,
                        reply_to: *reply_to,
                    });
                }
                // The position after the last comment in this thread, or 0. A thread can't reach
                // `u32::MAX`: a full block holds about 880 comments, so about 18M in `R` blocks.
                let position = match self.threads.get_prev(&(*post, u32::MAX)) {
                    Some(((p, last), _)) if p == post => last + 1,
                    _ => 0,
                };
                self.threads.insert((*post, position), id);
                self.comments.insert(
                    id,
                    Arc::new(Comment {
                        author: tx.author,
                        height,
                        post: *post,
                        reply_to: *reply_to,
                        body: body.clone(),
                        deleted: false,
                    }),
                );
            }
            TxPayload::Delete { target } => {
                let (is_post, author, deleted) =
                    match (self.posts.get(target), self.comments.get(target)) {
                        (Some(post), _) => (true, post.author, post.deleted),
                        (None, Some(comment)) => (false, comment.author, comment.deleted),
                        (None, None) => {
                            return Err(TxError::UnknownTarget {
                                tx: id,
                                target: *target,
                            });
                        }
                    };
                if deleted {
                    return Err(TxError::AlreadyDeleted {
                        tx: id,
                        target: *target,
                    });
                }
                if author != tx.author {
                    return Err(TxError::NotAuthor {
                        tx: id,
                        target: *target,
                    });
                }
                // Only the map that holds the target: imbl's get_mut copies the tree path even
                // for a missing key.
                if is_post {
                    let post = Arc::make_mut(&mut self.posts[target]);
                    post.deleted = true;
                    post.body = Bytes::new();
                } else {
                    let comment = Arc::make_mut(&mut self.comments[target]);
                    comment.deleted = true;
                    comment.body = Bytes::new();
                }
            }
        }
        self.seen_txs.insert(id);
        Ok(())
    }
}

/// The parent's state with `block` applied (design §4): advanced to the block's height, then
/// its txs in order, so a tx can rely on an earlier one in the same block. The parent is
/// never changed. Step 5 checks the height first (design §4), so another height is a caller bug.
pub fn apply_block(parent: &ChainState, block: &Arc<Block>) -> Result<ChainState, TxError> {
    let height = block.header.height;
    assert_eq!(
        height,
        parent.height + 1,
        "a block is applied at its parent's height + 1"
    );
    let mut state = parent.advanced();
    for tx in &block.txs {
        state.apply_tx(tx)?;
    }
    state.blocks.insert(height, Arc::clone(block));
    Ok(state)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use super::*;
    use crate::chain::block::BlockHeader;
    use crate::chain::types::{BlockId, Hash, Signature};

    const A: u8 = 1;
    const B: u8 = 2;
    /// In no state.
    const X: TxId = TxId(Hash([9; 32]));

    fn tx(author: u8, valid_until_height: u64, payload: TxPayload) -> Tx {
        Tx {
            author: AccountId([author; 32]),
            valid_until_height,
            payload,
            signature: Signature::from_bytes(&[0; 64]),
        }
    }

    fn post(author: u8, valid_until_height: u64, body: &'static str) -> Tx {
        let body = Bytes::from_static(body.as_bytes());
        tx(author, valid_until_height, TxPayload::CreatePost { body })
    }

    fn comment(
        author: u8,
        valid_until_height: u64,
        post: PostId,
        reply_to: Option<CommentId>,
        body: &'static str,
    ) -> Tx {
        let body = Bytes::from_static(body.as_bytes());
        let payload = TxPayload::CreateComment {
            post,
            reply_to,
            body,
        };
        tx(author, valid_until_height, payload)
    }

    fn delete(author: u8, valid_until_height: u64, target: TxId) -> Tx {
        tx(author, valid_until_height, TxPayload::Delete { target })
    }

    /// The state reads only the height and the txs.
    fn block(height: u64, txs: Vec<Tx>) -> Arc<Block> {
        Arc::new(Block {
            header: BlockHeader {
                parent: BlockId(Hash([0; 32])),
                height,
                timestamp: 0,
                tx_root: Hash([0; 32]),
                difficulty: NonZeroU64::new(1).unwrap(),
                nonce: 0,
            },
            txs,
        })
    }

    fn apply(state: &ChainState, txs: Vec<Tx>) -> Result<ChainState, TxError> {
        apply_block(state, &block(state.height + 1, txs))
    }

    /// Empty blocks until `height == to`.
    fn extend(mut state: ChainState, to: u64) -> ChainState {
        while state.height < to {
            state = apply(&state, vec![]).unwrap();
        }
        state
    }

    #[test]
    fn comments_take_the_next_position_in_their_thread() {
        let vu = 1 + TX_TTL;
        let (p, q) = (post(A, vu, "p"), post(A, vu, "q"));
        // Only then does Q's first comment come after P's comments in `threads`.
        assert!(p.id() < q.id());
        let c1 = comment(B, vu, p.id(), None, "c1");
        let state = apply(
            &ChainState::genesis(),
            vec![p.clone(), q.clone(), c1.clone()],
        )
        .unwrap();

        let vu = 2 + TX_TTL;
        let c2 = comment(B, vu, q.id(), None, "c2");
        let c3 = comment(B, vu, p.id(), None, "c3");
        let state = apply(&state, vec![c2.clone(), c3.clone()]).unwrap();
        assert_eq!(
            state.threads,
            OrdMap::from(vec![
                ((p.id(), 0), c1.id()),
                ((p.id(), 1), c3.id()),
                ((q.id(), 0), c2.id()),
            ])
        );
    }

    #[test]
    fn a_delete_empties_the_body_and_keeps_the_marker() {
        let vu = 1 + TX_TTL;
        let p = post(A, vu, "p");
        let c1 = comment(B, vu, p.id(), None, "c1");
        let c2 = comment(B, vu, p.id(), Some(c1.id()), "c2");
        let before = apply(
            &ChainState::genesis(),
            vec![p.clone(), c1.clone(), c2.clone()],
        )
        .unwrap();

        let vu = 2 + TX_TTL;
        let state = apply(&before, vec![delete(A, vu, p.id()), delete(B, vu, c2.id())]).unwrap();
        let post = &state.posts[&p.id()];
        assert!(post.deleted);
        assert!(post.body.is_empty());
        assert_eq!((post.author, post.height), (AccountId([A; 32]), 1));
        let comment = &state.comments[&c2.id()];
        assert!(comment.deleted);
        assert!(comment.body.is_empty());
        assert_eq!(
            (comment.author, comment.height, comment.reply_to),
            (AccountId([B; 32]), 1, Some(c1.id()))
        );
        assert_eq!(state.threads.get(&(p.id(), 1)), Some(&c2.id()));
        // The state before the delete is unchanged.
        let post = &before.posts[&p.id()];
        assert_eq!((post.body.as_ref(), post.deleted), (&b"p"[..], false));
        let comment = &before.comments[&c2.id()];
        assert_eq!(
            (comment.body.as_ref(), comment.deleted),
            (&b"c2"[..], false)
        );
    }

    #[test]
    fn replies_to_deleted_items_are_allowed() {
        let vu = 1 + TX_TTL;
        let p = post(A, vu, "p");
        let c = comment(B, vu, p.id(), None, "c");
        let state = apply(&ChainState::genesis(), vec![p.clone(), c.clone()]).unwrap();

        let vu = 2 + TX_TTL;
        let state = apply(&state, vec![delete(B, vu, c.id()), delete(A, vu, p.id())]).unwrap();

        let reply = comment(B, 3 + TX_TTL, p.id(), Some(c.id()), "reply");
        assert!(apply(&state, vec![reply]).is_ok());
    }

    #[test]
    fn bad_txs_fail_the_block_naming_the_tx_and_the_rule() {
        let vu = 1 + TX_TTL;
        let p = post(A, vu, "p");
        let q = post(A, vu, "q");
        let c = comment(B, vu, q.id(), None, "c");
        let d = post(A, vu, "d");
        let e = comment(B, vu, q.id(), None, "e");
        let txs = vec![
            p.clone(),
            q,
            c.clone(),
            d.clone(),
            delete(A, vu, d.id()),
            e.clone(),
            delete(B, vu, e.id()),
        ];
        let state = apply(&ChainState::genesis(), txs).unwrap();

        let vu = 2 + TX_TTL;
        let too_far = post(A, 3 + TX_TTL, "too far");
        let under_unknown = comment(B, vu, X, None, "under unknown");
        let under_comment = comment(B, vu, c.id(), None, "under a comment");
        let delete_unknown = delete(A, vu, X);
        let reply_to_unknown = comment(B, vu, p.id(), Some(X), "reply to unknown");
        let reply_to_other_thread = comment(B, vu, p.id(), Some(c.id()), "other thread");
        let delete_again = delete(A, vu, d.id());
        let delete_comment_again = delete(B, vu, e.id());
        let delete_by_other = delete(B, vu, p.id());
        let delete_comment_by_other = delete(A, vu, c.id());
        let table = [
            (
                &too_far,
                TxError::OutOfWindow {
                    tx: too_far.id(),
                    valid_until_height: 3 + TX_TTL,
                    height: 2,
                },
            ),
            (&p, TxError::Replay(p.id())),
            (
                &under_unknown,
                TxError::UnknownTarget {
                    tx: under_unknown.id(),
                    target: X,
                },
            ),
            // A reply names its post, not a comment.
            (
                &under_comment,
                TxError::UnknownTarget {
                    tx: under_comment.id(),
                    target: c.id(),
                },
            ),
            (
                &delete_unknown,
                TxError::UnknownTarget {
                    tx: delete_unknown.id(),
                    target: X,
                },
            ),
            (
                &reply_to_unknown,
                TxError::ReplyToNotInThread {
                    tx: reply_to_unknown.id(),
                    reply_to: X,
                },
            ),
            (
                &reply_to_other_thread,
                TxError::ReplyToNotInThread {
                    tx: reply_to_other_thread.id(),
                    reply_to: c.id(),
                },
            ),
            (
                &delete_again,
                TxError::AlreadyDeleted {
                    tx: delete_again.id(),
                    target: d.id(),
                },
            ),
            (
                &delete_comment_again,
                TxError::AlreadyDeleted {
                    tx: delete_comment_again.id(),
                    target: e.id(),
                },
            ),
            (
                &delete_by_other,
                TxError::NotAuthor {
                    tx: delete_by_other.id(),
                    target: p.id(),
                },
            ),
            (
                &delete_comment_by_other,
                TxError::NotAuthor {
                    tx: delete_comment_by_other.id(),
                    target: c.id(),
                },
            ),
        ];
        for (tx, err) in table {
            assert_eq!(apply(&state, vec![tx.clone()]).err(), Some(err));
        }
    }

    #[test]
    fn a_thread_lives_exactly_r_blocks() {
        let p = post(A, 1 + TX_TTL, "p");
        let state = apply(&ChainState::genesis(), vec![p.clone()]).unwrap();

        let vu = 2 + TX_TTL;
        let q = post(A, vu, "Q");
        // So a drop of P's thread that ran past P would reach Q's.
        assert!(p.id() < q.id());
        let p1 = comment(B, vu, p.id(), None, "p1");
        let q1 = comment(B, vu, q.id(), None, "q1");
        let state = apply(&state, vec![q.clone(), p1.clone(), q1.clone()]).unwrap();
        let state = extend(state, R);

        // Block R + 1 is the last that can reply to a post from block 1.
        let p2 = comment(B, R + 1 + TX_TTL, p.id(), None, "p2");
        let state = apply(&state, vec![p2.clone()]).unwrap();
        assert!(state.posts.contains_key(&p.id()));

        let p3 = comment(B, R + 2 + TX_TTL, p.id(), None, "p3");
        assert_eq!(
            apply(&state, vec![p3.clone()]).err(),
            Some(TxError::UnknownTarget {
                tx: p3.id(),
                target: p.id(),
            })
        );
        let state = apply(&state, vec![]).unwrap();
        assert!(!state.posts.contains_key(&p.id()));
        assert!(!state.comments.contains_key(&p1.id()));
        assert!(!state.comments.contains_key(&p2.id()));
        assert!(state.posts.contains_key(&q.id()));
        assert!(state.comments.contains_key(&q1.id()));
        assert_eq!(state.threads, OrdMap::unit((q.id(), 0), q1.id()));
        assert_eq!(state.blocks.len() as u64, R + 1);
        assert!(!state.blocks.contains_key(&1));
    }

    #[test]
    fn seen_txs_entries_are_dropped_exactly_when_the_tx_expires() {
        let t = post(A, 1 + TX_TTL, "t");
        let state = apply(&ChainState::genesis(), vec![t.clone()]).unwrap();
        let state = extend(state, TX_TTL);

        // Still in its window at TX_TTL + 1, so only `seen_txs` stops it.
        assert_eq!(
            apply(&state, vec![t.clone()]).err(),
            Some(TxError::Replay(t.id()))
        );
        let state = apply(&state, vec![]).unwrap();
        assert_eq!(
            apply(&state, vec![t.clone()]).err(),
            Some(TxError::OutOfWindow {
                tx: t.id(),
                valid_until_height: 1 + TX_TTL,
                height: TX_TTL + 2,
            })
        );
        let state = apply(&state, vec![]).unwrap();
        assert!(!state.seen_txs.contains(&t.id()));
    }
}
