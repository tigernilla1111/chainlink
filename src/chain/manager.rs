//! Accepting a block: one already stored, held or known invalid is dropped; one whose parent is
//! unknown waits in the store's orphan pool; one that fails is remembered as invalid with every
//! orphan waiting on it; the rest get their state built on their parent's, and the best tip is
//! the one with the most total work, first seen on a tie. A reorg is a change of `best`; nothing
//! is undone. The checks that need no state come with the item running `validate.rs`'s checks in
//! `accept`, pruning and the mempool with later items. See design §4 and §5.

use std::sync::Arc;

use primitive_types::U256;

use crate::chain::block::Block;
use crate::chain::state::{ChainState, TxError, apply_block};
use crate::chain::store::{BlockStore, StoreError};
use crate::chain::types::BlockId;
use crate::chain::{GENESIS_ID, genesis_block};

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

    /// Drops a block already seen, holds one whose parent is unknown, and otherwise checks its
    /// height, applies it and stores it, then does the same for the orphans it unblocks; `now` is
    /// the local clock in Unix seconds, read only by the orphan age cap. A block that fails, or
    /// whose parent is known invalid, is remembered as invalid without being applied, and so is
    /// every orphan waiting on it. Panics on a block that doesn't encode within
    /// `MAX_BLOCK_BYTES`, which only a local builder can make. See design §4.
    pub fn accept(&mut self, block: Arc<Block>, now: u64) -> Result<Accepted, ValidationError> {
        self.store.drop_expired_orphans(now);
        let id = block.header.id();
        if self.store.has_seen(id, &block) {
            return Ok(Accepted::AlreadySeen);
        }
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

    /// Checks the height of a block whose parent is stored, builds its state on the parent's and
    /// stores it. A failed apply stores nothing. See design §4.
    fn apply_and_store(&mut self, block: Arc<Block>) -> Result<(), ValidationError> {
        let parent = self.store.parent_of(&block)?;
        let parent_state = parent
            .state
            .as_ref()
            .expect("every stored block keeps its state until pruning lands");
        let state = apply_block(parent_state, &block)?;
        self.on_new_valid_block(block, state);
        Ok(())
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
    use rand::rngs::StdRng;
    use rand::seq::{IndexedRandom, SliceRandom};
    use rand::{Rng, SeedableRng};

    use super::*;
    use crate::chain::TX_TTL;
    use crate::chain::block::{BlockHeader, Tx, TxPayload};
    use crate::chain::store::{MAX_ORPHAN_AGE, MAX_ORPHANS_PER_PARENT};
    use crate::chain::types::{AccountId, Hash, Signature};

    const EASY: NonZeroU64 = NonZeroU64::new(1).unwrap();
    // Close to EASY, since ties are wanted here.
    const HARD: NonZeroU64 = NonZeroU64::new(2).unwrap();
    const A: u8 = 1;
    const NOW: u64 = 1_767_225_630; // genesis + 30 s, the clock every test passes

    /// The body is the nonce, so every post has its own ID.
    fn post(author: u8, valid_until_height: u64, nonce: u64) -> Tx {
        Tx {
            author: AccountId([author; 32]),
            valid_until_height,
            payload: TxPayload::CreatePost {
                body: Bytes::copy_from_slice(&nonce.to_le_bytes()),
            },
            signature: Signature::from_bytes(&[0; 64]),
        }
    }

    fn block(
        parent: BlockId,
        height: u64,
        difficulty: NonZeroU64,
        nonce: u64,
        txs: Vec<Tx>,
    ) -> Arc<Block> {
        Arc::new(Block {
            header: BlockHeader {
                parent,
                height,
                timestamp: 0,
                tx_root: Hash([0; 32]),
                difficulty,
                nonce,
            },
            txs,
        })
    }

    /// A tree's blocks, parents first: each goes on the last one half the time, else on any
    /// earlier block, at `difficulty(rng, k)` for the k-th, with one post of its own so every fork
    /// has its own state.
    fn random_blocks(
        seed: u64,
        n: usize,
        mut difficulty: impl FnMut(&mut StdRng, u64) -> NonZeroU64,
    ) -> Vec<Arc<Block>> {
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
            let difficulty = difficulty(&mut rng, k);
            blocks.push(block(parent, height, difficulty, k, txs));
        }
        blocks.split_off(1)
    }

    /// Accepts the blocks of `random_blocks`, with work 1 or 2, in order. `check` runs on genesis
    /// alone and after each accept, with the IDs in order. Returns how often the best tip moved to
    /// a fork, how often a new tip tied the best's work, and how often a tip was taller than the
    /// best.
    fn random_tree(
        seed: u64,
        n: usize,
        mut check: impl FnMut(&ChainManager, &[BlockId]),
    ) -> (usize, usize, usize) {
        let mut manager = ChainManager::new();
        let mut ids = vec![GENESIS_ID];
        let (mut reorgs, mut ties, mut taller) = (0, 0, 0);
        check(&manager, &ids);
        let blocks = random_blocks(
            seed,
            n,
            |rng, _| {
                if rng.random_bool(0.5) { EASY } else { HARD }
            },
        );
        for (k, block) in (1..).zip(blocks) {
            let before = manager.best;
            let id = block.header.id();
            let outcome = manager.accept(Arc::clone(&block), NOW);
            assert_eq!(outcome, Ok(Accepted::Stored), "seed {seed}, block {k}");
            ties += (manager.work(id) == manager.work(before)) as usize;
            reorgs += (manager.best != before && block.header.parent != before) as usize;
            let height_of = |id| manager.store.get(id).unwrap().block.header.height;
            let best_height = height_of(manager.best);
            taller += manager
                .store
                .tips()
                .iter()
                .any(|&t| height_of(t) > best_height) as usize;
            ids.push(id);
            check(&manager, &ids);
        }
        (reorgs, ties, taller)
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
            let (reorgs, ties, taller) = random_tree(seed, 100, |manager, ids| {
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
            assert!(
                taller > 0,
                "seed {seed} never had a tip taller than the best"
            );
        }
    }

    #[test]
    fn best_state_matches_a_replay_along_the_best_chain() {
        for seed in 0..3 {
            let (reorgs, _, _) = random_tree(seed, 100, |manager, ids| {
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
        for seed in 0..3 {
            let mut rng = StdRng::seed_from_u64(seed);
            let mut exps: Vec<u64> = (1..=60).collect();
            exps.shuffle(&mut rng);
            // Distinct powers of two: no two chains share a total, so the best tip doesn't depend
            // on order.
            let blocks = random_blocks(seed, 60, |_, k| {
                NonZeroU64::new(1 << exps[k as usize - 1]).unwrap()
            });
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
        let a = block(GENESIS_ID, 1, EASY, 1, vec![]);
        let o = block(BlockId(Hash([9; 32])), 1, EASY, 2, vec![]);
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
        let a = block(GENESIS_ID, 1, EASY, 1, vec![p.clone()]);
        assert_eq!(manager.accept(Arc::clone(&a), NOW), Ok(Accepted::Stored));
        let a = a.header.id();
        let b = block(a, 2, EASY, 2, vec![p.clone()]);
        let b_id = b.header.id();
        let c = block(b_id, 3, EASY, 3, vec![]);
        let c_id = c.header.id();
        let d = block(c_id, 4, EASY, 4, vec![]);
        let d_id = d.header.id();
        let e = block(d_id, 5, EASY, 5, vec![]);
        let bad = [
            // Would trip `apply_block`'s height assert if it got that far.
            (
                block(a, 3, EASY, 5, vec![]),
                Err(ValidationError::Store(StoreError::BadHeight {
                    height: 3,
                    parent: 1,
                })),
            ),
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
        let p = post(A, 1 + TX_TTL, 1);
        let g = block(GENESIS_ID, 1, EASY, 1, vec![p.clone()]);
        let g_id = g.header.id();
        // Replays `p`, so it fails once `g` arrives.
        let bad = block(g_id, 2, EASY, 2, vec![p.clone()]);
        let c = block(bad.header.id(), 3, EASY, 3, vec![]);
        let c_id = c.header.id();
        let d = block(c_id, 4, EASY, 4, vec![]);
        assert_eq!(
            manager.accept(Arc::clone(&bad), NOW),
            Ok(Accepted::Orphaned)
        );
        // Waits on an orphan.
        assert_eq!(manager.accept(c, NOW), Ok(Accepted::Orphaned));

        assert_eq!(manager.accept(g, NOW), Ok(Accepted::Stored));
        assert_eq!(manager.store.tips(), &HashSet::from([g_id]));
        assert_eq!(manager.best, g_id);
        assert_eq!(manager.accept(bad, NOW), Ok(Accepted::AlreadySeen));
        assert_eq!(
            manager.accept(d, NOW),
            Err(ValidationError::InvalidParent(c_id))
        );
    }

    #[test]
    fn orphans_past_the_age_cap_are_dropped() {
        let mut manager = ChainManager::new();
        let pa = block(GENESIS_ID, 1, EASY, 1, vec![post(A, 1 + TX_TTL, 1)]);
        let a = block(pa.header.id(), 2, EASY, 3, vec![post(A, 2 + TX_TTL, 3)]);
        // Its parent is never sent, so only the age cap takes it out of the pool.
        let unknown = BlockId(Hash([9; 32]));
        let b = block(unknown, 2, EASY, 4, vec![post(A, 2 + TX_TTL, 4)]);
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
}
