//! The checks that need no state: the header, its timestamp against the parent's chain, and the
//! txs. Pure functions: the caller passes the clock reading and the recent timestamps. The
//! manager runs them in order, `check_header` first, since it keeps the tx encoders below from
//! panicking on an oversize block. See design §4, steps 2, 5 and 6.

use std::num::NonZeroU64;

use primitive_types::U256;

use crate::chain::block::{Block, BlockHeader, Tx};
use crate::chain::codec::serialize;
use crate::chain::crypto::{sha256, verify};
use crate::chain::types::{Hash, TxId};
use crate::chain::{
    DIFFICULTY, MAX_BLOCK_BYTES, MAX_BODY_BYTES, MAX_TIMESTAMP_AHEAD, MEDIAN_BLOCKS,
};

/// `TimestampAhead` and `TxRootMismatch` may not be the block's fault, so the block isn't
/// remembered as invalid. See design §4.
#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum BlockError {
    // check_header
    #[error("difficulty {0} is not {DIFFICULTY}")]
    WrongDifficulty(NonZeroU64),
    #[error("the header hash is above the target for its difficulty")]
    InsufficientWork,
    #[error(
        "timestamp {timestamp} is more than {MAX_TIMESTAMP_AHEAD} s past the local clock at {now}"
    )]
    TimestampAhead { timestamp: u64, now: u64 },
    /// Only a block built in this process can be oversize: decoding caps at `MAX_BLOCK_BYTES`.
    /// Checked so `check_txs`'s encoders can't panic. A local bug, so there is nothing to remember.
    #[error("the block does not encode within {MAX_BLOCK_BYTES} bytes")]
    Oversize,
    // check_parent
    #[error("timestamp {timestamp} is not after the median {median} of recent blocks")]
    TimestampNotAfterMedian { timestamp: u64, median: u64 },
    // check_txs
    #[error("tx_root {header:?} is not the txs' merkle root {txs:?}")]
    TxRootMismatch { header: Hash, txs: Hash },
    #[error("tx {tx:?} has a {len}-byte body, outside 1..={MAX_BODY_BYTES}")]
    BodyLength { tx: TxId, len: usize },
    #[error("tx {0:?} has a body that is not UTF-8")]
    BodyNotUtf8(TxId),
    #[error("tx {0:?} has an invalid signature")]
    BadSignature(TxId),
}

/// Checks the header on its own, and the block's size. `now` is the local clock in Unix
/// seconds, passed in so this stays pure. See design §4, step 2.
pub fn check_header(block: &Block, now: u64) -> Result<(), BlockError> {
    let header = &block.header;
    if header.difficulty != DIFFICULTY {
        return Err(BlockError::WrongDifficulty(header.difficulty));
    }
    // The hash read big-endian must be at most U256::MAX / difficulty. Before the clock check,
    // so bad PoW is always remembered as invalid. See design §3.
    let hash = U256::from_big_endian(&header.id().0.0);
    if hash > U256::MAX / header.difficulty.get() {
        return Err(BlockError::InsufficientWork);
    }
    if header.timestamp > now + MAX_TIMESTAMP_AHEAD {
        return Err(BlockError::TimestampAhead {
            timestamp: header.timestamp,
            now,
        });
    }
    // bincode only caps decoding; this guards blocks from any caller (a miner, a test).
    serialize(block).map_err(|_| BlockError::Oversize)?;
    Ok(())
}

/// Checks the header's timestamp against its parent's chain; the store checks the height before
/// `apply_block`. `recent_timestamps` is the parent's timestamp, then its ancestors', newest
/// first; only the first `MEDIAN_BLOCKS` count, and near genesis there are fewer. The median is
/// `sorted[n / 2]`. Panics if `recent_timestamps` is empty. See design §4, step 5.
pub fn check_parent(header: &BlockHeader, recent_timestamps: &[u64]) -> Result<(), BlockError> {
    let mut recent: Vec<u64> = recent_timestamps
        .iter()
        .copied()
        .take(MEDIAN_BLOCKS)
        .collect();
    recent.sort_unstable();
    let median = recent[recent.len() / 2]; // never empty: the parent's timestamp is always passed
    if header.timestamp <= median {
        return Err(BlockError::TimestampNotAfterMedian {
            timestamp: header.timestamp,
            median,
        });
    }
    Ok(())
}

/// Checks the txs on their own. The merkle root first, so every later failure follows from txs
/// the header committed to. Call `check_header` first: an oversize block would panic the
/// encoders here. See design §4, step 6.
pub fn check_txs(block: &Block) -> Result<(), BlockError> {
    let txs = merkle_root(&block.txs);
    if block.header.tx_root != txs {
        return Err(BlockError::TxRootMismatch {
            header: block.header.tx_root,
            txs,
        });
    }
    block.txs.iter().try_for_each(check_tx)
}

/// Format, body size and signature of one tx. Deletes have no body.
fn check_tx(tx: &Tx) -> Result<(), BlockError> {
    if let Some(body) = tx.body() {
        if !(1..=MAX_BODY_BYTES).contains(&body.len()) {
            return Err(BlockError::BodyLength {
                tx: tx.id(),
                len: body.len(),
            });
        }
        if std::str::from_utf8(body).is_err() {
            return Err(BlockError::BodyNotUtf8(tx.id()));
        }
    }
    verify(tx.author, &tx.signed_bytes(), &tx.signature)
        .map_err(|_| BlockError::BadSignature(tx.id()))
}

/// The root of an RFC 6962-shaped merkle tree over the txs' full encodings, signature included:
/// leaf = SHA-256(0x00 ‖ tx), node = SHA-256(0x01 ‖ left ‖ right), a list of n > 1 splits at the
/// largest power of two below n, and the empty list hashes to SHA-256 of nothing. No leaf is
/// duplicated, so two tx lists can't share a root. See design §3.
pub fn merkle_root(txs: &[Tx]) -> Hash {
    let leaves: Vec<Hash> = txs
        .iter()
        .map(|tx| {
            let mut leaf = vec![0x00];
            leaf.extend(serialize(tx).expect("a tx from a block always encodes"));
            sha256(&leaf)
        })
        .collect();
    root(&leaves)
}

fn root(leaves: &[Hash]) -> Hash {
    match leaves {
        [] => sha256(b""),
        [leaf] => *leaf,
        _ => {
            let k = 1 << (leaves.len() - 1).ilog2(); // largest power of two below n
            let (left, right) = leaves.split_at(k);
            let mut node = vec![0x01];
            node.extend(root(left).0);
            node.extend(root(right).0);
            sha256(&node)
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use ed25519_dalek::{Signer, SigningKey};

    use super::*;
    use crate::chain::GENESIS_ID;
    use crate::chain::block::TxPayload;
    use crate::chain::types::{AccountId, BlockId, Signature};

    fn hex(hash: &Hash) -> String {
        hash.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Block 1 of this chain, mined offline once at `DIFFICULTY` (a debug build hashes at
    /// 0.08 MH/s, so tests never mine). Passes all three steps.
    fn block_1() -> Block {
        Block {
            header: BlockHeader {
                parent: GENESIS_ID,
                height: 1,
                timestamp: 1_767_225_630, // genesis + 30 s
                tx_root: merkle_root(&[]),
                difficulty: DIFFICULTY,
                nonce: 3_731_462,
            },
            txs: vec![],
        }
    }

    fn key(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }

    fn signed(n: u8, payload: TxPayload) -> Tx {
        let mut tx = Tx {
            author: AccountId(key(n).verifying_key().to_bytes()),
            valid_until_height: 100,
            payload,
            signature: Signature::from_bytes(&[0; 64]),
        };
        tx.signature = key(n).sign(&tx.signed_bytes());
        tx
    }

    fn post(n: u8, body: &[u8]) -> Tx {
        let body = Bytes::copy_from_slice(body);
        signed(n, TxPayload::CreatePost { body })
    }

    /// Filler author and signature, for the merkle vector.
    fn unsigned(author: u8, payload: TxPayload, signature: u8) -> Tx {
        Tx {
            author: AccountId([author; 32]),
            valid_until_height: 100,
            payload,
            signature: Signature::from_bytes(&[signature; 64]),
        }
    }

    /// `check_txs` reads only the header's `tx_root`.
    fn block(txs: Vec<Tx>) -> Block {
        Block {
            header: BlockHeader {
                tx_root: merkle_root(&txs),
                ..block_1().header
            },
            txs,
        }
    }

    #[test]
    fn bad_headers_are_errors_naming_the_check() {
        let b = block_1();
        // Computed outside Rust: 22 leading zero bits.
        assert_eq!(
            hex(&b.header.id().0),
            "00000348ef22a60aa0e6ca20f7b010e67b52268cecc57f1392cb7525cea8c377"
        );
        let t = b.header.timestamp;
        assert_eq!(check_header(&b, t), Ok(()));

        let one = NonZeroU64::new(1).unwrap();
        let table = [
            (
                Block {
                    header: BlockHeader {
                        difficulty: one,
                        ..block_1().header
                    },
                    ..block_1()
                },
                t,
                BlockError::WrongDifficulty(one),
            ),
            // 21 leading zero bits, one short of the target. Also ahead of the clock: bad PoW is
            // reported, so it is remembered. See design §4.
            (
                Block {
                    header: BlockHeader {
                        nonce: 5_069_833,
                        ..block_1().header
                    },
                    ..block_1()
                },
                t - MAX_TIMESTAMP_AHEAD - 1,
                BlockError::InsufficientWork,
            ),
            (
                Block {
                    txs: vec![post(1, &[b'x'; 1024]); 128],
                    ..block_1()
                },
                t,
                BlockError::Oversize,
            ),
        ];
        for (block, now, err) in table {
            assert_eq!(check_header(&block, now), Err(err));
        }
    }

    #[test]
    fn a_timestamp_may_be_at_most_max_timestamp_ahead_of_the_clock() {
        let b = block_1();
        let t = b.header.timestamp;
        assert_eq!(check_header(&b, t - MAX_TIMESTAMP_AHEAD), Ok(()));
        let now = t - MAX_TIMESTAMP_AHEAD - 1;
        assert_eq!(
            check_header(&b, now),
            Err(BlockError::TimestampAhead { timestamp: t, now })
        );
    }

    #[test]
    fn timestamp_must_beat_the_median_of_recent_blocks() {
        let at = |timestamp| BlockHeader {
            timestamp,
            ..block_1().header
        };
        // Newest first. The newest 11 give 5; the newest 1 to 10 give 0, 1, 6 or 7, all 12 give
        // 6, the oldest 11 give 6 and the floor-mean is 13.
        let recent = [0, 1, 6, 9, 10, 7, 100, 4, 3, 5, 2, 8];
        assert_eq!(
            check_parent(&at(5), &recent),
            Err(BlockError::TimestampNotAfterMedian {
                timestamp: 5,
                median: 5
            })
        );
        assert_eq!(check_parent(&at(6), &recent), Ok(()));

        // An even count takes the upper middle.
        let even = [40, 10, 30, 20];
        assert_eq!(
            check_parent(&at(30), &even),
            Err(BlockError::TimestampNotAfterMedian {
                timestamp: 30,
                median: 30
            })
        );
        assert_eq!(check_parent(&at(31), &even), Ok(()));
    }

    #[test]
    fn merkle_root_known_vector() {
        // Every variant, `reply_to` both ways, a count that isn't a power of two, and distinct
        // signatures, which the leaves cover.
        let body = |s: &'static str| Bytes::from_static(s.as_bytes());
        let txs = [
            unsigned(1, TxPayload::CreatePost { body: body("post") }, 11),
            unsigned(
                2,
                TxPayload::CreateComment {
                    post: TxId(Hash([6; 32])),
                    reply_to: None,
                    body: body("comment"),
                },
                12,
            ),
            unsigned(
                3,
                TxPayload::CreateComment {
                    post: TxId(Hash([7; 32])),
                    reply_to: Some(TxId(Hash([8; 32]))),
                    body: body("reply"),
                },
                13,
            ),
            unsigned(
                4,
                TxPayload::Delete {
                    target: TxId(Hash([9; 32])),
                },
                14,
            ),
            unsigned(
                5,
                TxPayload::CreatePost {
                    body: body("another post"),
                },
                15,
            ),
        ];
        assert_eq!(
            hex(&merkle_root(&[])),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&merkle_root(&txs)),
            "1842b8e5756dade118a9bc2f4c66e9d2b163933b19f3b72517f2ec300763b7c2"
        );
    }

    #[test]
    fn bad_txs_fail_the_block_naming_the_tx_and_the_check() {
        let delete = signed(
            2,
            TxPayload::Delete {
                target: TxId(Hash([9; 32])),
            },
        );
        assert_eq!(check_txs(&block(vec![delete.clone()])), Ok(()));

        let empty = signed(
            3,
            TxPayload::CreateComment {
                post: TxId(Hash([6; 32])),
                reply_to: None,
                body: Bytes::new(),
            },
        );
        let not_utf8 = post(1, &[0xff]);
        let table = [
            // block_1's empty root over a tx: the root is checked before the tx. See design §4.
            (
                Block {
                    txs: vec![not_utf8.clone()],
                    ..block_1()
                },
                BlockError::TxRootMismatch {
                    header: merkle_root(&[]),
                    txs: merkle_root(std::slice::from_ref(&not_utf8)),
                },
            ),
            (
                block(vec![delete.clone(), empty.clone()]),
                BlockError::BodyLength {
                    tx: empty.id(),
                    len: 0,
                },
            ),
            (
                block(vec![not_utf8.clone()]),
                BlockError::BodyNotUtf8(not_utf8.id()),
            ),
        ];
        for (block, err) in table {
            assert_eq!(check_txs(&block), Err(err));
        }
    }

    #[test]
    fn a_body_may_be_exactly_max_body_bytes() {
        // Two bytes a char, so counting chars instead of bytes fails.
        let full = "é".repeat(MAX_BODY_BYTES / 2);
        assert_eq!(check_txs(&block(vec![post(1, full.as_bytes())])), Ok(()));
        let over = post(1, (full + "x").as_bytes());
        assert_eq!(
            check_txs(&block(vec![over.clone()])),
            Err(BlockError::BodyLength {
                tx: over.id(),
                len: MAX_BODY_BYTES + 1
            })
        );
    }

    #[test]
    fn a_tx_signed_for_another_genesis_is_rejected() {
        let author = AccountId(key(1).verifying_key().to_bytes());
        let payload = TxPayload::CreatePost {
            body: Bytes::from_static(b"hello"),
        };
        let signed_for = |genesis: BlockId| Tx {
            author,
            valid_until_height: 100,
            payload: payload.clone(),
            signature: key(1).sign(&serialize(&(genesis, author, 100u64, &payload)).unwrap()),
        };
        assert_eq!(check_txs(&block(vec![signed_for(GENESIS_ID)])), Ok(()));
        let other = signed_for(BlockId(Hash([9; 32])));
        assert_eq!(
            check_txs(&block(vec![other.clone()])),
            Err(BlockError::BadSignature(other.id()))
        );
    }
}
