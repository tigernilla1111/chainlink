//! Canonical bincode (design §3): little-endian, fixed-width ints, at most `MAX_BLOCK_BYTES`,
//! no trailing bytes.

use bincode::{
    config::{Configuration, Fixint, Limit, LittleEndian},
    error::{DecodeError, EncodeError},
    serde::{decode_from_slice, encode_to_vec},
};

use crate::chain::MAX_BLOCK_BYTES;

// A block is the largest thing encoded.
const CONFIG: Configuration<LittleEndian, Fixint, Limit<MAX_BLOCK_BYTES>> =
    bincode::config::standard()
        .with_little_endian()
        .with_fixed_int_encoding()
        .with_limit::<MAX_BLOCK_BYTES>();

pub fn serialize<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, EncodeError> {
    let bytes = encode_to_vec(value, CONFIG)?;

    // bincode only checks the limit when decoding
    if bytes.len() > MAX_BLOCK_BYTES {
        return Err(EncodeError::Other("larger than MAX_BLOCK_BYTES"));
    }

    Ok(bytes)
}
pub fn deserialize<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, DecodeError> {
    let (value, bytes_read) = decode_from_slice(bytes, CONFIG)?;

    if bytes_read != bytes.len() {
        return Err(DecodeError::Other("trailing bytes".into()));
    }

    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use bytes::Bytes;

    use super::*;
    use crate::chain::block::{Block, BlockHeader, Tx, TxPayload};
    use crate::chain::types::{AccountId, BlockId, Hash, Signature};

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

    /// 1 KB posts, the last one cut short so the block is exactly `size` bytes.
    fn block_of_size(size: usize) -> Block {
        let len = |block: &Block| encode_to_vec(block, CONFIG).unwrap().len();
        let mut block = Block {
            header: header(),
            txs: vec![],
        };
        while len(&block) < size {
            block.txs.push(post(1024));
        }
        let over = len(&block) - size;
        *block.txs.last_mut().unwrap() = post(1024 - over);
        block
    }

    #[test]
    fn limit_is_max_block_bytes() {
        let full = block_of_size(MAX_BLOCK_BYTES);
        let bytes = serialize(&full).unwrap();
        assert_eq!(bytes.len(), MAX_BLOCK_BYTES);
        assert_eq!(deserialize::<Block>(&bytes).unwrap(), full);

        let oversize = block_of_size(MAX_BLOCK_BYTES + 1);
        assert!(matches!(serialize(&oversize), Err(EncodeError::Other(_))));
        // bincode only checks the limit when decoding
        let bytes = encode_to_vec(&oversize, CONFIG).unwrap();
        assert!(matches!(
            deserialize::<Block>(&bytes),
            Err(DecodeError::LimitExceeded)
        ));
    }

    #[test]
    fn header_id_known_vector() {
        // Pins little-endian, fixed-width ints and the header's field order.
        let hex: String = header()
            .id()
            .0
            .0
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex,
            "39bacd5c49c0d609e17ced202e055adcefad954bef95e97c9e365784141de7fd"
        );
    }
}
