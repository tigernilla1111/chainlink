//! Accepting a block: one already stored, held or known invalid is dropped; one whose header
//! fails on its own, or whose body misses its header's root, is dropped without being
//! remembered; one whose parent is unknown waits in the store's orphan pool; one that fails
//! against its parent, in its txs or in applying them is remembered as invalid with every orphan
//! waiting on it; the rest get their state built on their parent's, and the best tip is the one
//! with the most total work, first seen on a tie. A reorg is a change of `best`; nothing is
//! undone. Pruning and the mempool come with later items. See design §4 and §5.

use std::sync::Arc;

use primitive_types::U256;

use crate::chain::block::Block;
use crate::chain::state::{ChainState, TxError, apply_block};
use crate::chain::store::{BlockNode, BlockStore, StoreError};
use crate::chain::types::BlockId;
use crate::chain::validate::{BlockError, check_header, check_parent, check_root, check_txs};
use crate::chain::{GENESIS_ID, MEDIAN_BLOCKS, genesis_block};

pub struct ChainManager {
    store: BlockStore,
    best: BlockId,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Accepted {
    /// Stored, with any orphans it unblocked; `best` may have moved.
    Stored,
    /// Held until its parent arrives.
    Orphaned,
    /// Already stored, held as an orphan or known invalid: nothing changed.
    AlreadySeen,
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum ValidationError {
    #[error(transparent)]
    Block(#[from] BlockError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Tx(#[from] TxError),
    #[error("parent {0:?} is known invalid")]
    InvalidParent(BlockId),
}

impl ChainManager {
    /// Genesis alone, as the best tip.
    pub fn new() -> Self {
        Self {
            store: BlockStore::new(genesis_block(), ChainState::genesis()),
            best: GENESIS_ID,
        }
    }

    /// Drops a block already seen, then one whose header fails on its own or whose body misses
    /// its root, without remembering it; holds one whose parent is unknown; and otherwise checks
    /// it against its parent and its txs, applies it and stores it, then does the same for the
    /// orphans it unblocks. `now` is the local clock in Unix seconds, read by the clock check and
    /// the orphan age cap. A block that fails from here on, or whose parent is known invalid, is
    /// remembered as invalid without being applied, and so is every orphan waiting on it. See
    /// design §4.
    pub fn accept(&mut self, block: Arc<Block>, now: u64) -> Result<Accepted, ValidationError> {
        self.store.drop_expired_orphans(now);
        let id = block.header.id();
        if self.store.has_seen(id, &block) {
            return Ok(Accepted::AlreadySeen);
        }
        // Before the pool, so every orphan carries real work and the body its header commits to:
        // `has_seen` matches a held block by its header alone.
        check_header(&block, now)?;
        check_root(&block)?;
        // A known-invalid parent counts as known, so its child is marked, not held.
        let parent = block.header.parent;
        let result = if self.store.is_invalid(parent) {
            Err(ValidationError::InvalidParent(parent))
        } else if self.store.get(parent).is_none() {
            self.store.hold_orphan(block, now);
            return Ok(Accepted::Orphaned);
        } else {
            self.apply_and_store(block)
        };
        self.settle(id, result.is_ok());
        result.map(|()| Accepted::Stored)
    }

    /// Marks `id` invalid if it failed, then works down through the orphans waiting on it: those
    /// under a stored block are applied and stored, those under a failed one are marked with it,
    /// since a child of an invalid block is invalid. A work-list, not recursion: a chain of
    /// orphans can be as long as the pool allows. See design §4.
    fn settle(&mut self, id: BlockId, ok: bool) {
        let mut settled = vec![(id, ok)];
        while let Some((parent, parent_ok)) = settled.pop() {
            if !parent_ok {
                self.store.mark_invalid(parent);
            }
            for orphan in self.store.take_orphans(parent) {
                let id = orphan.header.id();
                let ok = parent_ok && self.apply_and_store(orphan).is_ok();
                settled.push((id, ok));
            }
        }
    }

    /// Checks a block whose parent is stored against it, height then timestamp, and its txs on
    /// their own, then builds its state on the parent's and stores it. A failed check or apply
    /// stores nothing, and the caller remembers the block: every failure here is one that follows
    /// from the header or from txs it committed to, since the header and the root were checked
    /// before the block could get here. See design §4, steps 5 to 8.
    fn apply_and_store(&mut self, block: Arc<Block>) -> Result<(), ValidationError> {
        let parent = self.store.parent_of(&block)?;
        check_parent(&block.header, &self.recent_timestamps(parent))?;
        check_txs(&block)?;
        let parent_state = parent
            .state
            .as_ref()
            .expect("every stored block keeps its state until pruning lands");
        let state = apply_block(parent_state, &block)?;
        self.on_new_valid_block(block, state);
        Ok(())
    }

    /// `parent`'s timestamp, then its ancestors', newest first, at most `MEDIAN_BLOCKS` of them
    /// and fewer near genesis. See design §4, step 5.
    fn recent_timestamps(&self, parent: &BlockNode) -> Vec<u64> {
        std::iter::successors(Some(parent), |node| {
            self.store.get(node.block.header.parent)
        })
        .take(MEDIAN_BLOCKS)
        .map(|node| node.block.header.timestamp)
        .collect()
    }

    /// Stores the block and makes it the best tip only on strictly more work, so the tip seen
    /// first wins a tie. See design §5.
    fn on_new_valid_block(&mut self, block: Arc<Block>, state: ChainState) -> BlockId {
        let id = self
            .store
            .insert(block, state)
            .expect("validated, so insert can't fail");
        if self.work(id) > self.work(self.best) {
            self.best = id;
        }
        id
    }

    fn work(&self, id: BlockId) -> U256 {
        self.store.get(id).expect("stored").total_work
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::num::NonZeroU64;

    use bytes::Bytes;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::StdRng;
    use rand::seq::{IndexedRandom, SliceRandom};
    use rand::{Rng, SeedableRng};

    use super::*;
    use crate::chain::block::{BlockHeader, Tx, TxPayload};
    use crate::chain::store::{MAX_ORPHAN_AGE, MAX_ORPHANS_PER_PARENT};
    use crate::chain::types::{AccountId, Hash, Signature};
    use crate::chain::validate::merkle_root;
    use crate::chain::{DIFFICULTY, GENESIS_HEADER, MAX_TIMESTAMP_AHEAD, TX_TTL};

    const A: u8 = 1;
    const NOW: u64 = 1_767_225_630; // genesis + 30 s, the clock every test passes

    fn key(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }

    /// A post by `author`'s key. The body is the nonce in decimal, so every post has its own ID
    /// and any nonce is UTF-8.
    fn post(author: u8, valid_until_height: u64, nonce: u64) -> Tx {
        let mut tx = Tx {
            author: AccountId(key(author).verifying_key().to_bytes()),
            valid_until_height,
            payload: TxPayload::CreatePost {
                body: Bytes::from(nonce.to_string()),
            },
            signature: Signature::from_bytes(&[0; 64]),
        };
        tx.signature = key(author).sign(&tx.signed_bytes());
        tx
    }

    /// A header on `parent` at `height` committing to `txs`, a second a block: after its parent
    /// chain's median, and within `MAX_TIMESTAMP_AHEAD` of `NOW` up to height 162 (`NOW` is
    /// genesis + 30, and `MAX_TIMESTAMP_AHEAD` is 132), where the trees here reach at most 100.
    fn header(parent: BlockId, height: u64, txs: &[Tx]) -> BlockHeader {
        BlockHeader {
            parent,
            height,
            timestamp: GENESIS_HEADER.timestamp + height,
            tx_root: merkle_root(txs),
            difficulty: DIFFICULTY,
            nonce: 0,
        }
    }

    /// `header` with the first nonce from its own whose hash meets `DIFFICULTY`, about 16 tries
    /// at the test value. See design §3.
    fn mine(mut header: BlockHeader, txs: Vec<Tx>) -> Arc<Block> {
        let target = U256::MAX / DIFFICULTY.get();
        while U256::from_big_endian(&header.id().0.0) > target {
            header.nonce += 1;
        }
        Arc::new(Block { header, txs })
    }

    /// `header` with the first nonce from its own whose hash misses `DIFFICULTY`: 15 in 16 do.
    fn unmined(mut header: BlockHeader, txs: Vec<Tx>) -> Arc<Block> {
        let target = U256::MAX / DIFFICULTY.get();
        while U256::from_big_endian(&header.id().0.0) <= target {
            header.nonce += 1;
        }
        Arc::new(Block { header, txs })
    }

    fn block(parent: BlockId, height: u64, txs: Vec<Tx>) -> Arc<Block> {
        mine(header(parent, height, &txs), txs)
    }

    /// A tree's blocks, parents first: each goes on the last one half the time, else on any
    /// earlier block, with one post of its own so every fork has its own state.
    fn random_blocks(seed: u64, n: usize) -> Vec<Arc<Block>> {
        let mut rng = StdRng::seed_from_u64(seed);
        let mut blocks = vec![genesis_block()];
        for k in 1..=n as u64 {
            let parent = if rng.random_bool(0.5) {
                blocks.last().unwrap()
            } else {
                blocks.choose(&mut rng).unwrap()
            };
            let (parent, height) = (parent.header.id(), parent.header.height + 1);
            let txs = vec![post(A, height + TX_TTL, k)];
            blocks.push(block(parent, height, txs));
        }
        blocks.split_off(1)
    }

    /// Accepts the blocks of `random_blocks` in order. `check` runs on genesis alone and after
    /// each accept, with the IDs in order. Returns how often the best tip moved to a fork and how
    /// often a new tip tied the best's work.
    fn random_tree(
        seed: u64,
        n: usize,
        mut check: impl FnMut(&ChainManager, &[BlockId]),
    ) -> (usize, usize) {
        let mut manager = ChainManager::new();
        let mut ids = vec![GENESIS_ID];
        let (mut reorgs, mut ties) = (0, 0);
        check(&manager, &ids);
        let blocks = random_blocks(seed, n);
        for (k, block) in (1..).zip(blocks) {
            let before = manager.best;
            let id = block.header.id();
            let outcome = manager.accept(Arc::clone(&block), NOW);
            assert_eq!(outcome, Ok(Accepted::Stored), "seed {seed}, block {k}");
            ties += (manager.work(id) == manager.work(before)) as usize;
            reorgs += (manager.best != before && block.header.parent != before) as usize;
            ids.push(id);
            check(&manager, &ids);
        }
        (reorgs, ties)
    }

    /// From `id` down to block 1, both included.
    fn ancestors(store: &BlockStore, mut id: BlockId) -> Vec<BlockId> {
        let mut chain = vec![];
        while id != GENESIS_ID {
            chain.push(id);
            id = store.get(id).unwrap().block.header.parent;
        }
        chain
    }

    #[test]
    fn best_is_the_first_seen_tip_with_the_most_work() {
        for seed in 0..3 {
            let (reorgs, ties) = random_tree(seed, 100, |manager, ids| {
                let n = ids.len() - 1;
                let tips = manager.store.tips();
                let most = tips.iter().map(|&t| manager.work(t)).max().unwrap();
                let expected = ids
                    .iter()
                    .find(|&&id| tips.contains(&id) && manager.work(id) == most);
                assert_eq!(Some(&manager.best), expected, "seed {seed}, {n} blocks");
            });
            assert!(reorgs > 0, "seed {seed} never reorged");
            assert!(ties > 0, "seed {seed} never tied");
        }
    }

    #[test]
    fn best_state_matches_a_replay_along_the_best_chain() {
        for seed in 0..3 {
            let (reorgs, _) = random_tree(seed, 100, |manager, ids| {
                let n = ids.len() - 1;
                let store = &manager.store;
                let replayed = ancestors(store, manager.best)
                    .into_iter()
                    .rev()
                    .fold(ChainState::genesis(), |state, id| {
                        apply_block(&state, &store.get(id).unwrap().block).unwrap()
                    });
                let stored = store.get(manager.best).unwrap().state.as_ref();
                assert_eq!(stored, Some(&replayed), "seed {seed}, {n} blocks");
            });
            assert!(reorgs > 0, "seed {seed} never reorged");
        }
    }

    #[test]
    fn any_arrival_order_gives_the_same_tree_and_best_tip() {
        for seed in [2, 4, 8] {
            let mut rng = StdRng::seed_from_u64(seed);
            let blocks = random_blocks(seed, 60);
            // At most MAX_ORPHANS_PER_PARENT children each, and 60 small blocks at one clock
            // reading, so no cap drops an orphan and the shuffled run can store every block.
            let mut children: HashMap<BlockId, usize> = HashMap::new();
            for block in &blocks {
                *children.entry(block.header.parent).or_default() += 1;
            }
            for (k, block) in (1..).zip(&blocks) {
                let n = children.get(&block.header.id()).copied().unwrap_or(0);
                assert!(
                    n <= MAX_ORPHANS_PER_PARENT,
                    "seed {seed}: block {k} has {n} children"
                );
            }

            let mut in_order = ChainManager::new();
            for (k, block) in (1..).zip(&blocks) {
                let outcome = in_order.accept(Arc::clone(block), NOW);
                assert_eq!(outcome, Ok(Accepted::Stored), "seed {seed}, block {k}");
            }
            // Every block has the same work, so a tie would make the best tip depend on arrival
            // order; these seeds have none.
            assert_eq!(
                in_order
                    .store
                    .tips()
                    .iter()
                    .filter(|&&t| in_order.work(t) == in_order.work(in_order.best))
                    .count(),
                1,
                "seed {seed}: the best tip ties another"
            );

            let mut shuffled = blocks.clone();
            shuffled.shuffle(&mut rng);
            let mut manager = ChainManager::new();
            let mut arrived = HashSet::new();
            let (mut orphaned, mut chained) = (0, 0);
            for block in &shuffled {
                let outcome = manager.accept(Arc::clone(block), NOW);
                assert!(
                    matches!(outcome, Ok(Accepted::Stored | Accepted::Orphaned)),
                    "seed {seed}: {outcome:?}"
                );
                let held = outcome == Ok(Accepted::Orphaned);
                orphaned += held as usize;
                // Held, so its parent isn't stored; arrived already, so that parent is held too.
                chained += (held && arrived.contains(&block.header.parent)) as usize;
                arrived.insert(block.header.id());
            }

            for block in &blocks {
                let id = block.header.id();
                assert!(manager.store.get(id).is_some(), "seed {seed}");
            }
            assert_eq!(manager.store.tips(), in_order.store.tips(), "seed {seed}");
            assert_eq!(manager.best, in_order.best, "seed {seed}");
            assert!(orphaned > 0, "seed {seed} never orphaned");
            assert!(chained > 0, "seed {seed} never chained orphans");
        }
    }

    #[test]
    fn blocks_already_stored_or_held_are_dropped() {
        let mut manager = ChainManager::new();
        let a = block(GENESIS_ID, 1, vec![]);
        let o = block(BlockId(Hash([9; 32])), 1, vec![]);
        assert_eq!(manager.accept(Arc::clone(&a), NOW), Ok(Accepted::Stored));
        assert_eq!(manager.accept(Arc::clone(&o), NOW), Ok(Accepted::Orphaned));
        for block in [Arc::clone(&a), o] {
            let tips = manager.store.tips().clone();

            assert_eq!(manager.accept(block, NOW), Ok(Accepted::AlreadySeen));
            assert_eq!(manager.best, a.header.id());
            assert_eq!(manager.store.tips(), &tips);
        }
    }

    #[test]
    fn bad_blocks_and_their_descendants_are_rejected_and_store_nothing() {
        let mut manager = ChainManager::new();
        let p = post(A, 1 + TX_TTL, 1);
        let a = block(GENESIS_ID, 1, vec![p.clone()]);
        assert_eq!(manager.accept(Arc::clone(&a), NOW), Ok(Accepted::Stored));
        let a = a.header.id();
        let high = block(a, 3, vec![]);
        let b = block(a, 2, vec![p.clone()]);
        let b_id = b.header.id();
        let c = block(b_id, 3, vec![]);
        let c_id = c.header.id();
        let d = block(c_id, 4, vec![]);
        let d_id = d.header.id();
        let e = block(d_id, 5, vec![]);
        let bad = [
            // Would trip `apply_block`'s height assert if it got that far.
            (
                Arc::clone(&high),
                Err(ValidationError::Store(StoreError::BadHeight {
                    height: 3,
                    parent: 1,
                })),
            ),
            // A failed check against the parent's header is remembered. See design §4.
            (high, Ok(Accepted::AlreadySeen)),
            // Any child of `a` would become best had it been stored.
            (
                Arc::clone(&b),
                Err(ValidationError::Tx(TxError::Replay(p.id()))),
            ),
            // Remembered, so it's dropped as already seen before it could be applied again.
            (b, Ok(Accepted::AlreadySeen)),
            // `c` hasn't arrived.
            (d, Ok(Accepted::Orphaned)),
            // Marked, not held, and `d` with it.
            (c, Err(ValidationError::InvalidParent(b_id))),
            // So `d` was taken out of the pool and marked.
            (e, Err(ValidationError::InvalidParent(d_id))),
        ];
        for (block, outcome) in bad {
            let id = block.header.id();
            let tips = manager.store.tips().clone();

            assert_eq!(manager.accept(block, NOW), outcome);
            assert_eq!(manager.best, a);
            assert_eq!(manager.store.tips(), &tips);
            assert!(manager.store.get(id).is_none());
        }
    }

    #[test]
    fn orphans_of_a_block_that_fails_are_invalid_too() {
        let mut manager = ChainManager::new();
        let g = GENESIS_HEADER.timestamp;
        let p = block(GENESIS_ID, 1, vec![]);
        let p_id = p.header.id();
        // Its parent's timestamp, so it fails the median once p is known; its header alone passes.
        let bad = mine(
            BlockHeader {
                timestamp: g + 1,
                ..header(p_id, 2, &[])
            },
            vec![],
        );
        let c = block(bad.header.id(), 3, vec![]);
        let c_id = c.header.id();
        let d = block(c_id, 4, vec![]);
        assert_eq!(
            manager.accept(Arc::clone(&bad), NOW),
            Ok(Accepted::Orphaned)
        );
        // Waits on an orphan.
        assert_eq!(manager.accept(c, NOW), Ok(Accepted::Orphaned));

        assert_eq!(manager.accept(p, NOW), Ok(Accepted::Stored));
        assert_eq!(manager.store.tips(), &HashSet::from([p_id]));
        assert_eq!(manager.best, p_id);
        assert_eq!(manager.accept(bad, NOW), Ok(Accepted::AlreadySeen));
        assert_eq!(
            manager.accept(d, NOW),
            Err(ValidationError::InvalidParent(c_id))
        );
    }

    #[test]
    fn orphans_past_the_age_cap_are_dropped() {
        let mut manager = ChainManager::new();
        let pa = block(GENESIS_ID, 1, vec![post(A, 1 + TX_TTL, 1)]);
        let a = block(pa.header.id(), 2, vec![post(A, 2 + TX_TTL, 3)]);
        // Its parent is never sent, so only the age cap takes it out of the pool.
        let unknown = BlockId(Hash([9; 32]));
        let b = block(unknown, 2, vec![post(A, 2 + TX_TTL, 4)]);
        let a_id = a.header.id();
        assert_eq!(manager.accept(a, NOW), Ok(Accepted::Orphaned));
        assert_eq!(manager.accept(Arc::clone(&b), NOW), Ok(Accepted::Orphaned));

        // Exactly MAX_ORPHAN_AGE old: kept, and connected under its parent.
        let now = NOW + MAX_ORPHAN_AGE;
        assert_eq!(manager.accept(pa, now), Ok(Accepted::Stored));
        assert!(manager.store.get(a_id).is_some());

        // One second past: swept before it can count as seen, so it's held afresh.
        assert_eq!(manager.accept(b, now + 1), Ok(Accepted::Orphaned));
    }

    #[test]
    fn each_check_rejects_its_block_remembering_only_what_the_header_vouches_for() {
        let mut manager = ChainManager::new();
        let a = block(GENESIS_ID, 1, vec![]);
        let a_id = a.header.id();
        assert_eq!(manager.accept(a, NOW), Ok(Accepted::Stored));
        let g = GENESIS_HEADER.timestamp;
        let good = block(a_id, 2, vec![post(A, 2 + TX_TTL, 5)]);
        // The mined header commits to the zero signature, so the root matches and the tx fails.
        // Also out of its window at height 2, so the row fails as `Tx` unless the signatures are
        // checked before the state.
        let bad_sig = {
            let mut tx = post(A, 1, 6);
            tx.signature = Signature::from_bytes(&[0; 64]);
            block(a_id, 2, vec![tx])
        };
        let one = NonZeroU64::new(1).unwrap();
        let early = block(a_id, 2, vec![]);
        let ahead = g + 2 - MAX_TIMESTAMP_AHEAD - 1;
        let table = [
            // Checked before the work, so no mining needed.
            (
                Arc::new(Block {
                    header: BlockHeader {
                        difficulty: one,
                        ..header(a_id, 2, &[])
                    },
                    txs: vec![],
                }),
                NOW,
                Err(ValidationError::Block(BlockError::WrongDifficulty(one))),
                Err(ValidationError::Block(BlockError::WrongDifficulty(one))),
            ),
            // Also over a mismatched root: the header is checked before the root.
            (
                Arc::new(Block {
                    header: BlockHeader {
                        difficulty: one,
                        tx_root: Hash([7; 32]),
                        ..header(a_id, 2, &[])
                    },
                    txs: vec![],
                }),
                NOW,
                Err(ValidationError::Block(BlockError::WrongDifficulty(one))),
                Err(ValidationError::Block(BlockError::WrongDifficulty(one))),
            ),
            (
                unmined(header(a_id, 2, &[]), vec![]),
                NOW,
                Err(ValidationError::Block(BlockError::InsufficientWork)),
                Err(ValidationError::Block(BlockError::InsufficientWork)),
            ),
            (
                Arc::clone(&early),
                ahead,
                Err(ValidationError::Block(BlockError::TimestampAhead {
                    timestamp: g + 2,
                    now: ahead,
                })),
                Err(ValidationError::Block(BlockError::TimestampAhead {
                    timestamp: g + 2,
                    now: ahead,
                })),
            ),
            // The median of a's and genesis's timestamps is a's.
            (
                mine(
                    BlockHeader {
                        timestamp: g + 1,
                        ..header(a_id, 2, &[])
                    },
                    vec![],
                ),
                NOW,
                Err(ValidationError::Block(
                    BlockError::TimestampNotAfterMedian {
                        timestamp: g + 1,
                        median: g + 1,
                    },
                )),
                Ok(Accepted::AlreadySeen),
            ),
            // good's header over bad_sig's body: the root is checked before the signature, and a
            // mismatch isn't remembered, even over a tx that would be.
            (
                Arc::new(Block {
                    txs: bad_sig.txs.clone(),
                    ..(*good).clone()
                }),
                NOW,
                Err(ValidationError::Block(BlockError::TxRootMismatch {
                    header: good.header.tx_root,
                    txs: bad_sig.header.tx_root,
                })),
                Err(ValidationError::Block(BlockError::TxRootMismatch {
                    header: good.header.tx_root,
                    txs: bad_sig.header.tx_root,
                })),
            ),
            // A tx ID doesn't cover the signature, so the signed post's ID is this one's.
            (
                bad_sig,
                NOW,
                Err(ValidationError::Block(BlockError::BadSignature(
                    post(A, 1, 6).id(),
                ))),
                Ok(Accepted::AlreadySeen),
            ),
        ];
        for (block, now, first, second) in table {
            let id = block.header.id();
            let tips = manager.store.tips().clone();

            assert_eq!(manager.accept(Arc::clone(&block), now), first);
            assert_eq!(manager.accept(block, now), second);
            assert_eq!(manager.best, a_id);
            assert_eq!(manager.store.tips(), &tips);
            assert!(manager.store.get(id).is_none());
        }
        // The same block that was ahead of the clock, now within it: the clock was its only
        // fault, and it wasn't remembered.
        assert_eq!(manager.accept(early, NOW), Ok(Accepted::Stored));
    }

    #[test]
    fn a_block_with_insufficient_work_is_never_held_as_an_orphan() {
        let mut manager = ChainManager::new();
        let p = block(GENESIS_ID, 1, vec![]);
        let p_id = p.header.id();
        let o = unmined(header(p_id, 2, &[]), vec![]);
        let o_id = o.header.id();
        let first = manager.accept(Arc::clone(&o), NOW);
        assert_eq!(
            first,
            Err(ValidationError::Block(BlockError::InsufficientWork))
        );
        // Neither held nor remembered, so the resend is checked afresh.
        assert_eq!(manager.accept(o, NOW), first);

        assert_eq!(manager.accept(p, NOW), Ok(Accepted::Stored));
        assert!(manager.store.get(o_id).is_none());
        assert_eq!(manager.store.tips(), &HashSet::from([p_id]));
        assert_eq!(manager.best, p_id);
    }

    #[test]
    fn an_unmined_child_of_an_invalid_parent_is_not_remembered() {
        let mut manager = ChainManager::new();
        // Mined, at the wrong height, so it's remembered.
        let bad = block(GENESIS_ID, 2, vec![]);
        let bad_id = bad.header.id();
        let c = unmined(header(bad_id, 3, &[]), vec![]);
        let c_id = c.header.id();
        assert_eq!(
            manager.accept(bad, NOW),
            Err(ValidationError::Store(StoreError::BadHeight {
                height: 2,
                parent: 0,
            }))
        );
        assert!(manager.store.is_invalid(bad_id));

        let first = manager.accept(Arc::clone(&c), NOW);
        assert_eq!(
            first,
            Err(ValidationError::Block(BlockError::InsufficientWork))
        );
        assert_eq!(manager.accept(c, NOW), first);
        assert!(!manager.store.is_invalid(c_id));
        // The set is as it was: a real failure is still in it.
        assert!(manager.store.is_invalid(bad_id));
    }

    #[test]
    fn a_timestamp_must_be_after_the_median_of_the_last_median_blocks() {
        let mut manager = ChainManager::new();
        let g = GENESIS_HEADER.timestamp;
        // Heights 1 to 11 at g + 1 to g + 11: the window is these, genesis left out, and the
        // median is g + 6, five seconds below the parent.
        let tip = (1..=MEDIAN_BLOCKS as u64).fold(GENESIS_ID, |parent, height| {
            let b = block(parent, height, vec![]);
            assert_eq!(manager.accept(Arc::clone(&b), NOW), Ok(Accepted::Stored));
            b.header.id()
        });
        let at = |timestamp| {
            mine(
                BlockHeader {
                    timestamp,
                    ..header(tip, 12, &[])
                },
                vec![],
            )
        };
        assert_eq!(
            manager.accept(at(g + 6), NOW),
            Err(ValidationError::Block(
                BlockError::TimestampNotAfterMedian {
                    timestamp: g + 6,
                    median: g + 6,
                }
            ))
        );
        assert_eq!(manager.accept(at(g + 7), NOW), Ok(Accepted::Stored));
    }

    #[test]
    fn a_forged_body_arriving_first_does_not_shadow_the_honest_block() {
        let mut manager = ChainManager::new();
        let p = block(GENESIS_ID, 1, vec![]);
        let p_id = p.header.id();
        let x = block(p_id, 2, vec![post(A, 2 + TX_TTL, 1)]);
        let x_id = x.header.id();
        // x's header over a body it didn't commit to, so it has x's ID.
        let forged = Arc::new(Block {
            txs: vec![],
            ..(*x).clone()
        });
        let y = block(x_id, 3, vec![]);
        let y_id = y.header.id();

        assert_eq!(
            manager.accept(forged, NOW),
            Err(ValidationError::Block(BlockError::TxRootMismatch {
                header: x.header.tx_root,
                txs: merkle_root(&[]),
            }))
        );
        // Not dropped as seen: the forged copy was never held.
        assert_eq!(manager.accept(Arc::clone(&x), NOW), Ok(Accepted::Orphaned));
        assert_eq!(manager.accept(y, NOW), Ok(Accepted::Orphaned));
        assert_eq!(manager.accept(p, NOW), Ok(Accepted::Stored));
        assert!(manager.store.get(x_id).is_some());
        assert_eq!(manager.store.tips(), &HashSet::from([y_id]));
        assert_eq!(manager.best, y_id);
    }
}
