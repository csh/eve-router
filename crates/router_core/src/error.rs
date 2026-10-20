//! The error for a file that the router reads or writes.

use std::path::{Path, PathBuf};

/// A read, a write or a JSON error for one file. The text names the file.
#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error("{}: {source}", .path.display())]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{}: {source}", .path.display())]
    Json { path: PathBuf, source: serde_json::Error },
}

impl FileError {
    pub fn io(path: &Path) -> impl FnOnce(std::io::Error) -> FileError + '_ {
        move |source| FileError::Io { path: path.to_path_buf(), source }
    }

    pub fn json(path: &Path) -> impl FnOnce(serde_json::Error) -> FileError + '_ {
        move |source| FileError::Json { path: path.to_path_buf(), source }
    }
}

/// An error while the router downloads, derives, reads or checks the SDE files.
#[derive(Debug, thiserror::Error)]
pub enum SdeError {
    #[error(transparent)]
    File(#[from] FileError),
    /// A derived table is missing or unreadable. The text says how to get it.
    #[error("{}: {source}. Unset {} to download it.", .path.display(), crate::sde_update::SKIP_ENV)]
    Missing { path: PathBuf, source: std::io::Error },
    #[error("{name}: {source}")]
    Utf8 { name: String, source: std::str::Utf8Error },
    #[error("{name}: {source}")]
    Line { name: String, source: serde_json::Error },
    #[error("types.jsonl: no published ships")]
    NoShips,
    #[error("typeDogma.jsonl: the Ansiblex ({}) has no {}", crate::ships::ANSIBLEX_TYPE, .0)]
    NoAnsiblexAttribute(&'static str),
    #[error("typeLists.jsonl: no list named \"{0}\"")]
    NoTypeList(&'static str),
    #[error("{url}: {source}")]
    Http { url: String, source: Box<ureq::Error> },
    #[error("{url}: no \"sde\" record with a build number")]
    NoBuild { url: String },
    #[error("{url}: no Content-Length")]
    NoContentLength { url: String },
    #[error("{url}: the server ignored the range request ({status})")]
    RangeIgnored { url: String, status: u16 },
    #[error("{url}: asked for {want} bytes, got {got}")]
    ShortBody { url: String, want: u64, got: u64 },
    #[error("{0} is not in the SDE zip")]
    NotInZip(String),
    /// A fault in the structure of the zip file.
    #[error("{0}")]
    Zip(String),
    /// A fault in one entry of the zip file.
    #[error("{name}: {message}")]
    Entry { name: String, message: String },
    /// No local SDE exists, and the download failed.
    #[error("Cannot download the SDE to {}: {source}", .dir.display())]
    Download { dir: PathBuf, source: Box<SdeError> },
}

/// Write `text` to `path` through a temporary file and a rename, so a reader never sees half a
/// file. The directory of `path` is made first.
pub fn write_atomic(path: &Path, tmp: &Path, text: &str) -> Result<(), FileError> {
    write_atomic_with(path, tmp, text, false)
}

/// As `write_atomic`, and the data reaches the disk before the rename. With `private`, a Unix file
/// gets the mode 0600 before it holds any data. Other platforms ignore `private`. A failed write
/// removes the temporary file and leaves the old file as it was.
pub fn write_atomic_with(path: &Path, tmp: &Path, text: &str, private: bool) -> Result<(), FileError> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(FileError::io(dir))?;
    }
    let result =
        write_synced(tmp, text, private).map_err(FileError::io(tmp)).and_then(|()| std::fs::rename(tmp, path).map_err(FileError::io(path)));
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

/// Write `text` to a new file and flush it to the disk.
fn write_synced(path: &Path, text: &str, private: bool) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    // The mode above applies only to a new file. This call fixes a file that an earlier crash left.
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = private;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_names_the_file() {
        let err = FileError::Io { path: "a/b.json".into(), source: std::io::Error::other("denied") };
        assert_eq!(err.to_string(), "a/b.json: denied");
        let bad = serde_json::from_str::<u32>("x").unwrap_err();
        let err = FileError::Json { path: "c.json".into(), source: bad };
        assert!(err.to_string().starts_with("c.json: expected value"), "{err}");
    }

    #[test]
    fn write_atomic_makes_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("new").join("f.json");
        write_atomic(&path, &path.with_extension("part"), "{}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
        assert!(!path.with_extension("part").exists());
    }

    #[test]
    fn a_failed_write_keeps_the_old_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.json");
        std::fs::write(&path, "old").unwrap();
        let part = path.with_extension("part");
        // A directory blocks the temporary file.
        std::fs::create_dir(&part).unwrap();
        assert!(write_atomic_with(&path, &part, "new", false).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
    }

    #[cfg(unix)]
    #[test]
    fn a_private_write_has_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.json");
        write_atomic_with(&path, &path.with_extension("part"), "{}", true).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
