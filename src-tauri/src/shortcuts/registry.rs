//! What is bound to which chord, and whether the system accepted it.
//!
//! Pure: the operating system is reached only through [`Registrar`], so the rules (minimal diff,
//! first-listed wins, silent twins, debounce) are tested with a fake.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri_plugin_global_shortcut::Shortcut;

/// The same action twice inside this window is one key press that arrived twice (key repeat, a
/// bouncing chord), not two requests.
pub const DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Capture { mode: &'static str, silent: bool },
    OpenHistory,
    OpenPalette,
    OpenClipboard,
}

/// One configured chord. `key` is the settings field it came from, so the UI can put the result
/// next to the right control. Order matters: on a duplicate chord the earlier binding wins.
#[derive(Clone, Debug)]
pub struct Binding {
    pub key: &'static str,
    pub action: Action,
    pub accelerator: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "reason", rename_all = "camelCase")]
pub enum Registration {
    Registered,
    /// Deliberately unbound.
    Empty,
    /// The accelerator string cannot be understood, or another binding holds the same chord.
    Invalid(String),
    /// The system would not give us the chord.
    Refused(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ShortcutStatus {
    pub key: &'static str,
    #[serde(flatten)]
    pub registration: Registration,
}

pub trait Registrar {
    fn register(&mut self, chord: Shortcut) -> Result<(), String>;
    fn unregister(&mut self, chord: Shortcut) -> Result<(), String>;
}

#[derive(Default)]
pub struct Registry {
    /// Chords we hold with the system, which is what a diff is taken against.
    held: HashSet<Shortcut>,
    table: HashMap<Shortcut, Action>,
    status: Vec<ShortcutStatus>,
    last_fired: HashMap<Action, Instant>,
}

impl Registry {
    /// Bring the system's registrations in line with `bindings`, touching only what differs.
    /// Chords that are still wanted are left alone; a chord the system refused last time is
    /// asked for again, since whatever held it may have let go.
    pub fn apply(
        &mut self,
        bindings: &[Binding],
        registrar: &mut impl Registrar,
    ) -> Vec<ShortcutStatus> {
        let mut wanted: Vec<(Shortcut, Action, &'static str)> = Vec::new();
        let mut status: Vec<ShortcutStatus> = Vec::new();
        for b in bindings {
            let registration = if b.accelerator.trim().is_empty() {
                Registration::Empty
            } else {
                match Shortcut::from_str(&b.accelerator) {
                    Err(e) => Registration::Invalid(format!("not a valid shortcut: {e}")),
                    Ok(chord) if wanted.iter().any(|(c, _, _)| *c == chord) => {
                        Registration::Invalid("already used by another action".into())
                    }
                    Ok(chord) => {
                        wanted.push((chord, b.action, b.key));
                        Registration::Registered // settled below, once the system has answered
                    }
                }
            };
            status.push(ShortcutStatus {
                key: b.key,
                registration,
            });
        }

        let keep: HashSet<Shortcut> = wanted.iter().map(|(c, _, _)| *c).collect();
        for chord in self.held.clone() {
            if !keep.contains(&chord) {
                if let Err(e) = registrar.unregister(chord) {
                    log::warn!("could not release a shortcut: {e}");
                }
                self.held.remove(&chord);
            }
        }

        self.table.clear();
        for (chord, action, key) in wanted {
            if !self.held.contains(&chord) {
                match registrar.register(chord) {
                    Ok(()) => {
                        self.held.insert(chord);
                    }
                    Err(reason) => {
                        log::warn!("could not register shortcut for {key}: {reason}");
                        if let Some(s) = status.iter_mut().find(|s| s.key == key) {
                            s.registration = Registration::Refused(reason);
                        }
                        continue;
                    }
                }
            }
            self.table.insert(chord, action);
        }
        self.status = status.clone();
        status
    }

    pub fn lookup(&self, chord: &Shortcut) -> Option<Action> {
        self.table.get(chord).copied()
    }

    pub fn status(&self) -> &[ShortcutStatus] {
        &self.status
    }

    /// Whether this press should do anything, recording it if so.
    pub fn should_fire(&mut self, action: Action, now: Instant) -> bool {
        if let Some(prev) = self.last_fired.get(&action) {
            if now.saturating_duration_since(*prev) < DEBOUNCE {
                return false;
            }
        }
        self.last_fired.insert(action, now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        calls: Vec<String>,
        refuse: Vec<Shortcut>,
    }

    impl Registrar for Fake {
        fn register(&mut self, chord: Shortcut) -> Result<(), String> {
            self.calls.push(format!("+{chord}"));
            if self.refuse.contains(&chord) {
                Err("taken by another app".into())
            } else {
                Ok(())
            }
        }
        fn unregister(&mut self, chord: Shortcut) -> Result<(), String> {
            self.calls.push(format!("-{chord}"));
            Ok(())
        }
    }

    fn chord(s: &str) -> Shortcut {
        Shortcut::from_str(s).unwrap()
    }

    fn b(key: &'static str, action: Action, acc: &str) -> Binding {
        Binding {
            key,
            action,
            accelerator: acc.into(),
        }
    }

    const VISIBLE: Action = Action::Capture {
        mode: "visible",
        silent: false,
    };
    const VISIBLE_SILENT: Action = Action::Capture {
        mode: "visible",
        silent: true,
    };

    #[test]
    fn saving_unchanged_bindings_makes_no_calls() {
        let bindings = [
            b("a", VISIBLE, "Alt+Shift+V"),
            b("h", Action::OpenHistory, "Alt+Shift+H"),
        ];
        let mut reg = Registry::default();
        let mut fake = Fake::default();
        reg.apply(&bindings, &mut fake);
        fake.calls.clear();

        reg.apply(&bindings, &mut fake);

        assert!(fake.calls.is_empty(), "{:?}", fake.calls);
    }

    #[test]
    fn a_change_touches_only_the_chords_involved() {
        let mut reg = Registry::default();
        let mut fake = Fake::default();
        reg.apply(
            &[
                b("a", VISIBLE, "Alt+Shift+V"),
                b("h", Action::OpenHistory, "Alt+Shift+H"),
            ],
            &mut fake,
        );
        fake.calls.clear();

        reg.apply(
            &[
                b("a", VISIBLE, "Alt+Shift+X"),
                b("h", Action::OpenHistory, "Alt+Shift+H"),
            ],
            &mut fake,
        );

        assert_eq!(
            fake.calls,
            vec![format!("-{}", chord("Alt+Shift+V")), format!("+{}", chord("Alt+Shift+X"))]
        );
        assert_eq!(reg.lookup(&chord("Alt+Shift+X")), Some(VISIBLE));
        assert_eq!(reg.lookup(&chord("Alt+Shift+V")), None);
    }

    #[test]
    fn clearing_a_binding_releases_its_chord() {
        let mut reg = Registry::default();
        let mut fake = Fake::default();
        reg.apply(&[b("a", VISIBLE, "Alt+Shift+V")], &mut fake);
        fake.calls.clear();

        let status = reg.apply(&[b("a", VISIBLE, "")], &mut fake);

        assert_eq!(fake.calls, vec![format!("-{}", chord("Alt+Shift+V"))]);
        assert_eq!(status[0].registration, Registration::Empty);
    }

    #[test]
    fn lookup_tells_a_silent_twin_from_its_editor_binding() {
        let mut reg = Registry::default();
        reg.apply(
            &[
                b("a", VISIBLE, "Alt+Shift+V"),
                b("s", VISIBLE_SILENT, "Alt+Shift+C"),
            ],
            &mut Fake::default(),
        );
        assert_eq!(reg.lookup(&chord("Alt+Shift+V")), Some(VISIBLE));
        assert_eq!(reg.lookup(&chord("Alt+Shift+C")), Some(VISIBLE_SILENT));
    }

    #[test]
    fn the_first_listed_binding_wins_a_duplicate_chord() {
        let mut reg = Registry::default();
        let mut fake = Fake::default();
        let status = reg.apply(
            &[
                b("a", VISIBLE, "Alt+Shift+V"),
                b("h", Action::OpenHistory, "Alt+Shift+V"),
            ],
            &mut fake,
        );
        assert_eq!(reg.lookup(&chord("Alt+Shift+V")), Some(VISIBLE));
        assert_eq!(status[0].registration, Registration::Registered);
        assert!(matches!(status[1].registration, Registration::Invalid(_)));
        assert_eq!(fake.calls.len(), 1);
    }

    #[test]
    fn a_refused_chord_is_reported_and_does_not_fire() {
        let mut reg = Registry::default();
        let mut fake = Fake {
            refuse: vec![chord("Alt+Shift+V")],
            ..Fake::default()
        };
        let status = reg.apply(
            &[
                b("a", VISIBLE, "Alt+Shift+V"),
                b("h", Action::OpenHistory, "Alt+Shift+H"),
            ],
            &mut fake,
        );
        assert_eq!(
            status[0].registration,
            Registration::Refused("taken by another app".into())
        );
        assert_eq!(status[1].registration, Registration::Registered);
        assert_eq!(reg.lookup(&chord("Alt+Shift+V")), None);
        assert_eq!(reg.status(), status.as_slice());
    }

    #[test]
    fn a_refused_chord_is_asked_for_again_on_the_next_apply() {
        let bindings = [b("a", VISIBLE, "Alt+Shift+V")];
        let mut reg = Registry::default();
        let mut fake = Fake {
            refuse: vec![chord("Alt+Shift+V")],
            ..Fake::default()
        };
        reg.apply(&bindings, &mut fake);
        fake.refuse.clear();

        let status = reg.apply(&bindings, &mut fake);

        assert_eq!(status[0].registration, Registration::Registered);
        assert_eq!(reg.lookup(&chord("Alt+Shift+V")), Some(VISIBLE));
    }

    #[test]
    fn an_unparseable_accelerator_is_invalid_and_costs_no_call() {
        let mut reg = Registry::default();
        let mut fake = Fake::default();
        let status = reg.apply(&[b("a", VISIBLE, "Alt+Shift+NotAKey")], &mut fake);
        assert!(matches!(status[0].registration, Registration::Invalid(_)));
        assert!(fake.calls.is_empty());
    }

    #[test]
    fn the_same_action_inside_the_debounce_window_fires_once() {
        let mut reg = Registry::default();
        let t = Instant::now();
        assert!(reg.should_fire(VISIBLE, t));
        assert!(!reg.should_fire(VISIBLE, t + Duration::from_millis(100)));
        assert!(reg.should_fire(VISIBLE_SILENT, t + Duration::from_millis(100)));
        assert!(reg.should_fire(VISIBLE, t + DEBOUNCE));
    }
}
