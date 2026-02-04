use bincode::{
    error::{DecodeError, EncodeError},
    serde::{decode_from_slice, encode_to_vec},
};

const MAX_ENCODING_BYTES: usize = 10000;

pub fn serialize<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, EncodeError> {
    let cfg = bincode::config::standard()
        .with_little_endian()
        .with_fixed_int_encoding()
        .with_limit::<MAX_ENCODING_BYTES>();

    encode_to_vec(value, cfg)
}
pub fn deserialize<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, DecodeError> {
    let cfg = bincode::config::standard()
        .with_little_endian()
        .with_fixed_int_encoding()
        .with_limit::<MAX_ENCODING_BYTES>();

    let (value, bytes_read) = decode_from_slice(bytes, cfg)?;

    // Optional but recommended: ensure no trailing bytes
    if bytes_read != bytes.len() {
        return Err(DecodeError::Other("trailing bytes".into()));
    }

    Ok(value)
}
