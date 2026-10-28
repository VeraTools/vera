//! Per-file metadata fingerprints. Only stamps captured around a stable read
//! and outside the filesystem timestamp race window may certify stored hashes.

use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use cap_std::fs::{Dir, File};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileStamp {
    value: String,
    changed_ns: i128,
}

impl FileStamp {
    pub(crate) fn value(&self) -> &str {
        &self.value
    }

    fn trustworthy(self, now: SystemTime) -> Option<String> {
        let cutoff = now.duration_since(UNIX_EPOCH).ok()?.as_nanos() as i128 - 3_000_000_000;
        (self.changed_ns <= cutoff).then_some(self.value)
    }
}

#[cfg(unix)]
pub(crate) fn from_metadata(metadata: &cap_std::fs::Metadata) -> Option<FileStamp> {
    use cap_std::fs::MetadataExt;
    if !metadata.is_file() {
        return None;
    }
    let mtime = i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec());
    let ctime = i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec());
    Some(FileStamp {
        value: format!(
            "u1:{}:{mtime}:{ctime}:{}:{}",
            metadata.len(),
            metadata.dev(),
            metadata.ino()
        ),
        changed_ns: mtime.max(ctime),
    })
}

pub(crate) fn from_handle(file: &File) -> io::Result<Option<FileStamp>> {
    #[cfg(unix)]
    {
        Ok(from_metadata(&file.metadata()?))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FileBasicInfo, GetFileInformationByHandle,
            GetFileInformationByHandleEx,
        };

        let mut basic = FILE_BASIC_INFO::default();
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the live file owns the handle; both output buffers have the
        // exact Win32 layout and remain valid for the duration of these calls.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileBasicInfo,
                (&mut basic as *mut FILE_BASIC_INFO).cast(),
                std::mem::size_of::<FILE_BASIC_INFO>() as u32,
            ) != 0
                && GetFileInformationByHandle(file.as_raw_handle(), &mut info) != 0
        };
        if !ok || basic.ChangeTime <= 0 {
            // Filesystems without ChangeTime support must always hash.
            return Ok(None);
        }
        let len = (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow);
        let id = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
        Ok(Some(FileStamp {
            value: format!(
                "w1:{len}:{}:{}:{}:{id}",
                basic.LastWriteTime, basic.ChangeTime, info.dwVolumeSerialNumber
            ),
            // Windows timestamps are 100 ns ticks since 1601-01-01.
            changed_ns: (i128::from(basic.LastWriteTime.max(basic.ChangeTime))
                - 116_444_736_000_000_000)
                * 100,
        }))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Ok(None)
    }
}

/// Descriptor-relative lstat on Unix, handle metadata on Windows.
pub(crate) fn at(root: &Dir, path: &Path) -> io::Result<Option<FileStamp>> {
    #[cfg(unix)]
    {
        Ok(from_metadata(&root.symlink_metadata(path)?))
    }
    #[cfg(not(unix))]
    {
        from_handle(&root.open(path)?)
    }
}

pub(crate) fn read_source_with_stamp_at(
    root: &Dir,
    path: &Path,
) -> io::Result<(String, Option<String>)> {
    #[cfg(all(test, unix))]
    read_counter(root, path, true);
    read_with_stamp(&mut root.open(path)?, |_| {})
}

#[cfg(all(test, unix))]
pub(crate) fn read_counter(root: &Dir, path: &Path, increment: bool) -> usize {
    use cap_std::fs::MetadataExt;
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    type Counts = HashMap<(u64, u64, std::path::PathBuf), usize>;
    static COUNTS: OnceLock<Mutex<Counts>> = OnceLock::new();
    let metadata = root.dir_metadata().unwrap();
    let mut counts = COUNTS.get_or_init(Mutex::default).lock().unwrap();
    let count = counts
        .entry((metadata.dev(), metadata.ino(), path.to_path_buf()))
        .or_default();
    *count += usize::from(increment);
    *count
}

fn read_with_stamp(
    file: &mut File,
    mut after_read: impl FnMut(&File),
) -> io::Result<(String, Option<String>)> {
    for attempt in 0..2 {
        let before = from_handle(file)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        after_read(file);
        let after = from_handle(file)?;
        let now = SystemTime::now();
        if before == after || attempt == 1 {
            let stamp = if before == after {
                after.and_then(|stamp| stamp.trustworthy(now))
            } else {
                None
            };
            let content = String::from_utf8(bytes)
                .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned());
            return Ok((content, stamp));
        }
        file.seek(SeekFrom::Start(0))?;
    }
    unreachable!("the second read always returns")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::tempdir;

    #[test]
    fn unchanged_handle_and_path_stamps_match() {
        let root = tempdir().unwrap();
        std::fs::write(root.path().join("file"), "before").unwrap();
        let dir = crate::discovery::open_root_dir(root.path()).unwrap();
        let file = dir.open("file").unwrap();
        assert_eq!(
            from_handle(&file).unwrap(),
            at(&dir, Path::new("file")).unwrap()
        );
        assert_eq!(from_handle(&file).unwrap(), from_handle(&file).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn restored_mtime_and_atomic_replace_change_stamp() {
        let root = tempdir().unwrap();
        let path = root.path().join("file");
        std::fs::write(&path, "before").unwrap();
        let dir = crate::discovery::open_root_dir(root.path()).unwrap();
        let before = at(&dir, Path::new("file")).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(10));
        std::fs::write(&path, "after!").unwrap();
        // File::set_modified is stable since Rust 1.75.
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        let edited = at(&dir, Path::new("file")).unwrap();
        assert_ne!(before, edited);
        std::fs::write(root.path().join("replacement"), "after!").unwrap();
        std::fs::rename(root.path().join("replacement"), &path).unwrap();
        assert_ne!(edited, at(&dir, Path::new("file")).unwrap());
    }

    #[test]
    fn recent_and_future_stamps_are_not_trusted() {
        let now = UNIX_EPOCH + Duration::from_secs(10);
        for changed_ns in [8_000_000_000, 11_000_000_000] {
            assert!(
                FileStamp {
                    value: "stamp".into(),
                    changed_ns
                }
                .trustworthy(now)
                .is_none()
            );
        }
        assert!(
            FileStamp {
                value: "stamp".into(),
                changed_ns: 7_000_000_000
            }
            .trustworthy(now)
            .is_some()
        );
    }

    #[cfg(unix)]
    #[test]
    fn changes_during_both_reads_never_get_a_stamp() {
        let root = tempdir().unwrap();
        let path = root.path().join("file");
        std::fs::write(&path, "before").unwrap();
        let dir = crate::discovery::open_root_dir(root.path()).unwrap();
        let mut file = dir.open("file").unwrap();
        let mut reads = 0;
        let (_, stamp) = read_with_stamp(&mut file, |_| {
            reads += 1;
            std::fs::write(&path, format!("edit {reads}")).unwrap();
        })
        .unwrap();
        assert_eq!(reads, 2);
        assert!(stamp.is_none());
    }
}
