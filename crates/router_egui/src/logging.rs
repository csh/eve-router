//! The log file and the panic report.
//!
//! The app writes one file for each day, `eve-router.YYYY-MM-DD.log`, in the `logs` folder of the
//! cache folder. The config folder holds the API keys, so the log does not go there. A background
//! thread writes the file, so a log call never blocks the UI thread. At the start, the app deletes
//! the files that are older than 7 days. The writer keeps a number of files and not an age, and an
//! app that does not run every day would keep too much.
//!
//! By default the log holds the events at the level info and above of `router_core` and
//! `eve_router` (the name of the crate of this binary) and nothing else. That includes the line that
//! each closed span writes, with its busy time. The spans of the parts of the route engine are at the
//! level debug, so they need a filter. The environment variable `EVE_ROUTER_LOG` replaces the
//! default filter. Its syntax is `target=level,target=level`, for example
//! `EVE_ROUTER_LOG=router_core=debug,ureq=debug`. A set variable also lets the `log` records of the
//! dependencies in.

use router_core::config::AppPaths;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tracing::{Dispatch, Level};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;

/// The environment variable that replaces the default filter.
pub const ENV: &str = "EVE_ROUTER_LOG";
/// The age at which the app deletes a log file.
const KEEP: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const PREFIX: &str = "eve-router";
const SUFFIX: &str = "log";

/// The running log. Keep the value alive until the program ends. Its drop flushes the file.
pub struct Logging {
    _guard: WorkerGuard,
}

/// The folder of the log files.
pub fn log_dir(paths: &AppPaths) -> PathBuf {
    paths.cache.join("logs")
}

/// Info, warnings and errors of the two crates of this workspace. A target that the filter does not name is off.
fn default_filter() -> Targets {
    Targets::new().with_target("router_core", Level::WARN).with_target("eve_router", Level::WARN)
}

/// The filter for the text of `EVE_ROUTER_LOG`, or the default filter when there is no text. A text
/// that is not valid gives the default filter and a note that says why.
fn filter_from(env: Option<&str>) -> (Targets, Option<String>) {
    let Some(text) = env else { return (default_filter(), None) };
    match text.parse::<Targets>() {
        Ok(filter) => (filter, None),
        Err(e) => (default_filter(), Some(format!("{ENV} is not valid ({e}). The default filter is in use."))),
    }
}

/// Delete the log files in `dir` that `now` finds older than `keep`. The result is the count of
/// deleted files. A file with another name stays. One file that cannot be deleted does not stop the rest.
fn prune_logs(dir: &Path, now: SystemTime, keep: Duration) -> std::io::Result<usize> {
    let Some(limit) = now.checked_sub(keep) else { return Ok(0) };
    let mut deleted = 0;
    for entry in std::fs::read_dir(dir)?.flatten() {
        let name = entry.file_name();
        let is_log = name.to_str().is_some_and(|n| n.starts_with("eve-router.") && n.ends_with(".log"));
        let old = entry.metadata().and_then(|m| m.modified()).is_ok_and(|time| time < limit);
        if is_log && old && std::fs::remove_file(entry.path()).is_ok() {
            deleted += 1;
        }
    }
    Ok(deleted)
}

/// The subscriber that writes the daily file in `dir`. With `console`, it also prints to stderr.
/// The guard flushes the file when it drops.
fn build(dir: &Path, filter: Targets, console: bool) -> std::io::Result<(Dispatch, WorkerGuard)> {
    let appender = Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix(PREFIX)
        .filename_suffix(SUFFIX)
        .build(dir)
        .map_err(std::io::Error::other)?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    // A span writes one line when it closes. The line holds the busy and idle time of the span.
    let spans = fmt::format::FmtSpan::CLOSE;
    let file = fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_thread_names(true)
        .with_span_events(spans.clone())
        .with_filter(filter.clone());
    let console = console.then(|| fmt::layer().with_writer(std::io::stderr).with_span_events(spans).with_filter(filter));
    let subscriber = tracing_subscriber::registry().with(file).with(console);
    Ok((Dispatch::new(subscriber), guard))
}

/// A panic hook that logs the message, the thread, the location and a backtrace as an error, and
/// then calls the hook that was set before. The hook logs to `dispatch`, so it works without a
/// global subscriber.
fn panic_hook(
    dispatch: Dispatch,
    previous: Box<dyn Fn(&PanicHookInfo<'_>) + Send + Sync>,
) -> impl Fn(&PanicHookInfo<'_>) + Send + Sync + 'static {
    move |info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|text| (*text).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "no message".into());
        let location = info.location().map_or_else(|| "unknown".to_string(), |at| format!("{}:{}", at.file(), at.line()));
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::dispatcher::with_default(&dispatch, || {
            tracing::error!(target: "eve_router::panic", thread = %thread, location = %location, "panic: {message}\n{backtrace}");
        });
        previous(info);
    }
}

/// Start the log: make the folder, delete the old files, set the global subscriber, and set the
/// panic hook. The result is `None` when the folder cannot be made or the subscriber cannot start.
/// The app then runs with no log.
pub fn init(paths: &AppPaths) -> Option<Logging> {
    let dir = log_dir(paths);
    std::fs::create_dir_all(&dir).ok()?;
    let _ = prune_logs(&dir, SystemTime::now(), KEEP);
    let env = std::env::var(ENV).ok().filter(|text| !text.trim().is_empty());
    let (filter, note) = filter_from(env.as_deref());
    let (dispatch, guard) = build(&dir, filter, cfg!(debug_assertions)).ok()?;
    tracing::dispatcher::set_global_default(dispatch.clone()).ok()?;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(panic_hook(dispatch, previous)));
    if env.is_some() {
        // The user asked for a filter, so the `log` records of the dependencies come in too.
        let _ = tracing_log::LogTracer::init();
    }
    if let Some(note) = note {
        tracing::warn!("{note}");
    }
    Some(Logging { _guard: guard })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_logs_go_and_new_logs_and_other_files_stay() {
        let tmp = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let make = |name: &str, age_days: u64| {
            let file = std::fs::File::create(tmp.path().join(name)).unwrap();
            file.set_modified(now - Duration::from_secs(age_days * 86_400)).unwrap();
        };
        make("eve-router.2026-10-01.log", 19);
        make("eve-router.2026-10-14.log", 6);
        make("eve-router.2026-10-20.log", 0);
        make("notes.txt", 30);
        assert_eq!(prune_logs(tmp.path(), now, Duration::from_secs(7 * 86_400)).unwrap(), 1);
        let mut left: Vec<String> = std::fs::read_dir(tmp.path()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, ["eve-router.2026-10-14.log", "eve-router.2026-10-20.log", "notes.txt"]);
    }

    #[test]
    fn a_panic_is_logged_with_its_thread_and_message() {
        let tmp = tempfile::tempdir().unwrap();
        let (dispatch, guard) = build(tmp.path(), filter_from(None).0, false).unwrap();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(panic_hook(dispatch.clone(), Box::new(|_| {}))));
        let _ = std::thread::Builder::new().name("worker-x".into()).spawn(|| panic!("boom")).unwrap().join();
        // The hook is global, so put the old one back.
        let _ = std::panic::take_hook();
        std::panic::set_hook(previous);
        drop(dispatch);
        drop(guard);
        let file = std::fs::read_dir(tmp.path()).unwrap().next().unwrap().unwrap().path();
        let text = std::fs::read_to_string(file).unwrap();
        assert!(text.contains("boom") && text.contains("worker-x"), "{text}");
    }
}
