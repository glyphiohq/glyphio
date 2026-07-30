# Installing Glyphio

Glyphio is **not yet notarized by Apple**. Notarization requires a Developer ID at $99/year,
and the project is donation-funded — so until that's covered, macOS will treat a downloaded
build as untrusted. Everything below is about getting past that cleanly.

Nothing here is a security bypass in the alarming sense: you are telling macOS that you trust
software whose publisher Apple hasn't vouched for. Decide that on the usual grounds — the
source is public, the build is reproducible from it, and the checksums are published with each
release.

## Option 1 — DMG from GitHub Releases

1. Download `Glyphio_<version>_aarch64.dmg` from
   [Releases](https://github.com/glyphiohq/glyphio/releases).
2. Verify the checksum against the one in the release notes:
   ```bash
   shasum -a 256 ~/Downloads/Glyphio_*.dmg
   ```
3. Open the DMG and drag Glyphio to Applications.
4. Launch it. macOS will refuse the first time — this is expected.
5. Open **System Settings → Privacy & Security**, scroll down, and click **Open Anyway** next
   to the message about Glyphio. Confirm.

On macOS 15 and later this System Settings step is the only route: the old
Control-click → Open shortcut no longer overrides Gatekeeper for unsigned apps.

You do this once. Updates installed through Glyphio's own updater are verified against the
project's signing key and don't repeat it.

## Option 2 — Build from source

No Gatekeeper involvement at all, because you built it.

```bash
git clone https://github.com/glyphiohq/glyphio.git
cd glyphio
npm install
npm run engine        # build + sign the expansion engine sidecar
npm run release       # → dist/Glyphio_<version>_<arch>.dmg
```

Requires Rust stable, Node 18+, and macOS Command Line Tools (full Xcode is not needed).

## First run

Glyphio asks for two macOS permissions, each with an in-app explanation:

| Permission | Why | Without it |
|---|---|---|
| **Accessibility** | Typing expansions into other apps | Snippets don't expand |
| **Screen Recording** | Capturing the screen | Captures come out black |

Screen Recording only takes effect on the **next** launch — macOS caches the answer for the
life of the process. Glyphio offers to relaunch itself when you grant it.

## Updates

Glyphio checks GitHub on launch for a newer release and tells you in **Settings → About**. It
sends nothing but the request; there is no telemetry. Turn the check off with the toggle in
that same section if you'd rather look yourself.

Updates are verified against Glyphio's own signing key before anything is installed. That key
is independent of Apple's, which is why updates work correctly on an unsigned build.

## When Glyphio becomes notarized

Nothing about installing will change except that the warnings stop. One thing *will* be
visible: signing identity is part of how macOS remembers permission grants, so switching to a
Developer ID resets them. You'll be asked for Accessibility and Screen Recording once more,
and everything else — snippets, history, settings, sync — is untouched.
