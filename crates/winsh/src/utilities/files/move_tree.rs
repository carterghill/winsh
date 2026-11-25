//! The rename fallback copies to a private directory on the destination volume.
//! Publish with one rename, then unlink only entries that were actually copied.
use super::{copy_link, reject_descendant};
use std::{
    collections::HashMap,
    fs::{self, File, FileTimes, Metadata},
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};
use walkdir::WalkDir;

pub(super) fn move_across(source: &Path, target: &Path) -> io::Result<()> {
    let source = source.components().as_path();
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::other("missing destination parent"))?;
    if fs::symlink_metadata(source)?.is_dir() {
        reject_descendant(source, parent)?;
    }
    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    let staging = builder.prefix(".winsh-mv-").tempdir_in(parent)?;
    let payload = staging.path().join("payload");
    let entries = stage(source, &payload)?;
    for entry in &entries {
        entry.verify(true)?;
    }
    fs::rename(&payload, target)?;
    for entry in entries.iter().rev() {
        entry.unlink().map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "destination is complete, could not remove {}: {e}",
                    entry.path.display()
                ),
            )
        })?;
    }
    Ok(())
}

struct Entry {
    path: PathBuf,
    id: (u64, u128),
    links: u64,
    meta: Metadata,
    modified: SystemTime,
    #[cfg(windows)]
    streams: Vec<(std::ffi::OsString, u64)>,
}

impl Entry {
    fn read(path: &Path) -> io::Result<Self> {
        #[cfg(windows)]
        {
            Self::from_handle(path, &windows::open_entry(path, false)?)
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = fs::symlink_metadata(path)?;
            Ok(Self {
                path: path.to_owned(),
                id: (meta.dev(), meta.ino().into()),
                links: meta.nlink(),
                modified: meta.modified()?,
                meta,
            })
        }
    }

    #[cfg(windows)]
    fn from_handle(path: &Path, file: &File) -> io::Result<Self> {
        let meta = file.metadata()?;
        let info = winapi_util::file::information(file)?;
        Ok(Self {
            path: path.to_owned(),
            id: windows::file_id(file, &info)?,
            links: info.number_of_links(),
            modified: meta.modified()?,
            streams: windows::named_streams(file)?,
            meta,
        })
    }

    fn check(&self, current: &Self, check_directory_time: bool) -> io::Result<()> {
        #[cfg(windows)]
        if self.streams != current.streams {
            return Err(io::Error::other(format!(
                "source streams changed during move: {}",
                self.path.display()
            )));
        }
        let directory = self.meta.is_dir();
        if self.id != current.id
            || self.meta.file_type() != current.meta.file_type()
            || ((!directory || check_directory_time) && self.modified != current.modified)
            || (self.meta.is_file() && self.meta.len() != current.meta.len())
        {
            return Err(io::Error::other(format!(
                "source changed during move: {}",
                self.path.display()
            )));
        }
        Ok(())
    }

    fn verify(&self, check_directory_time: bool) -> io::Result<()> {
        self.check(&Self::read(&self.path)?, check_directory_time)
    }

    fn unlink(&self) -> io::Result<()> {
        #[cfg(windows)]
        {
            // Check and remove through the same non-following handle.
            let file = windows::open_entry(&self.path, true)?;
            self.check(&Self::from_handle(&self.path, &file)?, false)?;
            windows::unlink(&file)
        }
        #[cfg(unix)]
        {
            self.verify(false)?;
            if self.meta.is_dir() {
                fs::remove_dir(&self.path)
            } else {
                fs::remove_file(&self.path)
            }
        }
    }
}

fn stage(source: &Path, payload: &Path) -> io::Result<Vec<Entry>> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut hard_links = HashMap::new();
    let mut directories = Vec::new();
    for item in WalkDir::new(source).follow_root_links(false) {
        let item = item?;
        let entry = Entry::read(item.path())?;
        let dest = if item.depth() == 0 {
            payload.to_owned()
        } else {
            if entry.id.0 != entries[0].id.0 {
                return Err(io::Error::other(format!(
                    "nested filesystem at {}",
                    item.path().display()
                )));
            }
            payload.join(
                item.path()
                    .strip_prefix(source)
                    .expect("entry under move source"),
            )
        };
        if entry.meta.is_symlink() {
            #[cfg(unix)]
            copy_link(item.path(), &dest, &entry.meta)?;
            #[cfg(windows)]
            windows::copy_reparse_link(&entry, &dest)?;
        } else {
            #[cfg(windows)]
            if windows::is_reparse_point(&entry.meta) {
                return Err(io::Error::other(format!(
                    "unsupported reparse point: {}",
                    item.path().display()
                )));
            }
            if entry.meta.is_dir() {
                fs::create_dir(&dest)?;
                #[cfg(windows)]
                windows::transfer_streams(&entry, &dest)?;
                directories.push((entries.len(), dest));
            } else if entry.meta.is_file() {
                if let Some(previous) = hard_links.get(&entry.id) {
                    fs::hard_link(previous, &dest)?;
                } else {
                    // Use the native file copy here. On NTFS it also copies
                    // alternate data streams, which a byte-stream copy loses.
                    if fs::copy(item.path(), &dest)? != entry.meta.len() {
                        return Err(io::Error::other(format!(
                            "source changed during copy: {}",
                            item.path().display()
                        )));
                    }
                    #[cfg(windows)]
                    windows::transfer_streams(&entry, &dest)?;
                    preserve_metadata(&dest, &entry.meta)?;
                    if entry.links > 1 && entry.id.1 != 0 {
                        hard_links.insert(entry.id, dest);
                    }
                }
            } else {
                return Err(io::Error::other(format!(
                    "unsupported file type: {}",
                    item.path().display()
                )));
            }
        }
        entries.push(entry);
    }
    // Creating children changes directory timestamps. Apply their metadata last.
    for (index, path) in directories.into_iter().rev() {
        preserve_metadata(&path, &entries[index].meta)?;
    }
    Ok(entries)
}

fn preserve_metadata(path: &Path, meta: &Metadata) -> io::Result<()> {
    #[cfg(windows)]
    let file = windows::open_attributes(path)?;
    #[cfg(unix)]
    let file = File::open(path)?;
    file.set_times(
        FileTimes::new()
            .set_accessed(meta.accessed()?)
            .set_modified(meta.modified()?),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The destination belongs to the caller. Never transfer setuid/setgid
        // to a file with different ownership.
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(meta.permissions().mode() & 0o1777),
        )
    }
    #[cfg(windows)]
    {
        fs::set_permissions(path, meta.permissions())
    }
}

#[cfg(windows)]
mod windows;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn fallback_publishes_files_and_complete_trees() {
        let work = tempfile::tempdir().unwrap();
        let source = work.path().join("source");
        let target = work.path().join("target");
        fs::write(&source, b"\0payload\xff").unwrap();
        fs::write(&target, b"longer original target").unwrap();
        let stamp = UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_modified(stamp)
            .unwrap();
        move_across(&source, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"\0payload\xff");
        assert_eq!(fs::metadata(&target).unwrap().modified().unwrap(), stamp);
        assert!(!source.exists());

        fs::create_dir_all(source.join("nested/empty")).unwrap();
        fs::rename(&target, source.join("nested/one")).unwrap();
        fs::hard_link(source.join("nested/one"), source.join("two")).unwrap();
        #[cfg(windows)]
        {
            fs::write(source.join("nested/one:notes"), b"extra stream").unwrap();
            fs::write(source.join("nested:notes"), b"directory stream").unwrap();
        }
        move_across(&source, &target).unwrap();
        assert!(!source.exists());
        assert!(target.join("nested/empty").is_dir());
        assert!(same_file::is_same_file(target.join("nested/one"), target.join("two")).unwrap());
        assert_eq!(fs::read(target.join("two")).unwrap(), b"\0payload\xff");
        #[cfg(windows)]
        {
            assert_eq!(fs::read(target.join("two:notes")).unwrap(), b"extra stream");
            assert_eq!(
                fs::read(target.join("nested:notes")).unwrap(),
                b"directory stream"
            );
        }
    }

    #[test]
    fn failed_publication_leaves_both_operands_intact() {
        let work = tempfile::tempdir().unwrap();
        let source = work.path().join("src");
        let target = work.path().join("dst");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(source.join("new"), b"new").unwrap();
        fs::write(target.join("old"), b"old").unwrap();
        #[cfg(windows)]
        {
            let mut permissions = fs::metadata(source.join("new")).unwrap().permissions();
            permissions.set_readonly(true);
            fs::set_permissions(source.join("new"), permissions).unwrap();
        }
        assert!(move_across(&source, &target).is_err());
        assert_eq!(fs::read(source.join("new")).unwrap(), b"new");
        assert_eq!(fs::read(target.join("old")).unwrap(), b"old");
        assert!(!target.join("new").exists());
        assert_eq!(fs::read_dir(work.path()).unwrap().count(), 2);
        assert!(move_across(&source, &source.join("child")).is_err());
        assert!(!source.join("child").exists());
    }

    #[test]
    fn cleanup_keeps_replaced_changed_and_new_source_entries() {
        let work = tempfile::tempdir().unwrap();
        let source = work.path().join("src");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("changed"), b"old").unwrap();
        fs::write(source.join("replaced"), b"old").unwrap();
        let entries = stage(&source, &work.path().join("payload")).unwrap();
        fs::write(source.join("changed"), b"new longer data").unwrap();
        fs::rename(source.join("replaced"), work.path().join("original")).unwrap();
        fs::write(source.join("replaced"), b"new").unwrap();
        fs::write(source.join("extra"), b"extra").unwrap();
        for entry in entries.iter().rev() {
            assert!(entry.unlink().is_err());
        }
        assert_eq!(fs::read(source.join("replaced")).unwrap(), b"new");
        assert_eq!(
            fs::read(source.join("changed")).unwrap(),
            b"new longer data"
        );
        assert_eq!(fs::read(source.join("extra")).unwrap(), b"extra");
    }

    #[cfg(windows)]
    #[test]
    fn fallback_recreates_junction_without_entering_target() {
        use std::process::Command;
        let work = tempfile::tempdir().unwrap();
        let reference = work.path().join("real");
        fs::create_dir(&reference).unwrap();
        fs::write(reference.join("untouched"), b"outside tree").unwrap();
        let source = work.path().join("src");
        let target = work.path().join("dst");
        fs::create_dir(&source).unwrap();
        let result = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command",
            "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:WINSH_TEST_LINK -Target $env:WINSH_TEST_TARGET | Out-Null"])
            .env("WINSH_TEST_LINK", source.join("link")).env("WINSH_TEST_TARGET", &reference).output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        move_across(&source, &target).unwrap();
        assert!(
            fs::symlink_metadata(target.join("link"))
                .unwrap()
                .is_symlink()
        );
        assert!(same_file::is_same_file(target.join("link"), &reference).unwrap());
        assert_eq!(
            fs::read(reference.join("untouched")).unwrap(),
            b"outside tree"
        );
        assert!(!source.exists());
    }

    #[cfg(windows)]
    #[test]
    fn copy_failure_preserves_previous_destination_and_source() {
        use std::os::windows::fs::OpenOptionsExt;
        let work = tempfile::tempdir().unwrap();
        let source = work.path().join("source");
        let target = work.path().join("target");
        fs::write(&source, b"new data").unwrap();
        fs::write(&target, b"old data").unwrap();
        let lock = File::options()
            .read(true)
            .share_mode(0)
            .open(&source)
            .unwrap();
        assert!(move_across(&source, &target).is_err());
        drop(lock);
        assert_eq!(fs::read(&source).unwrap(), b"new data");
        assert_eq!(fs::read(&target).unwrap(), b"old data");
        assert_eq!(fs::read_dir(work.path()).unwrap().count(), 2);
    }

    #[cfg(windows)]
    #[test]
    fn cleanup_failure_keeps_the_published_copy() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
        let work = tempfile::tempdir().unwrap();
        let source = work.path().join("source");
        let target = work.path().join("target");
        fs::write(&source, b"complete").unwrap();
        let _reader = File::options()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&source)
            .unwrap();
        let error = move_across(&source, &target).unwrap_err();
        assert!(
            error.to_string().contains("destination is complete"),
            "{error}"
        );
        assert_eq!(fs::read(&source).unwrap(), b"complete");
        assert_eq!(fs::read(&target).unwrap(), b"complete");
        assert_eq!(fs::read_dir(work.path()).unwrap().count(), 2);
    }
}
