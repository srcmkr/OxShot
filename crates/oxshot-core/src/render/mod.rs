//! CPU renderer (tiny-skia + cosmic-text). The same code renders editor sprites and the
//! exported image, so what the user sees is exactly what lands in the clipboard.
//!
//! CONTRACT (used by the GPUI editor):
//! - All images are straight (non-premultiplied) RGBA8 `image::RgbaImage`.
//! - A [`Layer`] is an image placed at an integer origin in base-image coordinates.
//! - [`Renderer::render_object`] renders ONE object into a tight sprite. For shapes with
//!   `Shape::reads_backdrop()` the caller passes the composite of everything below.
//! - [`Renderer::flatten`] produces the final export: canvas rect only, background filled.

mod geometry;
pub(crate) mod pixels;
mod shapes;
mod text;

use image::RgbaImage;
use tiny_skia::{Pixmap, PixmapPaint, Transform};

use crate::geom::{IRect, Point, Rect};
use crate::model::{CalloutShape, Document, Object, SharedImage, TextStyle};
use pixels::Backdrop;

pub use pixels::shadow_params;

/// An image positioned in base-image pixel coordinates.
#[derive(Debug, Clone)]
pub struct Layer {
    pub origin: (i32, i32),
    pub image: RgbaImage,
}

/// Layout information for editing text in place.
#[derive(Debug, Clone, Default)]
pub struct TextMetrics {
    /// Size of the laid-out text block.
    pub width: f32,
    pub height: f32,
    pub line_height: f32,
    /// Caret position (top of the caret, relative to the text origin) for every char
    /// boundary: `carets.len() == text.chars().count() + 1`.
    pub carets: Vec<Point>,
}

/// Sprites are clipped to the document extent grown by this margin, so that a shape
/// dragged far outside the image cannot allocate unbounded memory.
const WORK_MARGIN: i32 = 4096;

pub struct Renderer {
    text: text::TextEngine,
    /// Premultiplied copy of the last base image (keyed by `Arc` identity).
    base_cache: Option<(SharedImage, Pixmap)>,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Union of base rect and canvas rect: the area a composite covers.
fn doc_extent(doc: &Document) -> IRect {
    let base = IRect::new(0, 0, doc.base.width(), doc.base.height());
    let c = doc.canvas.rect;
    if c.w == 0 || c.h == 0 {
        return base;
    }
    if base.w == 0 || base.h == 0 {
        return c;
    }
    let x = base.x.min(c.x);
    let y = base.y.min(c.y);
    IRect::new(x, y, (base.right().max(c.right()) - x) as u32, (base.bottom().max(c.bottom()) - y) as u32)
}

impl Renderer {
    /// Creates a renderer. System fonts are loaded lazily on first text use.
    pub fn new() -> Self {
        Self { text: text::TextEngine::new(), base_cache: None }
    }

    /// Visual bounds of an object in base-image coordinates, including stroke width,
    /// arrow heads, text outline and drop shadow.
    pub fn bounds(&mut self, _doc: &Document, obj: &Object) -> Rect {
        let Some(b) = shapes::shape_bounds(&mut self.text, &obj.shape) else {
            return Rect::default();
        };
        if obj.shadow > 0.0 {
            let (off, sigma, _) = shadow_params(obj.shadow);
            let s = b.translate(Point::new(off.round(), off.round())).inflate(sigma * 3.0 + 1.0);
            b.union(&s)
        } else {
            b
        }
    }

    /// Renders a single object into a tight sprite. Returns `None` if nothing is visible.
    pub fn render_object(
        &mut self,
        doc: &Document,
        obj: &Object,
        backdrop: Option<&Layer>,
    ) -> Option<Layer> {
        let backdrop = match backdrop {
            Some(l) => Backdrop::Straight { origin: l.origin, img: &l.image },
            None => Backdrop::Straight { origin: (0, 0), img: &doc.base },
        };
        let (origin, pm) = self.render_premul(doc, obj, backdrop)?;
        Some(Layer { origin, image: pixels::straight_from_pixmap(&pm, None) })
    }

    /// Renders into a premultiplied pixmap; the sprite covers `bounds` (clipped to the
    /// work area).
    fn render_premul(
        &mut self,
        doc: &Document,
        obj: &Object,
        backdrop: Backdrop,
    ) -> Option<((i32, i32), Pixmap)> {
        if obj.opacity <= 0.0 {
            return None;
        }
        let b = self.bounds(doc, obj);
        let ext = doc_extent(doc);
        let work = IRect::new(
            ext.x - WORK_MARGIN,
            ext.y - WORK_MARGIN,
            ext.w + 2 * WORK_MARGIN as u32,
            ext.h + 2 * WORK_MARGIN as u32,
        );
        let ir = IRect::covering(&b).intersect(&work)?;
        let mut pm = Pixmap::new(ir.w, ir.h)?;
        let origin = (ir.x, ir.y);
        shapes::draw_shape(
            &mut self.text,
            doc,
            &obj.shape,
            &mut shapes::Target { pm: &mut pm, origin },
            backdrop,
        );
        if obj.shadow > 0.0 {
            pm = pixels::with_drop_shadow(&pm, obj.shadow)?;
        }
        if obj.opacity < 1.0 {
            pixels::apply_opacity(&mut pm, obj.opacity);
        }
        Some((origin, pm))
    }

    /// Premultiplied base image, cached across calls.
    fn base_pixmap(&mut self, doc: &Document) -> Option<&Pixmap> {
        let hit = matches!(&self.base_cache, Some((img, _)) if std::sync::Arc::ptr_eq(img, &doc.base));
        if !hit {
            let pm = pixels::pixmap_from_straight(&doc.base)?;
            self.base_cache = Some((doc.base.clone(), pm));
        }
        self.base_cache.as_ref().map(|(_, pm)| pm)
    }

    /// Composite in premultiplied form: canvas background (inside the canvas rect), base
    /// image and `doc.objects[..upto]`, covering [`doc_extent`].
    fn composite_premul(&mut self, doc: &Document, upto: usize) -> (IRect, Pixmap) {
        let ext = doc_extent(doc);
        let Some(mut pm) = Pixmap::new(ext.w.max(1), ext.h.max(1)) else {
            return (ext, Pixmap::new(1, 1).expect("1x1 pixmap"));
        };
        let bg = doc.canvas.background;
        if bg.a > 0 {
            let c = doc.canvas.rect;
            if let Some(r) = tiny_skia::Rect::from_xywh(
                (c.x - ext.x) as f32,
                (c.y - ext.y) as f32,
                c.w as f32,
                c.h as f32,
            ) {
                let mut p = tiny_skia::Paint::default();
                p.set_color_rgba8(bg.r, bg.g, bg.b, bg.a);
                pm.fill_rect(r, &p, Transform::identity(), None);
            }
        }
        if self.base_pixmap(doc).is_some()
            && let Some((_, base)) = &self.base_cache
        {
            pm.draw_pixmap(-ext.x, -ext.y, base.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
        for obj in doc.objects.iter().take(upto) {
            let backdrop = Backdrop::Premul { origin: (ext.x, ext.y), pm: &pm };
            if let Some((o, sprite)) = self.render_premul(doc, obj, backdrop) {
                pm.draw_pixmap(
                    o.0 - ext.x,
                    o.1 - ext.y,
                    sprite.as_ref(),
                    &PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
            }
        }
        (ext, pm)
    }

    /// Base image plus `doc.objects[..upto]`, covering the union of base rect and canvas rect.
    ///
    /// The canvas background is filled inside the canvas rect (below the base image), so
    /// this layer is exactly the backdrop that [`Renderer::flatten`] uses for object `upto`.
    pub fn composite(&mut self, doc: &Document, upto: usize) -> Layer {
        let (ext, pm) = self.composite_premul(doc, upto);
        Layer { origin: (ext.x, ext.y), image: pixels::straight_from_pixmap(&pm, None) }
    }

    /// Final image: canvas rect only, canvas background below the base image.
    pub fn flatten(&mut self, doc: &Document) -> RgbaImage {
        let (ext, pm) = self.composite_premul(doc, doc.objects.len());
        let c = doc.canvas.rect;
        if c.w == 0 || c.h == 0 {
            return RgbaImage::new(0, 0);
        }
        let crop = IRect::new(c.x - ext.x, c.y - ext.y, c.w, c.h);
        pixels::straight_from_pixmap(&pm, Some(crop))
    }

    pub fn text_metrics(
        &mut self,
        text: &str,
        style: &TextStyle,
        max_width: Option<f32>,
    ) -> TextMetrics {
        self.text.layout(text, style, max_width).metrics.clone()
    }

    /// Where a callout's text is laid out: the text origin (top-left, in image space) and
    /// the wrap width. Use together with [`Renderer::text_metrics`] for in-place editing.
    pub fn callout_text_frame(&self, callout: &CalloutShape) -> (Point, f32) {
        shapes::callout_frame(callout)
    }
}

#[cfg(test)]
mod tests;
