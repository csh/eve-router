//! Helpers for the tests.

use std::fs;
use std::path::Path;

/// Compare `text` with the snapshot file `tests/snapshots/<name>.txt` of the calling crate.
/// Set `UPDATE_SNAPSHOTS=1` to write the file instead.
#[macro_export]
macro_rules! assert_snapshot {
    ($name:expr, $text:expr) => {
        $crate::test_support::check_snapshot(std::path::Path::new(env!("CARGO_MANIFEST_DIR")), $name, &$text)
    };
}

pub fn check_snapshot(manifest_dir: &Path, name: &str, text: &str) {
    let path = manifest_dir.join("tests").join("snapshots").join(format!("{name}.txt"));
    if std::env::var("UPDATE_SNAPSHOTS").as_deref() == Ok("1") {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        return;
    }
    let expected = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}. Run with UPDATE_SNAPSHOTS=1.", path.display()));
    // Git can change the line ends of the snapshot file on Windows.
    assert_eq!(text, expected.replace("\r\n", "\n"), "snapshot {} changed", path.display());
}
