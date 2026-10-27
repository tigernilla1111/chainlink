//! Fork choice and reorg (design §5): a block's state is built on its parent's when it's
//! accepted, and the best tip is the one with the most total work, first seen on a tie. A
//! reorg is a change of `best`; nothing is undone. Orphans and the invalid set come with the
//! orphans and invalid block IDs item; the mempool and pruning with later items.

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

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum ValidationError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Tx(#[from] TxError),
}

impl ChainManager {
    /// Genesis alone, as the best tip.
    pub fn new() -> Self {
        Self {
            store: BlockStore::new(genesis_block(), ChainState::genesis()),
            best: GENESIS_ID,
        }
    }

    /// Design §4 steps 1, 3, 5's height check, 7, and 8 without pruning (the rest of 5, and 2
    /// and 6, are `validate.rs`): the block's state is its parent's with the block applied. A
    /// failed accept stores nothing; orphans and the invalid set come with the orphans and
    /// invalid block IDs item. A known block is a `Duplicate` error here, where step 1 drops it.
    pub fn accept(&mut self, block: Arc<Block>) -> Result<BlockId, ValidationError> {
        let parent = self.store.parent_of(&block)?;
        let parent_state = parent
            .state
            .as_ref()
            .expect("every stored block keeps its state until pruning lands");
        let state = apply_block(parent_state, &block)?;
        Ok(self.on_new_valid_block(block, state))
    }

    /// Design §5: stores the block and switches only on strictly more work, so the tip seen
    /// first wins a tie.
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
    use std::num::NonZeroU64;

    use bytes::Bytes;
    use rand::rngs::StdRng;
    use rand::seq::IndexedRandom;
    use rand::{Rng, SeedableRng};

    use super::*;
    use crate::chain::TX_TTL;
    use crate::chain::block::{BlockHeader, Tx, TxPayload};
    use crate::chain::types::{AccountId, Hash, Signature};

    const EASY: NonZeroU64 = NonZeroU64::new(1).unwrap();
    // Close to EASY, since ties are wanted here.
    const HARD: NonZeroU64 = NonZeroU64::new(2).unwrap();
    const A: u8 = 1;

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

    /// Each block goes on the last one half the time, else on any stored block, with work 1 or 2
    /// and one post of its own, so every fork has its own state. `check` runs on genesis alone and
    /// after each accept, with the IDs in order. Returns how often the best tip moved to a fork,
    /// how often a new tip tied the best's work, and how often a tip was taller than the best.
    fn random_tree(
        seed: u64,
        n: usize,
        mut check: impl FnMut(&ChainManager, &[BlockId]),
    ) -> (usize, usize, usize) {
        let mut rng = StdRng::seed_from_u64(seed);
        let mut manager = ChainManager::new();
        let mut ids = vec![GENESIS_ID];
        let (mut reorgs, mut ties, mut taller) = (0, 0, 0);
        check(&manager, &ids);
        for nonce in 1..=n as u64 {
            let parent = if rng.random_bool(0.5) {
                *ids.last().unwrap()
            } else {
                *ids.choose(&mut rng).unwrap()
            };
            let difficulty = if rng.random_bool(0.5) { EASY } else { HARD };
            let height = manager.store.get(parent).unwrap().block.header.height + 1;
            let txs = vec![post(A, height + TX_TTL, nonce)];
            let block = block(parent, height, difficulty, nonce, txs);

            let before = manager.best;
            let id = manager.accept(Arc::clone(&block)).unwrap();
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
    fn bad_blocks_are_errors_and_change_nothing() {
        let mut manager = ChainManager::new();
        let p = post(A, 1 + TX_TTL, 1);
        let a = manager
            .accept(block(GENESIS_ID, 1, EASY, 1, vec![p.clone()]))
            .unwrap();
        let unknown = BlockId(Hash([9; 32]));
        let bad = [
            (
                block(unknown, 1, EASY, 2, vec![]),
                ValidationError::Store(StoreError::UnknownParent(unknown)),
            ),
            (
                block(GENESIS_ID, 1, EASY, 1, vec![p.clone()]),
                ValidationError::Store(StoreError::Duplicate(a)),
            ),
            // Would trip `apply_block`'s height assert if it got that far.
            (
                block(a, 3, EASY, 3, vec![]),
                ValidationError::Store(StoreError::BadHeight {
                    height: 3,
                    parent: 1,
                }),
            ),
            // Any child of `a` would become best had it been stored.
            (
                block(a, 2, EASY, 4, vec![p.clone()]),
                ValidationError::Tx(TxError::Replay(p.id())),
            ),
        ];
        for (block, err) in bad {
            let id = block.header.id();
            let tips = manager.store.tips().clone();
            let stored = manager.store.get(id).is_some();

            assert_eq!(manager.accept(block), Err(err));
            assert_eq!(manager.best, a);
            assert_eq!(manager.store.tips(), &tips);
            assert_eq!(manager.store.get(id).is_some(), stored);
        }
    }
}
