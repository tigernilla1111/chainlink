//! Consensus and state. No networking or disk access.

pub mod block;
pub mod codec;
pub mod crypto;
pub mod manager;
pub mod state;
pub mod store;
pub mod types;
pub mod validate;

use std::num::NonZeroU64;
use std::sync::Arc;

use crate::chain::block::{Block, BlockHeader};
use crate::chain::types::{BlockId, Hash};

/// Largest serialized block: 128 KB (design §1).
pub const MAX_BLOCK_BYTES: usize = 128 * 1024;
/// How many blocks a post and its thread live: 7 days (design §1, §8).
pub const R: u64 = 20_160;
/// Longest a tx stays valid, in blocks: 1 h, provisional (design §1, §8).
pub const TX_TTL: u64 = 120;
// The state's block ring relies on it: advancing looks up block `h − TX_TTL − 1` in the last
// `R + 1` blocks.
const _: () = assert!(TX_TTL <= R);
/// The fixed difficulty, 2^22 expected hashes per block (design §1). Every header must carry
/// it until Phase 2's per-block retarget replaces the equality check in `validate`.
pub const DIFFICULTY: NonZeroU64 = NonZeroU64::new(1 << 22).unwrap();
/// Largest post or comment body, in UTF-8 bytes, provisional (design §1).
pub const MAX_BODY_BYTES: usize = 1024;
/// How far past the local clock a block's timestamp may be, in seconds, provisional (design §4).
pub const MAX_TIMESTAMP_AHEAD: u64 = 132;
/// A block's timestamp must be after the median timestamp of this many blocks ending at its
/// parent, fewer near genesis (design §4).
pub const MEDIAN_BLOCKS: usize = 11;

/// The block every chain starts from, the same on every node (design §1). Nodes compile it
/// in and never validate it, so its `tx_root` is a plain zero hash and its difficulty is 1.
pub const GENESIS_HEADER: BlockHeader = BlockHeader {
    parent: BlockId(Hash([0; 32])),
    height: 0,
    timestamp: 1_767_225_600, // 2026-01-01T00:00:00Z
    tx_root: Hash([0; 32]),
    difficulty: NonZeroU64::new(1).unwrap(),
    nonce: 0,
};

/// `GENESIS_HEADER.id()`, pinned by a test. Every tx's signed bytes start with it.
#[rustfmt::skip]
pub const GENESIS_ID: BlockId = BlockId(Hash([
    0x4a, 0x83, 0x3e, 0x66, 0x26, 0xb3, 0x2b, 0xfd,
    0x17, 0x2b, 0xe8, 0xf0, 0xa2, 0x44, 0x35, 0x5e,
    0xc8, 0xa5, 0x74, 0xb9, 0x6d, 0xde, 0xf1, 0x1d,
    0x27, 0x8b, 0x54, 0x7d, 0x98, 0x7f, 0xe1, 0x4c,
]));

/// The genesis block: `GENESIS_HEADER` and no txs.
pub fn genesis_block() -> Arc<Block> {
    Arc::new(Block {
        header: GENESIS_HEADER,
        txs: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genesis_id_is_the_genesis_headers_hash() {
        assert_eq!(GENESIS_HEADER.id(), GENESIS_ID);
    }
}
