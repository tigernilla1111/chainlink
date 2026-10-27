//! The tree of stored blocks, each with its total work and state snapshot, plus the tips, the
//! fork point of any two blocks, the orphans waiting on a parent and the IDs of blocks that
//! failed. See design §3, §4 and §5.

// std collections (not imbl): imbl's get_mut needs Clone, and BlockNode isn't.
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use primitive_types::U256;
use smallvec::SmallVec;

use crate::chain::block::Block;
use crate::chain::codec::serialize;
use crate::chain::state::ChainState;
use crate::chain::types::{BlockId, Hash};

/// The orphan pool's cap on total encoded bytes; a node limit, which peers needn't agree on. See
/// design §4.
pub const MAX_ORPHAN_BYTES: usize = 16 * 1024 * 1024;
/// How many orphans may wait on one missing parent; a node limit. See design §4.
pub const MAX_ORPHANS_PER_PARENT: usize = 4;
/// How long an orphan waits for its parent, in seconds; a node limit. See design §4.
pub const MAX_ORPHAN_AGE: u64 = 60;
/// How many failed blocks are remembered; a node limit. See design §4.
pub const MAX_INVALID_IDS: usize = 4096;

pub struct BlockNode {
    pub id: BlockId,
    pub block: Arc<Block>,
    /// Parent's `total_work` + this block's work.
    pub total_work: U256,
    pub children: SmallVec<[BlockId; 2]>,
    /// The state after this block, or None below the finality horizon. Pruning isn't built yet,
    /// so every stored block has one. See design §6.
    pub state: Option<ChainState>,
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
    /// Blocks that failed, so they and their children are dropped without a second look.
    invalid: HashSet<BlockId>,
    /// The same IDs oldest first, for the cap.
    invalid_order: VecDeque<BlockId>,
    /// Blocks waiting on a parent, keyed by it, each with the clock reading when it was held and
    /// its encoded size.
    orphans: HashMap<BlockId, Vec<(Arc<Block>, u64, usize)>>,
    orphan_bytes: usize,
}

impl BlockStore {
    /// Roots the tree at `genesis`, whose total work is its own work, with its state.
    pub fn new(genesis: Arc<Block>, state: ChainState) -> Self {
        let header = &genesis.header;
        let id = header.id();
        let total_work = header.work();
        let node = BlockNode {
            id,
            block: genesis,
            total_work,
            children: SmallVec::new(),
            state: Some(state),
        };
        Self {
            nodes: HashMap::from([(id, node)]),
            tips: HashSet::from([id]),
            invalid: HashSet::new(),
            invalid_order: VecDeque::new(),
            orphans: HashMap::new(),
            orphan_bytes: 0,
        }
    }

    /// The stored parent `block` would go under: `insert`'s checks, with nothing changed. The
    /// manager builds the block's state on the parent's between the two. See design §4.
    pub fn parent_of(&self, block: &Block) -> Result<&BlockNode, StoreError> {
        let header = &block.header;
        let id = header.id();
        if self.nodes.contains_key(&id) {
            return Err(StoreError::Duplicate(id));
        }
        let parent = self
            .nodes
            .get(&header.parent)
            .ok_or(StoreError::UnknownParent(header.parent))?;
        let parent_height = parent.block.header.height;
        if header.height.checked_sub(1) != Some(parent_height) {
            return Err(StoreError::BadHeight {
                height: header.height,
                parent: parent_height,
            });
        }
        Ok(parent)
    }

    /// Adds a block and its state under its stored parent, which stops being a tip. A failed
    /// insert changes nothing.
    pub fn insert(&mut self, block: Arc<Block>, state: ChainState) -> Result<BlockId, StoreError> {
        let parent = self.parent_of(&block)?;
        let total_work = parent.total_work + block.header.work();
        let id = block.header.id();
        self.nodes
            .get_mut(&block.header.parent)
            .expect("parent_of found it")
            .children
            .push(id);
        self.tips.remove(&block.header.parent);
        self.tips.insert(id);
        self.nodes.insert(
            id,
            BlockNode {
                id,
                block,
                total_work,
                children: SmallVec::new(),
                state: Some(state),
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

    /// Whether the block `id` names is stored, held as an orphan or known invalid. The caller has
    /// hashed the header already. See design §4, step 1.
    pub fn has_seen(&self, id: BlockId, block: &Block) -> bool {
        self.nodes.contains_key(&id)
            || self.invalid.contains(&id)
            || self
                .orphans
                .get(&block.header.parent)
                .is_some_and(|held| held.iter().any(|(b, _, _)| b.header == block.header))
    }

    pub fn is_invalid(&self, id: BlockId) -> bool {
        self.invalid.contains(&id)
    }

    /// Remembers a block that failed; past `MAX_INVALID_IDS` the oldest is forgotten.
    /// See design §4.
    pub fn mark_invalid(&mut self, id: BlockId) {
        if !self.invalid.insert(id) {
            return;
        }
        if self.invalid_order.len() == MAX_INVALID_IDS {
            let oldest = self.invalid_order.pop_front().expect("full, so not empty");
            self.invalid.remove(&oldest);
        }
        self.invalid_order.push_back(id);
    }

    /// Holds `block` for its missing parent, at the clock reading `now`. Past
    /// `MAX_ORPHANS_PER_PARENT` for that parent, or `MAX_ORPHAN_BYTES` for the pool, the oldest
    /// goes first, so the new block is always held. Panics on a block that doesn't encode within
    /// `MAX_BLOCK_BYTES`, which only a local builder can make, since decoding caps at it. See
    /// design §4.
    pub fn hold_orphan(&mut self, block: Arc<Block>, now: u64) {
        let bytes = serialize(&*block)
            .expect("decoded within MAX_BLOCK_BYTES, or built locally within it")
            .len();
        let parent = block.header.parent;
        if let Some(held) = self.orphans.get_mut(&parent)
            && held.len() == MAX_ORPHANS_PER_PARENT
        {
            let oldest = (0..held.len())
                .min_by_key(|&i| held[i].1)
                .expect("full, so not empty");
            let (_, _, dropped) = held.remove(oldest);
            self.orphan_bytes -= dropped;
        }
        self.make_room(bytes);
        self.orphans
            .entry(parent)
            .or_default()
            .push((block, now, bytes));
        self.orphan_bytes += bytes;
    }

    /// The orphans waiting on `parent`, released from the pool. Empty if none.
    pub fn take_orphans(&mut self, parent: BlockId) -> Vec<Arc<Block>> {
        let held = self.orphans.remove(&parent).unwrap_or_default();
        held.into_iter()
            .map(|(block, _, bytes)| {
                self.orphan_bytes -= bytes;
                block
            })
            .collect()
    }

    /// Drops the orphans held more than `MAX_ORPHAN_AGE` seconds before `now`. The manager runs it
    /// as each block arrives, the only time `chain` has a clock reading. See design §4.
    pub fn drop_expired_orphans(&mut self, now: u64) {
        let total = &mut self.orphan_bytes;
        self.orphans.retain(|_, held| {
            held.retain(|&(_, at, bytes)| {
                let keep = at + MAX_ORPHAN_AGE >= now;
                if !keep {
                    *total -= bytes;
                }
                keep
            });
            !held.is_empty()
        });
    }

    /// Drops the orphans held longest until `bytes` more fit under `MAX_ORPHAN_BYTES`, sorting the
    /// pool once; among equal clock readings, the lowest parent ID goes first, so `HashMap` order
    /// never decides.
    fn make_room(&mut self, bytes: usize) {
        if self.orphan_bytes + bytes <= MAX_ORPHAN_BYTES {
            return;
        }
        let mut all: Vec<(u64, Hash, usize, usize)> = self
            .orphans
            .iter()
            .flat_map(|(parent, held)| {
                held.iter()
                    .enumerate()
                    .map(move |(i, &(_, at, size))| (at, parent.0, i, size))
            })
            .collect();
        all.sort_unstable();
        let mut dropped = vec![];
        for (_, parent, i, size) in all {
            if self.orphan_bytes + bytes <= MAX_ORPHAN_BYTES {
                break;
            }
            self.orphan_bytes -= size;
            dropped.push((parent, i));
        }
        // Highest index first within each parent, so a removal doesn't shift the ones still due.
        dropped.sort_unstable_by(|a, b| b.cmp(a));
        for (parent, i) in dropped {
            let parent = BlockId(parent);
            let held = self.orphans.get_mut(&parent).expect("listed from the pool");
            held.remove(i);
            if held.is_empty() {
                self.orphans.remove(&parent);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use bytes::Bytes;

    use super::*;
    use crate::chain::MAX_BLOCK_BYTES;
    use crate::chain::block::{BlockHeader, Tx, TxPayload};
    use crate::chain::types::{AccountId, Hash, Signature};
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

    // The store never reads it.
    fn state() -> ChainState {
        ChainState::genesis()
    }

    /// `k`'s bytes in an otherwise zero ID: a missing parent, or an ID to remember.
    fn id(k: u64) -> BlockId {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&k.to_le_bytes());
        BlockId(Hash(bytes))
    }

    fn post(body_len: usize) -> Tx {
        Tx {
            author: AccountId([3; 32]),
            valid_until_height: 100,
            payload: TxPayload::CreatePost {
                body: Bytes::from(vec![b'x'; body_len]),
            },
            signature: Signature::from_bytes(&[4; 64]),
        }
    }

    /// A block on `id(0)` filled with 1 KB posts, the last one cut short so the block encodes to
    /// exactly `MAX_BLOCK_BYTES`.
    fn full_block() -> Block {
        let len = |block: &Block| serialize(block).unwrap().len();
        let mut full = Arc::unwrap_or_clone(block(id(0), 1, EASY, 0));
        let empty = len(&full);
        full.txs.push(post(1024));
        let per_post = len(&full) - empty;
        while len(&full) + per_post <= MAX_BLOCK_BYTES {
            full.txs.push(post(1024));
        }
        full.txs
            .push(post(MAX_BLOCK_BYTES - len(&full) - (per_post - 1024)));
        assert_eq!(len(&full), MAX_BLOCK_BYTES);
        full
    }

    /// `full` with nonce `k`, on its own missing parent `id(k)`, so the per-parent cap never
    /// fires. The bodies are shared, so the clone is cheap.
    fn full_orphan(full: &Block, k: u64) -> Arc<Block> {
        Arc::new(Block {
            header: BlockHeader {
                parent: id(k),
                nonce: k,
                ..full.header.clone()
            },
            txs: full.txs.clone(),
        })
    }

    /// The nonces of every held orphan, sorted.
    fn held(store: &BlockStore) -> Vec<u64> {
        let mut nonces: Vec<_> = store
            .orphans
            .values()
            .flatten()
            .map(|(block, _, _)| block.header.nonce)
            .collect();
        nonces.sort_unstable();
        nonces
    }

    /// Each block goes on the last one half the time, else on any stored block. `check` runs on
    /// genesis alone and after each insert.
    fn random_tree(
        seed: u64,
        n: usize,
        mut check: impl FnMut(&BlockStore, &HashMap<BlockId, BlockId>),
    ) -> (BlockStore, HashMap<BlockId, BlockId>) {
        let mut rng = StdRng::seed_from_u64(seed);
        let mut store = BlockStore::new(genesis(), state());
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
                .insert(block(parent, height, difficulty, nonce), state())
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
        let mut store = BlockStore::new(genesis(), state());
        let a = store.insert(block(g, 1, EASY, 1), state()).unwrap();
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

            assert_eq!(store.insert(block, state()), Err(err));
            assert_eq!(store.tips(), &tips);
            assert_eq!(store.get(parent).map(|n| n.children.clone()), children);
            assert_eq!(store.get(id).is_some(), stored);
        }

        assert_eq!(store.find_fork_point(unknown, a), None);
        assert_eq!(store.find_fork_point(a, unknown), None);
    }

    #[test]
    fn the_pool_holds_max_orphan_bytes_and_drops_the_oldest_past_it() {
        const FULL_BLOCKS: u64 = (MAX_ORPHAN_BYTES / MAX_BLOCK_BYTES) as u64;
        assert_eq!(MAX_ORPHAN_BYTES % MAX_BLOCK_BYTES, 0);
        let full = full_block();
        let mut store = BlockStore::new(genesis(), state());
        for k in 1..=FULL_BLOCKS {
            store.hold_orphan(full_orphan(&full, k), k);
        }
        assert_eq!(store.orphan_bytes, MAX_ORPHAN_BYTES);
        assert_eq!(held(&store), (1..=FULL_BLOCKS).collect::<Vec<_>>());

        store.hold_orphan(full_orphan(&full, FULL_BLOCKS + 1), FULL_BLOCKS + 1);
        assert_eq!(store.orphan_bytes, MAX_ORPHAN_BYTES);
        assert_eq!(held(&store), (2..=FULL_BLOCKS + 1).collect::<Vec<_>>());

        assert_eq!(store.take_orphans(id(FULL_BLOCKS + 1)).len(), 1);
        assert_eq!(store.orphan_bytes, MAX_ORPHAN_BYTES - MAX_BLOCK_BYTES);
    }

    #[test]
    fn a_parent_holds_max_orphans_per_parent_and_drops_its_oldest_past_it() {
        let mut store = BlockStore::new(genesis(), state());
        // Older than every orphan of id(1), so a drop from the whole pool would take it instead.
        store.hold_orphan(block(id(2), 1, EASY, 10), 0);
        for k in 1..=MAX_ORPHANS_PER_PARENT as u64 {
            store.hold_orphan(block(id(1), 1, EASY, k), k);
        }
        assert_eq!(held(&store), [1, 2, 3, 4, 10]);

        store.hold_orphan(block(id(1), 1, EASY, 5), 5);
        assert_eq!(held(&store), [2, 3, 4, 5, 10]);
        let size = serialize(&*block(id(1), 1, EASY, 0)).unwrap().len();
        assert_eq!(store.orphan_bytes, 5 * size);
    }

    #[test]
    fn the_pool_drops_several_orphans_of_one_parent_at_once() {
        const FULL_BLOCKS: u64 = (MAX_ORPHAN_BYTES / MAX_BLOCK_BYTES) as u64;
        let full = full_block();
        let mut store = BlockStore::new(genesis(), state());
        // The four small orphans are the oldest, and the pool is four small blocks short of
        // holding one more full block, so all four must go for the last one to fit.
        for nonce in 201..=204 {
            store.hold_orphan(block(id(200), 1, EASY, nonce), 0);
        }
        for k in 1..FULL_BLOCKS {
            store.hold_orphan(full_orphan(&full, k), k);
        }

        store.hold_orphan(full_orphan(&full, FULL_BLOCKS), FULL_BLOCKS);
        assert_eq!(held(&store), (1..=FULL_BLOCKS).collect::<Vec<_>>());
        assert!(!store.orphans.contains_key(&id(200)));
        assert_eq!(store.orphan_bytes, MAX_ORPHAN_BYTES);
    }

    #[test]
    fn expired_orphans_give_back_their_bytes() {
        let mut store = BlockStore::new(genesis(), state());
        store.hold_orphan(block(id(1), 1, EASY, 1), 0);
        let kept = block(id(2), 1, EASY, 2);
        store.hold_orphan(Arc::clone(&kept), 10);

        store.drop_expired_orphans(MAX_ORPHAN_AGE + 1);
        assert_eq!(held(&store), [2]);
        assert_eq!(store.orphan_bytes, serialize(&*kept).unwrap().len());
    }

    #[test]
    fn the_invalid_set_keeps_the_last_max_invalid_ids() {
        let mut store = BlockStore::new(genesis(), state());
        let last = MAX_INVALID_IDS as u64;
        for k in 1..=last {
            store.mark_invalid(id(k));
        }
        assert!(store.is_invalid(id(1)));
        assert!(store.is_invalid(id(last)));

        store.mark_invalid(id(last + 1));
        assert!(!store.is_invalid(id(1)));
        assert!(store.is_invalid(id(2)));
        assert!(store.is_invalid(id(last + 1)));
    }
}
