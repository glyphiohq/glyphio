//! Per-capture timing: one log line of stage milliseconds and image dimensions.
//!
//! Measurement only (PERF-01). A line carries a mode name taken from [`CaptureMode`], stage
//! names from the closed [`Stage`] enum, and numbers. There is no free-text input to the
//! formatter, so a window title, URL or clipboard content cannot reach the log through it.
//!
//! Captures are serialised by the lifecycle, so one global slot holds the capture in flight;
//! `begin` replaces any trace a failed capture left behind.

use std::sync::Mutex;
use std::time::Instant;

use super::lifecycle::CaptureMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Pixels acquired. For picker and scrolling modes this includes the user's selection time.
    Acquire,
    /// PNG encoding.
    Encode,
    /// History row, PNG and thumbnail written natively (absent on the legacy path).
    Persist,
    /// Hand-off to the editor or silent worker (pending state stored, window asked to open).
    Present,
}

impl Stage {
    pub const fn name(self) -> &'static str {
        match self {
            Stage::Acquire => "acquire",
            Stage::Encode => "encode",
            Stage::Persist => "persist",
            Stage::Present => "present",
        }
    }
}

/// `capture-trace mode=visible acquire=120ms encode=88ms present=4ms total=212ms size=5120x2880`
pub fn format_line(
    mode: CaptureMode,
    stages: &[(Stage, u128)],
    total_ms: u128,
    size: Option<(u32, u32)>,
) -> String {
    let mut line = format!("capture-trace mode={}", mode.as_str());
    for (stage, ms) in stages {
        line.push_str(&format!(" {}={}ms", stage.name(), ms));
    }
    line.push_str(&format!(" total={total_ms}ms"));
    if let Some((w, h)) = size {
        line.push_str(&format!(" size={w}x{h}"));
    }
    line
}

struct Active {
    mode: CaptureMode,
    started: Instant,
    last: Instant,
    stages: Vec<(Stage, u128)>,
    size: Option<(u32, u32)>,
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

fn with_active(f: impl FnOnce(&mut Active)) {
    if let Ok(mut slot) = ACTIVE.lock() {
        if let Some(a) = slot.as_mut() {
            f(a);
        }
    }
}

/// Start timing a capture (call when the shortcut or palette request is accepted).
pub fn begin(mode: CaptureMode) {
    let now = Instant::now();
    if let Ok(mut slot) = ACTIVE.lock() {
        *slot = Some(Active { mode, started: now, last: now, stages: Vec::new(), size: None });
    }
}

/// Record that `stage` just finished; its duration runs from the previous mark.
pub fn mark(stage: Stage) {
    with_active(|a| {
        let now = Instant::now();
        a.stages.push((stage, now.duration_since(a.last).as_millis()));
        a.last = now;
    });
}

/// Like [`mark`], but only if `stage` has not been recorded for this capture yet.
pub fn mark_if_missing(stage: Stage) {
    let missing = ACTIVE
        .lock()
        .ok()
        .and_then(|s| s.as_ref().map(|a| !a.stages.iter().any(|(s, _)| *s == stage)))
        .unwrap_or(false);
    if missing {
        mark(stage);
    }
}

pub fn set_size(width: u32, height: u32) {
    with_active(|a| a.size = Some((width, height)));
}

/// Log the line for the capture in flight and clear it. No-op if none is being timed.
pub fn finish() {
    let done = ACTIVE.lock().ok().and_then(|mut slot| slot.take());
    if let Some(a) = done {
        let total = a.started.elapsed().as_millis();
        log::info!("{}", format_line(a.mode, &a.stages, total, a.size));
    }
}

/// Drop the trace of a capture that failed or was cancelled; its timings would mislead.
pub fn discard() {
    if let Ok(mut slot) = ACTIVE.lock() {
        *slot = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_has_only_a_mode_name_stage_names_and_numbers() {
        let line = format_line(
            CaptureMode::Visible,
            &[(Stage::Acquire, 120), (Stage::Encode, 88), (Stage::Present, 4)],
            212,
            Some((5120, 2880)),
        );
        assert_eq!(
            line,
            "capture-trace mode=visible acquire=120ms encode=88ms present=4ms total=212ms size=5120x2880"
        );
    }

    #[test]
    fn every_field_value_is_numeric_except_the_mode_name() {
        for mode in [CaptureMode::Visible, CaptureMode::Snip, CaptureMode::ScrollingPage] {
            let line = format_line(mode, &[(Stage::Acquire, 1)], 2, Some((3, 4)));
            let mut fields = line.split(' ');
            assert_eq!(fields.next(), Some("capture-trace"));
            for field in fields {
                let (key, value) = field.split_once('=').expect("key=value");
                if key == "mode" {
                    assert_eq!(value, mode.as_str());
                } else if key == "size" {
                    let (w, h) = value.split_once('x').unwrap();
                    assert!(w.parse::<u32>().is_ok() && h.parse::<u32>().is_ok());
                } else {
                    assert!(value.strip_suffix("ms").unwrap().parse::<u128>().is_ok(), "{field}");
                }
            }
        }
    }

    #[test]
    fn stage_durations_run_between_marks_and_finish_clears() {
        begin(CaptureMode::Visible);
        mark(Stage::Acquire);
        set_size(10, 20);
        mark(Stage::Encode);
        let snapshot = ACTIVE.lock().unwrap().as_ref().map(|a| (a.stages.len(), a.size));
        assert_eq!(snapshot, Some((2, Some((10, 20)))));
        finish();
        assert!(ACTIVE.lock().unwrap().is_none());
    }
}
