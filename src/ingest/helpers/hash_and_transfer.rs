use std::path::Path;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use xxhash_rust::xxh3::Xxh3;

/// Streams a file, hashing its contents as it goes in 128KB chunks, matching suisai's hash pass.
/// Returns the xxh3-128 content hash and the number of bytes transferred/read.
pub async fn hash_and_read(src: &Path) -> Result<(String, u64), std::io::Error> {
    let mut src_file = File::open(src).await?;
    let mut hasher = Xxh3::new();
    let mut total_bytes = 0u64;
    let mut buf = vec![0u8; 128 * 1024]; // 128KB chunks matching suisai

    loop {
        let n = src_file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total_bytes += n as u64;
    }

    if total_bytes == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "File is empty (0 bytes)",
        ));
    }

    Ok((format!("{:032x}", hasher.digest128()), total_bytes))
}
