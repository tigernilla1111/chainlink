//! Blocks and transactions (design §3).

use bytes::Bytes;
use primitive_types::U256;
use serde::{Deserialize, Serialize};

use crate::chain::codec::serialize;
use crate::chain::crypto::sha256;
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

impl BlockHeader {
    pub fn id(&self) -> BlockId {
        BlockId(sha256(&serialize(self).expect("a header always encodes")))
    }

    /// `bits` as a Bitcoin-style compact target: the top byte is a size, bit 23 a sign bit, the
    /// low 23 bits a mantissa, `target = mantissa * 256^(size - 3)`, rounded down. None if the
    /// target is zero, negative, or doesn't fit in 256 bits.
    fn target(&self) -> Option<U256> {
        if self.bits & 0x0080_0000 != 0 {
            return None;
        }
        let size = (self.bits >> 24) as usize;
        let word = self.bits & 0x007f_ffff;
        let target = if size <= 3 {
            U256::from(word >> (8 * (3 - size)))
        } else {
            let shift = 8 * (size - 3);
            if shift + (32 - word.leading_zeros()) as usize > 256 {
                return None;
            }
            U256::from(word) << shift
        };
        (!target.is_zero()).then_some(target)
    }

    /// `2^256 / (target + 1)`, computed as `!target / (target + 1) + 1` so it fits in U256 (design §3).
    pub fn work(&self) -> Option<U256> {
        let target = self.target()?;
        Some(!target / (target + 1) + 1)
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
    use crate::chain::codec::deserialize;

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

    fn with_bits(bits: u32) -> BlockHeader {
        BlockHeader { bits, ..header() }
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
    fn target_and_work_known_values() {
        let cases = [
            (
                0x1d00ffff,
                U256::from(0xffffu64) << 208,
                U256::from(0x1_0001_0001u64),
            ),
            (0x0300ffff, U256::from(0xffffu64), U256::one() << 240),
            (0x22000080, U256::one() << 255, U256::one()),
            (0x207fffff, U256::from(0x7fffffu64) << 232, U256::from(2u64)),
            (0x2100ffff, U256::from(0xffffu64) << 240, U256::one()),
        ];
        for (bits, target, work) in cases {
            assert_eq!(with_bits(bits).target(), Some(target), "{bits:#010x}");
            assert_eq!(with_bits(bits).work(), Some(work), "{bits:#010x}");
        }
        assert_eq!(with_bits(0x02123456).target(), Some(U256::from(0x1234u64)));
    }

    #[test]
    fn bad_bits_have_no_target() {
        for bits in [
            0x1d000000, 0x1d800001, 0x23000001, 0x22000100, 0x21010000, 0x22000101, 0x2101ffff,
            0x227fffff, 0x0100ffff,
        ] {
            assert_eq!(with_bits(bits).target(), None, "{bits:#010x}");
        }
        assert_eq!(with_bits(0x1d800001).work(), None);
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
