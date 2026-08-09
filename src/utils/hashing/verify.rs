use std::path::Path;

use crate::error::{AppError, AppResult};
use tokio::fs::File;
use tokio::io::AsyncReadExt;

pub async fn verify_file(path: &Path, expected_hash: &str) -> AppResult<bool> {
    let mut file =
        File::open(path).await.map_err(|e| AppError::Storage(format!("failed to open file: {e}")))?;

    let mut hasher = super::blake3::Blake3Hasher::new();
    let mut buf = [0u8; 64 * 1024];

    loop {
        let n = file
            .read(&mut buf)
            .await
            .map_err(|e| AppError::Storage(format!("failed to read file: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    let computed = hasher.finalize();
    Ok(computed == expected_hash)
}
