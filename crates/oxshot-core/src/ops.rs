//! Destructive-but-undoable document operations. Every operation takes a document and
//! returns a new one (the editor keeps the old one for undo). Annotations stay editable
//! vector objects wherever possible; object ids are preserved.

mod border;
mod cut;
mod transform;

use std::sync::Arc;

use image::imageops::{self, FilterType};
use image::{Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::geom::{IRect, Point};
use crate::model::{Color, Document, Shape};
use crate::render::Renderer;
use transform::{ShapeMap, control_points};

/// Largest canvas/image side the operations will produce.
const MAX_SIDE: u32 = 32768;

// ---------------------------------------------------------------------------------------------
// Crop / trim

/// Non-destructive crop: only sets `canvas.rect` (size clamped to 1..=32768, position to
/// a sane range around the base image).
pub fn crop(doc: &Document, rect: IRect) -> Document {
    let mut out = doc.clone();
    let lim = MAX_SIDE as i32;
    out.canvas.rect = IRect::new(
        rect.x.clamp(-lim, lim),
        rect.y.clamp(-lim, lim),
        rect.w.clamp(1, MAX_SIDE),
        rect.h.clamp(1, MAX_SIDE),
    );
    out
}

/// Bakes the canvas rect into the base: the new base is exactly the canvas area (base
/// pasted over the canvas background), objects are translated, canvas = whole base.
pub fn trim_to_canvas(doc: &Document) -> Document {
    let c = doc.canvas.rect;
    if c.x == 0 && c.y == 0 && (c.w, c.h) == doc.base.dimensions() {
        return doc.clone();
    }
    let bg = doc.canvas.background;
    let mut base = RgbaImage::from_pixel(c.w, c.h, Rgba([bg.r, bg.g, bg.b, bg.a]));
    for y in 0..c.h {
        let sy = c.y + y as i32;
        if sy < 0 || sy >= doc.base.height() as i32 {
            continue;
        }
        for x in 0..c.w {
            let sx = c.x + x as i32;
            if sx < 0 || sx >= doc.base.width() as i32 {
                continue;
            }
            let s = *doc.base.get_pixel(sx as u32, sy as u32);
            let d = base.get_pixel_mut(x, y);
            *d = over(s, *d);
        }
    }
    let d = Point::new(-c.x as f32, -c.y as f32);
    let mut out = doc.clone();
    out.base = Arc::new(base);
    out.canvas.rect = IRect::new(0, 0, c.w, c.h);
    for o in &mut out.objects {
        o.shape = transform::translate(&o.shape, d);
    }
    out
}

/// Straight-alpha "source over".
fn over(s: Rgba<u8>, d: Rgba<u8>) -> Rgba<u8> {
    let sa = s.0[3] as f32 / 255.0;
    if sa >= 1.0 {
        return s;
    }
    let da = d.0[3] as f32 / 255.0;
    let oa = sa + da * (1.0 - sa);
    if oa <= 0.0 {
        return Rgba([0, 0, 0, 0]);
    }
    let ch = |i: usize| ((s.0[i] as f32 * sa + d.0[i] as f32 * da * (1.0 - sa)) / oa).round() as u8;
    Rgba([ch(0), ch(1), ch(2), (oa * 255.0).round() as u8])
}

// ---------------------------------------------------------------------------------------------
// Resize

/// High-quality resample; premultiplied when the image has transparency.
fn resample(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    if img.dimensions() == (w, h) {
        return img.clone();
    }
    let up = w as u64 * h as u64 >= img.width() as u64 * img.height() as u64;
    let filter = if up { FilterType::Lanczos3 } else { FilterType::CatmullRom };
    if img.pixels().all(|p| p.0[3] == 255) {
        return imageops::resize(img, w, h, filter);
    }
    let mut pre = img.clone();
    for p in pre.pixels_mut() {
        let a = p.0[3] as u32;
        for c in 0..3 {
            p.0[c] = ((p.0[c] as u32 * a + 127) / 255) as u8;
        }
    }
    let mut out = imageops::resize(&pre, w, h, filter);
    for p in out.pixels_mut() {
        let a = p.0[3] as u32;
        for c in 0..3 {
            p.0[c] = (p.0[c] as u32 * 255 + a / 2).checked_div(a).map_or(0, |v| v.min(255) as u8);
        }
    }
    out
}

/// Scales the base image (Lanczos3 up / Catmull-Rom down), the canvas rect and all object
/// geometry by (`sx`, `sy`). With `scale_styles`, stroke widths, font sizes, step radii and
/// paddings scale by `sqrt(sx * sy)` too; otherwise they keep their pixel size. Stamp
/// resources are unchanged (their rect scales).
pub fn resize(doc: &Document, sx: f32, sy: f32, scale_styles: bool) -> Document {
    let (w, h) = doc.base.dimensions();
    let sx = if sx.is_finite() && sx > 0.0 { sx } else { 1.0 };
    let sy = if sy.is_finite() && sy > 0.0 { sy } else { 1.0 };
    let nw = ((w as f32 * sx).round() as u32).clamp(1, MAX_SIDE);
    let nh = ((h as f32 * sy).round() as u32).clamp(1, MAX_SIDE);
    let mut out = doc.clone();
    if w > 0 && h > 0 {
        out.base = Arc::new(resample(&doc.base, nw, nh));
    }
    let c = doc.canvas.rect;
    let x0 = (c.x as f32 * sx).round() as i32;
    let y0 = (c.y as f32 * sy).round() as i32;
    let x1 = (c.right() as f32 * sx).round() as i32;
    let y1 = (c.bottom() as f32 * sy).round() as i32;
    out.canvas.rect = IRect::new(x0, y0, ((x1 - x0).max(1) as u32).min(MAX_SIDE), ((y1 - y0).max(1) as u32).min(MAX_SIDE));
    let g = (sx * sy).sqrt();
    let f = move |p: Point| Point::new(p.x * sx, p.y * sy);
    let map = ShapeMap { point: &f, geo: g, style: if scale_styles { g } else { 1.0 }, xscale: sx };
    for o in &mut out.objects {
        o.shape = map.shape(&o.shape, (sx, sy));
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Cut out

/// Seam style for [`cut_out`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CutEdge {
    /// Plain join.
    #[default]
    Straight,
    /// Torn zigzag gap with a soft shadow.
    Zigzag,
    /// Torn wavy gap with a soft shadow.
    Wave,
}

/// Maps one coordinate across a removed band `[from, to)`.
fn cut_coord(v: f32, from: f32, to: f32) -> f32 {
    if v < from {
        v
    } else if v < to {
        from
    } else {
        v - (to - from)
    }
}

/// Removes the band y ∈ [from, to) (or x ∈ [from, to) when `vertical`) from the base
/// image, e.g. to shorten a long screenshot. `from`/`to` are rounded to whole pixels and
/// clamped to the base image.
///
/// Objects:
/// - entirely before the band: unchanged; entirely after: shifted by `-(to - from)`;
/// - entirely inside the band: moved rigidly so they start at the seam;
/// - overlapping: every control point is mapped on its own — points before the band
///   stay, points after it move up/left, points inside snap to the seam. So an arrow
///   across the cut keeps its start and only its end moves; a rect spanning the cut
///   shrinks by the band;
/// - text, step numbers (center) and stamps (center) move rigidly with their anchor and
///   keep their size.
///
/// The canvas rect is mapped the same way (it shrinks by the part of the band it covers).
pub fn cut_out(doc: &Document, vertical: bool, from: f32, to: f32, edge: CutEdge) -> Document {
    let (w, h) = doc.base.dimensions();
    let len = if vertical { w } else { h };
    let (a, b) = (from.min(to), from.max(to));
    let from_px = (a.round().max(0.0) as u32).min(len);
    let to_px = (b.round().max(0.0) as u32).min(len);
    if to_px <= from_px {
        return doc.clone();
    }
    let (f, t) = (from_px as f32, to_px as f32);

    let base = if vertical {
        cut::transpose(&cut::cut_rows(&cut::transpose(&doc.base), from_px, to_px, edge))
    } else {
        cut::cut_rows(&doc.base, from_px, to_px, edge)
    };
    let mut out = doc.clone();
    out.base = Arc::new(base);

    let axis = |p: Point| if vertical { p.x } else { p.y };
    let on_axis = move |p: Point, v: f32| if vertical { Point::new(v, p.y) } else { Point::new(p.x, v) };
    let point = move |p: Point| on_axis(p, cut_coord(axis(p), f, t));
    let map = ShapeMap { point: &point, geo: 1.0, style: 1.0, xscale: 1.0 };
    for o in &mut out.objects {
        let pts = control_points(&o.shape);
        let min = pts.iter().map(|&p| axis(p)).fold(f32::INFINITY, f32::min);
        let max = pts.iter().map(|&p| axis(p)).fold(f32::NEG_INFINITY, f32::max);
        let rigid = matches!(o.shape, Shape::Text(_));
        o.shape = if min >= f && max < t {
            // Entirely inside the band: keep the shape, move it to the seam.
            transform::translate(&o.shape, on_axis(Point::default(), f - min))
        } else if rigid {
            let anchor = pts.first().copied().unwrap_or_default();
            let d = cut_coord(axis(anchor), f, t) - axis(anchor);
            transform::translate(&o.shape, on_axis(Point::default(), d))
        } else {
            map.shape(&o.shape, (1.0, 1.0))
        };
    }

    let c = doc.canvas.rect;
    let (c0, c1) = if vertical { (c.x, c.right()) } else { (c.y, c.bottom()) };
    let n0 = cut_coord(c0 as f32, f, t) as i32;
    let n1 = cut_coord(c1 as f32, f, t) as i32;
    let n = (n1 - n0).max(1) as u32;
    out.canvas.rect = if vertical { IRect::new(n0, c.y, n, c.h) } else { IRect::new(c.x, n0, c.w, n) };
    out
}

// ---------------------------------------------------------------------------------------------
// Border effects

/// Which sides of the image get a torn-paper edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TornEdges {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

impl TornEdges {
    pub fn any(&self) -> bool {
        self.top || self.right || self.bottom || self.left
    }
}

/// Border / frame effects applied by [`apply_border`]. `Default` = all off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BorderEffect {
    /// Border stroke drawn inside the (rounded/torn) outline; 0 = none.
    pub border_width: f32,
    pub border_color: Color,
    /// Rounded corners (anti-aliased); 0 = square.
    pub corner_radius: f32,
    pub torn: TornEdges,
    /// Soft drop shadow 0..=1 (offset 6px, blur 10px at 1).
    pub shadow: f32,
    /// Mirrored copy below the image 0..=1 (starting alpha `reflection * 0.5`, 30% high).
    pub reflection: f32,
    pub grayscale: bool,
    /// Clockwise rotation in degrees; the canvas grows to fit.
    pub rotation_deg: f32,
    /// Fill behind everything (usually transparent).
    pub background: Color,
}

impl Default for BorderEffect {
    fn default() -> Self {
        Self {
            border_width: 0.0,
            border_color: Color::BLACK,
            corner_radius: 0.0,
            torn: TornEdges::default(),
            shadow: 0.0,
            reflection: 0.0,
            grayscale: false,
            rotation_deg: 0.0,
            background: Color::TRANSPARENT,
        }
    }
}

impl BorderEffect {
    /// Pixel-size parameters (border width, corner radius) scaled by `factor`, e.g. for a
    /// preview on a downscaled thumbnail. Shadow and torn-edge sizes are fixed in pixels.
    pub fn scaled(&self, factor: f32) -> Self {
        Self {
            border_width: self.border_width * factor,
            corner_radius: self.corner_radius * factor,
            ..self.clone()
        }
    }
}

/// Flattens the document (objects included) and builds a new base with the effects.
/// The result has no objects, canvas = whole new base, transparent background.
pub fn apply_border(doc: &Document, fx: &BorderEffect) -> Document {
    let flat = Renderer::new().flatten(doc);
    Document::new(preview_border(&flat, fx))
}

/// Same pipeline as [`apply_border`] on an already flattened image (for live previews).
pub fn preview_border(img: &RgbaImage, fx: &BorderEffect) -> RgbaImage {
    border::render(img, fx)
}

#[cfg(test)]
mod tests;
