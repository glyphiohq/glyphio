/// Arguments the old login backends passed, accepted for one more release so an upgrade that is
/// started by one of them stays silent. The native login item passes none; see
/// [`note_launch_event`].
pub const LOGIN_LAUNCH_ARGUMENT: &str = "--hidden";
const LEGACY_LOGIN_LAUNCH_ARGUMENT: &str = "--launch-at-login";

/// `keyAELaunchedAsLogInItem`: the property of the launch Apple event when the system started us
/// because the user logged in.
const LAUNCHED_AS_LOGIN_ITEM: u32 = u32::from_be_bytes(*b"lgit");
/// `keyAEPropData`: where that property lives in the event.
const KEY_AE_PROP_DATA: u32 = u32::from_be_bytes(*b"prdt");

/// Whether the launch event's property says "started by logging in".
pub fn is_login_item_property(property: Option<u32>) -> bool {
    property == Some(LAUNCHED_AS_LOGIN_ITEM)
}

static LAUNCHED_BY_LOGIN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Read how this process was launched. Call once, early in `setup`, while the system's launch
/// event is still the current one. After it, [`should_open_settings`] knows about a login launch
/// even though the native login item passes no arguments.
pub fn note_launch_event() {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::NSAppleEventManager;
        let property = NSAppleEventManager::sharedAppleEventManager()
            .currentAppleEvent()
            .and_then(|event| event.paramDescriptorForKeyword(KEY_AE_PROP_DATA))
            .map(|descriptor| descriptor.enumCodeValue());
        let by_login = is_login_item_property(property);
        LAUNCHED_BY_LOGIN.store(by_login, std::sync::atomic::Ordering::SeqCst);
        log::info!("launched by login item: {by_login}");
    }
}

/// Whether startup should present the regular Settings window. A login launch is recognised by
/// the launch event (the native login item) or, for one more release, by the arguments the old
/// backends passed.
pub fn should_open_settings(args: impl IntoIterator<Item = impl AsRef<str>>) -> bool {
    if LAUNCHED_BY_LOGIN.load(std::sync::atomic::Ordering::SeqCst) {
        return false;
    }
    !args.into_iter().any(|argument| {
        matches!(
            argument.as_ref(),
            LOGIN_LAUNCH_ARGUMENT | LEGACY_LOGIN_LAUNCH_ARGUMENT
        )
    })
}

/// What the already-running copy does when another process is started and exits.
#[derive(Debug, PartialEq, Eq)]
pub enum SecondLaunch {
    /// A user launch (Finder, Dock, direct execution, a link): bring Settings forward.
    ShowSettings,
    /// A login launch while we are already running: stay quiet.
    Ignore,
}

/// Decide how to react to a second launch from its arguments.
pub fn second_launch_action(args: impl IntoIterator<Item = impl AsRef<str>>) -> SecondLaunch {
    if should_open_settings(args) {
        SecondLaunch::ShowSettings
    } else {
        SecondLaunch::Ignore
    }
}

#[cfg(test)]
mod tests {
    use super::{is_login_item_property, second_launch_action, should_open_settings, SecondLaunch};

    #[test]
    fn a_second_user_launch_surfaces_settings_in_the_running_copy() {
        assert_eq!(
            second_launch_action(["glyphio"]),
            SecondLaunch::ShowSettings
        );
        // A Glyphio link carries its own handling, but the running copy still comes forward.
        assert_eq!(
            second_launch_action(["glyphio", "glyphio://join?code=x"]),
            SecondLaunch::ShowSettings
        );
    }

    #[test]
    fn a_second_login_launch_does_nothing() {
        assert_eq!(
            second_launch_action(["glyphio", "--hidden"]),
            SecondLaunch::Ignore
        );
        assert_eq!(
            second_launch_action(["glyphio", "--launch-at-login"]),
            SecondLaunch::Ignore
        );
    }

    #[test]
    fn a_login_launch_starts_without_the_settings_window() {
        assert!(!should_open_settings(["glyphio", "--hidden"]));
        // A legacy LaunchAgent can start the freshly upgraded app once before the migration
        // removes it, so keep that launch silent too.
        assert!(!should_open_settings(["glyphio", "--launch-at-login"]));
        assert!(should_open_settings(["glyphio"]));
    }

    #[test]
    fn the_launch_event_property_identifies_a_login_launch() {
        assert!(is_login_item_property(Some(u32::from_be_bytes(*b"lgit"))));
        assert!(!is_login_item_property(Some(u32::from_be_bytes(*b"abcd"))));
        assert!(!is_login_item_property(None));
    }
}
