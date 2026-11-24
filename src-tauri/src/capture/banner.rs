//! The capture banner, drawn natively: the strip of timestamp, page details and note that sits
//! above a capture. It used to be drawn by the editor page (`ui/shared/banner.js`); drawing it
//! here lets a silent capture reach the clipboard with no web page at all, and the editor now
//! asks for the strip instead of drawing it.
//!
//! The rules (which lines appear, how a narrow capture sheds timestamp precision before the
//! date, when text is truncated) follow `banner.js` line for line. Text is drawn with the
//! bundled Inter (`assets/fonts`, SIL OFL 1.1; see NOTICES.md), so the result is the same on
//! every Mac and testable against golden images.

use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use chrono::{DateTime, Utc};
use image::{Rgba, RgbaImage};

use crate::platform::{DateStyle, LocaleDates};
use crate::settings::Settings;

/// Layout constants, in CSS px (multiplied by the capture's backing scale). Kept equal to
/// `BANNER_PRESENTATION` in `ui/shared/presentation.js`.
pub const PADDING: f32 = 16.0;
pub const LINE_GAP: f32 = 6.0;
pub const TIMESTAMP_PX: f32 = 20.0;
pub const URL_PX: f32 = 14.0;
pub const NOTE_PX: f32 = 14.0;
/// The smallest a timestamp shrinks to before it is truncated instead.
const MIN_TIMESTAMP_PX: f32 = 11.0;

/// What the banner shows and how it looks, from the user's settings.
#[derive(Clone, Debug)]
pub struct Style {
    pub show_timestamp: bool,
    pub show_window_title: bool,
    pub show_page_title: bool,
    pub show_page_url: bool,
    pub show_browser_profile: bool,
    /// "device-locale" | "iso-8601" | "utc-human"
    pub timestamp_format: String,
    /// "device" or an IANA zone.
    pub timezone: String,
    /// "device" or a BCP-47 tag.
    pub locale: String,
    pub bg: Rgba<u8>,
    pub fg: Rgba<u8>,
    pub muted: Rgba<u8>,
}

impl From<&Settings> for Style {
    fn from(s: &Settings) -> Self {
        Style {
            show_timestamp: s.show_timestamp,
            show_window_title: s.show_window_title,
            show_page_title: s.show_page_title,
            show_page_url: s.show_page_url,
            show_browser_profile: s.show_browser_profile,
            timestamp_format: s.timestamp_format.clone(),
            timezone: s.timezone.clone(),
            locale: s.locale.clone(),
            bg: parse_color(&s.banner_bg).unwrap_or(Rgba([0x1f, 0x29, 0x37, 0xff])),
            fg: parse_color(&s.banner_fg).unwrap_or(Rgba([0xff, 0xff, 0xff, 0xff])),
            muted: parse_color(&s.banner_muted).unwrap_or(Rgba([0xcb, 0xd5, 0xe1, 0xff])),
        }
    }
}

/// What the capture knows about itself, as stored on its history row.
#[derive(Clone, Debug, Default)]
pub struct Meta {
    /// RFC 3339; the banner always shows the original capture time.
    pub captured_at: String,
    pub window_title: String,
    pub page_title: String,
    pub page_url: String,
    pub profile: String,
    /// Backing scale (2 on Retina): the banner is laid out in points and drawn in pixels.
    pub dpr: f64,
}

impl Meta {
    pub fn from_row(row: &crate::history::CaptureMeta) -> Self {
        Meta {
            captured_at: row.captured_at.clone(),
            window_title: if row.title.is_empty() { row.url.clone() } else { row.title.clone() },
            page_title: row.page_title.clone(),
            page_url: row.page_url.clone(),
            profile: row.profile.clone(),
            dpr: row.dpr,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Timestamp,
    Muted,
    Note,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub role: Role,
    /// Empty for the timestamp, which is formatted to fit when drawn.
    pub text: String,
}

/// The banner's lines, top to bottom. Height, drawing and "is there a banner at all" all read
/// this one list, as in `bannerLines` in banner.js.
pub fn lines(style: &Style, meta: &Meta, note: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let window_title = meta.window_title.trim();
    let page_title = meta.page_title.trim();
    let muted = |text: &str| Line { role: Role::Muted, text: text.to_string() };

    if style.show_timestamp {
        out.push(Line { role: Role::Timestamp, text: String::new() });
    }
    if style.show_page_title && !page_title.is_empty() {
        out.push(muted(page_title));
    }
    // A browser's window title is the page title plus the browser's name: with the page title
    // already shown, it would say the same thing twice.
    let redundant = style.show_page_title && !page_title.is_empty() && window_title.starts_with(page_title);
    if style.show_window_title && !window_title.is_empty() && !redundant {
        out.push(muted(window_title));
    }
    if style.show_page_url && !meta.page_url.trim().is_empty() {
        out.push(muted(meta.page_url.trim()));
    }
    if style.show_browser_profile && !meta.profile.trim().is_empty() {
        out.push(muted(&format!("Profile: {}", meta.profile.trim())));
    }
    let note = note.trim();
    if !note.is_empty() {
        out.push(Line { role: Role::Note, text: note.to_string() });
    }
    out
}

fn line_px(role: Role) -> f32 {
    match role {
        Role::Timestamp => TIMESTAMP_PX,
        Role::Muted => URL_PX,
        Role::Note => NOTE_PX,
    }
}

/// The strip's height in CSS px; 0 when there is nothing to show.
pub fn css_height(style: &Style, meta: &Meta, note: &str) -> f32 {
    let lines = lines(style, meta, note);
    if lines.is_empty() {
        return 0.0;
    }
    let text: f32 = lines.iter().map(|l| line_px(l.role)).sum();
    PADDING * 2.0 + text + LINE_GAP * (lines.len() as f32 - 1.0)
}

/// The strip's height in pixels at the capture's scale, rounded as the page rounded it.
pub fn height_px(style: &Style, meta: &Meta, note: &str) -> u32 {
    (css_height(style, meta, note) * scale(meta)).round() as u32
}

fn scale(meta: &Meta) -> f32 {
    if meta.dpr > 0.0 { meta.dpr as f32 } else { 1.0 }
}

// ---- timestamp -----------------------------------------------------------------------------

/// The timestamp at decreasing lengths, longest first: a narrow capture sheds precision
/// before it loses the date.
pub fn timestamp_variants(captured_at: &str, style: &Style, dates: &dyn LocaleDates) -> Vec<String> {
    let Ok(at) = DateTime::parse_from_rfc3339(captured_at).map(|d| d.with_timezone(&Utc)) else {
        return Vec::new();
    };
    let zone = named_zone(&style.timezone);
    let locale = (style.locale != "device" && !style.locale.is_empty()).then_some(style.locale.as_str());
    match style.timestamp_format.as_str() {
        "iso-8601" => {
            let full = match zone {
                None => at.format("%Y-%m-%d %H:%M:%SZ").to_string(),
                Some(tz) => format!("{} {}", at.with_timezone(&tz).format("%Y-%m-%d %H:%M:%S"), style.timezone),
            };
            let no_seconds = drop_seconds(&full);
            let short: String = full.chars().take(16).collect();
            vec![full, no_seconds, short]
        }
        "utc-human" => vec![
            at.format("%-d %b %Y, %H:%M:%S UTC").to_string(),
            at.format("%-d %b %Y, %H:%M UTC").to_string(),
            at.format("%d/%m/%Y, %H:%M UTC").to_string(),
        ],
        _ => {
            let tz = zone.map(|_| style.timezone.as_str());
            let ms = at.timestamp_millis();
            let full = dates
                .format(ms, DateStyle::Medium, DateStyle::Long, locale, tz)
                .unwrap_or_else(|| fallback_format(at, zone, true));
            let medium = dates
                .format(ms, DateStyle::Medium, DateStyle::Short, locale, tz)
                .unwrap_or_else(|| fallback_format(at, zone, false));
            let short = dates
                .format(ms, DateStyle::Short, DateStyle::Short, locale, tz)
                .unwrap_or_else(|| medium.clone());
            vec![full, medium, short]
        }
    }
}

/// `full.replace(/:\d{2}(Z|\s|$)/, '$1')` from banner.js: drop the seconds field.
fn drop_seconds(full: &str) -> String {
    let b = full.as_bytes();
    for i in 0..b.len().saturating_sub(2) {
        if b[i] == b':' && b[i + 1].is_ascii_digit() && b[i + 2].is_ascii_digit() {
            let next = b.get(i + 3).copied();
            if matches!(next, None | Some(b'Z') | Some(b' ') | Some(b'\t')) {
                return format!("{}{}", &full[..i], &full[i + 3..]);
            }
        }
    }
    full.to_string()
}

fn named_zone(name: &str) -> Option<chrono_tz::Tz> {
    if name.is_empty() || name == "device" {
        return None;
    }
    name.parse().ok()
}

/// Used when the platform has no date formatter of its own.
fn fallback_format(at: DateTime<Utc>, zone: Option<chrono_tz::Tz>, seconds: bool) -> String {
    let pattern = if seconds { "%-d %b %Y, %H:%M:%S %Z" } else { "%-d %b %Y, %H:%M" };
    match zone {
        Some(tz) => at.with_timezone(&tz).format(pattern).to_string(),
        None => at.with_timezone(&chrono::Local).format(&pattern.replace("%Z", "%:z")).to_string(),
    }
}

// ---- text ----------------------------------------------------------------------------------

struct Fonts {
    regular: FontRef<'static>,
    bold: FontRef<'static>,
}

fn fonts() -> &'static Fonts {
    static FONTS: OnceLock<Fonts> = OnceLock::new();
    FONTS.get_or_init(|| Fonts {
        regular: FontRef::try_from_slice(include_bytes!("../../assets/fonts/Inter-Regular.otf"))
            .expect("bundled Inter Regular"),
        bold: FontRef::try_from_slice(include_bytes!("../../assets/fonts/Inter-Bold.otf")).expect("bundled Inter Bold"),
    })
}

/// A CSS `px` font size, which is the em size, as an ab_glyph scale (which is the font's
/// ascent-to-descent height).
fn em(font: &FontRef<'static>, px: f32) -> PxScale {
    font.pt_to_px_scale(px).unwrap_or(PxScale::from(px))
}

/// Width in pixels of `text` at `px`, with kerning.
pub fn measure(bold: bool, px: f32, text: &str) -> f32 {
    let f = if bold { &fonts().bold } else { &fonts().regular };
    let s = f.as_scaled(em(f, px));
    let mut width = 0.0;
    let mut prev = None;
    for c in text.chars() {
        let id = s.glyph_id(c);
        if let Some(p) = prev {
            width += s.kern(p, id);
        }
        width += s.h_advance(id);
        prev = Some(id);
    }
    width
}

/// Draw `text` with its em box's top at `y` (canvas `textBaseline = 'top'`).
fn draw_text(img: &mut RgbaImage, bold: bool, px: f32, text: &str, x: f32, y: f32, color: Rgba<u8>) {
    let f = if bold { &fonts().bold } else { &fonts().regular };
    let scale = em(f, px);
    let s = f.as_scaled(scale);
    let baseline = y + s.ascent();
    let mut pen = x;
    let mut prev = None;
    for c in text.chars() {
        let id = s.glyph_id(c);
        if let Some(p) = prev {
            pen += s.kern(p, id);
        }
        let glyph = id.with_scale_and_position(scale, ab_glyph::point(pen, baseline));
        if let Some(outline) = f.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let px_x = bounds.min.x as i64 + i64::from(gx);
                let px_y = bounds.min.y as i64 + i64::from(gy);
                if px_x >= 0 && px_y >= 0 && (px_x as u32) < img.width() && (px_y as u32) < img.height() {
                    blend(img.get_pixel_mut(px_x as u32, px_y as u32), color, coverage);
                }
            });
        }
        pen += s.h_advance(id);
        prev = Some(id);
    }
}

fn blend(dst: &mut Rgba<u8>, src: Rgba<u8>, coverage: f32) {
    let a = coverage.clamp(0.0, 1.0) * f32::from(src[3]) / 255.0;
    for i in 0..3 {
        dst[i] = (f32::from(src[i]) * a + f32::from(dst[i]) * (1.0 - a)).round() as u8;
    }
    dst[3] = (255.0 * a + f32::from(dst[3]) * (1.0 - a)).round() as u8;
}

/// The longest prefix of `text` that fits in `max_w` with an ellipsis, as `truncateToWidth`.
fn truncate(bold: bool, px: f32, text: &str, max_w: f32) -> String {
    if measure(bold, px, text) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let candidate: String = chars[..mid].iter().collect::<String>() + "…";
        if measure(bold, px, &candidate) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect::<String>() + "…"
}

/// The first variant that fits at the nominal size, else the shortest one shrunk (down to
/// 11 px per point), else the shortest truncated: `fitTimestamp`. Returns (text, px).
fn fit_timestamp(variants: &[String], max_w: f32, nominal: f32, scale: f32) -> (String, f32) {
    for text in variants {
        if measure(true, nominal, text) <= max_w {
            return (text.clone(), nominal);
        }
    }
    let Some(shortest) = variants.last() else {
        return (String::new(), nominal);
    };
    let min = MIN_TIMESTAMP_PX * scale;
    let mut px = nominal - scale;
    while px >= min {
        if measure(true, px, shortest) <= max_w {
            return (shortest.clone(), px);
        }
        px -= scale;
    }
    (truncate(true, min, shortest, max_w), min)
}

// ---- strip ---------------------------------------------------------------------------------

/// The banner strip for a capture `width_px` wide, or `None` when there is nothing to show.
pub fn render_strip(style: &Style, meta: &Meta, note: &str, width_px: u32, dates: &dyn LocaleDates) -> Option<RgbaImage> {
    let height = height_px(style, meta, note);
    if height == 0 || width_px == 0 {
        return None;
    }
    let s = scale(meta);
    let mut img = RgbaImage::from_pixel(width_px, height, style.bg);
    let x = PADDING * s;
    let avail = width_px as f32 - 2.0 * x;
    let mut y = x;
    for line in lines(style, meta, note) {
        if y > x {
            y += LINE_GAP * s;
        }
        match line.role {
            Role::Timestamp => {
                let variants = timestamp_variants(&meta.captured_at, style, dates);
                let (text, px) = fit_timestamp(&variants, avail, TIMESTAMP_PX * s, s);
                draw_text(&mut img, true, px, &text, x, y, style.fg);
            }
            role => {
                let px = line_px(role) * s;
                let color = if role == Role::Note { style.fg } else { style.muted };
                draw_text(&mut img, false, px, &truncate(false, px, &line.text, avail), x, y, color);
            }
        }
        y += line_px(line.role) * s;
    }
    Some(img)
}

/// `strip` stacked above `content`, as one image.
pub fn composite(strip: Option<&RgbaImage>, content: &RgbaImage) -> RgbaImage {
    let Some(strip) = strip else {
        return content.clone();
    };
    let mut out = RgbaImage::new(content.width(), strip.height() + content.height());
    image::imageops::replace(&mut out, strip, 0, 0);
    image::imageops::replace(&mut out, content, 0, i64::from(strip.height()));
    out
}

/// `#rgb`, `#rrggbb` or `#rrggbbaa`.
pub fn parse_color(s: &str) -> Option<Rgba<u8>> {
    let hex = s.trim().strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        3 => {
            let d = |i: usize| u8::from_str_radix(hex.get(i..i + 1)?, 16).ok().map(|v| v * 17);
            Some(Rgba([d(0)?, d(1)?, d(2)?, 255]))
        }
        6 => Some(Rgba([byte(0)?, byte(2)?, byte(4)?, 255])),
        8 => Some(Rgba([byte(0)?, byte(2)?, byte(4)?, byte(6)?])),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dates the same on every machine: the formatter falls back to fixed patterns.
    struct NoDates;
    impl LocaleDates for NoDates {
        fn format(&self, _: i64, _: DateStyle, _: DateStyle, _: Option<&str>, _: Option<&str>) -> Option<String> {
            None
        }
    }

    fn style() -> Style {
        let mut s = Style::from(&Settings::default());
        s.timestamp_format = "iso-8601".into();
        s
    }

    fn meta(dpr: f64) -> Meta {
        Meta {
            captured_at: "2026-11-24T01:29:55.000Z".into(),
            window_title: "Example Domain — Safari".into(),
            page_title: "Example Domain".into(),
            page_url: "https://example.com/".into(),
            profile: "Work".into(),
            dpr,
        }
    }

    #[test]
    fn lines_follow_the_settings_and_skip_what_the_capture_does_not_know() {
        let mut s = style();
        assert_eq!(lines(&s, &meta(1.0), "").iter().map(|l| l.role).collect::<Vec<_>>(), vec![Role::Timestamp, Role::Muted]);

        s.show_page_title = true;
        s.show_page_url = true;
        s.show_browser_profile = true;
        let all = lines(&s, &meta(1.0), "  a note ");
        let texts: Vec<_> = all.iter().map(|l| l.text.as_str()).collect();
        // The window title starts with the page title, so it is left out as redundant.
        assert_eq!(texts, vec!["", "Example Domain", "https://example.com/", "Profile: Work", "a note"]);

        let snip = Meta { window_title: String::new(), page_title: String::new(), page_url: String::new(), profile: String::new(), ..meta(1.0) };
        assert_eq!(lines(&s, &snip, "").len(), 1, "a snipped region shows only the timestamp");

        s.show_timestamp = false;
        assert!(lines(&s, &snip, "").is_empty());
        assert_eq!(height_px(&s, &snip, ""), 0);
    }

    #[test]
    fn height_matches_the_page_layout() {
        let s = style();
        // padding 16 * 2 + timestamp 20 + gap 6 + title 14 = 72 CSS px.
        assert_eq!(css_height(&s, &meta(1.0), ""), 72.0);
        assert_eq!(height_px(&s, &meta(2.0), ""), 144);
        assert_eq!(css_height(&s, &meta(1.0), "note"), 72.0 + 6.0 + 14.0);
    }

    #[test]
    fn timestamps_shed_precision_in_order() {
        let mut s = style();
        assert_eq!(
            timestamp_variants("2026-11-24T01:29:55.000Z", &s, &NoDates),
            vec!["2026-11-24 01:29:55Z", "2026-11-24 01:29Z", "2026-11-24 01:29"]
        );
        s.timezone = "Indian/Mauritius".into();
        assert_eq!(
            timestamp_variants("2026-11-24T01:29:55.000Z", &s, &NoDates),
            vec![
                "2026-11-24 05:29:55 Indian/Mauritius",
                "2026-11-24 05:29 Indian/Mauritius",
                "2026-11-24 05:29"
            ]
        );
        s.timestamp_format = "utc-human".into();
        assert_eq!(
            timestamp_variants("2026-11-24T01:29:55.000Z", &s, &NoDates),
            vec!["24 Nov 2026, 01:29:55 UTC", "24 Nov 2026, 01:29 UTC", "24/11/2026, 01:29 UTC"]
        );
        assert!(timestamp_variants("not a date", &s, &NoDates).is_empty());
    }

    #[test]
    fn a_narrow_capture_shrinks_then_truncates_the_timestamp() {
        let variants = vec!["2026-11-24 01:29:55Z".to_string(), "2026-11-24 01:29".to_string()];
        let (text, px) = fit_timestamp(&variants, 10_000.0, 20.0, 1.0);
        assert_eq!((text.as_str(), px), ("2026-11-24 01:29:55Z", 20.0));
        let short_w = measure(true, 20.0, "2026-11-24 01:29") + 0.5;
        assert_eq!(fit_timestamp(&variants, short_w, 20.0, 1.0).0, "2026-11-24 01:29");
        let (shrunk, px) = fit_timestamp(&variants, measure(true, 15.0, "2026-11-24 01:29") + 0.5, 20.0, 1.0);
        assert_eq!(shrunk, "2026-11-24 01:29");
        assert!((11.0..20.0).contains(&px));
        let (cut, px) = fit_timestamp(&variants, 40.0, 20.0, 1.0);
        assert!(cut.ends_with('…') && px == 11.0, "{cut} {px}");
    }

    #[test]
    fn colors_parse_like_css_hex() {
        assert_eq!(parse_color("#1f2937"), Some(Rgba([0x1f, 0x29, 0x37, 255])));
        assert_eq!(parse_color("#fff"), Some(Rgba([255, 255, 255, 255])));
        assert_eq!(parse_color("#00000080"), Some(Rgba([0, 0, 0, 0x80])));
        assert_eq!(parse_color("red"), None);
    }

    #[test]
    fn composite_puts_the_strip_above_the_content() {
        let content = RgbaImage::from_pixel(4, 3, Rgba([9, 9, 9, 255]));
        let strip = RgbaImage::from_pixel(4, 2, Rgba([1, 1, 1, 255]));
        let out = composite(Some(&strip), &content);
        assert_eq!(out.dimensions(), (4, 5));
        assert_eq!(out.get_pixel(0, 1), &Rgba([1, 1, 1, 255]));
        assert_eq!(out.get_pixel(0, 2), &Rgba([9, 9, 9, 255]));
        assert_eq!(composite(None, &content).dimensions(), (4, 3));
    }

    /// Golden images: the strip for every setting combination at scale 1 and 2. Regenerate
    /// with `GLYPHIO_BLESS=1 cargo test banner` after an intended change and look at them.
    #[test]
    fn strips_match_the_golden_images() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/banner");
        let bless = std::env::var_os("GLYPHIO_BLESS").is_some();
        if bless {
            std::fs::create_dir_all(&dir).unwrap();
        }
        let mut failures = Vec::new();
        for (name, setup) in combinations() {
            for dpr in [1.0, 2.0] {
                let mut s = style();
                setup(&mut s);
                let note = if name.contains("note") { "Ticket 4821: checkout total wrong" } else { "" };
                let strip = render_strip(&s, &meta(dpr), note, (480.0 * dpr) as u32, &NoDates).unwrap();
                let file = dir.join(format!("{name}@{dpr}x.png"));
                if bless {
                    strip.save(&file).unwrap();
                    continue;
                }
                let golden = image::open(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display())).to_rgba8();
                if golden != strip {
                    failures.push(file.display().to_string());
                }
            }
        }
        assert!(failures.is_empty(), "banner differs from golden images: {failures:?}");
    }

    type Setup = fn(&mut Style);
    fn combinations() -> Vec<(&'static str, Setup)> {
        vec![
            ("default", |_| {}),
            ("timestamp-only", |s| s.show_window_title = false),
            ("page-details", |s| {
                s.show_page_title = true;
                s.show_page_url = true;
                s.show_browser_profile = true;
            }),
            ("utc-note", |s| s.timestamp_format = "utc-human".into()),
            ("light-colors", |s| {
                s.bg = Rgba([255, 255, 255, 255]);
                s.fg = Rgba([17, 24, 39, 255]);
                s.muted = Rgba([75, 85, 99, 255]);
            }),
        ]
    }

    #[test]
    fn a_very_narrow_capture_still_gets_a_legible_strip() {
        let s = style();
        let strip = render_strip(&s, &meta(1.0), "", 60, &NoDates).unwrap();
        assert_eq!(strip.dimensions(), (60, 72));
    }
}
