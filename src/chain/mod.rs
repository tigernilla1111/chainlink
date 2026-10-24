//! Consensus and state. No networking or disk access.

pub mod block;
pub mod codec;
pub mod crypto;
pub mod state;
pub mod store;
pub mod types;

/// Largest serialized block: 128 KB (design §1).
pub const MAX_BLOCK_BYTES: usize = 128 * 1024;
