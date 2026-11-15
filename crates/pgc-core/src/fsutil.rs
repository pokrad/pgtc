//! Writing files safely: a temporary file next to the target, then a rename, so a crash
//! or a full disk never leaves a half-written file in place of the old one.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Who may read a newly created file (Unix; no effect on Windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// The usual permissions of a new file.
    Default,
    /// Only the owner (`0600`): files with passwords.
    Private,
}

/// The temporary file of `path`: the same name with `.tmp` added.
fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Replaces the content of `path` with `data` (the file is created if needed; its
/// folder must exist).
///
/// - An existing file keeps its permissions; a new one gets those of `access`.
/// - A symbolic link is followed: the file it points to is replaced, the link stays.
/// - Where no temporary file can be created (a folder the user may not write to),
///   the file is written in place.
pub fn atomic_write(path: &Path, data: &[u8], access: Access) -> io::Result<()> {
    let target = if path.is_symlink() {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
    } else {
        path.to_owned()
    };
    let existing = std::fs::metadata(&target).ok();
    let tmp = tmp_path(&target);

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if access == Access::Private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = access;

    let mut file = match options.open(&tmp) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            return options.open(&target)?.write_all(data);
        }
        Err(e) => return Err(e),
    };
    let written = file
        .write_all(data)
        .and_then(|()| file.sync_all())
        .and_then(|()| match &existing {
            Some(meta) => std::fs::set_permissions(&tmp, meta.permissions()),
            None => Ok(()),
        });
    drop(file);
    let done = written.and_then(|()| std::fs::rename(&tmp, &target));
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_replaces_and_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("query.sql");
        atomic_write(&path, b"one", Access::Default).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one");
        atomic_write(&path, b"two, longer", Access::Default).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two, longer");
        // a file without an extension, and one whose name ends in `.tmp`
        for name in ["pgpass", "x.tmp"] {
            let p = dir.path().join(name);
            atomic_write(&p, b"x", Access::Private).unwrap();
            assert_eq!(std::fs::read_to_string(&p).unwrap(), "x");
        }
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 3, "{names:?}");
    }

    #[test]
    fn a_missing_folder_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("f");
        assert!(atomic_write(&path, b"x", Access::Default).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn permissions_private_kept_and_links_followed() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("pgpass");
        atomic_write(&secret, b"a", Access::Private).unwrap();
        assert_eq!(mode(&secret), 0o600);
        // an existing file keeps what it has, whatever is asked for
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o640)).unwrap();
        atomic_write(&secret, b"b", Access::Private).unwrap();
        assert_eq!(mode(&secret), 0o640);

        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        atomic_write(&link, b"c", Access::Default).unwrap();
        assert!(link.is_symlink(), "the link stays a link");
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "c");
    }
}
