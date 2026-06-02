//! Download the host's release archive, verify it, and stage the new game
//! binary next to the running one so the updater can swap it in with a plain
//! same-volume rename.
//!
//! The archive itself goes to the system temp dir (it's read once then
//! deleted), but the *extracted* binary is staged in the install directory,
//! `rename(staged → target)` is only atomic within one filesystem, and `/tmp`
//! can be a different mount (tmpfs on Linux), so staging beside the target is
//! what guarantees the updater's swap can't tear.

use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use super::{asset, github::ReleaseAsset};

/// Bytes pulled from the socket per read. 64 KiB keeps progress updates smooth
/// without thrashing the channel.
const CHUNK: usize = 64 * 1024;

/// Download + verify + extract. `progress(received, total)` is called as bytes
/// arrive (`total` is `None` if the server omitted `Content-Length`). Returns
/// the staged binary path on success.
pub(crate) fn download_and_stage(
    agent: &ureq::Agent,
    asset: &ReleaseAsset,
    progress: &(dyn Fn(u64, Option<u64>) + Send + Sync),
) -> Result<PathBuf, String> {
    let target = std::env::current_exe().map_err(|e| format!("cannot locate current exe: {e}"))?;
    let install_dir = target
        .parent()
        .ok_or_else(|| "current exe has no parent directory".to_owned())?;

    let archive_path =
        std::env::temp_dir().join(format!("ashwend-update-{}.archive", std::process::id()));
    let staged_path = install_dir.join(format!(".ashwend-update.{}.staged", std::process::id()));

    let result = (|| {
        download_archive(agent, asset, &archive_path, progress)?;
        verify_sha256(&archive_path, asset.digest.as_deref())?;
        extract_game_binary(&archive_path, &staged_path)?;
        set_executable(&staged_path)?;
        Ok(staged_path.clone())
    })();

    // The archive is only ever read for extraction, drop it either way.
    let _ = fs::remove_file(&archive_path);
    if result.is_err() {
        let _ = fs::remove_file(&staged_path);
    }
    result
}

fn download_archive(
    agent: &ureq::Agent,
    asset: &ReleaseAsset,
    dest: &Path,
    progress: &(dyn Fn(u64, Option<u64>) + Send + Sync),
) -> Result<(), String> {
    if asset.browser_download_url.is_empty() {
        return Err("release asset has no download url".to_owned());
    }
    let response = agent
        .get(&asset.browser_download_url)
        .set(
            "User-Agent",
            &format!("ashwend/{}", crate::protocol::GAME_VERSION),
        )
        .call()
        .map_err(|e| format!("download request failed: {e}"))?;

    let total = response
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .or(if asset.size > 0 {
            Some(asset.size)
        } else {
            None
        });

    let mut reader = response.into_reader();
    let mut file =
        fs::File::create(dest).map_err(|e| format!("cannot create download file: {e}"))?;
    let mut buf = vec![0u8; CHUNK];
    let mut received: u64 = 0;
    progress(0, total);
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("download read failed: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("download write failed: {e}"))?;
        received += n as u64;
        progress(received, total);
    }
    file.sync_all().ok();
    Ok(())
}

fn verify_sha256(path: &Path, digest: Option<&str>) -> Result<(), String> {
    let Some(digest) = digest else {
        // GitHub didn't publish a digest for this asset; nothing to check
        // against. Don't fail the update over a missing checksum.
        eprintln!("update: release asset has no digest; skipping checksum verification");
        return Ok(());
    };
    let expected = digest
        .strip_prefix("sha256:")
        .ok_or_else(|| format!("unsupported digest format: {digest}"))?
        .trim()
        .to_ascii_lowercase();

    let mut file = fs::File::open(path).map_err(|e| format!("cannot reopen download: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("hash read failed: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = hex_lower(&hasher.finalize());
    if actual != expected {
        return Err(format!(
            "checksum mismatch: expected {expected}, got {actual}"
        ));
    }
    Ok(())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// True when an archive entry path refers to the wanted member, tolerating a
/// `./` prefix or an extra leading directory.
fn entry_matches(name: &str, member: &str) -> bool {
    let name = name.trim_start_matches("./");
    name == member || name.ends_with(&format!("/{member}"))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn extract_game_binary(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = fs::File::open(archive).map_err(|e| format!("cannot open archive: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("invalid zip archive: {e}"))?;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("zip entry {i} unreadable: {e}"))?;
        if entry.is_file() && entry_matches(entry.name(), asset::ARCHIVE_GAME_MEMBER) {
            let mut out =
                fs::File::create(dest).map_err(|e| format!("cannot create staged binary: {e}"))?;
            std::io::copy(&mut entry, &mut out).map_err(|e| format!("extract failed: {e}"))?;
            out.sync_all().ok();
            return Ok(());
        }
    }
    Err(format!(
        "archive did not contain {}",
        asset::ARCHIVE_GAME_MEMBER
    ))
}

#[cfg(target_os = "linux")]
fn extract_game_binary(archive: &Path, dest: &Path) -> Result<(), String> {
    use flate2::read::GzDecoder;
    let file = fs::File::open(archive).map_err(|e| format!("cannot open archive: {e}"))?;
    let mut tar = tar::Archive::new(GzDecoder::new(file));
    let entries = tar
        .entries()
        .map_err(|e| format!("invalid tar archive: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("tar entry unreadable: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("tar path unreadable: {e}"))?
            .to_string_lossy()
            .into_owned();
        if entry_matches(&path, asset::ARCHIVE_GAME_MEMBER) {
            let mut out =
                fs::File::create(dest).map_err(|e| format!("cannot create staged binary: {e}"))?;
            std::io::copy(&mut entry, &mut out).map_err(|e| format!("extract failed: {e}"))?;
            out.sync_all().ok();
            return Ok(());
        }
    }
    Err(format!(
        "archive did not contain {}",
        asset::ARCHIVE_GAME_MEMBER
    ))
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let perms = fs::Permissions::from_mode(0o755);
    fs::set_permissions(path, perms).map_err(|e| format!("cannot set exec bit: {e}"))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_matching_tolerates_prefixes_and_avoids_false_positives() {
        // macOS bundle member.
        assert!(entry_matches(
            "Ashwend.app/Contents/MacOS/ashwend",
            "Ashwend.app/Contents/MacOS/ashwend"
        ));
        assert!(entry_matches(
            "./Ashwend.app/Contents/MacOS/ashwend",
            "Ashwend.app/Contents/MacOS/ashwend"
        ));
        // Flat member must not match the sibling updater binary.
        assert!(entry_matches("ashwend", "ashwend"));
        assert!(!entry_matches("ashwend-updater", "ashwend"));
        assert!(entry_matches("ashwend.exe", "ashwend.exe"));
        assert!(!entry_matches("ashwend-updater.exe", "ashwend.exe"));
    }

    #[test]
    fn hex_lower_encodes_bytes() {
        assert_eq!(hex_lower(&[0x00, 0x0f, 0xff, 0xa5]), "000fffa5");
    }

    #[test]
    fn verify_sha256_accepts_match_and_rejects_mismatch() {
        let path = std::env::temp_dir().join(format!("ashwend-hash-test-{}", uuid::Uuid::new_v4()));
        fs::write(&path, b"hello world").unwrap();
        // sha256("hello world")
        let good = "sha256:b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        assert!(verify_sha256(&path, Some(good)).is_ok());
        assert!(verify_sha256(&path, Some("sha256:deadbeef")).is_err());
        // Missing digest is tolerated (warn-only).
        assert!(verify_sha256(&path, None).is_ok());
        // Unsupported algorithm prefix is rejected.
        assert!(verify_sha256(&path, Some("md5:abc")).is_err());
        let _ = fs::remove_file(&path);
    }
}
