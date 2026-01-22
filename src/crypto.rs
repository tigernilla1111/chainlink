const SHA512_OUTPUT_BYTES: usize = 64;

pub type PublicKeyBytes = ();
pub type SignatureBytes = ();
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Hash)]
pub struct HashBytes(#[serde(with = "serde_bytes")] [u8; SHA512_OUTPUT_BYTES]);
