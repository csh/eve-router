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

/// A fault while the app looks for a new version, installs it, or restarts.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("{0}")]
    Update(#[from] self_update::Error),
}

/// Write `text` to `path` through a temporary file and a rename, so a reader never sees half a
/// file. The directory of `path` is made first.
pub fn write_atomic(path: &Path, tmp: &Path, text: &str) -> Result<(), FileError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(FileError::io(dir))?;
    }
    std::fs::write(tmp, text).map_err(FileError::io(tmp))?;
    std::fs::rename(tmp, path).map_err(FileError::io(path))
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
}
