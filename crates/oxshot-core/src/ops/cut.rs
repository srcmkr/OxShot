//! Removing a horizontal band from an image, with a straight, zigzag or wavy torn seam.

use image::RgbaImage;

use super::CutEdge;

/// Seam amplitude, period and gap (px).
const AMPLITUDE: f32 = 6.0;
const PERIOD: f32 = 16.0;
const GAP: f32 = 8.0;
/// Shadow along the torn edges, cast into the gap.
const SHADOW_ALPHA: f32 = 0.28;
const SHADOW_FALLOFF: f32 = 1.8;

/// Seam offset and slope at `x`.
fn profile(edge: CutEdge, x: f32) -> (f32, f32) {
    match edge {
        CutEdge::Straight => (0.0, 0.0),
        CutEdge::Zigzag => {
            let u = (x / PERIOD).rem_euclid(1.0);
            let v = AMPLITUDE * (4.0 * (u - 0.5).abs() - 1.0);
            let slope = 4.0 * AMPLITUDE / PERIOD;
            (v, slope)
        }
        CutEdge::Wave => {
            let k = std::f32::consts::TAU / PERIOD;
            (AMPLITUDE * (x * k).sin(), AMPLITUDE * k * (x * k).cos())
        }
    }
}

#[inline]
fn premul(p: [u8; 4]) -> [f32; 4] {
    let a = p[3] as f32 / 255.0;
    [p[0] as f32 * a, p[1] as f32 * a, p[2] as f32 * a, p[3] as f32]
}

/// Removes rows `from..to` (already clamped, `from < to <= h`). The result is
/// `h - (to - from)` rows high; rows below the band move up.
pub(crate) fn cut_rows(img: &RgbaImage, from: u32, to: u32, edge: CutEdge) -> RgbaImage {
    let (w, h) = img.dimensions();
    let cut = to - from;
    let nh = h - cut;
    let mut out = RgbaImage::new(w, nh);
    let row_bytes = w as usize * 4;
    let src = img.as_raw();
    {
        let dst = out.as_mut();
        dst[..from as usize * row_bytes].copy_from_slice(&src[..from as usize * row_bytes]);
        dst[from as usize * row_bytes..].copy_from_slice(&src[to as usize * row_bytes..]);
    }
    if edge == CutEdge::Straight || nh == 0 {
        return out;
    }

    // Torn seam: rows around `from` are recomposed from both halves. The top half may
    // borrow rows from inside the removed band (and the bottom half likewise), so the
    // jagged edges show real content instead of being clipped.
    let s = from as f32;
    // Vertical edge distance, widened so the gap measured across the slanted edge stays
    // close to GAP.
    let max_slope = match edge {
        CutEdge::Zigzag => 4.0 * AMPLITUDE / PERIOD,
        _ => AMPLITUDE * std::f32::consts::TAU / PERIOD,
    };
    let gap = GAP * (1.0 + (0.7 * max_slope).powi(2)).sqrt();
    let reach = gap / 2.0 + AMPLITUDE + 2.0;
    let y0 = (s - reach).floor().max(0.0) as u32;
    let y1 = ((s + reach).ceil() as u32).min(nh);
    let top_src = |y: u32| -> u32 {
        // Rows of the upper half continue into the band.
        if cut > 0 { y.min(to - 1).min(h - 1) } else { y.min(h - 1) }
    };
    let bot_src = |y: u32| -> u32 { (y + cut).max(from).min(h - 1) };
    for x in 0..w {
        let (e, slope) = profile(edge, x as f32 + 0.5);
        let norm = (1.0 + slope * slope).sqrt();
        let te = s - gap / 2.0 + e;
        let be = s + gap / 2.0 + e;
        for y in y0..y1 {
            let yc = y as f32 + 0.5;
            let tc = ((te - yc) / norm + 0.5).clamp(0.0, 1.0);
            let bc = ((yc - be) / norm + 0.5).clamp(0.0, 1.0);
            let t = premul(img.get_pixel(x, top_src(y)).0);
            let b = premul(img.get_pixel(x, bot_src(y)).0);
            let rest = (1.0 - tc - bc).max(0.0);
            // Shadow cast by both edges into the gap (stronger below the upper piece).
            let d_top = ((yc - te) / norm).max(0.0);
            let d_bot = ((be - yc) / norm).max(0.0);
            let sh = SHADOW_ALPHA
                * (-d_top / SHADOW_FALLOFF).exp().max(0.45 * (-d_bot / SHADOW_FALLOFF).exp())
                * rest;
            let mut px = [0f32; 4];
            for c in 0..4 {
                px[c] = t[c] * tc + b[c] * bc;
            }
            px[3] += sh * 255.0;
            let a = px[3].min(255.0);
            let out_px = if a <= 0.0 {
                [0, 0, 0, 0]
            } else {
                let k = 255.0 / a;
                [
                    (px[0] * k).round().min(255.0) as u8,
                    (px[1] * k).round().min(255.0) as u8,
                    (px[2] * k).round().min(255.0) as u8,
                    a.round() as u8,
                ]
            };
            out.put_pixel(x, y, image::Rgba(out_px));
        }
    }
    out
}

/// Swaps x and y.
pub(crate) fn transpose(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let mut out = RgbaImage::new(h, w);
    for (x, y, p) in img.enumerate_pixels() {
        out.put_pixel(y, x, *p);
    }
    out
}
