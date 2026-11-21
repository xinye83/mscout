//! File helpers shared by the stores.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Replace `path` with `bytes` so that a crash or power loss leaves either the
/// old file or the new one, never a truncated mix of the two.
///
/// The bytes go to a sibling `<name>.tmp`, are flushed to disk, and the temp file
/// is then renamed over the target (`fs::rename` replaces an existing file on
/// Windows as well). On failure the temp file is removed and the target is left
/// as it was.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp = temp_path(path);
    let result = write_synced(&temp, bytes).and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mscout-fsutil-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    #[test]
    fn writes_a_new_file_and_leaves_no_temp() {
        let dir = scratch("new");
        let path = dir.join("settings.json");
        write_atomic(&path, b"{\"a\":1}").expect("write");
        assert_eq!(fs::read(&path).expect("read"), b"{\"a\":1}");
        assert!(!dir.join("settings.json.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replaces_an_existing_file() {
        let dir = scratch("replace");
        let path = dir.join("profile.json");
        fs::write(&path, b"old contents that are longer").expect("seed");
        write_atomic(&path, b"new").expect("write");
        assert_eq!(fs::read(&path).expect("read"), b"new");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_write_keeps_the_old_file() {
        let dir = scratch("fail");
        let path = dir.join("profile.json");
        fs::write(&path, b"old").expect("seed");
        // A directory where the temp file should go makes File::create fail.
        fs::create_dir_all(dir.join("profile.json.tmp")).expect("block temp");
        assert!(write_atomic(&path, b"new").is_err());
        assert_eq!(fs::read(&path).expect("read"), b"old");
        let _ = fs::remove_dir_all(&dir);
    }
}
