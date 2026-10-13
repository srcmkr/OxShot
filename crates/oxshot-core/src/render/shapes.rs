//! Per-shape bounds and drawing. Every shape draws into a fresh, transparent pixmap
//! whose top-left corner sits at `origin` in base-image coordinates.

use image::imageops::{self, FilterType};
use tiny_skia::{
    BlendMode, FillRule, FilterQuality, LineCap, LineJoin, Mask, Paint, Path, Pattern, Pixmap,
    PixmapPaint, SpreadMode, StrokeDash, Transform,
};

use super::geometry as geo;
use super::pixels::{self, Backdrop};
use super::text::{TextEngine, TextLayout};
use crate::geom::{IRect, Point, Rect};
use crate::model::*;

/// Drawing target: a pixmap placed at `origin` in image space.
pub(crate) struct Target<'a> {
    pub pm: &'a mut Pixmap,
    pub origin: (i32, i32),
}

impl Target<'_> {
    fn tf(&self) -> Transform {
        Transform::from_translate(-self.origin.0 as f32, -self.origin.1 as f32)
    }

    fn fill(&mut self, path: &Path, color: Color) {
        let tf = self.tf();
        self.pm.fill_path(path, &paint(color), FillRule::Winding, tf, None);
    }

    fn stroke(&mut self, path: &Path, color: Color, width: f32, dash: Option<StrokeDash>) {
        if width <= 0.0 {
            return;
        }
        let tf = self.tf();
        let s = tiny_skia::Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            dash,
            ..Default::default()
        };
        self.pm.stroke_path(path, &paint(color), &s, tf, None);
    }

    /// Coverage mask of `path` in pixmap space.
    fn mask(&self, path: &Path) -> Option<Mask> {
        let mut m = Mask::new(self.pm.width(), self.pm.height())?;
        m.fill_path(path, FillRule::Winding, true, self.tf());
        Some(m)
    }

    /// Draws a premultiplied pixmap whose top-left is at image position `at`.
    fn blit(&mut self, src: &Pixmap, at: (i32, i32), mask: Option<&Mask>) {
        self.pm.draw_pixmap(
            at.0 - self.origin.0,
            at.1 - self.origin.1,
            src.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            mask,
        );
    }
}

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c.r, c.g, c.b, c.a);
    p.anti_alias = true;
    p
}

fn opaque(c: Color) -> Color {
    c.with_alpha(255)
}

fn ts_rect(r: Rect) -> Option<tiny_skia::Rect> {
    let r = r.normalized();
    tiny_skia::Rect::from_xywh(r.x, r.y, r.w.max(0.01), r.h.max(0.01))
}

fn stroke_pad(s: &Option<Stroke>) -> f32 {
    s.map_or(0.0, |s| s.width.max(0.0) * 0.5)
}

/// Snaps a text origin to whole pixels for crisp baselines.
fn text_origin(p: Point) -> Point {
    Point::new(p.x.round(), p.y.round())
}

/// Text origin and wrap width inside a callout box.
pub(crate) fn callout_frame(c: &CalloutShape) -> (Point, f32) {
    let r = c.rect.normalized();
    let pad = c.padding.max(0.0);
    (Point::new(r.x + pad, r.y + pad), (r.w - 2.0 * pad).max(1.0))
}

/// Font size and layout for a step number, shrunk so that it fits the circle.
fn step_layout(text: &mut TextEngine, s: &StepShape) -> std::sync::Arc<TextLayout> {
    let label = s.number.to_string();
    let r = s.radius.max(1.0);
    let shrink = match label.len() {
        1 => 1.0,
        2 => 0.85,
        3 => 0.7,
        _ => 0.58,
    };
    let mut style = TextStyle {
        size: r * 1.1 * shrink,
        bold: true,
        italic: false,
        underline: false,
        outline: None,
        ..TextStyle::default()
    };
    let layout = text.layout(&label, &style, None);
    let ink_w = layout.ink.map_or(0.0, |i| i.w);
    if ink_w > r * 1.5 {
        style.size *= r * 1.5 / ink_w;
        return text.layout(&label, &style, None);
    }
    layout
}

fn connector_arrow(s: &StepShape, c: &Connector) -> ArrowShape {
    ArrowShape {
        start: s.center,
        end: c.target,
        curve: None,
        color: s.fill,
        width: c.width,
        style: ArrowStyle::Line,
        heads: match c.style {
            ConnectorStyle::Line => ArrowHeads::None,
            ConnectorStyle::Arrow => ArrowHeads::End,
        },
        dashed: false,
    }
}

fn text_bounds(layout: &TextLayout, origin: Point, outline: Option<Stroke>) -> Option<Rect> {
    let halo = outline.map_or(0.0, |o| o.width.max(0.0));
    layout.ink.map(|i| i.translate(text_origin(origin)).inflate(halo + 1.0))
}

/// Visual bounds of a shape (without shadow). `None` if the shape draws nothing.
pub(crate) fn shape_bounds(text: &mut TextEngine, shape: &Shape) -> Option<Rect> {
    let r = match shape {
        Shape::Arrow(a) => geo::arrow_bounds(a),
        Shape::Rect(s) => s.rect.normalized().inflate(stroke_pad(&s.stroke) + 1.0),
        Shape::Ellipse(s) => s.rect.normalized().inflate(stroke_pad(&s.stroke) + 1.0),
        Shape::Text(t) => {
            let l = text.layout(&t.text, &t.style, t.max_width);
            return text_bounds(&l, t.origin, t.style.outline);
        }
        Shape::Callout(c) => {
            let mut r = c.rect.normalized();
            if let Some(t) = c.tail {
                r = r.union(&Rect::new(t.x, t.y, 0.0, 0.0));
            }
            let mut r = r.inflate(stroke_pad(&c.border) + 1.0);
            let (o, w) = callout_frame(c);
            let l = text.layout(&c.text, &c.style, Some(w));
            if let Some(tb) = text_bounds(&l, o, c.style.outline) {
                r = r.union(&tb);
            }
            r
        }
        Shape::Step(s) => {
            let rad = s.radius.max(1.0) + stroke_pad(&s.border) + 1.0;
            let mut r = Rect::new(s.center.x - rad, s.center.y - rad, 2.0 * rad, 2.0 * rad);
            if let Some(c) = &s.connector {
                r = r.union(&geo::arrow_bounds(&connector_arrow(s, c)));
            }
            r
        }
        Shape::Highlight(h) => h.rect.normalized(),
        Shape::Polygon(p) => {
            Rect::bounding(&p.points)?.inflate(stroke_pad(&p.stroke) + 1.0)
        }
        Shape::Freehand(f) => Rect::bounding(&f.points)?.inflate(f.width.max(0.0) * 0.5 + 1.0),
        Shape::Blur(b) => b.rect.normalized(),
        Shape::Stamp(s) => s.rect.normalized(),
        Shape::Magnifier(m) => {
            let rad = m.radius.max(1.0) + m.border.width.max(0.0) * 0.5 + 1.0;
            let mut r = Rect::new(m.glass.x - rad, m.glass.y - rad, 2.0 * rad, 2.0 * rad);
            if m.show_source {
                let rs = m.source_radius() + magnifier_thin(m) * 0.5 + 1.0;
                r = r.union(&Rect::new(m.source.x - rs, m.source.y - rs, 2.0 * rs, 2.0 * rs));
            }
            r
        }
        Shape::Brace(b) => {
            let p = geo::brace_path(b.start, b.end, b.depth)?;
            let pb = p.bounds();
            Rect::new(pb.x(), pb.y(), pb.width(), pb.height()).inflate(b.width.max(0.0) * 0.5 + 1.0)
        }
    };
    (r.w.is_finite() && r.h.is_finite() && r.w > 0.0 && r.h > 0.0).then_some(r)
}

fn magnifier_thin(m: &MagnifierShape) -> f32 {
    (m.border.width * 0.5).clamp(1.0, 3.0)
}

/// Draws a laid-out text block with its halo.
fn draw_text(t: &mut Target, layout: &TextLayout, origin: Point, style: &TextStyle) {
    let o = text_origin(origin);
    let tf = t.tf().pre_translate(o.x, o.y);
    if let Some(path) = &layout.path {
        if let Some(halo) = style.outline.filter(|h| h.width > 0.0 && h.color.a > 0) {
            let s = tiny_skia::Stroke {
                width: halo.width * 2.0,
                line_join: LineJoin::Round,
                line_cap: LineCap::Round,
                ..Default::default()
            };
            t.pm.stroke_path(path, &paint(halo.color), &s, tf, None);
            // Fill the counters too, so the halo is solid behind the glyphs.
            t.pm.fill_path(path, &paint(halo.color), FillRule::Winding, tf, None);
        }
        t.pm.fill_path(path, &paint(style.color), FillRule::Winding, tf, None);
    }
    for bm in &layout.bitmaps {
        let x = (o.x as i32) + bm.x - t.origin.0;
        let y = (o.y as i32) + bm.y - t.origin.1;
        if bm.tint {
            // Coverage mask -> colored pixels.
            let Some(mut colored) = Pixmap::new(bm.pixmap.width(), bm.pixmap.height()) else { continue };
            colored.fill(tiny_skia::Color::from_rgba8(style.color.r, style.color.g, style.color.b, style.color.a));
            let mask = Mask::from_pixmap(bm.pixmap.as_ref(), tiny_skia::MaskType::Alpha);
            colored.apply_mask(&mask);
            t.pm.draw_pixmap(x, y, colored.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        } else {
            t.pm.draw_pixmap(x, y, bm.pixmap.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
    }
}

fn draw_arrow(t: &mut Target, a: &ArrowShape) {
    let g = geo::arrow_geometry(a);
    let c = opaque(a.color);
    if let Some(shaft) = &g.shaft {
        let w = a.width.max(0.5);
        let dash = a.dashed.then(|| StrokeDash::new(vec![w * 2.2, w * 1.8], 0.0)).flatten();
        t.stroke(shaft, c, w, dash);
    }
    if let Some(fill) = &g.fill {
        t.fill(fill, c);
    }
}

/// Draws `shape` into a fresh pixmap. Colors with partial alpha are applied as layer
/// opacity where parts would otherwise overlap visibly (arrow shaft vs. head).
pub(crate) fn draw_shape(
    text: &mut TextEngine,
    doc: &Document,
    shape: &Shape,
    t: &mut Target,
    backdrop: Backdrop,
) {
    match shape {
        Shape::Arrow(a) => {
            draw_arrow(t, a);
            if a.color.a < 255 {
                pixels::apply_opacity(t.pm, a.color.a as f32 / 255.0);
            }
        }
        Shape::Rect(s) => {
            let Some(p) = geo::rounded_rect(s.rect, s.corner_radius) else { return };
            if let Some(f) = s.fill {
                t.fill(&p, f);
            }
            if let Some(st) = s.stroke {
                t.stroke(&p, st.color, st.width, None);
            }
        }
        Shape::Ellipse(s) => {
            let Some(p) = ts_rect(s.rect).and_then(tiny_skia::PathBuilder::from_oval) else { return };
            if let Some(f) = s.fill {
                t.fill(&p, f);
            }
            if let Some(st) = s.stroke {
                t.stroke(&p, st.color, st.width, None);
            }
        }
        Shape::Text(s) => {
            let l = text.layout(&s.text, &s.style, s.max_width);
            draw_text(t, &l, s.origin, &s.style);
        }
        Shape::Callout(c) => {
            if let Some(p) = geo::bubble_path(c.rect, c.corner_radius, c.tail) {
                t.fill(&p, c.fill);
                if let Some(b) = c.border {
                    t.stroke(&p, b.color, b.width, None);
                }
            }
            let (o, w) = callout_frame(c);
            let l = text.layout(&c.text, &c.style, Some(w));
            draw_text(t, &l, o, &c.style);
        }
        Shape::Step(s) => draw_step(text, t, s),
        Shape::Highlight(h) => {
            let r = h.rect.normalized();
            let ir = IRect::covering(&r);
            let (Some(mut region), Some(path)) = (backdrop.region(ir, false), ts_rect(r).map(tiny_skia::PathBuilder::from_rect)) else {
                return;
            };
            let mut p = paint(h.color);
            p.blend_mode = BlendMode::Multiply;
            p.anti_alias = false;
            let full = tiny_skia::Rect::from_xywh(0.0, 0.0, region.width() as f32, region.height() as f32);
            if let Some(full) = full {
                region.fill_rect(full, &p, Transform::identity(), None);
            }
            let mask = t.mask(&path);
            t.blit(&region, (ir.x, ir.y), mask.as_ref());
        }
        Shape::Polygon(p) => {
            let closed = p.closed && p.points.len() > 2;
            if let Some(f) = p.fill
                && p.points.len() > 2
                && let Some(path) = geo::polygon(&p.points)
            {
                t.fill(&path, f);
            }
            if let Some(st) = p.stroke
                && let Some(path) = geo::polyline_path(&p.points, closed)
            {
                t.stroke(&path, st.color, st.width, None);
            }
        }
        Shape::Freehand(f) => {
            if let Some(path) = geo::smooth_path(&f.points) {
                t.stroke(&path, f.color, f.width.max(0.5), None);
            }
        }
        Shape::Blur(b) => draw_blur(t, b, backdrop),
        Shape::Stamp(s) => draw_stamp(doc, t, s),
        Shape::Magnifier(m) => draw_magnifier(t, m, backdrop),
        Shape::Brace(b) => {
            if let Some(p) = geo::brace_path(b.start, b.end, b.depth) {
                t.stroke(&p, b.color, b.width.max(0.5), None);
            }
        }
    }
}

fn draw_step(text: &mut TextEngine, t: &mut Target, s: &StepShape) {
    let r = s.radius.max(1.0);
    if let Some(c) = &s.connector
        && c.target.distance(s.center) > r
    {
        draw_arrow(t, &connector_arrow(s, c));
    }
    let Some(circle) = geo::circle(s.center, r) else { return };
    t.fill(&circle, s.fill);
    if let Some(b) = s.border {
        t.stroke(&circle, b.color, b.width, None);
    }
    let layout = step_layout(text, s);
    if let (Some(path), Some(ink)) = (&layout.path, layout.ink) {
        let c = ink.center();
        let tf = t.tf().pre_translate(s.center.x - c.x, s.center.y - c.y);
        t.pm.fill_path(path, &paint(s.text_color), FillRule::Winding, tf, None);
    }
}

fn draw_blur(t: &mut Target, b: &BlurShape, backdrop: Backdrop) {
    let r = b.rect.normalized();
    let ir = IRect::covering(&r);
    if ir.w == 0 || ir.h == 0 {
        return;
    }
    let strength = b.strength.clamp(1.0, 20.0);
    let (region, at) = match b.mode {
        BlurMode::Gaussian => {
            let sigma = strength * 1.2;
            let m = (sigma * 3.0).ceil() as i32;
            let er = IRect::new(ir.x - m, ir.y - m, ir.w + 2 * m as u32, ir.h + 2 * m as u32);
            let Some(mut region) = backdrop.region(er, true) else { return };
            let (w, h) = (region.width() as usize, region.height() as usize);
            pixels::gaussian_blur(region.data_mut(), w, h, 4, sigma);
            (region, (er.x, er.y))
        }
        BlurMode::Pixelate => {
            let Some(mut region) = backdrop.region(ir, true) else { return };
            pixels::pixelate(&mut region, (strength * 2.0 + 2.0).round() as u32);
            (region, (ir.x, ir.y))
        }
    };
    let Some(path) = ts_rect(r).map(tiny_skia::PathBuilder::from_rect) else { return };
    let mask = t.mask(&path);
    t.blit(&region, at, mask.as_ref());
}

fn draw_stamp(doc: &Document, t: &mut Target, s: &StampShape) {
    let r = s.rect.normalized();
    let Some(img) = doc.resources.get(&s.resource) else {
        // Missing resource: a neutral placeholder so the object stays visible.
        if let Some(p) = geo::rounded_rect(r, 4.0) {
            t.fill(&p, Color::rgba(128, 128, 128, 90));
            t.stroke(&p, Color::rgba(90, 90, 90, 200), 1.5, None);
        }
        return;
    };
    let (dw, dh) = (r.w.round().max(1.0) as u32, r.h.round().max(1.0) as u32);
    if img.width() == 0 || img.height() == 0 || dw > 16384 || dh > 16384 {
        return;
    }
    // Resample in premultiplied space so transparent edges don't darken.
    let mut pre = (**img).clone();
    for p in pre.pixels_mut() {
        let a = p.0[3] as u32;
        for c in 0..3 {
            p.0[c] = ((p.0[c] as u32 * a + 127) / 255) as u8;
        }
    }
    let scaled = if (dw, dh) == pre.dimensions() {
        pre
    } else {
        imageops::resize(&pre, dw, dh, FilterType::CatmullRom)
    };
    let Some(mut pm) = Pixmap::new(dw, dh) else { return };
    for (d, s) in pm.data_mut().as_chunks_mut::<4>().0.iter_mut().zip(scaled.as_raw().as_chunks::<4>().0.iter()) {
        let a = s[3];
        d.copy_from_slice(&[s[0].min(a), s[1].min(a), s[2].min(a), a]);
    }
    // Place with the fractional remainder (position and scale) applied by tiny-skia.
    let tf = t
        .tf()
        .pre_translate(r.x, r.y)
        .pre_scale(r.w / dw as f32, r.h / dh as f32);
    let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };
    t.pm.draw_pixmap(0, 0, pm.as_ref(), &paint, tf, None);
}

fn draw_magnifier(t: &mut Target, m: &MagnifierShape, backdrop: Backdrop) {
    let rad = m.radius.max(1.0);
    let zoom = m.zoom.max(0.01);
    let rs = m.source_radius();
    let border = m.border;
    if m.show_source {
        let thin = magnifier_thin(m);
        let mut pb = tiny_skia::PathBuilder::new();
        if let Some(tangents) = geo::outer_tangents(m.source, rs, m.glass, rad) {
            for (a, b) in tangents {
                pb.move_to(a.x, a.y);
                pb.line_to(b.x, b.y);
            }
        }
        pb.push_circle(m.source.x, m.source.y, rs.max(0.5));
        if let Some(p) = pb.finish() {
            t.stroke(&p, border.color, thin, None);
        }
    }
    let Some(glass) = geo::circle(m.glass, rad) else { return };
    let src = Rect::new(m.source.x - rs, m.source.y - rs, 2.0 * rs, 2.0 * rs).inflate(2.0 + 2.0 / zoom);
    let ir = IRect::covering(&src);
    if let Some(region) = backdrop.region(ir, false) {
        t.fill(&glass, Color::WHITE);
        let tf = Transform::from_translate(ir.x as f32 - m.source.x, ir.y as f32 - m.source.y)
            .post_scale(zoom, zoom)
            .post_translate(m.glass.x, m.glass.y);
        let quality = if zoom >= 1.0 { FilterQuality::Bicubic } else { FilterQuality::Bilinear };
        let p = Paint {
            shader: Pattern::new(region.as_ref(), SpreadMode::Pad, quality, 1.0, tf),
            anti_alias: true,
            ..Default::default()
        };
        let path_tf = t.tf();
        t.pm.fill_path(&glass, &p, FillRule::Winding, path_tf, None);
    }
    if border.width > 0.0 {
        t.stroke(&glass, border.color, border.width, None);
    }
}
