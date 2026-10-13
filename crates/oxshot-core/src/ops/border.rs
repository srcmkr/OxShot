//! Image border effects: rounded corners, border stroke, torn edges, drop shadow,
//! reflection, grayscale and rotation.

use image::RgbaImage;
use tiny_skia::{
    FillRule, FilterQuality, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap, PixmapPaint,
    Transform,
};

use super::BorderEffect;
use crate::render::pixels;

const KAPPA: f32 = 0.552_284_8;
/// Depth of a paper tear (px).
const TEAR: f32 = 10.0;

/// Deterministic hash noise in 0..1.
fn hash(i: i64, seed: u32) -> f32 {
    let mut x = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (seed as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

/// Smooth 1-D value noise.
fn value_noise(t: f32, seed: u32) -> f32 {
    let i = t.floor();
    let f = t - i;
    let s = f * f * (3.0 - 2.0 * f);
    let a = hash(i as i64, seed);
    let b = hash(i as i64 + 1, seed);
    a + (b - a) * s
}

/// Paper-tear profile in 0..1: a few octaves, the finest one nearly jagged.
fn tear(t: f32, seed: u32) -> f32 {
    let n = 0.5 * value_noise(t / 37.0, seed)
        + 0.3 * value_noise(t / 11.0, seed ^ 0x51)
        + 0.2 * hash((t / 3.0).floor() as i64, seed ^ 0xA7);
    n.clamp(0.0, 1.0)
}

/// Outline of the image: rounded rect with optional torn sides.
fn outline(w: f32, h: f32, fx: &BorderEffect) -> Option<Path> {
    let torn = [fx.torn.top, fx.torn.right, fx.torn.bottom, fx.torn.left];
    let amp = TEAR.min(w.min(h) / 6.0);
    let r = fx.corner_radius.clamp(0.0, w.min(h) / 2.0);
    let corners = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
    // Corner i (start of side i) is rounded if neither adjacent side is torn.
    let rad = |i: usize| if !torn[i] && !torn[(i + 3) % 4] { r } else { 0.0 };
    let side_len = |i: usize| if i.is_multiple_of(2) { w } else { h };
    let disp = |i: usize, t: f32| amp * (0.25 + 0.75 * tear(t, i as u32 * 7919 + 13));
    let mut pb = PathBuilder::new();
    let mut first = true;
    for side in 0..4 {
        let (sx, sy) = corners[side];
        let (ex, ey) = corners[(side + 1) % 4];
        let len = (ex - sx).abs().max((ey - sy).abs());
        let (dx, dy) = ((ex - sx) / len, (ey - sy) / len);
        // Inward normal for a clockwise outline in y-down space.
        let (nx, ny) = (-dy, dx);
        let r0 = rad(side);
        let r1 = rad((side + 1) % 4);
        let mut add = |x: f32, y: f32, pb: &mut PathBuilder| {
            if first {
                pb.move_to(x, y);
                first = false;
            } else {
                pb.line_to(x, y);
            }
        };
        if torn[side] {
            let steps = (len / 2.0).ceil().max(1.0) as usize;
            // Where the neighbouring side is torn too, keep clear of its tear line so the
            // corner is cut cleanly instead of poking out.
            let prev = (side + 3) % 4;
            let next = (side + 1) % 4;
            let prev_d = if torn[prev] { disp(prev, side_len(prev)) } else { 0.0 };
            let next_d = if torn[next] { disp(next, 0.0) } else { 0.0 };
            for k in 0..=steps {
                let t = len * k as f32 / steps as f32;
                let d = disp(side, t);
                let t = t + (prev_d - t).max(0.0) - (next_d - (len - t)).max(0.0);
                add(sx + dx * t + nx * d, sy + dy * t + ny * d, &mut pb);
            }
        } else {
            add(sx + dx * r0, sy + dy * r0, &mut pb);
            add(ex - dx * r1, ey - dy * r1, &mut pb);
        }
        if r1 > 0.0 {
            // Arc around the end corner into the next side.
            let (nsx, nsy) = corners[(side + 1) % 4];
            let (nex, ney) = corners[(side + 2) % 4];
            let nl = ((nex - nsx).abs()).max((ney - nsy).abs());
            let (ndx, ndy) = ((nex - nsx) / nl, (ney - nsy) / nl);
            let (ax, ay) = (ex - dx * r1, ey - dy * r1);
            let (bx, by) = (nsx + ndx * r1, nsy + ndy * r1);
            pb.cubic_to(
                ax + dx * r1 * KAPPA,
                ay + dy * r1 * KAPPA,
                bx - ndx * r1 * KAPPA,
                by - ndy * r1 * KAPPA,
                bx,
                by,
            );
        }
    }
    pb.close();
    pb.finish()
}

fn grayscale(img: &mut RgbaImage) {
    for p in img.pixels_mut() {
        let [r, g, b, a] = p.0;
        let l = (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32).round() as u8;
        p.0 = [l, l, l, a];
    }
}

/// Exact rotation by a multiple of 90° (clockwise quarter turns).
fn rotate_quarter(pm: &Pixmap, turns: u32) -> Option<Pixmap> {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let (nw, nh) = if turns % 2 == 1 { (h, w) } else { (w, h) };
    let mut out = Pixmap::new(nw as u32, nh as u32)?;
    let src = pm.data();
    let dst = out.data_mut();
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = match turns % 4 {
                1 => (h - 1 - y, x),
                2 => (w - 1 - x, h - 1 - y),
                3 => (y, w - 1 - x),
                _ => (x, y),
            };
            let s = (y * w + x) * 4;
            let d = (ny * nw + nx) * 4;
            dst[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    Some(out)
}

/// Rotates clockwise by `deg`, expanding the pixmap to fit.
fn rotate(pm: Pixmap, deg: f32) -> Pixmap {
    let deg = deg.rem_euclid(360.0);
    let quarter = (deg / 90.0).round();
    if (deg - quarter * 90.0).abs() < 0.01 {
        return rotate_quarter(&pm, quarter as u32 % 4).unwrap_or(pm);
    }
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    let a = deg.to_radians();
    let (s, c) = (a.sin().abs(), a.cos().abs());
    let nw = (w * c + h * s).ceil() as u32 + 2;
    let nh = (w * s + h * c).ceil() as u32 + 2;
    let Some(mut out) = Pixmap::new(nw, nh) else { return pm };
    let tf = Transform::from_translate(-w / 2.0, -h / 2.0)
        .post_rotate(deg)
        .post_translate(nw as f32 / 2.0, nh as f32 / 2.0);
    let paint = PixmapPaint { quality: FilterQuality::Bicubic, ..Default::default() };
    out.draw_pixmap(0, 0, pm.as_ref(), &paint, tf, None);
    out
}

/// The whole effect pipeline on a flattened image.
pub(crate) fn render(img: &RgbaImage, fx: &BorderEffect) -> RgbaImage {
    let mut img = img.clone();
    if img.width() == 0 || img.height() == 0 {
        return img;
    }
    if fx.grayscale {
        grayscale(&mut img);
    }
    let Some(mut content) = pixels::pixmap_from_straight(&img) else { return img };
    let (w, h) = (img.width() as f32, img.height() as f32);

    // Shape: rounded corners / torn edges, plus an inner border following the outline.
    let shaped = fx.corner_radius > 0.0
        || fx.torn.any()
        || fx.border_width > 0.0;
    if shaped && let Some(path) = outline(w, h, fx) {
        let Some(mut mask) = Mask::new(content.width(), content.height()) else { return img };
        mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
        if fx.border_width > 0.0 && fx.border_color.a > 0 {
            let mut p = Paint::default();
            let c = fx.border_color;
            p.set_color_rgba8(c.r, c.g, c.b, c.a);
            p.anti_alias = true;
            let stroke = tiny_skia::Stroke {
                width: fx.border_width * 2.0,
                line_join: LineJoin::Round,
                ..Default::default()
            };
            content.stroke_path(&path, &p, &stroke, Transform::identity(), None);
        }
        content.apply_mask(&mask);
    }

    if fx.rotation_deg.rem_euclid(360.0) != 0.0 {
        content = rotate(content, fx.rotation_deg);
    }
    let (cw, ch) = (content.width(), content.height());

    // Reflection height and drop shadow geometry.
    let refl = fx.reflection.clamp(0.0, 1.0);
    let rh = if refl > 0.0 { (ch as f32 * 0.3).round() as u32 } else { 0 };
    let sh = fx.shadow.clamp(0.0, 1.0);
    let (off, sigma) = (6.0 * sh, (10.0 * sh).max(1.0));
    let margin = if sh > 0.0 { (sigma * 3.0).ceil() as i32 } else { 0 };
    let off_i = off.round() as i32;
    let pad_lt = (margin - off_i).max(0);
    let pad_rb = if sh > 0.0 { margin + off_i } else { 0 };
    let ow = cw + (pad_lt + pad_rb) as u32;
    let oh = ch + rh + (pad_lt + pad_rb) as u32;
    let Some(mut out) = Pixmap::new(ow, oh) else { return img };
    let bg = fx.background;
    if bg.a > 0 {
        out.fill(tiny_skia::Color::from_rgba8(bg.r, bg.g, bg.b, bg.a));
    }
    let (cx, cy) = (pad_lt, pad_lt);

    if sh > 0.0 {
        // Blurred alpha of the content, offset and darkened.
        let mut a = vec![0u8; (ow * oh) as usize];
        let src = content.data();
        for y in 0..ch as usize {
            let oy = y as i32 + cy + off_i;
            if oy < 0 || oy >= oh as i32 {
                continue;
            }
            for x in 0..cw as usize {
                let ox = x as i32 + cx + off_i;
                if ox >= 0 && ox < ow as i32 {
                    a[oy as usize * ow as usize + ox as usize] = src[(y * cw as usize + x) * 4 + 3];
                }
            }
        }
        pixels::gaussian_blur(&mut a, ow as usize, oh as usize, 1, sigma);
        let k = 0.35 + 0.25 * sh;
        if let Some(mut shadow) = Pixmap::new(ow, oh) {
            for (d, v) in shadow.data_mut().as_chunks_mut::<4>().0.iter_mut().zip(&a) {
                d[3] = (*v as f32 * k) as u8;
            }
            out.draw_pixmap(0, 0, shadow.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
    }

    if rh > 0 && let Some(mut r) = Pixmap::new(cw, rh) {
        let stride = cw as usize * 4;
        let src = content.data();
        let dst = r.data_mut();
        for j in 0..rh as usize {
            let t = 1.0 - j as f32 / rh as f32;
            let f = refl * 0.5 * t * t;
            let k = (f.clamp(0.0, 1.0) * 256.0) as u32;
            let s = (ch as usize - 1 - j) * stride;
            for i in 0..stride {
                dst[j * stride + i] = ((src[s + i] as u32 * k) >> 8) as u8;
            }
        }
        out.draw_pixmap(cx, cy + ch as i32, r.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }

    out.draw_pixmap(cx, cy, content.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    pixels::straight_from_pixmap(&out, None)
}
