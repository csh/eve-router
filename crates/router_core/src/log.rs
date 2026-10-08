//! The log of the network operations: one row for each fetch of a source. The Log window shows it.

use chrono::{DateTime, Local, TimeZone};
use std::fmt::Display;

/// The most rows that a log keeps. A new row pushes the oldest row out.
pub const MAX: usize = 500;

/// One operation and its result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    /// The time of the operation, in Unix seconds.
    pub time: u64,
    /// The operation, for example `nexum.fetch`.
    pub op: &'static str,
    pub ok: bool,
    /// For a success, a short result such as "12 wormholes". For a failure, the cause.
    pub reason: String,
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

/// "1 wormhole", "12 wormholes".
pub fn wormholes(count: usize) -> String {
    format!("{count} wormhole{}", if count == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(time: u64) -> LogEntry {
        LogEntry { time, op: "nexum.fetch", ok: true, reason: String::new() }
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
    fn the_count_text_has_a_singular() {
        assert_eq!((wormholes(1).as_str(), wormholes(12).as_str()), ("1 wormhole", "12 wormholes"));
    }
}
