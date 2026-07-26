use std::fs;
use std::io::{self, Write};
use std::path::Path;

use atomicwrites::{AllowOverwrite, AtomicFile};
use serde::Serialize;

pub(crate) fn write_json_atomic<T>(path: &Path, value: &T) -> io::Result<()>
where
    T: Serialize,
{
    let bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    write_bytes_atomic(path, &bytes)
}

pub(crate) fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "持久化路径没有父目录"))?;
    fs::create_dir_all(parent)?;
    AtomicFile::new(path, AllowOverwrite)
        .write(|file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_existing_contents() {
        let path = std::env::temp_dir().join(format!(
            "rebook-atomic-write-{}-{}.json",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        write_bytes_atomic(&path, b"old").unwrap();
        write_bytes_atomic(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        fs::remove_file(path).unwrap();
    }
}
