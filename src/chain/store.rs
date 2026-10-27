//! Block tree: nodes, tips, total work, fork point (design §3, §5).

// std collections (not imbl): imbl's get_mut needs Clone, and BlockNode isn't.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use primitive_types::U256;
use smallvec::SmallVec;

use crate::chain::block::Block;
use crate::chain::types::BlockId;

pub struct BlockNode {
    pub id: BlockId,
    pub block: Arc<Block>,
    /// Parent's `total_work` + this block's work.
    pub total_work: U256,
    pub children: SmallVec<[BlockId; 2]>,
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum StoreError {
    #[error("block {0:?} is already stored")]
    Duplicate(BlockId),
    #[error("parent {0:?} is not stored")]
    UnknownParent(BlockId),
    #[error("height {height} is not the parent's {parent} + 1")]
    BadHeight { height: u64, parent: u64 },
}

pub struct BlockStore {
    nodes: HashMap<BlockId, BlockNode>,
    tips: HashSet<BlockId>,
}

impl BlockStore {
    /// Roots the tree at `genesis`, whose total work is its own work.
    pub fn new(genesis: Arc<Block>) -> Self {
        let header = &genesis.header;
        let id = header.id();
        let total_work = header.work();
        let node = BlockNode {
            id,
            block: genesis,
            total_work,
            children: SmallVec::new(),
        };
        Self {
            nodes: HashMap::from([(id, node)]),
            tips: HashSet::from([id]),
        }
    }

    /// Adds a block under its stored parent, which stops being a tip. A failed insert changes
    /// nothing.
    pub fn insert(&mut self, block: Arc<Block>) -> Result<BlockId, StoreError> {
        let header = &block.header;
        let id = header.id();
        if self.nodes.contains_key(&id) {
            return Err(StoreError::Duplicate(id));
        }
        let parent = self
            .nodes
            .get_mut(&header.parent)
            .ok_or(StoreError::UnknownParent(header.parent))?;
        let parent_height = parent.block.header.height;
        if header.height.checked_sub(1) != Some(parent_height) {
            return Err(StoreError::BadHeight {
                height: header.height,
                parent: parent_height,
            });
        }
        let total_work = parent.total_work + header.work();
        parent.children.push(id);
        self.tips.remove(&header.parent);
        self.tips.insert(id);
        self.nodes.insert(
            id,
            BlockNode {
                id,
                block,
                total_work,
                children: SmallVec::new(),
            },
        );
        Ok(id)
    }

    pub fn get(&self, id: BlockId) -> Option<&BlockNode> {
        self.nodes.get(&id)
    }

    /// Blocks with no children.
    pub fn tips(&self) -> &HashSet<BlockId> {
        &self.tips
    }

    /// The last block `a` and `b` have in common. None if either is unknown.
    pub fn find_fork_point(&self, mut a: BlockId, mut b: BlockId) -> Option<BlockId> {
        let header = |id: BlockId| self.nodes.get(&id).map(|n| &n.block.header);
        while header(a)?.height > header(b)?.height {
            a = header(a)?.parent;
        }
        while header(b)?.height > header(a)?.height {
            b = header(b)?.parent;
        }
        while a != b {
            a = header(a)?.parent;
            b = header(b)?.parent;
        }
        Some(a)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use super::*;
    use crate::chain::block::BlockHeader;
    use crate::chain::types::Hash;
    use rand::rngs::StdRng;
    use rand::seq::IndexedRandom;
    use rand::{Rng, SeedableRng};

    const EASY: NonZeroU64 = NonZeroU64::new(1).unwrap();
    const HARD: NonZeroU64 = NonZeroU64::new(256).unwrap();

    fn block(parent: BlockId, height: u64, difficulty: NonZeroU64, nonce: u64) -> Arc<Block> {
        Arc::new(Block {
            header: BlockHeader {
                parent,
                height,
                timestamp: 0,
                tx_root: Hash([0; 32]),
                difficulty,
                nonce,
            },
            txs: vec![],
        })
    }

    fn genesis() -> Arc<Block> {
        block(BlockId(Hash([0; 32])), 0, EASY, 0)
    }

    /// Each block goes on the last one half the time, else on any stored block. `check` runs on
    /// genesis alone and after each insert.
    fn random_tree(
        seed: u64,
        n: usize,
        mut check: impl FnMut(&BlockStore, &HashMap<BlockId, BlockId>),
    ) -> (BlockStore, HashMap<BlockId, BlockId>) {
        let mut rng = StdRng::seed_from_u64(seed);
        let mut store = BlockStore::new(genesis());
        let mut ids = vec![genesis().header.id()];
        let mut parents = HashMap::new();
        check(&store, &parents);
        for nonce in 1..=n as u64 {
            let parent = if rng.random_bool(0.5) {
                *ids.last().unwrap()
            } else {
                *ids.choose(&mut rng).unwrap()
            };
            let difficulty = if rng.random_bool(0.5) { EASY } else { HARD };
            let height = store.get(parent).unwrap().block.header.height + 1;
            let id = store
                .insert(block(parent, height, difficulty, nonce))
                .unwrap();
            parents.insert(id, parent);
            ids.push(id);
            check(&store, &parents);
        }
        (store, parents)
    }

    /// From `id` itself down to genesis, both included.
    fn ancestors(parents: &HashMap<BlockId, BlockId>, mut id: BlockId) -> Vec<BlockId> {
        let mut chain = vec![id];
        while let Some(&parent) = parents.get(&id) {
            chain.push(parent);
            id = parent;
        }
        chain
    }

    fn all_ids(parents: &HashMap<BlockId, BlockId>) -> Vec<BlockId> {
        let mut ids: Vec<_> = parents.keys().copied().collect();
        ids.push(genesis().header.id());
        ids
    }

    #[test]
    fn tips_are_exactly_the_childless_blocks() {
        for seed in 0..3 {
            let (store, _) = random_tree(seed, 100, |store, parents| {
                let n = parents.len();
                let mut childless: HashSet<_> = all_ids(parents).into_iter().collect();
                for parent in parents.values() {
                    childless.remove(parent);
                }
                assert_eq!(store.tips(), &childless, "seed {seed}, {n} blocks");

                let mut children: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();
                for (&child, &parent) in parents {
                    children.entry(parent).or_default().insert(child);
                }
                for id in all_ids(parents) {
                    let node = store.get(id).unwrap();
                    assert_eq!(node.id, id);
                    let stored: HashSet<_> = node.children.iter().copied().collect();
                    assert_eq!(stored.len(), node.children.len());
                    assert_eq!(stored, children.remove(&id).unwrap_or_default());
                }
            });
            assert!(store.tips().len() > 1, "seed {seed} never forked");
        }
    }

    #[test]
    fn fork_point_is_the_lowest_common_ancestor() {
        for seed in 0..3 {
            let (store, parents) = random_tree(seed, 100, |_, _| {});
            let ids = all_ids(&parents);
            let lists: Vec<_> = ids.iter().map(|&id| ancestors(&parents, id)).collect();
            let sets: Vec<HashSet<_>> = lists.iter().map(|l| l.iter().copied().collect()).collect();
            for (&a, a_ancestors) in ids.iter().zip(&lists) {
                for (&b, b_ancestors) in ids.iter().zip(&sets) {
                    let lca = a_ancestors.iter().find(|id| b_ancestors.contains(id));
                    assert_eq!(store.find_fork_point(a, b).as_ref(), lca);
                }
            }
        }
    }

    #[test]
    fn total_work_is_the_sum_along_the_chain() {
        for seed in 0..3 {
            let (store, parents) = random_tree(seed, 100, |_, _| {});
            for id in all_ids(&parents) {
                let sum = ancestors(&parents, id)
                    .into_iter()
                    .map(|a| U256::from(store.get(a).unwrap().block.header.difficulty.get()))
                    .fold(U256::zero(), |a, w| a + w);
                assert_eq!(store.get(id).unwrap().total_work, sum);
            }
        }
    }

    #[test]
    fn bad_inserts_are_errors_and_change_nothing() {
        let g = genesis().header.id();
        let mut store = BlockStore::new(genesis());
        let a = store.insert(block(g, 1, EASY, 1)).unwrap();
        let unknown = BlockId(Hash([9; 32]));
        let bad = [
            (block(g, 1, EASY, 1), StoreError::Duplicate(a)),
            (
                block(unknown, 1, EASY, 2),
                StoreError::UnknownParent(unknown),
            ),
            (
                block(a, 3, EASY, 3),
                StoreError::BadHeight {
                    height: 3,
                    parent: 1,
                },
            ),
            (
                block(g, 0, EASY, 4),
                StoreError::BadHeight {
                    height: 0,
                    parent: 0,
                },
            ),
            (genesis(), StoreError::Duplicate(g)),
        ];
        for (block, err) in bad {
            let id = block.header.id();
            let parent = block.header.parent;
            let tips = store.tips().clone();
            let children = store.get(parent).map(|n| n.children.clone());
            let stored = store.get(id).is_some();

            assert_eq!(store.insert(block), Err(err));
            assert_eq!(store.tips(), &tips);
            assert_eq!(store.get(parent).map(|n| n.children.clone()), children);
            assert_eq!(store.get(id).is_some(), stored);
        }

        assert_eq!(store.find_fork_point(unknown, a), None);
        assert_eq!(store.find_fork_point(a, unknown), None);
    }
}
