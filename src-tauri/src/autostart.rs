//! The login item: "start Glyphio when I log in".
//!
//! On macOS this is `SMAppService.mainApp`, the system's own login-item service: it shows up in
//! System Settings → General → Login Items, needs no Automation permission, runs no script and
//! writes no LaunchAgent plist (which EDR products read as persistence malware). The decisions
//! — when to register, what to do about a pending approval, which old items to clean up — are
//! plain functions over a [`LoginItem`], so they are tested with a fake and the macOS binding is
//! only the last few lines.
//!
//! The item is changed only when the user changes the setting. Launch reads its status and
//! registers once if the setting is on and nothing is registered; it never retries a refusal.

use std::path::Path;

/// What the system says about the login item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Enabled,
    NotRegistered,
    /// Registered, but the user has to switch it on in System Settings → Login Items.
    RequiresApproval,
    /// The system cannot find this app as something it can register (running from a disk
    /// image or a build folder, say).
    NotFound,
}

pub trait LoginItem {
    fn status(&self) -> Status;
    fn register(&mut self) -> Result<(), String>;
    fn unregister(&mut self) -> Result<(), String>;
}

/// What happened, for the caller to log and for Settings to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing needed doing.
    Unchanged,
    Registered,
    Unregistered,
    /// The user must approve Glyphio in System Settings → General → Login Items.
    NeedsApproval,
    /// The system would not do it, with its own words. Shown as is; never retried silently.
    Failed(String),
}

/// The user changed the setting: make the item match.
pub fn apply_setting(item: &mut impl LoginItem, want: bool) -> Outcome {
    match (want, item.status()) {
        (true, Status::Enabled) | (false, Status::NotRegistered | Status::NotFound) => {
            Outcome::Unchanged
        }
        (true, Status::RequiresApproval) => Outcome::NeedsApproval,
        (true, _) => match item.register() {
            Ok(()) if item.status() == Status::RequiresApproval => Outcome::NeedsApproval,
            Ok(()) => Outcome::Registered,
            Err(e) => Outcome::Failed(e),
        },
        (false, _) => match item.unregister() {
            Ok(()) => Outcome::Unregistered,
            Err(e) => Outcome::Failed(e),
        },
    }
}

/// At launch: read the status and, only if the setting is on and nothing is registered, register
/// once. A pending approval is reported, not retried, and an item that is on while the setting
/// is off is left alone: only the user's toggle removes it.
pub fn reconcile_at_launch(item: &mut impl LoginItem, want: bool) -> Outcome {
    if !want {
        return Outcome::Unchanged;
    }
    match item.status() {
        Status::Enabled => Outcome::Unchanged,
        Status::RequiresApproval => Outcome::NeedsApproval,
        Status::NotFound => Outcome::Failed(
            "macOS can't register this copy of Glyphio as a login item — move it to Applications"
                .into(),
        ),
        Status::NotRegistered => apply_setting(item, true),
    }
}

/// Result of the one-time removal of the login item the old AppleScript backend created.
#[derive(Debug, PartialEq, Eq)]
pub enum Migration {
    AlreadyDone,
    Removed,
    /// Automation was refused or the script failed; the user can remove it by hand.
    ManualRemoval,
}

/// Remove the scripted item once. A fresh install (`!existing_install`) is treated as already
/// done, so a new user is never asked for Automation to look for something that was never there.
/// `already_done` is the marker from a previous launch; `remove`
/// is the single remaining scripted call, run only here.
pub fn migrate_scripted_item(
    already_done: bool,
    remove: impl FnOnce() -> Result<(), String>,
) -> Migration {
    if already_done {
        return Migration::AlreadyDone;
    }
    match remove() {
        Ok(()) => Migration::Removed,
        Err(e) => {
            log::warn!("could not remove the old Glyphio login item: {e}");
            Migration::ManualRemoval
        }
    }
}

/// The note Settings shows when the old item could not be removed automatically.
pub const MANUAL_REMOVAL_NOTE: &str =
    "Remove the old “Glyphio” entry in System Settings → General → Login Items.";

/// The identifier the old LaunchAgent's bundle must carry.
const BUNDLE_IDENTIFIER: &str = "io.glyphio.app";
const EXECUTABLE_SUFFIX: &str = "/Contents/MacOS/glyphio";

/// First `<string>` of the `ProgramArguments` array in a LaunchAgent plist.
fn first_program_argument(plist: &str) -> Option<String> {
    let after = &plist[plist.find("<key>ProgramArguments</key>")?..];
    let start = after.find("<string>")? + "<string>".len();
    let end = start + after[start..].find("</string>")?;
    Some(after[start..end].trim().to_string())
}

/// Whether a LaunchAgent plist was written by Glyphio's former backend: it starts the
/// `Contents/MacOS/glyphio` executable of a bundle whose identifier is ours, wherever that
/// bundle lives. Anything else with a Glyphio-like name is somebody else's and stays.
pub fn is_legacy_glyphio_agent(plist: &str, bundle_identifier: impl Fn(&Path) -> Option<String>) -> bool {
    let Some(program) = first_program_argument(plist) else {
        return false;
    };
    let Some(bundle) = program.strip_suffix(EXECUTABLE_SUFFIX) else {
        return false;
    };
    bundle_identifier(Path::new(bundle)).as_deref() == Some(BUNDLE_IDENTIFIER)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        status: Status,
        after_register: Status,
        fail: Option<String>,
        calls: Vec<&'static str>,
    }

    impl Fake {
        fn at(status: Status) -> Self {
            Self { status, after_register: Status::Enabled, fail: None, calls: vec![] }
        }
    }

    impl LoginItem for Fake {
        fn status(&self) -> Status {
            self.status
        }
        fn register(&mut self) -> Result<(), String> {
            self.calls.push("register");
            match &self.fail {
                Some(e) => Err(e.clone()),
                None => {
                    self.status = self.after_register;
                    Ok(())
                }
            }
        }
        fn unregister(&mut self) -> Result<(), String> {
            self.calls.push("unregister");
            self.status = Status::NotRegistered;
            Ok(())
        }
    }

    #[test]
    fn turning_it_on_registers_once() {
        let mut item = Fake::at(Status::NotRegistered);
        assert_eq!(apply_setting(&mut item, true), Outcome::Registered);
        assert_eq!(item.calls, ["register"]);
    }

    #[test]
    fn turning_it_on_when_already_on_does_nothing() {
        let mut item = Fake::at(Status::Enabled);
        assert_eq!(apply_setting(&mut item, true), Outcome::Unchanged);
        assert!(item.calls.is_empty());
    }

    #[test]
    fn turning_it_off_unregisters_and_off_when_off_does_nothing() {
        let mut item = Fake::at(Status::Enabled);
        assert_eq!(apply_setting(&mut item, false), Outcome::Unregistered);
        let mut off = Fake::at(Status::NotRegistered);
        assert_eq!(apply_setting(&mut off, false), Outcome::Unchanged);
        assert!(off.calls.is_empty());
    }

    #[test]
    fn a_registration_awaiting_approval_is_reported_not_retried() {
        let mut item = Fake::at(Status::NotRegistered);
        item.after_register = Status::RequiresApproval;
        assert_eq!(apply_setting(&mut item, true), Outcome::NeedsApproval);
        assert_eq!(apply_setting(&mut item, true), Outcome::NeedsApproval);
        assert_eq!(item.calls, ["register"], "the second request does not register again");
    }

    #[test]
    fn a_refusal_carries_the_systems_words() {
        let mut item = Fake::at(Status::NotRegistered);
        item.fail = Some("Operation not permitted".into());
        assert_eq!(
            apply_setting(&mut item, true),
            Outcome::Failed("Operation not permitted".into())
        );
    }

    #[test]
    fn launch_registers_once_only_when_wanted_and_missing() {
        let mut missing = Fake::at(Status::NotRegistered);
        assert_eq!(reconcile_at_launch(&mut missing, true), Outcome::Registered);
        let mut present = Fake::at(Status::Enabled);
        assert_eq!(reconcile_at_launch(&mut present, true), Outcome::Unchanged);
        assert!(present.calls.is_empty());
    }

    #[test]
    fn launch_never_touches_the_item_when_the_setting_is_off() {
        let mut item = Fake::at(Status::Enabled);
        assert_eq!(reconcile_at_launch(&mut item, false), Outcome::Unchanged);
        assert!(item.calls.is_empty());
    }

    #[test]
    fn launch_surfaces_a_pending_approval_instead_of_retrying() {
        let mut item = Fake::at(Status::RequiresApproval);
        assert_eq!(reconcile_at_launch(&mut item, true), Outcome::NeedsApproval);
        assert!(item.calls.is_empty());
    }

    #[test]
    fn launch_explains_an_app_the_system_cannot_register() {
        let mut item = Fake::at(Status::NotFound);
        assert!(matches!(reconcile_at_launch(&mut item, true), Outcome::Failed(_)));
        assert!(item.calls.is_empty());
    }

    #[test]
    fn the_scripted_item_is_removed_once() {
        let mut ran = 0;
        assert_eq!(migrate_scripted_item(false, || { ran += 1; Ok(()) }), Migration::Removed);
        assert_eq!(migrate_scripted_item(true, || { ran += 1; Ok(()) }), Migration::AlreadyDone);
        assert_eq!(ran, 1);
    }

    #[test]
    fn a_denied_removal_asks_for_manual_removal() {
        assert_eq!(
            migrate_scripted_item(false, || Err("Not authorized to send Apple events".into())),
            Migration::ManualRemoval
        );
    }

    fn agent(program: &str) -> String {
        format!(
            "<plist><dict><key>Label</key><string>Glyphio</string><key>ProgramArguments</key>\
             <array><string>{program}</string><string>--hidden</string></array></dict></plist>"
        )
    }

    #[test]
    fn a_glyphio_launch_agent_is_recognised_at_any_path() {
        let ours = |_: &Path| Some("io.glyphio.app".to_string());
        assert!(is_legacy_glyphio_agent(&agent("/Applications/Glyphio.app/Contents/MacOS/glyphio"), ours));
        assert!(is_legacy_glyphio_agent(&agent("/Users/a/Downloads/Glyphio 2.app/Contents/MacOS/glyphio"), ours));
    }

    #[test]
    fn someone_elses_agent_with_a_glyphio_name_is_left_alone() {
        let other = |_: &Path| Some("com.example.other".to_string());
        assert!(!is_legacy_glyphio_agent(&agent("/Applications/Glyphio.app/Contents/MacOS/glyphio"), other));
        let ours = |_: &Path| Some("io.glyphio.app".to_string());
        assert!(!is_legacy_glyphio_agent(&agent("/usr/local/bin/glyphio-helper"), ours));
        assert!(!is_legacy_glyphio_agent("<plist></plist>", ours));
        assert!(!is_legacy_glyphio_agent(&agent("/Applications/Glyphio.app/Contents/MacOS/glyphio"), |_: &Path| None));
    }
}
