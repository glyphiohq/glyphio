//! Chords that belong to the operating system and cannot be dependable Glyphio bindings, as
//! plain data per platform so both tables are testable on any host.

use std::str::FromStr;

use tauri_plugin_global_shortcut::Shortcut;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    MacOs,
    Windows,
}

/// `(accelerator, reason)`; the reason completes "… is <reason>".
pub type Reserved = &'static [(&'static str, &'static str)];

const MACOS: Reserved = &[
    ("Command+Space", "reserved for Spotlight"),
    ("Command+Tab", "reserved for switching applications"),
    ("Command+Shift+3", "reserved for macOS screenshots"),
    ("Command+Shift+4", "reserved for macOS screenshots"),
    ("Command+Shift+5", "reserved for macOS screenshots and recording"),
    ("Control+Command+Q", "reserved for locking your Mac"),
    ("Alt+Command+Escape", "reserved for Force Quit"),
];

// `Command` is the Windows key here. Chords with it are mostly the system's; the common ones
// are listed, and the rest the system simply refuses at registration, which setup reports.
const WINDOWS: Reserved = &[
    ("Alt+Space", "reserved for the window menu"),
    ("Alt+Tab", "reserved for switching windows"),
    ("Alt+F4", "reserved for closing windows"),
    ("Control+Alt+Delete", "reserved for the security screen"),
    ("Control+Shift+Escape", "reserved for Task Manager"),
    ("Command+L", "reserved for locking the PC"),
    ("Command+D", "reserved for showing the desktop"),
    ("Command+Space", "reserved for switching input language"),
];

pub fn reserved_for(os: Os) -> Reserved {
    match os {
        Os::MacOs => MACOS,
        Os::Windows => WINDOWS,
    }
}

pub fn current_os() -> Os {
    if cfg!(target_os = "windows") {
        Os::Windows
    } else {
        Os::MacOs
    }
}

/// Why `shortcut` is reserved on `os`, if it is.
pub fn reason(os: Os, shortcut: &Shortcut) -> Option<&'static str> {
    reserved_for(os)
        .iter()
        .find(|(chord, _)| Shortcut::from_str(chord).is_ok_and(|reserved| &reserved == shortcut))
        .map(|(_, why)| *why)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(s: &str) -> Shortcut {
        Shortcut::from_str(s).unwrap()
    }

    #[test]
    fn every_listed_chord_parses() {
        for os in [Os::MacOs, Os::Windows] {
            for (c, _) in reserved_for(os) {
                assert!(Shortcut::from_str(c).is_ok(), "{c} on {os:?}");
            }
        }
    }

    #[test]
    fn reserved_chords_differ_per_platform() {
        assert!(reason(Os::MacOs, &chord("Alt+Space")).is_none());
        assert!(reason(Os::Windows, &chord("Alt+Space")).is_some());
        assert!(reason(Os::MacOs, &chord("Command+Space")).is_some());
        assert!(reason(Os::Windows, &chord("Control+Alt+Delete")).is_some());
    }

    #[test]
    fn no_default_shortcut_is_reserved_on_any_platform() {
        let s = crate::settings::Settings::default();
        let defaults = [
            &s.shortcut_capture_full, &s.shortcut_capture_visible, &s.shortcut_capture_snip,
            &s.shortcut_capture_front_window, &s.shortcut_capture_page, &s.shortcut_capture_scroll,
            &s.shortcut_capture_scroll_page, &s.shortcut_open_history, &s.shortcut_open_palette,
            &s.shortcut_open_clipboard,
        ];
        for os in [Os::MacOs, Os::Windows] {
            for d in defaults {
                assert!(reason(os, &chord(d)).is_none(), "{d} is reserved on {os:?}");
            }
        }
    }

    #[test]
    fn the_palette_default_is_not_the_windows_window_menu() {
        let s = crate::settings::Settings::default();
        assert_eq!(s.shortcut_open_palette, "Control+Shift+Space");
    }
}
