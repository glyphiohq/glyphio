//! Debug builds only: notice when the main thread is late, and say which command ran last.
//!
//! A command that does slow work on the main thread freezes every window and the menu bar.
//! Every command is recorded as it is invoked, and a background thread asks the main thread
//! to run a no-op every [`PERIOD`]; when it runs more than [`THRESHOLD`] late, the stall is
//! logged with the last command and a running tally of the commands that preceded stalls.
//! Release builds record nothing and start no thread.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::AppHandle;

const PERIOD: Duration = Duration::from_millis(100);
const THRESHOLD: Duration = Duration::from_millis(250);

static LAST_COMMAND: Mutex<String> = Mutex::new(String::new());
static TALLY: Mutex<Option<StallTally>> = Mutex::new(None);

/// Record a command as it is invoked. Cheap, and a no-op in release builds.
pub fn note_command(name: &str) {
    if !cfg!(debug_assertions) {
        return;
    }
    if let Ok(mut last) = LAST_COMMAND.lock() {
        last.clear();
        last.push_str(name);
    }
}

/// Start watching the main thread (debug builds only).
pub fn start(app: &AppHandle) {
    if !cfg!(debug_assertions) {
        return;
    }
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("main-thread-watchdog".into())
        .spawn(move || loop {
            let sent = Instant::now();
            let posted = app.run_on_main_thread(move || {
                if let Some(late) = lateness(sent.elapsed(), THRESHOLD) {
                    report(late);
                }
            });
            if posted.is_err() {
                return; // the app is shutting down
            }
            std::thread::sleep(PERIOD);
        });
}

fn report(late: Duration) {
    let last = LAST_COMMAND.lock().map(|s| s.clone()).unwrap_or_default();
    let mut tally = TALLY.lock().unwrap();
    let tally = tally.get_or_insert_with(StallTally::default);
    tally.record(&last);
    log::warn!(
        "main thread {} ms late; last command: {}; stalls so far: {}",
        late.as_millis(),
        if last.is_empty() { "(none)" } else { last.as_str() },
        tally.summary()
    );
}

/// How late a main-thread tick ran, if late enough to report.
pub fn lateness(waited: Duration, threshold: Duration) -> Option<Duration> {
    (waited > threshold).then_some(waited)
}

/// Stalls counted by the command that ran last before each.
#[derive(Default, Debug)]
pub struct StallTally(BTreeMap<String, u32>);

impl StallTally {
    pub fn record(&mut self, command: &str) {
        let key = if command.is_empty() { "(none)" } else { command };
        *self.0.entry(key.to_string()).or_default() += 1;
    }

    /// `command ×n` entries, most stalls first, then by name.
    pub fn summary(&self) -> String {
        let mut rows: Vec<_> = self.0.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        rows.iter()
            .map(|(name, n)| format!("{name} ×{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_tick_past_the_threshold_is_late() {
        let t = Duration::from_millis(250);
        assert_eq!(lateness(Duration::from_millis(250), t), None);
        assert_eq!(lateness(Duration::from_millis(40), t), None);
        assert_eq!(
            lateness(Duration::from_millis(900), t),
            Some(Duration::from_millis(900))
        );
    }

    #[test]
    fn the_tally_names_the_worst_offender_first() {
        let mut tally = StallTally::default();
        for c in ["save_settings", "list_captures", "save_settings", ""] {
            tally.record(c);
        }
        assert_eq!(tally.summary(), "save_settings ×2, (none) ×1, list_captures ×1");
    }
}
