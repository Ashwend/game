//! Small filesystem helpers shared by the sidecar-file writers.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// Write `contents` to `path` atomically: create the parent directory, write to
/// a per-process temp file, fsync it, then rename over the target so a reader
/// never sees a half-written file. The temp file is removed if any step fails.
///
/// Adopts the save layer's hardening (a propagated `sync_all` and temp cleanup
/// on error) for the small sidecar files (the analytics id, the skipped-version
/// marker) that each previously rolled their own variant that swallowed the
/// fsync result and leaked the temp file on failure. The save layer keeps its
/// own writer because it additionally needs the Windows backup-and-restore
/// replace dance, which these tiny files do not.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    let result = (|| -> io::Result<()> {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_round_trips_and_overwrites() {
        let path = std::env::temp_dir()
            .join(format!("ashwend-write-atomic-{}", uuid::Uuid::new_v4()))
            .join("nested")
            .join("file.bin");
        write_atomic(&path, b"first").expect("first write");
        assert_eq!(fs::read(&path).expect("read"), b"first");
        write_atomic(&path, b"second").expect("overwrite");
        assert_eq!(fs::read(&path).expect("read"), b"second");
        let _ = fs::remove_file(&path);
    }
}
