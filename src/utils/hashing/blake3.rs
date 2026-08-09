use crate::error::{AppError, AppResult};
use tokio::io::AsyncReadExt;

const CHUNK_SIZE: usize = 64 * 1024;

#[derive(Debug)]
pub struct Blake3Hasher {
    hasher: blake3::Hasher,
}

impl Blake3Hasher {
    pub fn new() -> Self {
        Self {
            hasher: blake3::Hasher::new(),
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.hasher.update(data);
    }

    pub fn finalize(self) -> String {
        let hash = self.hasher.finalize();
        hex::encode(hash.as_bytes())
    }
}

pub async fn hash_bytes(data: &[u8]) -> String {
    let mut hasher = Blake3Hasher::new();
    hasher.update(data);
    hasher.finalize()
}

pub async fn hash_stream(reader: impl tokio::io::AsyncRead + Unpin) -> AppResult<String> {
    let mut hasher = Blake3Hasher::new();
    let mut reader = reader;
    let mut buf = vec![0u8; CHUNK_SIZE];

    loop {
        let n = reader
            .read(&mut buf)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(hasher.finalize())
}

pub fn verify(hash: &str, data: &[u8]) -> bool {
    let computed = blake3::hash(data);
    let expected = match hex::decode(hash) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    computed.as_bytes() == expected.as_slice()
}

pub fn prefix<'a>(hash: &'a str, len: usize) -> &'a str {
    // `get` instead of byte-slicing so a `len` that lands inside a multi-byte
    // UTF-8 character cannot panic; falls back to the whole string instead.
    hash.get(..len.min(hash.len())).unwrap_or(hash)
}
