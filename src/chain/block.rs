//! Blocks and transactions (design §3).

use std::num::NonZeroU64;

use bytes::Bytes;
use primitive_types::U256;
use serde::{Deserialize, Serialize};

use crate::chain::codec::serialize;
use crate::chain::crypto::sha256;
use crate::chain::types::{AccountId, BlockId, CommentId, Hash, PostId, Signature, TxId};
use crate::chain::{GENESIS_ID, TX_TTL};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockHeader {
    pub parent: BlockId,
    pub height: u64,
    pub timestamp: u64,
    /// Merkle root over the txs, bodies included.
    pub tx_root: Hash,
    /// The header hash, read big-endian, must be at most `U256::MAX / difficulty` (design §3).
    /// Fixed until Phase 2, which retargets it every block (design §1).
    pub difficulty: NonZeroU64,
    pub nonce: u64,
}

impl BlockHeader {
    pub fn id(&self) -> BlockId {
        BlockId(sha256(&serialize(self).expect("a header always encodes")))
    }

    /// A block's work is its difficulty (design §3).
    pub fn work(&self) -> U256 {
        U256::from(self.difficulty.get())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub txs: Vec<Tx>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tx {
    pub author: AccountId,
    /// Between the height of the block that includes it and `TX_TTL` above it.
    pub valid_until_height: u64,
    pub payload: TxPayload,
    pub signature: Signature,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxPayload {
    CreatePost {
        body: Bytes,
    },
    CreateComment {
        post: PostId,
        reply_to: Option<CommentId>,
        body: Bytes,
    },
    /// A post or a comment.
    Delete {
        target: TxId,
    },
}

impl Tx {
    /// SHA-256 of the bytes the author signed: the genesis ID, then author, valid_until_height
    /// and payload (design §3). The genesis ID keeps a tx from being replayed on another network.
    /// Panics on a tx too big for a block, so check a loose tx's size before calling this.
    pub fn id(&self) -> TxId {
        let signed = (
            GENESIS_ID,
            self.author,
            self.valid_until_height,
            &self.payload,
        );
        TxId(sha256(
            &serialize(&signed).expect("a tx from a block always encodes"),
        ))
    }

    /// `height ≤ valid_until_height ≤ height + TX_TTL`: the tx may be included at `height`
    /// (design §4). The mempool's arrival check uses it with the best height + 1.
    pub fn in_window(&self, height: u64) -> bool {
        height <= self.valid_until_height && self.valid_until_height <= height + TX_TTL
    }

    /// All body reads go through here, so bodies can move out of the tx later.
    pub fn body(&self) -> Option<&Bytes> {
        match &self.payload {
            TxPayload::CreatePost { body } | TxPayload::CreateComment { body, .. } => Some(body),
            TxPayload::Delete { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::codec::deserialize;

    fn header() -> BlockHeader {
        BlockHeader {
            parent: BlockId(Hash([1; 32])),
            height: 7,
            timestamp: 1_700_000_000,
            tx_root: Hash([2; 32]),
            difficulty: NonZeroU64::new(1).unwrap(),
            nonce: 42,
        }
    }

    fn tx() -> Tx {
        Tx {
            author: AccountId([4; 32]),
            valid_until_height: 100,
            payload: TxPayload::CreateComment {
                post: TxId(Hash([5; 32])),
                reply_to: Some(TxId(Hash([6; 32]))),
                body: Bytes::from_static(b"hello"),
            },
            signature: Signature::from_bytes(&[7; 64]),
        }
    }

    #[test]
    fn encoding_is_fixed_width() {
        assert_eq!(serialize(&header()).unwrap().len(), 32 + 8 + 8 + 32 + 8 + 8);
        assert_eq!(
            serialize(&Signature::from_bytes(&[0; 64])).unwrap().len(),
            64
        );
    }

    #[test]
    fn zero_difficulty_does_not_decode() {
        // After parent, height, timestamp and tx_root.
        const AT: usize = 32 + 8 + 8 + 32;
        let mut bytes = serialize(&header()).unwrap();
        assert_eq!(deserialize::<BlockHeader>(&bytes).unwrap(), header());
        bytes[AT..AT + 8].fill(0);
        assert!(deserialize::<BlockHeader>(&bytes).is_err());
    }

    #[test]
    fn block_round_trip() {
        let block = Block {
            header: header(),
            txs: vec![tx()],
        };
        let decoded: Block = deserialize(&serialize(&block).unwrap()).unwrap();
        assert_eq!(decoded, block);
    }

    #[test]
    fn tx_id_known_vector() {
        // Pins the genesis prefix, the field order, the u32 variant index, the u8 `Option` tag
        // and the u64 body length.
        let hex: String = tx().id().0.0.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "8039f358d319575b00d5782158cd5dfb52606807dc943c72fd4d96710df42416"
        );
    }

    #[test]
    fn in_window_is_height_to_height_plus_tx_ttl() {
        let valid_until = |valid_until_height| Tx {
            valid_until_height,
            ..tx()
        };
        assert!(!valid_until(49).in_window(50));
        assert!(valid_until(50).in_window(50));
        assert!(valid_until(50 + TX_TTL).in_window(50));
        assert!(!valid_until(51 + TX_TTL).in_window(50));
    }
}
