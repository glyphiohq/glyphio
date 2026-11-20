//! macOS privacy permissions: Accessibility and Screen Recording.

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef) -> bool;
}

/// Whether the app may post synthetic events / observe keystrokes. On modern macOS this is
/// THE Accessibility check that matters for the engine too: TCC attributes a bundled child
/// process to its **responsible app**, so one "Glyphio" grant covers expansion (the engine
/// sidecar) and scrolling capture alike.
pub fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Ask macOS to show the Accessibility permission dialog — which also ADDS the correct
/// "Glyphio" row to System Settings, so users never hunt for binaries with the "+" button.
pub fn request_accessibility() -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;
    let key = CFString::from_static_string("AXTrustedCheckOptionPrompt");
    let dict = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), CFBoolean::true_value().as_CFType())]);
    unsafe { AXIsProcessTrustedWithOptions(dict.as_concrete_TypeRef()) }
}

/// Whether the Screen Recording TCC permission is granted (checked without prompting).
pub fn screen_recording_granted() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// Trigger the OS Screen Recording prompt. macOS applies the grant on the app's next launch.
pub fn request_screen_recording() -> bool {
    unsafe { CGRequestScreenCaptureAccess() }
}

/// A real ScreenCaptureKit content query: succeeds when capture is usable by this process.
pub fn screen_recording_probe() -> bool {
    screencapturekit::shareable_content::SCShareableContent::get().is_ok()
}

/// `tccutil reset <service> <bundle id>`, user scope. Only this app's own entry is touched.
pub fn reset(kind: crate::platform::PermissionKind) -> anyhow::Result<()> {
    let service = match kind {
        crate::platform::PermissionKind::Accessibility => "Accessibility",
        crate::platform::PermissionKind::ScreenRecording => "ScreenCapture",
    };
    let out = std::process::Command::new("/usr/bin/tccutil")
        .args(["reset", service, crate::autostart::BUNDLE_IDENTIFIER])
        .output()
        .map_err(|e| anyhow::anyhow!("could not run tccutil: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        anyhow::bail!("tccutil failed: {}", String::from_utf8_lossy(&out.stderr).trim())
    }
}
