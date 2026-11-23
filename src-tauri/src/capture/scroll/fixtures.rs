//! Synthetic pages that look like the real thing, for the stitching tests.
//!
//! Hash noise (every row unique) is the easy case for a stitcher. Real pages are mostly
//! background: short lines of text with blank space between paragraphs, the same letters over
//! and over, flat code blocks, cards that repeat their layout at a fixed pitch, and a header,
//! composer and sidebar that never move. These pages have all of that.
//!
//! Pages are drawn a row at a time from their layout, so a 60 000 px page never has to exist
//! in memory: a frame is drawn from the rows it shows.

use image::RgbaImage;

pub type Rgb = [u8; 3];

const WHITE: Rgb = [255, 255, 255];
const INK: Rgb = [33, 37, 41];
const SOFT_INK: Rgb = [108, 117, 125];
const LINK: Rgb = [13, 110, 253];
const CODE_BG: Rgb = [246, 248, 250];
const CARD_BG: Rgb = [250, 250, 252];
const RULE: Rgb = [222, 226, 230];
const PAGE_BG: Rgb = [240, 242, 245];

/// Something that can draw its rows.
pub trait Rows {
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    /// Draw row `y` into `out` (RGBA, `width` pixels).
    fn row(&self, y: u32, out: &mut [u8]);
}

/// Rows `top..top + h` of `page` as an image.
pub fn render(page: &dyn Rows, top: u32, h: u32) -> RgbaImage {
    let w = page.width();
    let mut img = RgbaImage::new(w, h);
    let stride = w as usize * 4;
    for (i, row) in img.chunks_exact_mut(stride).enumerate() {
        page.row(top + i as u32, row);
    }
    img
}

/// The whole page as an image (only for pages small enough to hold).
pub fn whole(page: &dyn Rows) -> RgbaImage {
    render(page, 0, page.height())
}

fn hash(a: u32, b: u32) -> u32 {
    let mut v = a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85EB_CA6B);
    v ^= v >> 15;
    v = v.wrapping_mul(0x2C1B_3C6D);
    v ^= v >> 12;
    v = v.wrapping_mul(0x297A_2D39);
    v ^ (v >> 15)
}

fn fill(out: &mut [u8], from: u32, to: u32, c: Rgb) {
    let (from, to) = (from as usize * 4, (to as usize * 4).min(out.len()));
    if from >= to {
        return;
    }
    let span = &mut out[from..to];
    if c[0] == c[1] && c[1] == c[2] && c[0] == 255 {
        span.fill(255);
        return;
    }
    // One pixel, then double the filled part: a handful of copies instead of one per pixel.
    span[..4].copy_from_slice(&[c[0], c[1], c[2], 255]);
    let mut done = 4;
    while done < span.len() {
        let n = done.min(span.len() - done);
        span.copy_within(..n, done);
        done += n;
    }
}

/// A line of text in a fixed-pitch font of 40 glyphs: the letters repeat, the lines do not.
#[derive(Clone, Copy)]
struct Text {
    /// Which line of the document, so every line has its own words.
    line: u32,
    /// Glyph cell in pixels; glyphs are drawn on a 5×7 grid scaled to fit.
    cell_w: u32,
    cell_h: u32,
    colour: Rgb,
    /// Fraction of the column the line fills, in hundredths (the last line of a paragraph is short).
    fill_pct: u32,
}

impl Text {
    /// Draw text row `r` (0..cell_h) between `x0` and `x1`.
    fn row(&self, r: u32, x0: u32, x1: u32, out: &mut [u8]) {
        let grid_y = r * 9 / self.cell_h.max(1);
        if grid_y == 0 || grid_y > 7 {
            return; // leading above and below the glyphs
        }
        let gy = grid_y - 1;
        let end = x0 + (x1 - x0) * self.fill_pct / 100;
        let (mut x, mut i) = (x0, 0u32);
        while x + self.cell_w <= end {
            let h = hash(self.line, i);
            // Word lengths 2–9, single-cell spaces.
            let word = 2 + h % 8;
            for _ in 0..word {
                if x + self.cell_w > end {
                    break;
                }
                let glyph = hash(self.line.wrapping_add(i), 7) % 40;
                let bits = hash(glyph, gy) & 0x1f;
                let px_w = (self.cell_w * 5 / 6).max(5);
                for gx in 0..5u32 {
                    if bits & (1 << gx) != 0 {
                        let a = x + gx * px_w / 5;
                        let b = x + (gx + 1) * px_w / 5;
                        fill(out, a, b.max(a + 1), self.colour);
                    }
                }
                x += self.cell_w;
                i += 1;
            }
            x += self.cell_w;
            i += 1;
        }
    }
}

#[derive(Clone, Copy)]
enum Block {
    /// Blank space.
    Gap,
    /// A heading: one line of large text.
    Heading { line: u32 },
    /// Lines of body text; the last one stops short.
    Paragraph { first_line: u32, lines: u32 },
    /// A code sample on a tinted background.
    Code { first_line: u32, lines: u32 },
    /// A picture: a smooth gradient with a little texture.
    Picture { seed: u32 },
    /// A horizontal rule.
    Rule,
}

/// A long article: headings, paragraphs, code samples, pictures, rules and blank space.
pub struct Article {
    width: u32,
    scale: u32,
    margin: u32,
    /// (top row, height, block), in page order.
    blocks: Vec<(u32, u32, Block)>,
    height: u32,
}

impl Article {
    /// An article at least `min_height` px tall at `scale` px per point.
    pub fn new(width: u32, min_height: u32, scale: u32, seed: u32) -> Self {
        let line_h = 22 * scale;
        let mut blocks = Vec::new();
        let (mut y, mut line, mut n) = (0u32, seed.wrapping_mul(1000), 0u32);
        let push = |blocks: &mut Vec<_>, y: &mut u32, h: u32, b: Block| {
            blocks.push((*y, h, b));
            *y += h;
        };
        push(&mut blocks, &mut y, 24 * scale, Block::Gap);
        while y < min_height {
            let r = hash(seed, n);
            n += 1;
            match r % 10 {
                0 => {
                    push(&mut blocks, &mut y, 28 * scale, Block::Gap);
                    push(&mut blocks, &mut y, 34 * scale, Block::Heading { line });
                    line += 1;
                    push(&mut blocks, &mut y, 10 * scale, Block::Gap);
                }
                1 => {
                    let lines = 4 + r / 10 % 12;
                    push(&mut blocks, &mut y, 18 * lines * scale + 24 * scale, Block::Code { first_line: line, lines });
                    line += lines;
                    push(&mut blocks, &mut y, 14 * scale, Block::Gap);
                }
                2 => {
                    push(&mut blocks, &mut y, (160 + r / 10 % 200) * scale, Block::Picture { seed: r });
                    push(&mut blocks, &mut y, 14 * scale, Block::Gap);
                }
                3 => {
                    push(&mut blocks, &mut y, 20 * scale, Block::Gap);
                    push(&mut blocks, &mut y, scale, Block::Rule);
                    push(&mut blocks, &mut y, 20 * scale, Block::Gap);
                }
                _ => {
                    let lines = 2 + r / 10 % 7;
                    push(&mut blocks, &mut y, lines * line_h, Block::Paragraph { first_line: line, lines });
                    line += lines;
                    push(&mut blocks, &mut y, 12 * scale, Block::Gap);
                }
            }
        }
        push(&mut blocks, &mut y, 60 * scale, Block::Gap);
        Self { width, scale, margin: width / 10, blocks, height: y }
    }

    fn block_at(&self, y: u32) -> (u32, u32, Block) {
        let i = self.blocks.partition_point(|b| b.0 + b.1 <= y);
        self.blocks[i.min(self.blocks.len() - 1)]
    }
}

impl Rows for Article {
    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn row(&self, y: u32, out: &mut [u8]) {
        let (w, s, m) = (self.width, self.scale, self.margin);
        fill(out, 0, w, WHITE);
        let (top, h, block) = self.block_at(y);
        let r = y - top;
        match block {
            Block::Gap => {}
            Block::Rule => fill(out, m, w - m, RULE),
            Block::Heading { line } => {
                Text { line, cell_w: 15 * s, cell_h: 34 * s, colour: INK, fill_pct: 40 + line % 40 }
                    .row(r, m, w - m, out)
            }
            Block::Paragraph { first_line, lines } => {
                let line_h = h / lines;
                let i = r / line_h;
                let last = i + 1 == lines;
                let fill_pct = if last { 20 + hash(first_line, i) % 60 } else { 92 + hash(first_line, i) % 8 };
                let colour = if hash(first_line + i, 3).is_multiple_of(7) { LINK } else { INK };
                Text { line: first_line + i, cell_w: 8 * s, cell_h: line_h, colour, fill_pct }
                    .row(r % line_h, m, w - m, out);
            }
            Block::Code { first_line, lines } => {
                fill(out, m, w - m, CODE_BG);
                let pad = 12 * s;
                if r >= pad && r < h - pad {
                    let line_h = 18 * s;
                    let i = (r - pad) / line_h;
                    if i < lines {
                        let indent = (hash(first_line, i) % 4) * 4 * 8 * s;
                        Text {
                            line: first_line + i,
                            cell_w: 8 * s,
                            cell_h: line_h,
                            colour: SOFT_INK,
                            fill_pct: 30 + hash(first_line + i, 1) % 60,
                        }
                        .row((r - pad) % line_h, m + pad + indent, w - m - pad, out);
                    }
                }
            }
            Block::Picture { seed } => {
                let (x0, x1) = (m, w - m);
                for x in x0..x1 {
                    let t = hash(seed ^ x, r) & 7;
                    let i = x as usize * 4;
                    out[i] = (40 + (x - x0) * 150 / (x1 - x0)) as u8 + t as u8;
                    out[i + 1] = (60 + r * 120 / h) as u8 + t as u8;
                    out[i + 2] = (seed % 120 + 80) as u8;
                }
            }
        }
    }
}

/// A feed of cards that all share one layout (avatar, name, two lines, a thumbnail) at a fixed
/// pitch, on a tinted background: every card's chrome repeats exactly, only the text differs.
pub struct Feed {
    width: u32,
    scale: u32,
    cards: u32,
}

impl Feed {
    const CARD_PT: u32 = 132;
    const GAP_PT: u32 = 12;

    pub fn new(width: u32, cards: u32, scale: u32) -> Self {
        Self { width, scale, cards }
    }

    fn pitch(&self) -> u32 {
        (Self::CARD_PT + Self::GAP_PT) * self.scale
    }
}

impl Rows for Feed {
    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.cards * self.pitch() + Self::GAP_PT * self.scale
    }

    fn row(&self, y: u32, out: &mut [u8]) {
        let (w, s) = (self.width, self.scale);
        fill(out, 0, w, PAGE_BG);
        let gap = Self::GAP_PT * s;
        if y < gap {
            return;
        }
        let (card, r) = ((y - gap) / self.pitch(), (y - gap) % self.pitch());
        if r >= Self::CARD_PT * s || card >= self.cards {
            return;
        }
        let (x0, x1) = (w / 8, w - w / 8);
        fill(out, x0, x1, CARD_BG);
        if r == 0 || r == Self::CARD_PT * s - 1 {
            fill(out, x0, x1, RULE);
        }
        // Avatar: a disc, the same on every card.
        let (cx, cy, rad) = (x0 + 28 * s, 30 * s, 16 * s);
        let dy = r.abs_diff(cy);
        if dy < rad {
            let half = ((rad * rad - dy * dy) as f64).sqrt() as u32;
            fill(out, cx - half, cx + half, LINK);
        }
        let text_x = x0 + 56 * s;
        let thumb_x = x1 - 96 * s;
        // Name, then two lines of post text.
        for (i, (top, h, colour)) in [(14, 16, INK), (40, 18, SOFT_INK), (62, 18, SOFT_INK)].into_iter().enumerate() {
            if r >= top * s && r < (top + h) * s {
                Text {
                    line: card * 3 + i as u32,
                    cell_w: 7 * s,
                    cell_h: h * s,
                    colour,
                    fill_pct: if i == 0 { 30 } else { 60 + hash(card, i as u32) % 40 },
                }
                .row(r - top * s, text_x, thumb_x - 12 * s, out);
            }
        }
        // Thumbnail.
        if r >= 14 * s && r < 110 * s {
            for x in thumb_x..thumb_x + 80 * s {
                let v = hash(card, (x - thumb_x) / (8 * s) + r / (8 * s) * 64) & 0x7f;
                let i = x as usize * 4;
                out[i..i + 3].copy_from_slice(&[64 + v as u8, 90, 150 - v as u8 / 2]);
            }
        }
    }
}

/// Fixed chrome drawn over a scrolling page: a header, a composer bar and a left sidebar, as in
/// a chat or mail app. Each is its own little page of text.
pub struct Chrome {
    pub header: u32,
    pub footer: u32,
    pub sidebar: u32,
    scale: u32,
}

impl Chrome {
    pub fn new(header: u32, footer: u32, sidebar: u32, scale: u32) -> Self {
        Self { header, footer, sidebar, scale }
    }

    /// Draw the chrome over viewport row `vy` of a `view_h` viewport.
    pub fn row(&self, vy: u32, view_h: u32, width: u32, out: &mut [u8]) {
        let s = self.scale;
        if vy < self.header {
            fill(out, 0, width, [248, 249, 250]);
            if vy == self.header - 1 {
                fill(out, 0, width, RULE);
            }
            let top = self.header / 2 - 8 * s;
            if vy >= top && vy < top + 16 * s {
                Text { line: 9_000_001, cell_w: 9 * s, cell_h: 16 * s, colour: INK, fill_pct: 35 }
                    .row(vy - top, self.sidebar + 16 * s, width, out);
            }
        } else if vy >= view_h - self.footer {
            let r = vy - (view_h - self.footer);
            fill(out, 0, width, [248, 249, 250]);
            if r == 0 {
                fill(out, 0, width, RULE);
            }
            // The input box, with placeholder text.
            if r >= 10 * s && r < self.footer.saturating_sub(10 * s) {
                fill(out, self.sidebar + 12 * s, width - 12 * s, WHITE);
                let top = 10 * s + 8 * s;
                if r >= top && r < top + 14 * s {
                    Text { line: 9_000_002, cell_w: 7 * s, cell_h: 14 * s, colour: SOFT_INK, fill_pct: 25 }
                        .row(r - top, self.sidebar + 24 * s, width - 24 * s, out);
                }
            }
        }
        if self.sidebar > 0 {
            fill(out, 0, self.sidebar, [52, 58, 64]);
            let item_h = 28 * s;
            let i = vy / item_h;
            Text { line: 8_000_000 + i, cell_w: 6 * s, cell_h: item_h, colour: [222, 226, 230], fill_pct: 70 }
                .row(vy % item_h, 10 * s, self.sidebar - 10 * s, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_article_is_largely_background_with_long_blank_runs() {
        let a = Article::new(800, 6000, 2, 1);
        let img = whole(&a);
        let white = img.pixels().filter(|p| p.0 == [255, 255, 255, 255]).count();
        let share = white as f64 / (img.width() * img.height()) as f64;
        assert!(share > 0.4, "white share {share}");
        // Some stretch of blank rows taller than the stitcher's 48-row probe exists.
        let blank_row = |y: u32| (0..img.width()).all(|x| img.get_pixel(x, y).0 == [255, 255, 255, 255]);
        let longest = (0..img.height()).fold((0u32, 0u32), |(run, best), y| {
            let run = if blank_row(y) { run + 1 } else { 0 };
            (run, best.max(run))
        });
        assert!(longest.1 > 48, "longest blank run {}", longest.1);
    }

    #[test]
    fn feed_cards_repeat_their_layout_exactly() {
        let f = Feed::new(600, 6, 2);
        let pitch = f.pitch();
        let img = whole(&f);
        // The avatar column is the same on every card.
        let x = 600 / 8 + 28 * 2;
        for y in 24..24 + 60 {
            assert_eq!(img.get_pixel(x, y), img.get_pixel(x, y + pitch));
        }
    }
}
