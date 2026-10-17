//! Fireshare's video id: the xxh3-128 hex digest of the file's first 16 MiB (`util.video_id`).

use std::io::Read;
use std::path::Path;

use crate::error::{AppError, AppResult};

const HASHED_BYTES: u64 = 16 * 1024 * 1024;

pub fn video_id(path: &Path) -> AppResult<String> {
    let mut head = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(HASHED_BYTES).read_to_end(&mut head))
        .map_err(|e| AppError::io(path.display(), e))?;
    Ok(digest(&head))
}

/// The same 32 hex digits as Python's `xxhash.xxh3_128_hexdigest`.
fn digest(bytes: &[u8]) -> String {
    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| ((i * 31 + i / 251) % 256) as u8).collect()
    }

    /// Vectors from Python `xxhash` 4.0.1: `xxh3_128_hexdigest(data(n)[:16 MiB])`.
    #[test]
    fn matches_python() {
        assert_eq!(digest(&data(0)), "99aa06d3014798d86001c324468d497f");
        assert_eq!(digest(&data(1000)), "baaec9f881a60ea82bc8f72e8da57c62");
    }

    #[test]
    fn hashes_only_the_first_16_mib() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clip.mp4");
        std::fs::write(&path, data(20 * 1024 * 1024)).unwrap();
        assert_eq!(video_id(&path).unwrap(), "f41e8246dced794415440ba9302f7186");
    }
}
