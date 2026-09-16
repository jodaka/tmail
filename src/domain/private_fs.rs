//! Owner-only on-disk storage (tickets ty57, 3tp3).
//!
//! Tmail persists state other local users must not read — rendered
//! messages, draft revisions, credentials — so files written through
//! these helpers are created owner-only (`0600`) and directories `0700`
//! from the very first byte. Entries an older version created under a
//! looser umask are repaired lazily on the next read or write, bounded by
//! the highest directory the caller owns (`repair_root`): nothing above
//! it is ever changed. Non-Unix platforms keep the platform defaults.

use std::fs;
use std::path::Path;

/// Create `dir` and any missing ancestor, owner-only on Unix. The mode
/// applies to the directories this call creates; existing ancestors keep
/// theirs and are repaired by [`restrict_dir_chain`] instead.
pub fn create_dir_all(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)
    }
}

/// Best-effort repair of a directory chain: `dir`, then each parent up to
/// and including `repair_root`, is restricted to the owner. Nothing above
/// `repair_root` is touched, and a path outside it stops the walk.
pub fn restrict_dir_chain(repair_root: &Path, dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut current = Some(dir);
        while let Some(path) = current {
            if !path.starts_with(repair_root) {
                break;
            }
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
            if path == repair_root {
                break;
            }
            current = path.parent();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (repair_root, dir);
    }
}

/// Best-effort chmod of one file to owner-only.
pub fn restrict_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Open `path` for writing, created owner-only from the first byte and
/// repaired when a previous file (or crash leftover) was looser. The
/// caller is responsible for making the result visible atomically (temp
/// file + rename) and for syncing it before the write is acknowledged.
pub fn open(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // `mode` applies only to a fresh file; an existing one keeps its
        // old permissions, so set them explicitly.
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

/// Write `payload` to `path` using [`open`]. The caller is responsible
/// for the atomic-replacement dance, exactly as with [`open`].
pub fn write(path: &Path, payload: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = open(path)?;
    file.write_all(payload)
}

/// Read `path`, repairing the file and its parent directories to
/// owner-only on the way: this is where an entry written by an older
/// version under a looser umask is fixed, so privacy never depends on a
/// full tree sweep at startup. A failed repair never fails the read.
pub fn read(repair_root: &Path, path: &Path) -> std::io::Result<Vec<u8>> {
    let bytes = fs::read(path)?;
    restrict_file(path);
    if let Some(parent) = path.parent() {
        restrict_dir_chain(repair_root, parent);
    }
    Ok(bytes)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &Path) -> u32 {
        fs::metadata(path)
            .expect("path exists")
            .permissions()
            .mode()
            & 0o777
    }

    fn chmod(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod");
    }

    #[test]
    fn created_directories_are_owner_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("a").join("b");

        create_dir_all(&nested).expect("create");

        assert_eq!(mode(&nested), 0o700);
        assert_eq!(mode(&dir.path().join("a")), 0o700);
    }

    #[test]
    fn repair_stops_at_the_repair_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("tmail");
        let deep = root.join("cache").join("scope");
        fs::create_dir_all(&deep).expect("seed dirs");
        chmod(dir.path(), 0o755);
        chmod(&root, 0o755);
        chmod(&root.join("cache"), 0o755);
        chmod(&deep, 0o755);

        restrict_dir_chain(&root, &deep);

        assert_eq!(mode(&deep), 0o700);
        assert_eq!(mode(&root.join("cache")), 0o700);
        assert_eq!(mode(&root), 0o700);
        assert_eq!(mode(dir.path()), 0o755, "above the root is untouched");
    }

    #[test]
    fn opened_files_are_owner_only_and_replace_a_stale_mode() {
        use std::io::Write;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("entry.json");
        fs::write(&path, b"old").expect("seed");
        chmod(&path, 0o644);

        let mut file = open(&path).expect("open");
        file.write_all(b"new").expect("write");
        drop(file);

        assert_eq!(mode(&path), 0o600);
        assert_eq!(fs::read(&path).expect("read back"), b"new");
    }

    #[test]
    fn reading_repairs_the_file_and_its_chain() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("data");
        let nested = root.join("scope");
        fs::create_dir_all(&nested).expect("seed dirs");
        let path = nested.join("entry.json");
        fs::write(&path, b"payload").expect("seed");
        chmod(&path, 0o644);
        chmod(&root, 0o755);
        chmod(&nested, 0o755);

        assert_eq!(read(&root, &path).expect("read"), b"payload");

        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&nested), 0o700);
        assert_eq!(mode(&root), 0o700);
    }
}
