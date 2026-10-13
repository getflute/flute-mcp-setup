//! Timestamped backups and atomic replacement of config files.

use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

/// Highest `-N` suffix tried for a backup name before giving up.
pub const MAX_BACKUP_SUFFIX: u32 = 1000;

/// The first free name of `<file>.bak-<stamp>`, `<file>.bak-<stamp>-1`, …
/// `<file>.bak-<stamp>-MAX_BACKUP_SUFFIX`. A name is free only if nothing —
/// not even a dangling symlink — exists there. `None` when none is free.
pub fn backup_path(target: &Path, stamp: &str) -> Option<PathBuf> {
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let is_free =
        |p: &PathBuf| std::fs::symlink_metadata(p).is_err_and(|e| e.kind() == ErrorKind::NotFound);
    std::iter::once(target.with_file_name(format!("{name}.bak-{stamp}")))
        .chain(
            (1..=MAX_BACKUP_SUFFIX)
                .map(|n| target.with_file_name(format!("{name}.bak-{stamp}-{n}"))),
        )
        .find(is_free)
}

/// Copy `target` next to itself. `None` when there is nothing to back up.
pub fn backup(target: &Path, stamp: &str) -> std::io::Result<Option<PathBuf>> {
    match std::fs::symlink_metadata(target) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
        Ok(_) => {}
    }

    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let no_free_name = || {
        std::io::Error::new(
            ErrorKind::AlreadyExists,
            format!("no free backup name next to {}", target.display()),
        )
    };
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    std::fs::copy(target, temp.path())?;
    let mut dest = backup_path(target, stamp).ok_or_else(no_free_name)?;
    loop {
        match temp.persist_noclobber(&dest) {
            Ok(_) => return Ok(Some(dest)),
            Err(e) if e.error.kind() == ErrorKind::AlreadyExists => {
                temp = e.file;
                dest = backup_path(target, stamp).ok_or_else(no_free_name)?;
            }
            Err(e) => return Err(e.error),
        }
    }
}

/// Replace `target` with `contents` via a temp file + rename in the same
/// directory. A symlinked target is written through to the file it points at.
/// On Unix an existing file keeps its permissions; a new file gets 0600.
pub fn write_atomic(target: &Path, contents: &str) -> std::io::Result<()> {
    let target = match std::fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::canonicalize(target)?,
        _ => target.to_path_buf(),
    };
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(contents.as_bytes())?;
    if let Ok(meta) = std::fs::metadata(&target) {
        tmp.as_file().set_permissions(meta.permissions())?;
    }
    tmp.as_file().sync_all()?;
    tmp.persist(&target).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_copies_existing_file_with_stamp() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("config.toml");
        std::fs::write(&file, "original").unwrap();
        let backup = backup(&file, "20261013-101500").unwrap().unwrap();
        assert_eq!(backup, tmp.path().join("config.toml.bak-20261013-101500"));
        assert_eq!(std::fs::read_to_string(backup).unwrap(), "original");
    }

    #[test]
    fn backup_of_missing_file_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(backup(&tmp.path().join("nope.json"), "x").unwrap(), None);
    }

    #[test]
    fn backup_never_overwrites_an_earlier_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("c.json");
        std::fs::write(&file, "one").unwrap();
        let first = backup(&file, "s").unwrap().unwrap();
        std::fs::write(&file, "two").unwrap();
        let second = backup(&file, "s").unwrap().unwrap();
        assert_eq!(second, tmp.path().join("c.json.bak-s-1"));
        assert_eq!(std::fs::read_to_string(first).unwrap(), "one");
        assert_eq!(std::fs::read_to_string(second).unwrap(), "two");
    }

    #[cfg(unix)]
    #[test]
    fn backup_does_not_follow_a_dangling_symlink_at_the_backup_name() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("c.json");
        std::fs::write(&file, "original").unwrap();
        let occupied = backup_path(&file, "s").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("missing"), &occupied).unwrap();

        let saved = backup(&file, "s").unwrap().unwrap();

        assert_eq!(saved, tmp.path().join("c.json.bak-s-1"));
        assert!(
            std::fs::symlink_metadata(&occupied)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(saved).unwrap(), "original");
        assert!(!tmp.path().join("missing").exists());
    }

    #[test]
    fn backup_gives_up_instead_of_searching_forever() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("c.json");
        std::fs::write(&file, "original").unwrap();
        std::fs::write(tmp.path().join("c.json.bak-s"), "").unwrap();
        for n in 1..=MAX_BACKUP_SUFFIX {
            std::fs::write(tmp.path().join(format!("c.json.bak-s-{n}")), "").unwrap();
        }

        let err = backup(&file, "s").unwrap_err();

        assert_eq!(err.kind(), ErrorKind::AlreadyExists);
        let next = tmp
            .path()
            .join(format!("c.json.bak-s-{}", MAX_BACKUP_SUFFIX + 1));
        assert!(!next.exists());
    }

    #[test]
    fn write_atomic_creates_parent_dirs_and_replaces_content() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("new/dir/config.toml");
        write_atomic(&file, "a").unwrap();
        write_atomic(&file, "b").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "b");
        let leftovers = std::fs::read_dir(file.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 1, "temp files must not be left behind");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_keeps_permissions_and_new_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let existing = tmp.path().join("existing.json");
        std::fs::write(&existing, "{}").unwrap();
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_atomic(&existing, "{ }").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&existing), 0o644);
        let fresh = tmp.path().join("fresh.json");
        write_atomic(&fresh, "{}").unwrap();
        assert_eq!(mode(&fresh), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_writes_through_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("dotfiles/config.toml");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, "old").unwrap();
        let link = tmp.path().join("config.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, "new").unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
    }
}
