//! Blocks and transactions (design §3).

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::chain::types::{AccountId, BlockId, CommentId, Hash, PostId, Signature, TxId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockHeader {
    pub parent: BlockId,
    pub height: u64,
    pub timestamp: u64,
    /// Merkle root over the txs, bodies included.
    pub tx_root: Hash,
    /// Difficulty target, fixed for now.
    pub bits: u32,
    pub nonce: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub txs: Vec<Tx>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tx {
    pub author: AccountId,
    /// At most `TX_TTL` above the block that includes it.
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
    use crate::chain::codec::{deserialize, serialize};

    fn header() -> BlockHeader {
        BlockHeader {
            parent: BlockId(Hash([1; 32])),
            height: 7,
            timestamp: 1_700_000_000,
            tx_root: Hash([2; 32]),
            bits: 0x1d00ffff,
            nonce: 42,
        }
    }

    #[test]
    fn encoding_is_fixed_width() {
        assert_eq!(serialize(&header()).unwrap().len(), 32 + 8 + 8 + 32 + 4 + 8);
        assert_eq!(
            serialize(&Signature::from_bytes(&[0; 64])).unwrap().len(),
            64
        );
    }

    #[test]
    fn block_round_trip() {
        let block = Block {
            header: header(),
            txs: vec![Tx {
                author: AccountId([4; 32]),
                valid_until_height: 100,
                payload: TxPayload::CreateComment {
                    post: TxId(Hash([5; 32])),
                    reply_to: Some(TxId(Hash([6; 32]))),
                    body: Bytes::from_static(b"hello"),
                },
                signature: Signature::from_bytes(&[7; 64]),
            }],
        };
        let decoded: Block = deserialize(&serialize(&block).unwrap()).unwrap();
        assert_eq!(decoded, block);
    }
}
