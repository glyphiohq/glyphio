//! The application log: `~/Library/Logs/Glyphio/glyphio.log`, plus stderr.
//!
//! A Finder-launched app's stderr goes nowhere, which once made capture failures
//! undiagnosable — the file is the record. That makes it a long-lived artifact on disk, so it
//! is treated as one: capped in size, owner-readable only, and deliberately boring.
//!
//! # What must never be logged
//!
//! The log is a plain file that survives reboots and gets pasted into bug reports. Everything
//! Glyphio touches that is worth stealing is therefore banned from it:
//!
//! * **Page identity** — URLs, page titles, browser profile names. `capture::ax::BrowserMeta`
//!   exists to put these on an image the user is looking at, not into a file they forget about.
//!   A URL in a log is a browsing history someone else can read.
//! * **Snippet content** — triggers and replacements are the text the user types all day.
//! * **Credentials** — sync tokens, invite codes, OIDC responses. `sync` keeps these out of
//!   config files and the database too; the log is the same class of place.
//! * **Clipboard contents**, for the same reason.
//!
//! Log the *shape* of a failure, never its payload: `"page capture failed: {e}"`, not the page.
//! [`redact`] is the backstop for text arriving from outside this crate (the engine sidecar's
//! stderr, mainly) where we don't control the phrasing.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Keep the log useful for a long debugging session, but never let it become a disk problem.
/// Two files means the rollover can't hide the failure that caused it.
const MAX_BYTES: u64 = 4 * 1024 * 1024;
/// Longest single line written. Anything longer is a runaway from a subprocess, not a message.
const MAX_LINE: usize = 2_000;

pub fn init() {
    let sink = log_path().and_then(|path| Sink::open(path).ok()).map(std::sync::Mutex::new);
    let logger = Box::leak(Box::new(Logger { sink }));
    let _ = log::set_logger(logger).map(|()| log::set_max_level(log::LevelFilter::Info));
}

fn log_path() -> Option<PathBuf> {
    let dir = dirs::home_dir()?.join("Library/Logs/Glyphio");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("glyphio.log"))
}

/// Strip the things that must never reach the log from a line we did not write ourselves.
///
/// This is a backstop, not a licence: our own call sites are expected not to pass secrets in
/// the first place. It exists because the engine sidecar's stderr is piped through verbatim,
/// and a future espanso — or one someone ran with `-v` — may say more than today's does.
pub fn redact(line: &str) -> std::borrow::Cow<'_, str> {
    if !line.contains("://") {
        return std::borrow::Cow::Borrowed(line);
    }
    // A URL identifies a page the user was on. Keep the scheme and host so "couldn't reach
    // sync.example.com" still diagnoses itself; drop the path, query and fragment, which is
    // where the identifying part and any token live.
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find("://") {
        let after = at + 3;
        let host_end = rest[after..].find(['/', '?', '#']).map_or(rest.len(), |i| after + i);
        let tail_end =
            rest[host_end..].find(char::is_whitespace).map_or(rest.len(), |i| host_end + i);
        out.push_str(&rest[..host_end]);
        if tail_end > host_end {
            out.push_str("/…");
        }
        rest = &rest[tail_end..];
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

struct Logger {
    sink: Option<std::sync::Mutex<Sink>>,
}

impl log::Log for Logger {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        let body = record.args().to_string();
        let body = truncate(&body);
        let line = format!(
            "{} [{}] {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            record.level(),
            body
        );
        eprintln!("{line}");
        if let Some(sink) = &self.sink {
            if let Ok(mut sink) = sink.lock() {
                sink.write_line(&line);
            }
        }
    }

    fn flush(&self) {}
}

fn truncate(body: &str) -> std::borrow::Cow<'_, str> {
    if body.len() <= MAX_LINE {
        return std::borrow::Cow::Borrowed(body);
    }
    let mut end = MAX_LINE;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    std::borrow::Cow::Owned(format!("{}… [{} bytes truncated]", &body[..end], body.len() - end))
}

/// The log file and everything needed to roll it over.
struct Sink {
    path: PathBuf,
    file: std::fs::File,
    written: u64,
}

impl Sink {
    fn open(path: PathBuf) -> std::io::Result<Self> {
        let file = open_private(&path)?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self { path, file, written })
    }

    fn write_line(&mut self, line: &str) {
        if self.written + line.len() as u64 + 1 > MAX_BYTES {
            self.rotate();
        }
        if writeln!(self.file, "{line}").is_ok() {
            self.written += line.len() as u64 + 1;
        }
    }

    /// Move the full log aside and start a fresh one, so at most two files exist and the older
    /// one is complete rather than truncated mid-incident.
    fn rotate(&mut self) {
        let previous = self.path.with_extension("log.1");
        if std::fs::rename(&self.path, &previous).is_err() {
            return; // keep appending rather than lose the log entirely
        }
        match open_private(&self.path) {
            Ok(file) => {
                self.file = file;
                self.written = 0;
            }
            Err(_) => {
                let _ = std::fs::rename(&previous, &self.path);
            }
        }
    }
}

/// Open for append, owner-only. On a shared Mac the default 0644 would let any other local
/// account read whatever the log happens to contain.
fn open_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts.open(path)?;
    // `mode` only applies at creation; fix up a file an older build made world-readable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A URL in the log is a browsing history anyone with the file can read. Keep enough to
    /// diagnose a connection ("which host?"), drop the part that identifies the page.
    #[test]
    fn a_url_keeps_its_host_and_loses_its_path() {
        assert_eq!(
            redact("GET https://mail.example.com/u/0/inbox?token=sec3t failed"),
            "GET https://mail.example.com/… failed"
        );
        assert_eq!(redact("connecting to https://sync.example.com"), "connecting to https://sync.example.com");
        // An invite link carries a joinable credential in its query string.
        assert_eq!(
            redact("glyphio://join?server=https://s.example.com&token=abc"),
            "glyphio://join/…"
        );
    }

    #[test]
    fn ordinary_lines_are_left_exactly_as_they_are() {
        for line in ["engine daemon started", "capture (snip) failed: user cancelled", ""] {
            assert_eq!(redact(line), line);
        }
    }

    /// A subprocess that dumps a megabyte to stderr must not put a megabyte in the log.
    #[test]
    fn a_runaway_line_is_cut_short() {
        let huge = "x".repeat(MAX_LINE * 3);
        let cut = truncate(&huge);
        assert!(cut.len() < MAX_LINE + 64, "got {} bytes", cut.len());
        assert!(cut.ends_with("bytes truncated]"));
        assert_eq!(truncate("short"), "short");
    }

    /// Rotation keeps two files: the live one, and the complete previous one.
    #[test]
    fn the_log_rolls_over_instead_of_growing_without_end() {
        let dir = std::env::temp_dir().join(format!("glyphio-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("glyphio.log");
        let mut sink = Sink::open(path.clone()).unwrap();

        let line = "y".repeat(1024);
        for _ in 0..(MAX_BYTES / 1024 + 8) {
            sink.write_line(&line);
        }

        assert!(path.exists(), "a fresh log is open after rotating");
        assert!(path.with_extension("log.1").exists(), "the previous log is kept whole");
        assert!(std::fs::metadata(&path).unwrap().len() < MAX_BYTES);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "no group/other access");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
