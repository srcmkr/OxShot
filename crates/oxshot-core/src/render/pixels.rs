//! Pixel-level helpers: straight <-> premultiplied conversion, backdrop sampling, fast
//! Gaussian approximation (3-pass box blur), pixelation and drop shadows.

use image::RgbaImage;
use tiny_skia::{Pixmap, PixmapPaint, Transform};

use crate::geom::IRect;

/// Premultiplies one straight RGBA pixel.
#[inline]
fn premul(p: [u8; 4]) -> [u8; 4] {
    match p[3] {
        255 => p,
        0 => [0; 4],
        a => {
            let m = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
            [m(p[0]), m(p[1]), m(p[2]), a]
        }
    }
}

/// Converts one premultiplied RGBA pixel back to straight alpha.
#[inline]
fn demul(p: [u8; 4]) -> [u8; 4] {
    match p[3] {
        255 => p,
        0 => [0; 4],
        a => {
            let d = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
            [d(p[0]), d(p[1]), d(p[2]), a]
        }
    }
}

/// Premultiplied copy of a whole straight image.
pub(crate) fn pixmap_from_straight(img: &RgbaImage) -> Option<Pixmap> {
    let mut pm = Pixmap::new(img.width(), img.height())?;
    for (dst, src) in pm.data_mut().as_chunks_mut::<4>().0.iter_mut().zip(img.as_raw().as_chunks::<4>().0.iter()) {
        dst.copy_from_slice(&premul([src[0], src[1], src[2], src[3]]));
    }
    Some(pm)
}

/// Straight-alpha copy of `pm` (or of the sub-rect `r` of it, in pixmap coordinates).
pub(crate) fn straight_from_pixmap(pm: &Pixmap, r: Option<IRect>) -> RgbaImage {
    let full = IRect::new(0, 0, pm.width(), pm.height());
    let r = r.and_then(|r| r.intersect(&full)).unwrap_or(full);
    let mut out = RgbaImage::new(r.w, r.h);
    let stride = pm.width() as usize * 4;
    let data = pm.data();
    for (row, dst_row) in out.as_mut().chunks_exact_mut(r.w as usize * 4).enumerate() {
        let start = (r.y as usize + row) * stride + r.x as usize * 4;
        let src_row = &data[start..start + r.w as usize * 4];
        for (d, s) in dst_row.as_chunks_mut::<4>().0.iter_mut().zip(src_row.as_chunks::<4>().0.iter()) {
            d.copy_from_slice(&demul([s[0], s[1], s[2], s[3]]));
        }
    }
    out
}

/// What a backdrop-reading shape samples: the composite of everything below it.
#[derive(Clone, Copy)]
pub(crate) enum Backdrop<'a> {
    Straight { origin: (i32, i32), img: &'a RgbaImage },
    Premul { origin: (i32, i32), pm: &'a Pixmap },
}

impl Backdrop<'_> {
    fn extent(&self) -> IRect {
        match self {
            Backdrop::Straight { origin, img } => {
                IRect::new(origin.0, origin.1, img.width(), img.height())
            }
            Backdrop::Premul { origin, pm } => IRect::new(origin.0, origin.1, pm.width(), pm.height()),
        }
    }

    /// Premultiplied pixel at absolute (image space) coordinates; caller guarantees in-bounds.
    #[inline]
    fn px(&self, x: i32, y: i32) -> [u8; 4] {
        match self {
            Backdrop::Straight { origin, img } => {
                let p = img.get_pixel((x - origin.0) as u32, (y - origin.1) as u32).0;
                premul(p)
            }
            Backdrop::Premul { origin, pm } => {
                let i = ((y - origin.1) as usize * pm.width() as usize + (x - origin.0) as usize) * 4;
                let d = pm.data();
                [d[i], d[i + 1], d[i + 2], d[i + 3]]
            }
        }
    }

    /// Premultiplied copy of the absolute rect `r`. Outside the backdrop the result is
    /// transparent, or (with `clamp`) the nearest edge pixel is repeated.
    pub(crate) fn region(&self, r: IRect, clamp: bool) -> Option<Pixmap> {
        let mut pm = Pixmap::new(r.w.max(1), r.h.max(1))?;
        let ext = self.extent();
        if ext.w == 0 || ext.h == 0 {
            return Some(pm);
        }
        let w = r.w as usize;
        let data = pm.data_mut();
        for row in 0..r.h as usize {
            let mut y = r.y + row as i32;
            if !(ext.y..ext.bottom()).contains(&y) {
                if !clamp {
                    continue;
                }
                y = y.clamp(ext.y, ext.bottom() - 1);
            }
            let dst = &mut data[row * w * 4..(row + 1) * w * 4];
            // Fast path: copy the in-bounds span directly.
            let x0 = r.x.max(ext.x);
            let x1 = r.right().min(ext.right());
            if x1 > x0 {
                let off = (x0 - r.x) as usize * 4;
                match self {
                    Backdrop::Premul { origin, pm: src } => {
                        let s = ((y - origin.1) as usize * src.width() as usize
                            + (x0 - origin.0) as usize)
                            * 4;
                        let n = (x1 - x0) as usize * 4;
                        dst[off..off + n].copy_from_slice(&src.data()[s..s + n]);
                    }
                    Backdrop::Straight { .. } => {
                        for x in x0..x1 {
                            let o = (x - r.x) as usize * 4;
                            dst[o..o + 4].copy_from_slice(&self.px(x, y));
                        }
                    }
                }
            }
            if clamp {
                for (i, d) in dst.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let x = r.x + i as i32;
                    if x < ext.x || x >= ext.right() {
                        d.copy_from_slice(&self.px(x.clamp(ext.x, ext.right() - 1), y));
                    }
                }
            }
        }
        Some(pm)
    }
}

/// Box sizes approximating a Gaussian with `n` successive box blurs (Kutskir).
fn boxes_for_gauss(sigma: f32, n: usize) -> Vec<usize> {
    let w_ideal = (12.0 * sigma * sigma / n as f32 + 1.0).sqrt();
    let mut wl = w_ideal.floor() as i64;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wl = wl.max(1);
    let wu = wl + 2;
    let nf = n as f32;
    let wlf = wl as f32;
    let m_ideal =
        (12.0 * sigma * sigma - nf * wlf * wlf - 4.0 * nf * wlf - 3.0 * nf) / (-4.0 * wlf - 4.0);
    let m = m_ideal.round().max(0.0) as usize;
    (0..n).map(|i| if i < m { wl as usize } else { wu as usize }).collect()
}

/// One horizontal box pass with radius `r` over interleaved channels (edge clamped).
fn box_h(src: &[u8], dst: &mut [u8], w: usize, h: usize, ch: usize, r: usize) {
    let size = (2 * r + 1) as u32;
    let half = size / 2;
    for y in 0..h {
        let row = &src[y * w * ch..(y + 1) * w * ch];
        let out = &mut dst[y * w * ch..(y + 1) * w * ch];
        for c in 0..ch {
            let at = |x: isize| row[(x.clamp(0, w as isize - 1) as usize) * ch + c] as u32;
            let mut acc: u32 = (-(r as isize)..=r as isize).map(at).sum();
            for x in 0..w {
                out[x * ch + c] = ((acc + half) / size) as u8;
                acc += at(x as isize + r as isize + 1);
                acc -= at(x as isize - r as isize);
            }
        }
    }
}

/// One vertical box pass (row-major, cache friendly running column sums).
fn box_v(src: &[u8], dst: &mut [u8], w: usize, h: usize, ch: usize, r: usize) {
    let size = (2 * r + 1) as u32;
    let half = size / 2;
    let stride = w * ch;
    let row = |y: isize| y.clamp(0, h as isize - 1) as usize * stride;
    let mut acc = vec![0u32; stride];
    for dy in -(r as isize)..=r as isize {
        let o = row(dy);
        for (a, s) in acc.iter_mut().zip(&src[o..o + stride]) {
            *a += *s as u32;
        }
    }
    for y in 0..h {
        let out = &mut dst[y * stride..(y + 1) * stride];
        for (d, a) in out.iter_mut().zip(&acc) {
            *d = ((*a + half) / size) as u8;
        }
        let add = row(y as isize + r as isize + 1);
        let sub = row(y as isize - r as isize);
        for i in 0..stride {
            acc[i] = acc[i] + src[add + i] as u32 - src[sub + i] as u32;
        }
    }
}

/// In-place Gaussian blur approximation (3 box passes per axis) of interleaved data
/// with `ch` channels. Works on premultiplied RGBA or single-channel alpha.
pub(crate) fn gaussian_blur(data: &mut [u8], w: usize, h: usize, ch: usize, sigma: f32) {
    if sigma < 0.3 || w == 0 || h == 0 {
        return;
    }
    let mut tmp = vec![0u8; data.len()];
    for size in boxes_for_gauss(sigma, 3) {
        let r = (size - 1) / 2;
        if r == 0 {
            continue;
        }
        box_h(data, &mut tmp, w, h, ch, r);
        box_v(&tmp, data, w, h, ch, r);
    }
}

/// Replaces each `block`×`block` cell (aligned to the pixmap origin) by its average.
pub(crate) fn pixelate(pm: &mut Pixmap, block: u32) {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let b = block.max(1) as usize;
    let data = pm.data_mut();
    for by in (0..h).step_by(b) {
        for bx in (0..w).step_by(b) {
            let (x1, y1) = ((bx + b).min(w), (by + b).min(h));
            let mut sum = [0u32; 4];
            for y in by..y1 {
                for x in bx..x1 {
                    let i = (y * w + x) * 4;
                    for c in 0..4 {
                        sum[c] += data[i + c] as u32;
                    }
                }
            }
            let n = ((x1 - bx) * (y1 - by)) as u32;
            let avg = sum.map(|s| ((s + n / 2) / n) as u8);
            for y in by..y1 {
                for x in bx..x1 {
                    let i = (y * w + x) * 4;
                    data[i..i + 4].copy_from_slice(&avg);
                }
            }
        }
    }
}

/// Multiplies every (premultiplied) pixel by `opacity`.
pub(crate) fn apply_opacity(pm: &mut Pixmap, opacity: f32) {
    let o = (opacity.clamp(0.0, 1.0) * 256.0) as u32;
    if o >= 256 {
        return;
    }
    for v in pm.data_mut() {
        *v = ((*v as u32 * o) >> 8) as u8;
    }
}

/// Drop shadow parameters for a shadow strength in 0..=1: (offset, blur sigma, alpha).
pub fn shadow_params(strength: f32) -> (f32, f32, f32) {
    let s = strength.clamp(0.0, 1.0);
    (3.0, 4.0, 0.45 * s)
}

/// Returns `pm` drawn over its own soft black drop shadow.
pub(crate) fn with_drop_shadow(pm: &Pixmap, strength: f32) -> Option<Pixmap> {
    let (offset, sigma, alpha) = shadow_params(strength);
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let mut a: Vec<u8> = pm.data().as_chunks::<4>().0.iter().map(|p| p[3]).collect();
    gaussian_blur(&mut a, w, h, 1, sigma);
    let mut out = Pixmap::new(pm.width(), pm.height())?;
    let off = offset.round() as usize;
    let k = (alpha * 256.0) as u32;
    let data = out.data_mut();
    for y in off..h {
        for x in off..w {
            let v = ((a[(y - off) * w + (x - off)] as u32 * k) >> 8) as u8;
            data[(y * w + x) * 4 + 3] = v;
        }
    }
    out.draw_pixmap(0, 0, pm.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premul_roundtrip_is_close() {
        for a in [1u8, 17, 128, 254, 255] {
            for c in [0u8, 1, 100, 200, 255] {
                let p = demul(premul([c, c, c, a]));
                if a > 64 {
                    assert!((p[0] as i32 - c as i32).abs() <= 2, "a={a} c={c} got {p:?}");
                }
            }
        }
    }

    #[test]
    fn blur_preserves_flat_areas() {
        let mut d = vec![200u8; 40 * 30 * 4];
        gaussian_blur(&mut d, 40, 30, 4, 5.0);
        assert!(d.iter().all(|v| *v == 200));
    }
}
