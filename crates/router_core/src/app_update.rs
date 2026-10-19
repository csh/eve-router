//! Look for a new release on GitHub, install it, and restart into it.
//!
//! `check` reads the full release list, so it also finds a pre-release. The `/releases/latest`
//! endpoint hides a pre-release, and every `v0.*` release is one. `install` pins the tag that
//! `check` found, so the user gets the release that the screen showed.

use crate::error::UpdateError;
use self_update::backends::github::{Update, UpdateBuilder};
use self_update::version::cmp_versions;
use std::cmp::Ordering;
use std::convert::Infallible;

const OWNER: &str = "csh";
const REPO: &str = "eve-router";
const BIN: &str = "eve-router-egui";
/// The release asset with the checksum of every other asset.
const SUMS: &str = "SHA256SUMS";

/// Set this variable to `1` to skip the check at the start.
pub const SKIP_ENV: &str = "EVE_ROUTER_SKIP_UPDATE_CHECK";
/// The argument of a process that `restart` starts. That process does not look for an update, so a
/// release with a wrong version number cannot cause a loop of restarts.
pub const UPDATED_ARG: &str = "--updated";

/// A release that is newer than the running version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Available {
    /// The Git tag, for example `v0.4.0`.
    pub tag: String,
    /// The version, for example `0.4.0`.
    pub version: String,
    /// The text of the release.
    pub notes: Option<String>,
}

/// Whether `restart` started this process.
pub fn just_updated() -> bool {
    std::env::args().any(|arg| arg == UPDATED_ARG)
}

fn builder(api_base: Option<&str>) -> UpdateBuilder {
    let mut builder = Update::configure();
    builder
        .repo_owner(OWNER)
        .repo_name(REPO)
        .bin_name(BIN)
        .current_version(env!("CARGO_PKG_VERSION"))
        // The archive holds one folder, named like the archive. `{{ version }}` has no `v`.
        .bin_path_in_archive("eve-router-v{{ version }}-{{ target }}/{{ bin }}")
        // No question on the console, and no output.
        .unattended()
        .show_download_progress(false);
    if let Some(url) = api_base {
        builder.api_base_url(url);
    }
    builder
}

/// The newest release that is newer than the running version, or `None`.
pub fn check() -> Result<Option<Available>, UpdateError> {
    check_at(None)
}

fn check_at(api_base: Option<&str>) -> Result<Option<Available>, UpdateError> {
    let releases = builder(api_base).build()?.get_newer_releases()?;
    let newest = releases.into_vec().into_iter().max_by(|a, b| cmp_versions(a.version(), b.version()).unwrap_or(Ordering::Equal));
    Ok(newest.map(|release| Available {
        tag: format!("v{}", release.version()),
        version: release.version().to_owned(),
        notes: release.body().map(str::to_owned),
    }))
}

/// Download the release, check it against `SHA256SUMS`, and replace the running file.
/// `on_progress` gets the bytes so far and the total, if the server sends it.
pub fn install(available: &Available, on_progress: impl Fn(u64, Option<u64>) + Send + Sync + 'static) -> Result<(), UpdateError> {
    let mut builder = builder(None);
    builder.release_tag(&available.tag).checksum_from_asset(SUMS).progress_callback(on_progress);
    builder.build()?.update()?;
    Ok(())
}

/// Start the new file with `UPDATED_ARG`, then end this process. It returns only if the start fails.
pub fn restart() -> Result<Infallible, UpdateError> {
    Ok(self_update::restart::restart_with([UPDATED_ARG])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::serve;
    use std::time::Duration;

    fn release(tag: &str, prerelease: bool) -> String {
        format!(
            r#"{{"tag_name":"{tag}","name":"{tag}","body":"notes of {tag}","created_at":"2026-10-01T00:00:00Z",
            "prerelease":{prerelease},"html_url":"https://example.invalid/{tag}","assets":[]}}"#
        )
    }

    fn list(releases: &[String]) -> String {
        format!("[{}]", releases.join(","))
    }

    /// Every release of the project is a pre-release, and the check must still find it. The
    /// newest by version wins, whatever the order of the list.
    #[test]
    fn the_check_finds_the_newest_pre_release() {
        let current = env!("CARGO_PKG_VERSION");
        let body = list(&[release("v0.0.1", true), release("v99.1.0", true), release("v99.0.0", true), release("nightly", true)]);
        let (url, _requests) = serve("200 OK", &body, Duration::ZERO);
        let found = check_at(Some(&url)).unwrap().unwrap();
        assert_eq!(found, Available { tag: "v99.1.0".into(), version: "99.1.0".into(), notes: Some("notes of v99.1.0".into()) });
        assert_ne!(found.version, current);
    }

    #[test]
    fn an_old_release_is_not_an_update() {
        let (url, _requests) =
            serve("200 OK", &list(&[release("v0.0.1", false), release(&format!("v{}", env!("CARGO_PKG_VERSION")), true)]), Duration::ZERO);
        assert_eq!(check_at(Some(&url)).unwrap(), None);
    }

    #[test]
    fn a_server_fault_is_an_error() {
        let (url, _requests) = serve("500 Internal Server Error", "{}", Duration::ZERO);
        assert!(check_at(Some(&url)).is_err());
    }
}
