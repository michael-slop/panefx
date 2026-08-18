//! An in-memory log the TUI can read.
//!
//! # Why this exists
//!
//! The daemon is `#![windows_subsystem = "windows"]`, so it has **no console at
//! all** and every `eprintln!` goes nowhere. That is correct for a background
//! service — a stray terminal window whose close button kills the daemon is
//! worse — but it means failures are completely silent.
//!
//! Two real bugs hid behind that in one session:
//!
//!   * `reposition failed: Invalid window handle` looped once per frame for an
//!     hour while a stranded pane sat in the middle of the screen. Nobody could
//!     see the message, so the visible symptom was "a floating pane" with no
//!     clue attached.
//!   * A wallpaper surface failed to create, and the daemon reported success to
//!     the TUI because the only evidence was an `eprintln!`.
//!
//! Both were found by restarting the daemon by hand with
//! `-RedirectStandardError`, which is not a thing a user should ever have to do.
//!
//! So: keep the last N lines in memory, ship them over the control channel the
//! TUI is already connected to, and show them in a tab. No files, no disk
//! growth, no log rotation to get wrong — and it is exactly where you already
//! are when something looks off.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

/// How many lines to keep.
///
/// Sized for the failure this exists to catch: a per-frame error at 10fps fills
/// this in under a minute, which is long enough to see the pattern and short
/// enough that the buffer never grows.
const CAPACITY: usize = 500;

/// Severity, so the TUI can colour a real problem differently from chatter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Info,
    Warn,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    /// Seconds since the daemon started. Deliberately not a wall clock: the
    /// useful question is "how long after startup", and it avoids a time crate.
    pub at: u64,
    pub level: Level,
    pub text: String,
    /// How many times this line repeated consecutively.
    ///
    /// The floating-pane bug logged the SAME line every frame. Without
    /// collapsing, 500 identical lines would push every other clue out of the
    /// buffer — the noise would erase the signal it came with.
    pub count: u32,
}

struct Buffer {
    entries: VecDeque<Entry>,
    start: std::time::Instant,
}

fn buffer() -> &'static Mutex<Buffer> {
    static BUF: OnceLock<Mutex<Buffer>> = OnceLock::new();
    BUF.get_or_init(|| {
        Mutex::new(Buffer {
            entries: VecDeque::with_capacity(CAPACITY),
            start: std::time::Instant::now(),
        })
    })
}

/// Record a line, collapsing an immediate repeat into a count.
pub fn push(level: Level, text: impl Into<String>) {
    let text = text.into();
    // Still write to stderr: `-RedirectStandardError` remains the way to debug
    // a daemon that dies before the TUI can connect.
    match level {
        Level::Info => println!("{text}"),
        Level::Warn => eprintln!("{text}"),
    }

    let Ok(mut b) = buffer().lock() else {
        return;
    };
    let at = b.start.elapsed().as_secs();

    if let Some(last) = b.entries.back_mut() {
        if last.text == text && last.level == level {
            last.count = last.count.saturating_add(1);
            last.at = at;
            return;
        }
    }
    if b.entries.len() == CAPACITY {
        b.entries.pop_front();
    }
    b.entries.push_back(Entry {
        at,
        level,
        text,
        count: 1,
    });
}

/// The most recent `n` entries, oldest first.
pub fn tail(n: usize) -> Vec<Entry> {
    let Ok(b) = buffer().lock() else {
        return Vec::new();
    };
    let skip = b.entries.len().saturating_sub(n);
    b.entries.iter().skip(skip).cloned().collect()
}

/// Log an informational line.
#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        $crate::log::push($crate::log::Level::Info, format!($($arg)*))
    };
}

/// Log a warning — something the user may need to act on.
#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        $crate::log::push($crate::log::Level::Warn, format!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_collapse_into_a_count() {
        // THE reason this buffer exists: the floating-pane bug logged one
        // identical line per frame. Without collapsing, a minute of that would
        // evict every other clue from the buffer.
        // A UNIQUE message, and no assertion on the buffer's absolute length.
        // The buffer is a process-wide static and `cargo test` runs tests in
        // parallel, so another test logging at the same moment used to make this
        // fail intermittently -- a flaky test is worse than no test, because it
        // trains you to re-run instead of read.
        let msg = "repeats_collapse_probe: Invalid window handle";
        push(Level::Warn, msg);
        push(Level::Warn, msg);
        push(Level::Warn, msg);
        let t = tail(CAPACITY);
        let mine: Vec<_> = t.iter().filter(|e| e.text == msg).collect();
        assert_eq!(mine.len(), 1, "three identical lines must collapse to one entry");
        assert_eq!(mine[0].count, 3, "and carry the repeat count");
    }

    #[test]
    fn a_different_line_starts_a_new_entry() {
        push(Level::Warn, "first distinct line");
        push(Level::Warn, "second distinct line");
        let t = tail(2);
        assert_eq!(t.len(), 2);
        assert_ne!(t[0].text, t[1].text);
    }

    #[test]
    fn the_buffer_is_bounded() {
        // A daemon that runs for weeks must not grow without limit.
        for i in 0..(CAPACITY + 50) {
            push(Level::Info, format!("line {i}"));
        }
        assert!(tail(usize::MAX).len() <= CAPACITY);
    }

    #[test]
    fn tail_returns_the_newest_lines_last() {
        push(Level::Info, "older marker line");
        push(Level::Info, "newer marker line");
        let t = tail(2);
        assert_eq!(t.last().unwrap().text, "newer marker line");
    }
}
