//! A PNG encoder fed one row at a time, for an image whose height is only known at the end.
//!
//! A scrolling capture can be 60 000 px tall at Retina scale. Held as RGBA that is hundreds of
//! megabytes before the encoder even starts, so the stitcher hands each fresh row here instead:
//! it is filtered and deflated straight away, and only the compressed stream is kept. PNG puts
//! the height in its header, before the pixel data, which is why the file is assembled in
//! [`PngStream::finish`] rather than written as it goes.

use std::io::Write;

use flate2::write::ZlibEncoder;
use flate2::Compression;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
/// Largest IDAT chunk written. Any size up to 2³¹ is legal; this keeps decoders' buffers small.
const IDAT_CHUNK: usize = 1 << 20;
const BPP: usize = 4;

/// RGBA8 rows in, a PNG file out.
pub(super) struct PngStream {
    width: u32,
    height: u32,
    stride: usize,
    prev: Vec<u8>,
    /// One filtered row per candidate filter, reused for every row.
    scratch: [Vec<u8>; 5],
    z: ZlibEncoder<Vec<u8>>,
}

impl PngStream {
    pub fn new(width: u32) -> Self {
        let stride = width as usize * BPP;
        Self {
            width,
            height: 0,
            stride,
            prev: vec![0; stride],
            scratch: std::array::from_fn(|_| vec![0; stride + 1]),
            // Level 1: a screenshot is mostly flat runs, which the fast level already finds,
            // and the user is waiting for the editor to open.
            z: ZlibEncoder::new(Vec::new(), Compression::fast()),
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Append whole rows of `width` RGBA pixels.
    pub fn write_rows(&mut self, rgba: &[u8]) -> std::io::Result<()> {
        assert_eq!(rgba.len() % self.stride, 0, "partial row");
        for row in rgba.chunks_exact(self.stride) {
            if self.height > 0 && row == self.prev.as_slice() {
                // A repeat of the row above (blank space, a flat panel, the inside of a
                // letter): the Up filter makes it all zeros, the best any filter can do.
                let up = &mut self.scratch[2];
                up[0] = 2;
                up[1..].fill(0);
                self.z.write_all(up)?;
            } else {
                let best = filter_row(row, &self.prev, &mut self.scratch);
                self.z.write_all(&self.scratch[best])?;
                self.prev.copy_from_slice(row);
            }
            self.height += 1;
        }
        Ok(())
    }

    /// The finished file and its height.
    pub fn finish(self) -> std::io::Result<(Vec<u8>, u32)> {
        let idat = self.z.finish()?;
        let chunks = idat.len().div_ceil(IDAT_CHUNK).max(1);
        let mut out = Vec::with_capacity(SIGNATURE.len() + 25 + idat.len() + chunks * 12 + 12);
        out.extend_from_slice(&SIGNATURE);
        let mut ihdr = [0u8; 13];
        ihdr[..4].copy_from_slice(&self.width.to_be_bytes());
        ihdr[4..8].copy_from_slice(&self.height.to_be_bytes());
        ihdr[8] = 8; // bit depth
        ihdr[9] = 6; // RGBA; compression, filter and interlace methods stay 0
        chunk(&mut out, b"IHDR", &ihdr);
        if idat.is_empty() {
            chunk(&mut out, b"IDAT", &[]);
        }
        for part in idat.chunks(IDAT_CHUNK) {
            chunk(&mut out, b"IDAT", part);
        }
        chunk(&mut out, b"IEND", &[]);
        Ok((out, self.height))
    }
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc.finalize().to_be_bytes());
}

/// Filter `row` every way PNG allows and return the index of the one most likely to compress
/// best (the smallest sum of bytes read as signed values, libpng's heuristic).
fn filter_row(row: &[u8], prev: &[u8], out: &mut [Vec<u8>; 5]) -> usize {
    for (kind, buf) in out.iter_mut().enumerate() {
        buf[0] = kind as u8;
    }
    let [none, sub, up, avg, paeth] = out;
    none[1..].copy_from_slice(row);
    for i in 0..row.len() {
        let a = if i >= BPP { row[i - BPP] } else { 0 };
        let b = prev[i];
        let c = if i >= BPP { prev[i - BPP] } else { 0 };
        let x = row[i];
        sub[i + 1] = x.wrapping_sub(a);
        up[i + 1] = x.wrapping_sub(b);
        avg[i + 1] = x.wrapping_sub(((a as u16 + b as u16) / 2) as u8);
        paeth[i + 1] = x.wrapping_sub(paeth_predictor(a, b, c));
    }
    let cost = |buf: &Vec<u8>| -> u64 { buf[1..].iter().map(|&v| (v as i8).unsigned_abs() as u64).sum() };
    (0..5).min_by_key(|&k| cost(&out[k])).unwrap_or(0)
}

fn paeth_predictor(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;

    fn decode(png: &[u8]) -> RgbaImage {
        image::load_from_memory_with_format(png, image::ImageFormat::Png).expect("valid PNG").to_rgba8()
    }

    #[test]
    fn rows_written_in_pieces_decode_to_the_same_image() {
        let img = RgbaImage::from_fn(37, 91, |x, y| {
            image::Rgba([(x * 7 + y) as u8, (y * 3) as u8, (x ^ y) as u8, if (x + y) % 5 == 0 { 128 } else { 255 }])
        });
        let mut png = PngStream::new(37);
        let raw = img.as_raw();
        let stride = 37 * 4;
        // Uneven batches, as the stitcher sends them.
        for (from, to) in [(0, 1), (1, 40), (40, 41), (41, 91)] {
            png.write_rows(&raw[from * stride..to * stride]).unwrap();
        }
        assert_eq!(png.height(), 91);
        let (bytes, h) = png.finish().unwrap();
        assert_eq!(h, 91);
        assert_eq!(decode(&bytes), img);
    }

    #[test]
    fn repeated_rows_round_trip() {
        let mut img = RgbaImage::from_pixel(50, 40, image::Rgba([250, 250, 250, 255]));
        for x in 10..20 {
            for y in 5..9 {
                img.put_pixel(x, y, image::Rgba([10, 20, 30, 255]));
            }
        }
        let mut png = PngStream::new(50);
        png.write_rows(img.as_raw()).unwrap();
        assert_eq!(decode(&png.finish().unwrap().0), img);
    }

    #[test]
    fn a_large_image_is_split_over_several_data_chunks() {
        // Noise does not compress, so the stream is well over one chunk.
        let img = RgbaImage::from_fn(512, 1024, |x, y| {
            let v = (x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA6B)).wrapping_mul(0x2C1B_3C6D);
            image::Rgba([(v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, 255])
        });
        let mut png = PngStream::new(512);
        png.write_rows(img.as_raw()).unwrap();
        let (bytes, _) = png.finish().unwrap();
        assert!(bytes.len() > IDAT_CHUNK);
        assert_eq!(decode(&bytes), img);
    }

    #[test]
    fn flat_screenshot_rows_compress_to_a_small_fraction() {
        let img = RgbaImage::from_fn(800, 600, |x, y| {
            if y % 20 < 12 && x % 9 < 5 && (x / 60 + y / 20) % 4 != 0 {
                image::Rgba([30, 30, 30, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            }
        });
        let mut png = PngStream::new(800);
        png.write_rows(img.as_raw()).unwrap();
        let (bytes, _) = png.finish().unwrap();
        assert!(bytes.len() * 20 < img.as_raw().len(), "{} bytes", bytes.len());
        assert_eq!(decode(&bytes), img);
    }
}
