# Linux support

Status: **in progress**. The app builds, tests and starts on Linux. Process control, the login
item and install detection are real, and so is the portable data folder. Capture, window
queries, input, the clipboard counter and permissions still answer through
`src/platform/unsupported.rs`, and there is no engine sidecar or package yet. This document is
the plan the remaining pieces follow, and it is updated as each one lands.

## The short version

Most of Glyphio is already portable (see `WINDOWS.md`, "What ports unchanged"): the Tauri shell,
all of `ui/`, the stores, sync and capture orchestration. On Linux, two things decide how much
works: whether the session is **X11 or Wayland**, and the fact that **Wayland gives apps no
global view of the screen or input**. X11 gets everything. Wayland gets what the desktop
portals offer, and the rest is switched off with an explanation instead of failing silently.

| Capability | X11 | Wayland |
| --- | --- | --- |
| Visible screen, front window, region | XGetImage over `x11rb`; region from our own picker or the screenshot portal | `org.freedesktop.portal.Screenshot`. Its interactive mode is the region picker |
| Front window details (title, pid, bounds) | `_NET_ACTIVE_WINDOW`, `_NET_WM_PID`, `_NET_WM_NAME` | Not available. Window modes fall back to the portal picker |
| Scrolling capture | XTest scroll events plus the pure stitcher | Off: there is no input injection |
| Paste into the front app | XTest Ctrl+V | Off: Glyphio copies and says "press Ctrl+V" |
| Clipboard history | Text and images through the clipboard plugin, with a content hash standing in for macOS's change counter | Same, through `wl-data-control` where the compositor offers it (wlroots, KDE). GNOME doesn't, so history only sees copies made while a Glyphio window has focus |
| Concealed clipboard (password managers) | `x-kde-passwordManagerHint` target | Same |
| Global hotkeys | `tauri-plugin-global-shortcut` | `org.freedesktop.portal.GlobalShortcuts` (KDE, GNOME 48+), else a `glyphio --capture <mode>` command the user binds in their desktop's settings |
| Text expansion (engine) | espanso's X11 backend | espanso's Wayland backend over evdev/uinput, which needs the user in the `input` group |
| Page identity (URL, title) | Window-title parsing only. AT-SPI is a later option | Not available |
| OCR | `tesseract` if installed, otherwise the action is hidden | Same |
| Launch at login | XDG autostart entry (done) | Same (done) |
| Permissions | None to ask for | The portal asks per capture. The engine needs `input` group membership, explained in setup |

## Portable copy (done)

A copy is **portable** when a folder named `Glyphio Data` sits next to it: beside
`Glyphio.app` on macOS, beside the `.AppImage` on Linux, or inside the unpacked portable
tarball. All data lives in that folder: snippets, history, clipboard, settings, the engine's
config, and the log (`logs/`). A portable copy never registers a login item and is never
offered a move into Applications or warned about other installed copies. `src/paths.rs`
(`choose_location`) decides this once per process.

These still live in the user's profile, not the data folder:

- **Sync credentials** stay in the OS keychain (macOS Keychain, Secret Service on Linux),
  deliberately. A token on a USB stick is a token anyone who finds the stick can use. A portable
  copy signs in again on each new computer.
- **The webview's own storage** (WebKit caches, and the handful of `localStorage` flags the
  setup window keeps). Nothing a user would miss is there.

On macOS, a quarantined app run from a download or a stick may be *translocated*: Gatekeeper
runs it from a random read-only path, where the data folder can't be seen. Remove the
quarantine (`xattr -dr com.apple.quarantine Glyphio.app`) or copy it with Finder first.

## Order of work

Each step is one pull request and leaves the tree green on all three CI jobs.

1. **Foundation and portable mode** (done). `src/platform/linux/` with process control, autostart
   and install. The platform's log folder (`~/.local/state/Glyphio`). The portable data folder.
   The data root is added to the asset-protocol scope at runtime. The engine's runtime folder
   moves to `$XDG_RUNTIME_DIR` (or the temp folder) when a deep data root would make its
   socket paths too long. An `App (Linux)` CI job builds and tests the app on Ubuntu.
2. **Capture, clipboard and paste.** X11 backend with `x11rb`, the portal backend with `ashpd`,
   picked at runtime from `XDG_SESSION_TYPE`/`WAYLAND_DISPLAY`. Our own region picker on X11 is a
   frozen-screenshot overlay window, the same shape as the one `WINDOWS.md` calls the long pole.
   It is shared with Windows when that port resumes. Setup gets a Linux step list (no macOS
   permission steps) and a `linux` platform branch in `ui/`.
3. **Hotkeys.** The GlobalShortcuts portal on Wayland, plus `glyphio --capture <mode>` handled
   through the single-instance plugin, so any desktop can bind a key to it.
4. **Engine sidecar.** Built on the espanso 2.4 rebase, which brings upstream's newer Wayland
   work. Two builds, `glyphio-engine` (X11) and `glyphio-engine-wayland`, with the supervisor
   choosing one by session type. `scripts/build-engine.sh` grows a Linux branch, and setup
   explains the `input` group on Wayland.
5. **Packaging and CI.** `tauri.linux.conf.json` with AppImage and `.deb` targets, a portable
   tarball (the executable, its sidecars, and an empty `Glyphio Data/`), a CI job that builds
   those bundles, and `linux-x86_64` in the updater manifest. Linux builds
   can't come from the maintainer's Mac, so this step needs a decision about the updater signing
   key in CI.

## Building on Linux today

```sh
sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev libxdo-dev
# tauri-build insists the sidecars exist; placeholders do until step 4.
for n in glyphio-engine glyphio-ocr; do
  f=src-tauri/binaries/$n-x86_64-unknown-linux-gnu; printf '#!/bin/sh\nexit 1\n' > "$f"; chmod +x "$f"
done
cargo test --manifest-path src-tauri/Cargo.toml --workspace
```
