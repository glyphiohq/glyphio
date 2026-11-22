//! Joining scrolling-capture frames into one tall image. Pure: pixels in, pixels out.
//!
//! Three things make a naive "match the bottom of one frame to the top of the next" fail on
//! real pages, and each is handled here:
//!
//! * **Sticky bands.** A fixed header or composer bar is identical in every frame. Left in,
//!   it repeats down the result and anchors matches to the wrong rows. Rows that stay the same
//!   across all frames are detected at the top and bottom edges, kept once (header from the
//!   first frame, footer from the last) and left out of the overlap signature. Columns that
//!   stay the same (a fixed sidebar) are left out of the signature too.
//! * **A partial last scroll.** The page stops at its end, so the final overlap can be
//!   anything up to a full frame. The final frame is searched over the full range.
//! * **A known scroll distance.** When the platform measured how far the page moved, that is
//!   the expected overlap and a few pixels either way is enough to search.

use image::RgbaImage;

/// How far (px) around the nominal overlap the stitcher searches when nothing measured it.
const NARROW_SEARCH: i64 = 90;
/// Search slack around a measured scroll distance (rounding between points and pixels).
const MEASURED_SEARCH: i64 = 4;
/// Mean-absolute-difference (0-255) below which two bands are "the same".
const MATCH_THRESHOLD: f64 = 8.0;
/// Candidates within this much of the best difference are ties; the one nearest the expected
/// overlap wins, which keeps flat or repeated content from pulling the join elsewhere.
const TIE: f64 = 0.25;
/// Mean difference below which a row counts as unchanged between frames.
const SAME_ROW: f64 = 1.5;

/// Downsampled grayscale of every 4th pixel of a row band, skipping `skip_left` and
/// `skip_right` columns, taking every `row_step`th row — a cheap alignment-stable signature.
pub(super) fn band_gray(img: &RgbaImage, from_row: u32, rows: u32) -> Vec<u8> {
    band_signature(img, from_row, rows, 1, (0, 0))
}

fn band_signature(
    img: &RgbaImage,
    from_row: u32,
    rows: u32,
    row_step: u32,
    columns: (u32, u32),
) -> Vec<u8> {
    let mut out = Vec::with_capacity(((rows / row_step.max(1) + 1) * (img.width() / 4 + 1)) as usize);
    let (skip_left, skip_right) = columns;
    let x_end = img.width().saturating_sub(skip_right);
    for y in (from_row..(from_row + rows).min(img.height())).step_by(row_step.max(1) as usize) {
        for x in (skip_left..x_end).step_by(4) {
            let p = img.get_pixel(x, y);
            out.push(((p[0] as u16 + p[1] as u16 + p[2] as u16) / 3) as u8);
        }
    }
    out
}

pub(super) fn mad(a: &[u8], b: &[u8]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return f64::MAX;
    }
    let sum: u64 = a.iter().zip(b).map(|(x, y)| (*x as i64 - *y as i64).unsigned_abs()).sum();
    sum as f64 / a.len() as f64
}

/// Rows that stay the same across every frame at the top and bottom, and columns at the left
/// and right. Needs three frames to tell a fixed element from content that happened not to
/// move, and a middle that did move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct Sticky {
    pub top: u32,
    pub bottom: u32,
    pub left: u32,
    pub right: u32,
}

fn row_same(frames: &[RgbaImage], y: u32) -> bool {
    let base = band_gray(&frames[0], y, 1);
    frames[1..].iter().all(|f| mad(&base, &band_gray(f, y, 1)) < SAME_ROW)
}

fn column_same(frames: &[RgbaImage], x: u32) -> bool {
    let col = |img: &RgbaImage| -> Vec<u8> {
        (0..img.height())
            .step_by(4)
            .map(|y| {
                let p = img.get_pixel(x, y);
                ((p[0] as u16 + p[1] as u16 + p[2] as u16) / 3) as u8
            })
            .collect()
    };
    let base = col(&frames[0]);
    frames[1..].iter().all(|f| mad(&base, &col(f)) < SAME_ROW)
}

/// A band of one flat colour carries no information to tell it from blank page margin.
fn has_detail(img: &RgbaImage, from_row: u32, rows: u32) -> bool {
    let sig = band_gray(img, from_row, rows);
    match (sig.iter().min(), sig.iter().max()) {
        (Some(lo), Some(hi)) => hi - lo > 8,
        _ => false,
    }
}

pub(super) fn detect_sticky(frames: &[RgbaImage]) -> Sticky {
    let none = Sticky::default();
    if frames.len() < 3 || frames.iter().any(|f| f.dimensions() != frames[0].dimensions()) {
        return none;
    }
    let (w, h) = frames[0].dimensions();
    let cap = h / 3;
    let mut top = 0;
    while top < cap && row_same(frames, top) {
        top += 1;
    }
    let mut bottom = 0;
    while bottom < cap && row_same(frames, h - 1 - bottom) {
        bottom += 1;
    }
    if top > 0 && !has_detail(&frames[0], 0, top) {
        top = 0;
    }
    if bottom > 0 && !has_detail(&frames[0], h - bottom, bottom) {
        bottom = 0;
    }
    // Columns are only used to keep fixed sidebars out of the match.
    let col_cap = w / 3;
    let mut left = 0;
    while left < col_cap && column_same(frames, left) {
        left += 1;
    }
    let mut right = 0;
    while right < col_cap && column_same(frames, w - 1 - right) {
        right += 1;
    }
    // The middle must have moved, or "same everywhere" means no scrolling happened.
    let (core_top, core_h) = (top, h - top - bottom);
    if core_h < 16 || frames[0].dimensions() != frames[1].dimensions() {
        return none;
    }
    let moved = mad(
        &band_signature(&frames[0], core_top, core_h, 4, (left, right)),
        &band_signature(&frames[1], core_top, core_h, 4, (left, right)),
    ) > SAME_ROW;
    if !moved {
        return none;
    }
    Sticky { top, bottom, left, right }
}

/// Where `next`'s core starts matching `prev`'s core: how many core rows overlap.
struct Search<'a> {
    prev: &'a RgbaImage,
    next: &'a RgbaImage,
    sticky: Sticky,
    core_h: u32,
    probe: u32,
    expected: i64,
}

impl Search<'_> {
    fn columns(&self) -> (u32, u32) {
        (self.sticky.left, self.sticky.right)
    }

    fn diff(&self, prev_sig: &[u8], o: u32, row_step: u32) -> f64 {
        mad(
            prev_sig,
            &band_signature(self.next, self.sticky.top + o - self.probe, self.probe, row_step, self.columns()),
        )
    }

    fn prev_sig(&self, row_step: u32) -> Vec<u8> {
        band_signature(
            self.prev,
            self.sticky.top + self.core_h - self.probe,
            self.probe,
            row_step,
            self.columns(),
        )
    }

    /// Pick the best of `candidates` (difference, overlap): the lowest difference, with
    /// near-ties settled in favour of the overlap nearest the expected one.
    fn pick(&self, candidates: &[(f64, u32)]) -> Option<(f64, u32)> {
        let best = candidates.iter().map(|c| c.0).fold(f64::MAX, f64::min);
        candidates
            .iter()
            .filter(|c| c.0 <= best + TIE)
            .min_by_key(|c| (c.1 as i64 - self.expected).abs())
            .copied()
    }

    /// Exhaustive search within `lo..=hi`.
    fn window(&self, lo: u32, hi: u32) -> Option<(f64, u32)> {
        let sig = self.prev_sig(1);
        let all: Vec<(f64, u32)> = (lo..=hi).map(|o| (self.diff(&sig, o, 1), o)).collect();
        self.pick(&all)
    }

    /// The whole range: a coarse pass on every 4th row and offset, then refinement around the
    /// most promising coarse positions. Bounded cost whatever the frame height.
    fn full(&self) -> Option<(f64, u32)> {
        let (lo, hi) = (self.probe, self.core_h - 1);
        let coarse_sig = self.prev_sig(4);
        let mut coarse: Vec<(f64, u32)> =
            (lo..=hi).step_by(4).map(|o| (self.diff(&coarse_sig, o, 4), o)).collect();
        let best = coarse.iter().map(|c| c.0).fold(f64::MAX, f64::min);
        coarse.retain(|c| c.0 <= best + 2.0);
        coarse.sort_by_key(|c| (c.1 as i64 - self.expected).abs());
        let sig = self.prev_sig(1);
        let mut refined = Vec::new();
        for (_, centre) in coarse.into_iter().take(3) {
            for o in centre.saturating_sub(4).max(lo)..=(centre + 4).min(hi) {
                refined.push((self.diff(&sig, o, 1), o));
            }
        }
        self.pick(&refined)
    }
}

/// Join `frames` into one image.
///
/// `scroll_px[i]` is how far the page moved between frame `i` and `i + 1` in pixels when the
/// platform measured it. `nominal_scroll_px` is how far the loop asked for.
pub(super) fn stitch(
    frames: Vec<RgbaImage>,
    scroll_px: &[Option<i64>],
    nominal_scroll_px: i64,
) -> RgbaImage {
    assert!(!frames.is_empty(), "stitch called with ≥1 frame");
    let (w, h) = frames[0].dimensions();
    let sticky = detect_sticky(&frames);
    let core_h = h - sticky.top - sticky.bottom;
    let probe = 48.min(core_h / 4).max(8).min(core_h.saturating_sub(1).max(1));

    // Core rows each frame contributes, as (frame index, first core row).
    let mut pieces: Vec<(usize, u32)> = vec![(0, 0)];
    for i in 1..frames.len() {
        let measured = scroll_px.get(i - 1).copied().flatten().filter(|d| *d > 0);
        let expected = (core_h as i64 - measured.unwrap_or(nominal_scroll_px))
            .clamp(probe as i64, core_h as i64 - 1);
        let search = Search {
            prev: &frames[i - 1],
            next: &frames[i],
            sticky,
            core_h,
            probe,
            expected,
        };
        let last = i + 1 == frames.len();
        let radius = match (measured, last) {
            (Some(_), _) => Some(MEASURED_SEARCH),
            (None, true) => None,
            (None, false) => Some(NARROW_SEARCH),
        };
        let confident = |m: Option<(f64, u32)>| m.filter(|(d, _)| *d <= MATCH_THRESHOLD);
        let mut found = match radius {
            Some(r) => {
                let lo = (expected - r).clamp(probe as i64, core_h as i64 - 1) as u32;
                let hi = (expected + r).clamp(probe as i64, core_h as i64 - 1) as u32;
                confident(search.window(lo, hi))
            }
            None => confident(search.full()),
        };
        if found.is_none() && radius.is_some() {
            found = confident(search.full());
        }
        // No confident match anywhere (an animation, a bounce): trust the expected value.
        let overlap = found.map(|(_, o)| o).unwrap_or(expected as u32);
        if overlap < core_h {
            pieces.push((i, overlap));
        }
    }

    let n = frames.len();
    let total_h = sticky.top
        + sticky.bottom
        + pieces.iter().map(|&(_, from)| core_h - from).sum::<u32>();
    let mut out = RgbaImage::new(w, total_h);
    let mut y = 0u32;
    let mut copy = |src: &RgbaImage, from: u32, rows: u32, y: &mut u32| {
        let view = image::imageops::crop_imm(src, 0, from, w, rows);
        image::imageops::replace(&mut out, &view.to_image(), 0, *y as i64);
        *y += rows;
    };
    copy(&frames[0], 0, sticky.top, &mut y);
    for &(i, from) in &pieces {
        copy(&frames[i], sticky.top + from, core_h - from, &mut y);
    }
    copy(&frames[n - 1], h - sticky.bottom, sticky.bottom, &mut y);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// Unique-looking rows: a hash of (x, y), so no two rows repeat by accident.
    fn noise(w: u32, h: u32, seed: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            let mut v = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA6B) ^ seed.wrapping_mul(0xC2B2_AE35);
            v ^= v >> 15;
            v = v.wrapping_mul(0x2C1B_3C6D);
            v ^= v >> 12;
            Rgba([(v & 0xff) as u8, ((v >> 8) & 0xff) as u8, ((v >> 16) & 0xff) as u8, 255])
        })
    }

    fn cut(page: &RgbaImage, top: u32, h: u32) -> RgbaImage {
        image::imageops::crop_imm(page, 0, top, page.width(), h).to_image()
    }

    fn same_rows(a: &RgbaImage, b: &RgbaImage) {
        assert_eq!(a.dimensions(), b.dimensions(), "dimensions");
        assert!(a.as_raw() == b.as_raw(), "pixels differ");
    }

    #[test]
    fn stitch_reconstructs_a_tall_page_from_overlapping_frames() {
        let page = noise(64, 600, 1);
        let frames: Vec<_> = [0, 120, 240, 360, 400].iter().map(|&t| cut(&page, t, 200)).collect();
        // The last scroll is partial (40 of 120 rows), the case the old ±90 search missed.
        let out = stitch(frames, &[None; 4], 120);
        same_rows(&out, &page);
    }

    #[test]
    fn a_partial_final_scroll_at_realistic_sizes_leaves_no_repeated_band() {
        // 1 600 px viewport over a 3 300 px page: offsets 0, 960, 1700 — the last scroll is
        // 740 of 960, so the true overlap is 860, far from the nominal 640.
        let page = noise(48, 3300, 2);
        let frames: Vec<_> = [0, 960, 1700].iter().map(|&t| cut(&page, t, 1600)).collect();
        same_rows(&stitch(frames.clone(), &[None, None], 960), &page);
        same_rows(&stitch(frames, &[Some(960), Some(740)], 960), &page);
    }

    #[test]
    fn a_block_that_really_appears_twice_stays_twice() {
        let mut page = noise(48, 3300, 3);
        let block = cut(&page, 1000, 120);
        image::imageops::replace(&mut page, &block, 0, 1900);
        let frames: Vec<_> = [0, 960, 1700].iter().map(|&t| cut(&page, t, 1600)).collect();
        same_rows(&stitch(frames, &[None, None], 960), &page);
    }

    /// A window with a fixed header, a fixed composer bar and a fixed sidebar around a
    /// scrolling document.
    fn app_frames(offsets: &[u32]) -> (Vec<RgbaImage>, RgbaImage, RgbaImage, RgbaImage) {
        let (w, view_h, head, foot, side) = (64u32, 1600u32, 80u32, 100u32, 8u32);
        let doc = noise(w, 3300, 4);
        let header = noise(w, head, 5);
        let footer = noise(w, foot, 6);
        let frames = offsets
            .iter()
            .map(|&off| {
                let mut f = cut(&doc, off, view_h);
                image::imageops::replace(&mut f, &header, 0, 0);
                image::imageops::replace(&mut f, &footer, 0, (view_h - foot) as i64);
                let sidebar = noise(side, view_h, 7);
                image::imageops::replace(&mut f, &sidebar, 0, 0);
                f
            })
            .collect();
        (frames, doc, header, footer)
    }

    #[test]
    fn sticky_bands_are_found_from_three_frames() {
        let (frames, ..) = app_frames(&[0, 960, 1700]);
        let s = detect_sticky(&frames);
        assert_eq!((s.top, s.bottom, s.left), (80, 100, 8));
        // Two frames cannot tell fixed from unmoved.
        assert_eq!(detect_sticky(&frames[..2]), Sticky::default());
    }

    #[test]
    fn a_page_with_nothing_fixed_has_no_sticky_bands() {
        let page = noise(48, 3300, 8);
        let frames: Vec<_> = [0, 960, 1700].iter().map(|&t| cut(&page, t, 1600)).collect();
        assert_eq!(detect_sticky(&frames), Sticky::default());
    }

    #[test]
    fn headers_and_footers_appear_once() {
        // Visible document rows per frame are 1420 (1600 less the two bars); scrolling 960.
        let (frames, doc, header, footer) = app_frames(&[0, 960, 1700]);
        for scrolls in [[None, None], [Some(960), Some(740)]] {
            let out = stitch(frames.clone(), &scrolls, 960);
            // header + document rows [80, 3200) + footer
            let want_h = 80 + (3200 - 80) + 100;
            assert_eq!(out.height(), want_h, "{scrolls:?}");
            for x in 8..out.width() {
                for y in (0..80).step_by(3) {
                    assert_eq!(out.get_pixel(x, y), header.get_pixel(x, y), "header row {y}");
                }
                for y in (80..3200).step_by(7) {
                    assert_eq!(out.get_pixel(x, y), doc.get_pixel(x, y), "document row {y}");
                }
                for y in 0..100 {
                    assert_eq!(out.get_pixel(x, 3200 + y), footer.get_pixel(x, y), "footer row {y}");
                }
            }
        }
    }

    #[test]
    fn a_single_frame_is_returned_as_is() {
        let page = noise(32, 100, 9);
        same_rows(&stitch(vec![page.clone()], &[], 60), &page);
    }
}
