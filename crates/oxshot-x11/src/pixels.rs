//! Pixel conversion from X server image data to straight RGBA.

use image::RgbaImage;

/// A 32-bit-per-pixel TrueColor format (depth 24 or 32).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelFormat {
    r_shift: u32,
    g_shift: u32,
    b_shift: u32,
    big_endian: bool,
}

impl PixelFormat {
    /// Returns `None` for unsupported formats (anything but 32 bpp with 8-bit channels).
    pub fn new(bpp: u8, r_mask: u32, g_mask: u32, b_mask: u32, big_endian: bool) -> Option<Self> {
        let shift = |m: u32| (m.count_ones() == 8 && (m >> m.trailing_zeros()) == 0xff).then(|| m.trailing_zeros());
        (bpp == 32).then_some(())?;
        Some(Self { r_shift: shift(r_mask)?, g_shift: shift(g_mask)?, b_shift: shift(b_mask)?, big_endian })
    }

    /// The common little-endian BGRX layout (`0x00RRGGBB` per pixel).
    #[cfg(test)]
    pub const BGRX: PixelFormat = PixelFormat { r_shift: 16, g_shift: 8, b_shift: 0, big_endian: false };

    fn is_bgrx(&self) -> bool {
        !self.big_endian && self.r_shift == 16 && self.g_shift == 8 && self.b_shift == 0
    }

    /// Converts rows of `src` (`stride` bytes apart) into `dst` (tightly packed RGBA rows).
    fn convert_rows(&self, src: &[u8], stride: usize, width: usize, dst: &mut [u8]) {
        let row_bytes = width * 4;
        for (src_row, dst_row) in src.chunks(stride).zip(dst.chunks_exact_mut(row_bytes)) {
            let src_row = &src_row[..row_bytes];
            if self.is_bgrx() {
                // B G R X -> R G B 255; written as u32 ops so LLVM vectorizes it.
                for (s, d) in src_row.as_chunks::<4>().0.iter().zip(dst_row.as_chunks_mut::<4>().0) {
                    let p = u32::from_le_bytes(*s);
                    let o = ((p >> 16) & 0xff) | (p & 0xff00) | ((p & 0xff) << 16) | 0xff00_0000;
                    d.copy_from_slice(&o.to_le_bytes());
                }
            } else {
                for (s, d) in src_row.as_chunks::<4>().0.iter().zip(dst_row.as_chunks_mut::<4>().0) {
                    let p = if self.big_endian { u32::from_be_bytes(*s) } else { u32::from_le_bytes(*s) };
                    d[0] = (p >> self.r_shift) as u8;
                    d[1] = (p >> self.g_shift) as u8;
                    d[2] = (p >> self.b_shift) as u8;
                    d[3] = 255;
                }
            }
        }
    }

    /// Converts a ZPixmap image (`stride` bytes per row) into an opaque RGBA image.
    /// Large images are converted on several threads.
    pub fn to_rgba(self, src: &[u8], stride: usize, width: u32, height: u32) -> RgbaImage {
        let (w, h) = (width as usize, height as usize);
        assert!(stride >= w * 4 && src.len() >= stride * h.saturating_sub(1) + w * 4, "short image data");
        let mut out = vec![0u8; w * h * 4];
        let threads = if w * h >= 1 << 20 {
            std::thread::available_parallelism().map_or(1, |n| n.get()).min(8)
        } else {
            1
        };
        if threads <= 1 || h < threads {
            self.convert_rows(src, stride, w, &mut out);
        } else {
            let rows_per = h.div_ceil(threads);
            std::thread::scope(|s| {
                for (i, dst) in out.chunks_mut(rows_per * w * 4).enumerate() {
                    let start = i * rows_per * stride;
                    let src = &src[start..];
                    s.spawn(move || self.convert_rows(src, stride, w, dst));
                }
            });
        }
        RgbaImage::from_raw(width, height, out).expect("buffer size matches")
    }
}

/// Converts XFixes cursor pixels (premultiplied ARGB in native u32) to straight RGBA.
pub fn cursor_to_rgba(width: u16, height: u16, argb: &[u32]) -> Option<RgbaImage> {
    let mut out = Vec::with_capacity(argb.len() * 4);
    for &p in argb.iter().take(width as usize * height as usize) {
        let a = (p >> 24) as u8;
        let un = |c: u32| -> u8 {
            let c = (c & 0xff) as u16;
            if a == 0 { 0 } else { ((c * 255 + a as u16 / 2) / a as u16).min(255) as u8 }
        };
        out.extend_from_slice(&[un(p >> 16), un(p >> 8), un(p), a]);
    }
    RgbaImage::from_raw(width as u32, height as u32, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgrx_to_rgba_with_stride() {
        // 2x2 image, stride 12 (one padding pixel per row).
        let src = [
            1, 2, 3, 0, 4, 5, 6, 0, 99, 99, 99, 99, //
            7, 8, 9, 0x7f, 10, 11, 12, 0, 99, 99, 99, 99,
        ];
        let img = PixelFormat::BGRX.to_rgba(&src, 12, 2, 2);
        assert_eq!(img.as_raw(), &[3, 2, 1, 255, 6, 5, 4, 255, 9, 8, 7, 255, 12, 11, 10, 255]);
    }

    #[test]
    fn generic_masks_and_big_endian() {
        // RGBX little-endian: r mask 0xff, g 0xff00, b 0xff0000.
        let f = PixelFormat::new(32, 0xff, 0xff00, 0xff0000, false).unwrap();
        assert_eq!(f.to_rgba(&[1, 2, 3, 0], 4, 1, 1).as_raw(), &[1, 2, 3, 255]);
        // BGRX visual, but MSB-first byte order: bytes are X R G B.
        let f = PixelFormat::new(32, 0xff0000, 0xff00, 0xff, true).unwrap();
        assert_eq!(f.to_rgba(&[0, 1, 2, 3], 4, 1, 1).as_raw(), &[1, 2, 3, 255]);
        assert!(PixelFormat::new(16, 0xf800, 0x7e0, 0x1f, false).is_none());
    }

    #[test]
    fn threaded_matches_single() {
        let (w, h) = (1500u32, 900u32);
        let src: Vec<u8> = (0..w * h * 4).map(|i| (i * 7 % 251) as u8).collect();
        let img = PixelFormat::BGRX.to_rgba(&src, w as usize * 4, w, h);
        for (s, d) in src.as_chunks::<4>().0.iter().zip(img.as_raw().as_chunks::<4>().0) {
            assert_eq!(d, &[s[2], s[1], s[0], 255]);
        }
    }

    #[test]
    fn cursor_unpremultiply() {
        let img = cursor_to_rgba(2, 1, &[0x80402010, 0x00000000]).unwrap();
        assert_eq!(img.as_raw(), &[128, 64, 32, 128, 0, 0, 0, 0]);
    }
}
