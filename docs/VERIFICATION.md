# macOS release verification

This is the release gate for Glyphio behavior that depends on macOS permissions, focus, the
menu bar, the pasteboard, and real target applications. Automated tests cover the portable
contracts; a release candidate is not verified until this matrix passes against the exact signed
app that will be shipped.

## Record the candidate

Run from a clean checkout at the release commit and record the following in the release issue or
pull request. Keep failed rows: a rerun should add evidence rather than erase it.

| Field | Value |
| --- | --- |
| Commit | `<full git SHA>` |
| Glyphio version | `<version>` |
| Artifact SHA-256 | `<sha256>` |
| macOS version and build | `<sw_vers output>` |
| Hardware | `<Apple silicon model or Intel model>` |
| Tester and date | `<name, YYYY-MM-DD>` |
| App signature | `<codesign authority/designated requirement>` |
| Engine signature | `<codesign authority/designated requirement>` |

Use a dedicated macOS test account or test Mac so permission resets do not disrupt a working
installation. Build with `npm run release` for a release candidate, or build the app bundle and
run `scripts/sign-bundle.sh` with the stable Glyphio development identity for a development
candidate. Verify the exact artifact before opening it:

```bash
codesign --verify --deep --strict --verbose=2 /path/to/Glyphio.app
codesign -d -r- /path/to/Glyphio.app
codesign -d -r- /path/to/Glyphio.app/Contents/MacOS/glyphio-engine
```

Record each row as `PASS`, `FAIL`, or `BLOCKED`, with a short observation and a screenshot or log
link for any failure. `BLOCKED` does not satisfy the release gate.

## Permission matrix

Exercise both the first-prompt path and the settings-only path. Reset permissions only in the
dedicated test account, quit Glyphio before a reset, and relaunch the same signed artifact after it.

| State and action | Expected observation | Result / evidence |
| --- | --- | --- |
| Accessibility absent on first launch | One Accessibility row explains text expansion and scrolling capture and offers the legitimate first prompt; no duplicate grant control is present. | |
| Accessibility prompt declined | The row offers only **Open Accessibility settings**. | |
| Accessibility granted in System Settings, then return to Glyphio | The existing view refreshes to **Granted** without navigating away or restarting. | |
| Accessibility revoked after being granted | The row refreshes to not granted and offers settings only; it does not resurrect the spent prompt. | |
| Screen Recording absent on first capture | One Screen Recording row explains that every capture mode needs it and offers the legitimate first prompt. | |
| Screen Recording prompt declined | The row offers only **Open Screen Recording settings**. | |
| Screen Recording accepted | The row offers only **Relaunch Glyphio** until the app has relaunched. | |
| Relaunch after Screen Recording grant | The row reports **Granted** and capture succeeds. | |
| Screen Recording revoked after being granted | The row refreshes to not granted and offers settings only. | |

## Capture and delivery matrix

For every row, start with another application frontmost. Trigger once from the capture palette and
once from its configured global shortcut. During capture, note the frontmost application before and
after the menu-bar indicator changes.

| Scenario | Expected observation | Result / evidence |
| --- | --- | --- |
| Visible Area, editor delivery | The Glyphio menu-bar icon becomes active immediately; the target app remains frontmost; the editor opens only after pixels are captured; history and clipboard both contain the image; success is shown briefly. | |
| Region (snip), editor delivery | Selection works without an early editor/settings window surfacing; the menu-bar lifecycle and final delivery match Visible Area. | |
| Full Window picker, editor delivery | The picker remains usable, Glyphio does not cover the target, and the selected window is delivered to history and clipboard. | |
| Frontmost Window, editor delivery | The originally frontmost non-Glyphio window is captured without Glyphio stealing focus. | |
| Browser Page, editor delivery | Only browser content is captured and the active/success states remain visible in the menu bar. | |
| Scrolling Area, editor delivery | The indicator stays active through selection and scrolling; progress is not silent; stitched output reaches history and clipboard. | |
| Scrolling Page, editor delivery | The indicator stays active for the whole page capture and the target browser remains frontmost. | |
| Escape during Scrolling Area and Scrolling Page | Escape finishes early, retains frames already captured, produces a usable history item, and returns Escape to the target app afterwards. | |
| Duplicate scrolling request | The running capture continues undamaged and the second request reports non-disruptively. | |
| Capture error (for example missing permission) | The menu-bar icon shows failure, the status row exposes details, and Glyphio does not activate itself merely to report the error. | |
| Every mode, silent delivery | No editor appears; active feedback remains visible; the image reaches history and clipboard before success is shown. | |
| Clipboard write failure after history save | Failure replaces success, the exact capture remains in history, and the menu-bar recovery action opens that saved capture so **Copy** can retry. | |
| Recovery copy after the pasteboard is available | **Copy** places the saved image on the clipboard and confirms success. | |
| Delayed/stale editor or silent worker | It cannot consume or acknowledge a newer capture; each capture settles at most once. | |

The clipboard-failure row requires a debugger or development-only fault injection at the native
clipboard command boundary; do not corrupt the user's pasteboard database or install a pasteboard
replacement to provoke it. The automated delivery tests remain the authoritative repeatable check
for the exact failure payload and recovery target.

## Shortcut matrix

| Scenario | Expected observation | Result / evidence |
| --- | --- | --- |
| Record a capture shortcut | Selecting the recorder and pressing Command-Shift-S records and displays `⌘⇧S`; typed text is never accepted as shortcut input. | |
| Modifier-only press | Nothing is saved and the recorder asks for a non-modifier key. | |
| Reserved macOS chord | The recorder explains the reservation and leaves the prior shortcut unchanged. | |
| Existing Glyphio chord | The recorder identifies the conflict and leaves the prior shortcut unchanged. | |
| Clear and save | The capture action has no global binding after save and relaunch. | |
| Reset and save | The documented default returns and works immediately. | |
| Existing pre-recorder settings | Existing serialized shortcut strings load, display in macOS notation, and still fire. | |

## Expansion delivery matrix

Create a snippet containing ASCII, non-ASCII, multiple lines, and punctuation. Test in TextEdit and
one second application whose bundle identifier is recorded in the evidence.

| Scenario | Expected observation | Result / evidence |
| --- | --- | --- |
| General delivery is Paste, no application override | The expansion is complete in both apps; Glyphio restores the previous clipboard after the configured delay. | |
| Add a `keys` override for TextEdit's bundle identifier | TextEdit receives typed-key delivery while the second app continues using Paste. | |
| Add a `paste` override while the general method is Keys | Only that exact bundle identifier uses Paste; a similarly named app does not match. | |
| Remove an override and reload/relaunch | The removed application returns to the general method; no stale generated rule remains. | |

## Automated gates

Run these against the same commit before the signed session:

```bash
npm test
bash scripts/verify-edr-surface.sh
cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --workspace
cargo clippy --manifest-path espanso/Cargo.toml -p espanso-config --all-targets -- -D warnings
cargo test --manifest-path espanso/Cargo.toml -p espanso-config
```

The frontend tests cover capture delivery outcomes, permission presentation, and shortcut event
recording. The app workspace covers lifecycle state, exact-once sessions, scrolling, persisted
settings, and generated delivery configuration. The expansion-engine test pins how application
configuration contributes to the active match set.

For a managed-device release candidate, additionally verify that enabling **Launch at Login**
creates a visible entry in **System Settings → General → Login Items** and does not create a file
in `~/Library/LaunchAgents`. Record the app's SHA-256 and Developer ID authority in the release
evidence. A self-signed development build is appropriate for local development, but is not
suitable evidence for an enterprise EDR allow rule.
