//! The log of the network operations: one row for each fetch of a source. The Log window shows it.

use chrono::{DateTime, Local, TimeZone};
use std::fmt::Display;

/// The most rows that a log keeps. A new row pushes the oldest row out.
pub const MAX: usize = 500;

/// The operation of a log row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogOp {
    NexumFetch,
    NexumSignatures,
    EveScoutFetch,
    EveMetroFetch,
    Refresh,
}

impl Display for LogOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LogOp::NexumFetch => "nexum.fetch",
            LogOp::NexumSignatures => "nexum.signatures",
            LogOp::EveScoutFetch => "evescout.fetch",
            LogOp::EveMetroFetch => "evemetro.fetch",
            LogOp::Refresh => "refresh",
        })
    }
}

/// What happened in an operation. The message of the row gives the details.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogEntryKind {
    /// The data loaded.
    Loaded,
    /// The cache is fresh, so the operation did not fetch.
    CacheFresh,
    /// A timeout, a network error, a 5xx status or bad JSON.
    Offline,
    /// The server refused the key.
    Auth,
    /// The map was not found.
    NotFound,
    /// EVE Metro denies access to the linked character.
    NoAccess,
    /// Nexum did not give the signatures of some systems.
    SignaturesFailed,
    /// The refresh worker stopped.
    WorkerStopped,
}

impl LogEntryKind {
    /// True for a kind that is not a problem.
    pub fn is_ok(self) -> bool {
        matches!(self, LogEntryKind::Loaded | LogEntryKind::CacheFresh)
    }
}

/// One operation and its result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    /// The time of the operation, in Unix seconds.
    pub time: u64,
    pub op: LogOp,
    pub kind: LogEntryKind,
    /// A short text, such as "Loaded 12 wormhole connections". For a failure, the cause.
    pub message: String,
}

/// Add `new` to the end of `log`. The log keeps the newest `MAX` rows.
pub fn push(log: &mut Vec<LogEntry>, new: impl IntoIterator<Item = LogEntry>) {
    log.extend(new);
    let extra = log.len().saturating_sub(MAX);
    log.drain(..extra);
}

/// The clock time of a Unix time in a time zone, with seconds, for example "14:02:09".
pub fn clock_text<Tz: TimeZone>(t: u64, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    let utc = i64::try_from(t).ok().and_then(|t| DateTime::from_timestamp(t, 0));
    utc.map_or_else(String::new, |d| d.with_timezone(tz).format("%H:%M:%S").to_string())
}

/// The clock time of a Unix time in the local time zone of the user, with seconds.
pub fn local_clock_text(t: u64) -> String {
    clock_text(t, &Local)
}

/// The message of a load: "Loaded 1 wormhole connection", "Loaded 12 wormhole connections".
pub fn loaded(count: usize) -> String {
    format!("Loaded {count} wormhole connection{}", if count == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(time: u64) -> LogEntry {
        LogEntry { time, op: LogOp::NexumFetch, kind: LogEntryKind::Loaded, message: String::new() }
    }

    #[test]
    fn an_operation_has_its_text_and_a_kind_has_its_result() {
        use crate::sources::FetchError;
        assert_eq!(LogOp::NexumFetch.to_string(), "nexum.fetch");
        assert_eq!(LogOp::NexumSignatures.to_string(), "nexum.signatures");
        assert_eq!(LogOp::EveScoutFetch.to_string(), "evescout.fetch");
        assert_eq!(LogOp::EveMetroFetch.to_string(), "evemetro.fetch");
        assert_eq!(LogOp::Refresh.to_string(), "refresh");
        assert!(LogEntryKind::Loaded.is_ok() && LogEntryKind::CacheFresh.is_ok());
        assert!(!FetchError::Auth.kind().is_ok() && !FetchError::NotFound.kind().is_ok());
        assert_eq!(FetchError::Offline("503".into()).kind(), LogEntryKind::Offline);
    }

    #[test]
    fn the_log_keeps_the_newest_rows() {
        let mut log: Vec<LogEntry> = (0..MAX as u64).map(row).collect();
        push(&mut log, [row(1000), row(1001)]);
        assert_eq!((log.len(), log[0].time, log[MAX - 1].time), (MAX, 2, 1001));
    }

    #[test]
    fn the_clock_text_has_seconds() {
        // 2026-10-05T12:00:05Z.
        assert_eq!(clock_text(1_791_201_605, &chrono::Utc), "12:00:05");
        assert_eq!(clock_text(u64::MAX, &chrono::Utc), "");
        assert_eq!(local_clock_text(1_791_201_605).len(), 8);
    }

    #[test]
    fn the_load_message_has_a_singular() {
        assert_eq!((loaded(1).as_str(), loaded(12).as_str()), ("Loaded 1 wormhole connection", "Loaded 12 wormhole connections"));
    }
}
