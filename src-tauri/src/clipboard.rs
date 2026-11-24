//! Clipboard history: everything you copy, searchable, pasteable again.
//!
//! # What this records, and what it refuses to
//!
//! A clipboard manager is a log of what you copy, and what people copy includes passwords.
//! So the rules are not "record everything and let the user clean up afterwards":
//!
//! * Content marked concealed by whoever put it there is never recorded — password managers
//!   mark their payload, and honouring that mark is the line between this feature and a
//!   credential logger. Same for transient/auto-generated content.
//! * Apps on the ignore list are never recorded, matched on the app that had focus.
//! * Nothing leaves the device. There is no sync path for clipboard history — not a policy,
//!   an absence of code, exactly as with capture history.
//! * The database and every stored image are owner-only on disk.
//! * The whole thing can be turned off, and cleared, from Settings.
//!
//! # How it watches
//!
//! Neither macOS nor Windows offers a clipboard notification worth building on, but both
//! expose a counter that changes when the clipboard does. So: poll the counter (cheap, no
//! content is touched), and read the clipboard only when it has actually moved.

mod store;

pub fn can_send_paste() -> bool {
    crate::platform::current().input.can_send_paste()
}

pub fn send_paste() -> anyhow::Result<()> {
    crate::platform::current().input.send_paste()
}
pub use store::{ClipEntry, ClipStore, NewClip};

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crate::AppState;

/// How often the counter is checked while the clipboard is in use. Fast enough that the entry
/// is there by the time the picker opens, slow enough to be invisible: this is an integer
/// comparison, not a read.
const POLL: Duration = Duration::from_millis(500);
/// How often once nothing has been copied for [`IDLE_AFTER`]. A copy after a quiet spell is
/// at most this late, and the history is only ever opened by a person, so it is never noticed.
const IDLE_POLL: Duration = Duration::from_secs(2);
const IDLE_AFTER: Duration = Duration::from_secs(60);
/// How often to look again while the session is locked or switched away.
const LOCKED_POLL: Duration = Duration::from_secs(5);

/// The wait before the next look at the clipboard: quick while things are being copied,
/// slower after a quiet minute, and resting while nobody is at the Mac.
pub fn next_poll(since_last_change: Duration, locked: bool) -> Duration {
    if locked {
        LOCKED_POLL
    } else if since_last_change < IDLE_AFTER {
        POLL
    } else {
        IDLE_POLL
    }
}

/// Largest image worth keeping. A copied screenshot is normal; a copied 200MB layered export
/// is not something to quietly write to disk on every copy.
const MAX_IMAGE_BYTES: usize = 24 * 1024 * 1024;

/// Largest text worth keeping. Truncating would be worse than skipping — a half-pasted
/// config file is a bug report — so anything past this is left out of the history entirely.
const MAX_TEXT_BYTES: usize = 1024 * 1024;

/// A clipboard change Glyphio made itself, which the watcher should not record.
///
/// When the app writes to the clipboard the content is *already* in its history under a
/// better identity — a capture, or the entry the user just picked — so recording our own write
/// only produces a second row saying less than the first. Storing the counter *after* the
/// write makes this exact rather than a race: we ignore that one specific change and nothing
/// else.
static IGNORE: AtomicI64 = AtomicI64::new(i64::MIN);

/// Call immediately after writing to the clipboard from inside the app.
pub fn ignore_own_write() {
    IGNORE.store(crate::platform::current().pasteboard.change_counter(), Ordering::Relaxed);
}

/// Empty the system clipboard, and keep the watcher from recording the change.
pub fn empty(app: &AppHandle) -> anyhow::Result<()> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    app.clipboard().clear()?;
    ignore_own_write();
    Ok(())
}

/// Start watching the clipboard for the life of the app.
///
/// Runs whatever the setting says: the loop keeps ticking and re-reads the setting each time,
/// so turning history on or off takes effect immediately instead of at the next launch.
pub fn watch(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        // Whatever is already on the clipboard at launch was copied before we were asked to
        // watch, so adopt the counter without reading it.
        let seen = AtomicI64::new(crate::platform::current().pasteboard.change_counter());
        let mut last_change = std::time::Instant::now();
        let mut wait = POLL;
        loop {
            std::thread::sleep(wait);
            let platform = crate::platform::current();
            let locked = platform.windows.session_locked();
            wait = next_poll(last_change.elapsed(), locked);
            if locked {
                continue; // nobody is copying; don't even read the counter
            }
            let counter = platform.pasteboard.change_counter();
            if counter == seen.load(Ordering::Relaxed) {
                continue;
            }
            seen.store(counter, Ordering::Relaxed);
            last_change = std::time::Instant::now();
            wait = POLL;
            if IGNORE.load(Ordering::Relaxed) == counter {
                continue; // our own write; already in the history
            }
            if let Err(e) = consider(&app) {
                log::debug!("clipboard entry skipped: {e}");
            }
        }
    });
}

/// Decide whether the current clipboard is worth recording, and record it if so.
fn consider(app: &AppHandle) -> anyhow::Result<()> {
    let state = app.state::<AppState>();
    let (enabled, max_items, max_bytes, ignored) = {
        let s = state.settings.lock().unwrap();
        (
            s.clipboard_history,
            s.clipboard_max_items,
            s.clipboard_max_bytes(),
            s.clipboard_ignore_apps.clone(),
        )
    };
    if !enabled {
        return Ok(());
    }
    if crate::platform::current().pasteboard.is_concealed() {
        // Deliberately not logged in any detail: the interesting fact is that something
        // asked not to be recorded, not what it was.
        log::debug!("clipboard entry marked concealed — not recorded");
        return Ok(());
    }
    let source = crate::platform::current().windows.foreground_app_name();
    if !source.is_empty()
        && ignored.iter().any(|a| a.eq_ignore_ascii_case(&source) || {
            let a = a.to_lowercase();
            !a.is_empty() && source.to_lowercase().contains(&a)
        })
    {
        return Ok(());
    }

    let Some(clip) = read_clipboard(app) else {
        return Ok(()); // a format we don't keep (a file promise, a custom type)
    };
    if let Some(entry) = state.clips.record(clip, &source, max_items, max_bytes)? {
        let _ = app.emit("clipboard-changed", entry.id.clone());
    } else {
        let _ = app.emit("clipboard-changed", String::new());
    }
    Ok(())
}

/// Text first, then image. Text wins because an app that offers both (a browser copying a
/// styled selection, say) means the text: that is what pasting it would give you.
fn read_clipboard(app: &AppHandle) -> Option<NewClip> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let cb = app.clipboard();
    if let Ok(text) = cb.read_text() {
        if !text.trim().is_empty() {
            if text.len() > MAX_TEXT_BYTES {
                log::debug!("copied text too large to keep ({} bytes)", text.len());
                return None;
            }
            // Text in between makes a later copy of the same image a new copy again.
            LAST_IMAGE.lock().unwrap().0 = None;
            return Some(NewClip::Text(text));
        }
    }
    let image = cb.read_image().ok()?;
    let (w, h) = (image.width(), image.height());
    if w == 0 || h == 0 {
        return None;
    }
    // Copying the same image again (or an app re-asserting it) changes the counter but not
    // the pixels; the PNG encode is the expensive part, so skip it for a repeat.
    if !LAST_IMAGE.lock().unwrap().is_new(w, h, image.rgba()) {
        log::debug!("copied image unchanged since the last one — not converted again");
        return None;
    }
    let rgba = image.rgba().to_vec();
    let buf = image::RgbaImage::from_raw(w, h, rgba)?;
    let mut png = Vec::new();
    buf.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    if png.len() > MAX_IMAGE_BYTES {
        log::debug!("copied image too large to keep ({} bytes)", png.len());
        return None;
    }
    Some(NewClip::Image { png, width: w, height: h })
}

static LAST_IMAGE: std::sync::Mutex<SeenImage> = std::sync::Mutex::new(SeenImage(None));

/// The last image the watcher converted, as a digest of its size and pixels.
pub struct SeenImage(Option<(u32, u32, u64)>);

impl SeenImage {
    /// Whether these pixels differ from the last image seen; remembers them either way.
    pub fn is_new(&mut self, w: u32, h: u32, rgba: &[u8]) -> bool {
        use std::hash::Hasher;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        hasher.write(rgba);
        let key = (w, h, hasher.finish());
        let new = self.0 != Some(key);
        self.0 = Some(key);
        new
    }
}

/// Put an entry back on the clipboard, ready to be pasted.
///
/// Re-using an entry counts as copying it, so it moves to the top of the history — done here
/// explicitly rather than by letting the watcher notice, because the watcher is told to ignore
/// this write. Otherwise picking an old entry would either duplicate it or depend on timing.
pub fn put_back(app: &AppHandle, entry: &ClipEntry) -> anyhow::Result<()> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    match entry.kind.as_str() {
        "image" => {
            let path = entry
                .image_path
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("this image is no longer on disk"))?;
            let img = image::open(path)?.to_rgba8();
            let (w, h) = img.dimensions();
            let tauri_image = tauri::image::Image::new_owned(img.into_raw(), w, h);
            app.clipboard().write_image(&tauri_image)?;
        }
        _ => {
            let text = entry
                .text
                .clone()
                .ok_or_else(|| anyhow::anyhow!("this entry has no text"))?;
            app.clipboard().write_text(text)?;
        }
    }
    ignore_own_write();
    let _ = app.state::<AppState>().clips.touch(&entry.id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{fake::Fake, WindowQuery};

    #[test]
    fn the_watcher_slows_after_a_quiet_minute_and_rests_while_locked() {
        let at_desk = Fake::default();
        let away = Fake { locked: true, ..Default::default() };
        let pace = |f: &Fake, idle: u64| next_poll(Duration::from_secs(idle), f.session_locked());
        assert_eq!(pace(&at_desk, 0), POLL);
        assert_eq!(pace(&at_desk, 59), POLL);
        assert_eq!(pace(&at_desk, 60), IDLE_POLL);
        assert_eq!(pace(&at_desk, 3600), IDLE_POLL);
        assert_eq!(pace(&away, 0), LOCKED_POLL);
    }

    #[test]
    fn the_same_pixels_are_only_converted_once() {
        let mut seen = SeenImage(None);
        assert!(seen.is_new(2, 1, &[1, 2, 3, 4, 5, 6, 7, 8]));
        assert!(!seen.is_new(2, 1, &[1, 2, 3, 4, 5, 6, 7, 8]));
        assert!(seen.is_new(2, 1, &[1, 2, 3, 4, 5, 6, 7, 9]), "different pixels");
        assert!(seen.is_new(1, 2, &[1, 2, 3, 4, 5, 6, 7, 9]), "same bytes, different shape");
    }
}
