//! Text layout (cosmic-text) and glyph outlines as tiny-skia paths. Glyphs are drawn as
//! vector outlines, so the halo is a real stroke around the letter shapes.

use std::collections::HashMap;
use std::sync::Arc;

use cosmic_text::{
    Attrs, Buffer, Command, Family, FontSystem, LineEnding, LineIter, Metrics, Shaping, Style, SwashCache,
    SwashContent, Weight, Wrap,
};
use tiny_skia::{Path, PathBuilder, Pixmap};

use super::TextMetrics;
use crate::geom::{Point, Rect};
use crate::model::{Align, TextStyle};

/// Line height as a multiple of the font size.
pub(crate) const LINE_HEIGHT: f32 = 1.25;

/// A glyph without an outline (color emoji, bitmap fonts), positioned in layout space.
pub(crate) struct GlyphBitmap {
    pub x: i32,
    pub y: i32,
    /// Premultiplied pixels.
    pub pixmap: Pixmap,
    /// `true` for coverage masks that take the text color.
    pub tint: bool,
}

/// A shaped text block in local coordinates (origin = top-left of the block).
pub(crate) struct TextLayout {
    /// All glyph outlines plus underlines (nonzero fill rule).
    pub path: Option<Path>,
    pub bitmaps: Vec<GlyphBitmap>,
    pub metrics: TextMetrics,
    /// Tight ink bounds (without halo) in local coordinates.
    pub ink: Option<Rect>,
}

#[derive(Hash, PartialEq, Eq)]
struct Key {
    text: String,
    family: String,
    size: u32,
    bold: bool,
    italic: bool,
    underline: bool,
    align: u8,
    max_width: Option<u32>,
}

/// Font database, glyph cache and a small layout cache.
pub(crate) struct TextEngine {
    fonts: Option<FontSystem>,
    swash: SwashCache,
    cache: HashMap<Key, Arc<TextLayout>>,
}

impl TextEngine {
    pub fn new() -> Self {
        Self { fonts: None, swash: SwashCache::new(), cache: HashMap::new() }
    }

    /// Lays out `text`, reusing a cached layout when possible.
    pub fn layout(&mut self, text: &str, style: &TextStyle, max_width: Option<f32>) -> Arc<TextLayout> {
        let key = Key {
            text: text.to_owned(),
            family: style.family.clone(),
            size: style.size.to_bits(),
            bold: style.bold,
            italic: style.italic,
            underline: style.underline,
            align: style.align as u8,
            max_width: max_width.map(f32::to_bits),
        };
        if let Some(l) = self.cache.get(&key) {
            return l.clone();
        }
        if self.cache.len() > 512 {
            self.cache.clear();
        }
        let layout = Arc::new(self.layout_uncached(text, style, max_width));
        self.cache.insert(key, layout.clone());
        layout
    }

    fn layout_uncached(&mut self, text: &str, style: &TextStyle, max_width: Option<f32>) -> TextLayout {
        let fonts = self.fonts.get_or_insert_with(FontSystem::new);
        let size = if style.size.is_finite() { style.size.clamp(1.0, 2000.0) } else { 20.0 };
        let line_h = (size * LINE_HEIGHT).round().max(1.0);
        let max_width = max_width.filter(|w| w.is_finite()).map(|w| w.max(1.0));

        let family = match style.family.as_str() {
            "" | "sans-serif" | "sans" => Family::SansSerif,
            "serif" => Family::Serif,
            "monospace" => Family::Monospace,
            name => Family::Name(name),
        };
        let attrs = Attrs::new()
            .family(family)
            .weight(if style.bold { Weight::BOLD } else { Weight::NORMAL })
            .style(if style.italic { Style::Italic } else { Style::Normal });

        let mut buffer = Buffer::new(fonts, Metrics::new(size, line_h));
        let mut b = buffer.borrow_with(fonts);
        b.set_wrap(Wrap::WordOrGlyph);
        b.set_size(max_width, None);
        b.set_text(text, &attrs, Shaping::Advanced, None);
        b.shape_until_scroll(false);

        let mut runs: Vec<Run> = buffer
            .layout_runs()
            .map(|r| Run {
                line_i: r.line_i,
                top: r.line_top,
                baseline: r.line_y.round(),
                w: r.line_w,
                dx: 0.0,
                glyphs: r.glyphs.to_vec(),
            })
            .collect();

        let widest = runs.iter().map(|r| r.w).fold(0.0f32, f32::max);
        let block_w = max_width.unwrap_or(widest);
        let height = runs.last().map_or(line_h, |r| r.top + line_h);
        let align = match style.align {
            Align::Left => 0.0,
            Align::Center => 0.5,
            Align::Right => 1.0,
        };
        for r in &mut runs {
            r.dx = ((block_w - r.w) * align).max(0.0);
        }

        // Glyph outlines.
        let mut pb = PathBuilder::new();
        let mut bitmaps = Vec::new();
        for run in &runs {
            let dx = run.dx;
            for g in &run.glyphs {
                let phys = g.physical((dx, run.baseline), 1.0);
                let gx = dx + g.x + g.font_size * g.x_offset;
                let gy = run.baseline + g.y - g.font_size * g.y_offset;
                if let Some(cmds) = self.swash.get_outline_commands(fonts, phys.cache_key) {
                    push_outline(&mut pb, cmds, gx, gy);
                } else if let Some(img) = self.swash.get_image(fonts, phys.cache_key) {
                    let p = img.placement;
                    if p.width == 0 || p.height == 0 {
                        continue;
                    }
                    let Some(mut pm) = Pixmap::new(p.width, p.height) else { continue };
                    let tint = match img.content {
                        SwashContent::Mask => {
                            for (d, a) in pm.data_mut().as_chunks_mut::<4>().0.iter_mut().zip(&img.data) {
                                d[3] = *a;
                            }
                            true
                        }
                        SwashContent::Color => {
                            for (d, s) in pm.data_mut().as_chunks_mut::<4>().0.iter_mut().zip(img.data.as_chunks::<4>().0.iter()) {
                                let a = s[3] as u32;
                                let m = |c: u8| ((c as u32 * a + 127) / 255) as u8;
                                d.copy_from_slice(&[m(s[0]), m(s[1]), m(s[2]), s[3]]);
                            }
                            false
                        }
                        SwashContent::SubpixelMask => continue,
                    };
                    bitmaps.push(GlyphBitmap { x: phys.x + p.left, y: phys.y - p.top, pixmap: pm, tint });
                }
            }
            if style.underline && !run.glyphs.is_empty() {
                let x0 = run.glyphs.iter().map(|g| g.x).fold(f32::MAX, f32::min) + dx;
                let x1 = run.glyphs.iter().map(|g| g.x + g.w).fold(f32::MIN, f32::max) + dx;
                let thick = (size / 14.0).max(1.0);
                let y = run.baseline + (size * 0.12).max(1.5);
                if let Some(r) = tiny_skia::Rect::from_xywh(x0, y, (x1 - x0).max(0.5), thick) {
                    pb.push_rect(r);
                }
            }
        }
        let path = pb.finish();

        let mut ink = path.as_ref().map(|p| {
            let b = p.bounds();
            Rect::new(b.x(), b.y(), b.width(), b.height())
        });
        for bm in &bitmaps {
            let r = Rect::new(bm.x as f32, bm.y as f32, bm.pixmap.width() as f32, bm.pixmap.height() as f32);
            ink = Some(ink.map_or(r, |i| i.union(&r)));
        }

        // Carets for every char boundary, mapped onto cosmic-text's logical lines.
        let raw: Vec<_> = LineIter::new(text).collect();
        let mut lines: Vec<(usize, usize)> =
            raw.iter().map(|(r, ending)| (r.start, r.end + ending.as_str().len())).collect();
        // cosmic-text appends an empty line after a trailing line break.
        if raw.last().is_none_or(|(_, ending)| *ending != LineEnding::None) {
            lines.push((text.len(), text.len()));
        }
        let mut carets = Vec::with_capacity(text.chars().count() + 1);
        let mut li = 0usize;
        for b in text.char_indices().map(|(i, _)| i).chain(std::iter::once(text.len())) {
            while li + 1 < lines.len() && b >= lines[li].1 && b >= lines[li + 1].0 {
                li += 1;
            }
            let (ls, le) = lines[li];
            // Offsets inside the line terminator snap to the end of the line text.
            let content_end = ls + text[ls..le].trim_end_matches(['\r', '\n']).len();
            carets.push(caret_at(&runs, li, b.min(content_end) - ls, line_h));
        }

        let metrics = TextMetrics { width: block_w, height, line_height: line_h, carets };
        TextLayout { path, bitmaps, metrics, ink }
    }
}

/// One visual (wrapped) line.
struct Run {
    line_i: usize,
    top: f32,
    baseline: f32,
    w: f32,
    /// Horizontal alignment offset.
    dx: f32,
    glyphs: Vec<cosmic_text::LayoutGlyph>,
}

/// Caret position for byte offset `b` inside logical line `line_i`.
fn caret_at(runs: &[Run], line_i: usize, b: usize, line_h: f32) -> Point {
    let line_runs: Vec<&Run> = runs.iter().filter(|r| r.line_i == line_i).collect();
    let Some(last) = line_runs.last() else {
        // Line not laid out (e.g. trailing empty line): below the last run.
        let top = runs.iter().filter(|r| r.line_i < line_i).map(|r| r.top + line_h).fold(0.0, f32::max);
        return Point::new(0.0, top);
    };
    for (k, run) in line_runs.iter().enumerate() {
        let end = run.glyphs.iter().map(|g| g.end).max().unwrap_or(0);
        let is_last = k + 1 == line_runs.len();
        if b < end || is_last {
            for g in &run.glyphs {
                if b >= g.start && b < g.end {
                    let frac = (b - g.start) as f32 / (g.end - g.start).max(1) as f32;
                    let x = if g.level.is_rtl() { g.x + g.w * (1.0 - frac) } else { g.x + g.w * frac };
                    return Point::new(run.dx + x, run.top);
                }
            }
            // After the last glyph of the run (or an empty run).
            let x = run
                .glyphs
                .iter()
                .filter(|g| g.end <= b)
                .map(|g| if g.level.is_rtl() { g.x } else { g.x + g.w })
                .fold(0.0f32, f32::max);
            let x = if run.glyphs.is_empty() { 0.0 } else { x };
            return Point::new(run.dx + x, run.top);
        }
    }
    Point::new(last.dx, last.top)
}

/// Appends swash outline commands (y-up, glyph origin) at baseline position (`gx`, `gy`).
fn push_outline(pb: &mut PathBuilder, cmds: &[Command], gx: f32, gy: f32) {
    macro_rules! m {
        ($v:expr) => {
            (gx + $v.x, gy - $v.y)
        };
    }
    for c in cmds {
        match *c {
            Command::MoveTo(p) => {
                let (x, y) = m!(p);
                pb.move_to(x, y);
            }
            Command::LineTo(p) => {
                let (x, y) = m!(p);
                pb.line_to(x, y);
            }
            Command::QuadTo(c, p) => {
                let ((cx, cy), (x, y)) = (m!(c), m!(p));
                pb.quad_to(cx, cy, x, y);
            }
            Command::CurveTo(c1, c2, p) => {
                let ((ax, ay), (bx, by), (x, y)) = (m!(c1), m!(c2), m!(p));
                pb.cubic_to(ax, ay, bx, by, x, y);
            }
            Command::Close => pb.close(),
        }
    }
}
