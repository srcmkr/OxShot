//! Coordinate mapping of annotation shapes (used by resize, cut-out and trim).

use crate::geom::{Point, Rect};
use crate::model::*;

/// How a geometric operation maps a shape.
pub(crate) struct ShapeMap<'a> {
    /// Maps a point in old base coordinates to new base coordinates.
    pub point: &'a dyn Fn(Point) -> Point,
    /// Factor for geometric radii (corner radius, magnifier radius, brace depth).
    pub geo: f32,
    /// Factor for stroke widths, font sizes, step radii, paddings.
    pub style: f32,
    /// Factor for horizontal extents (text wrap width).
    pub xscale: f32,
}

impl ShapeMap<'_> {
    fn p(&self, p: Point) -> Point {
        (self.point)(p)
    }

    /// Maps a rect by its two corners (so a rect spanning a cut band shrinks).
    fn rect(&self, r: Rect) -> Rect {
        let r = r.normalized();
        Rect::from_points(self.p(r.origin()), self.p(Point::new(r.right(), r.bottom())))
    }

    /// Moves a rect rigidly with its center (keeps its size, scaled by `xscale`/`geo`).
    fn rigid_rect(&self, r: Rect, scale: (f32, f32)) -> Rect {
        let r = r.normalized();
        let c = self.p(r.center());
        let (w, h) = (r.w * scale.0, r.h * scale.1);
        Rect::new(c.x - w / 2.0, c.y - h / 2.0, w, h)
    }

    fn stroke(&self, s: Stroke) -> Stroke {
        Stroke { width: s.width * self.style, ..s }
    }

    fn text(&self, t: &TextStyle) -> TextStyle {
        TextStyle {
            size: t.size * self.style,
            outline: t.outline.map(|o| self.stroke(o)),
            ..t.clone()
        }
    }

    /// Maps every geometric property of `shape`.
    ///
    /// `rect_scale` is the size factor for shapes that move rigidly (stamps): `(sx, sy)`
    /// when resizing, `(1, 1)` for cuts and translations.
    pub fn shape(&self, shape: &Shape, rect_scale: (f32, f32)) -> Shape {
        match shape {
            Shape::Arrow(a) => Shape::Arrow(ArrowShape {
                start: self.p(a.start),
                end: self.p(a.end),
                curve: a.curve.map(|c| self.p(c)),
                width: a.width * self.style,
                ..a.clone()
            }),
            Shape::Rect(r) => Shape::Rect(RectShape {
                rect: self.rect(r.rect),
                stroke: r.stroke.map(|s| self.stroke(s)),
                corner_radius: r.corner_radius * self.geo,
                ..r.clone()
            }),
            Shape::Ellipse(e) => Shape::Ellipse(EllipseShape {
                rect: self.rect(e.rect),
                stroke: e.stroke.map(|s| self.stroke(s)),
                ..e.clone()
            }),
            Shape::Text(t) => Shape::Text(TextShape {
                origin: self.p(t.origin),
                style: self.text(&t.style),
                max_width: t.max_width.map(|w| w * self.xscale),
                ..t.clone()
            }),
            Shape::Callout(c) => Shape::Callout(CalloutShape {
                rect: self.rect(c.rect),
                tail: c.tail.map(|t| self.p(t)),
                style: self.text(&c.style),
                border: c.border.map(|s| self.stroke(s)),
                corner_radius: c.corner_radius * self.geo,
                padding: c.padding * self.style,
                ..c.clone()
            }),
            Shape::Step(s) => Shape::Step(StepShape {
                center: self.p(s.center),
                radius: s.radius * self.style,
                border: s.border.map(|b| self.stroke(b)),
                connector: s.connector.as_ref().map(|c| Connector {
                    target: self.p(c.target),
                    width: c.width * self.style,
                    ..c.clone()
                }),
                ..s.clone()
            }),
            Shape::Highlight(h) => Shape::Highlight(HighlightShape { rect: self.rect(h.rect), ..h.clone() }),
            Shape::Polygon(p) => Shape::Polygon(PolygonShape {
                points: p.points.iter().map(|&q| self.p(q)).collect(),
                stroke: p.stroke.map(|s| self.stroke(s)),
                ..p.clone()
            }),
            Shape::Freehand(f) => Shape::Freehand(FreehandShape {
                points: f.points.iter().map(|&q| self.p(q)).collect(),
                width: f.width * self.style,
                ..f.clone()
            }),
            Shape::Blur(b) => Shape::Blur(BlurShape { rect: self.rect(b.rect), ..b.clone() }),
            Shape::Stamp(s) => Shape::Stamp(StampShape { rect: self.rigid_rect(s.rect, rect_scale), ..s.clone() }),
            Shape::Magnifier(m) => Shape::Magnifier(MagnifierShape {
                source: self.p(m.source),
                glass: self.p(m.glass),
                radius: m.radius * self.geo,
                border: self.stroke(m.border),
                ..m.clone()
            }),
            Shape::Brace(b) => Shape::Brace(BraceShape {
                start: self.p(b.start),
                end: self.p(b.end),
                depth: b.depth * self.geo,
                width: b.width * self.style,
                ..b.clone()
            }),
        }
    }
}

/// All control points of a shape (rect corners included).
pub(crate) fn control_points(shape: &Shape) -> Vec<Point> {
    let corners = |r: Rect| {
        let r = r.normalized();
        vec![r.origin(), Point::new(r.right(), r.bottom())]
    };
    match shape {
        Shape::Arrow(a) => [Some(a.start), Some(a.end), a.curve].into_iter().flatten().collect(),
        Shape::Rect(r) => corners(r.rect),
        Shape::Ellipse(e) => corners(e.rect),
        Shape::Text(t) => vec![t.origin],
        Shape::Callout(c) => {
            let mut v = corners(c.rect);
            v.extend(c.tail);
            v
        }
        Shape::Step(s) => {
            let mut v = vec![s.center];
            v.extend(s.connector.as_ref().map(|c| c.target));
            v
        }
        Shape::Highlight(h) => corners(h.rect),
        Shape::Polygon(p) => p.points.clone(),
        Shape::Freehand(f) => f.points.clone(),
        Shape::Blur(b) => corners(b.rect),
        Shape::Stamp(s) => corners(s.rect),
        Shape::Magnifier(m) => vec![m.source, m.glass],
        Shape::Brace(b) => vec![b.start, b.end],
    }
}

/// Translates a shape by `d` without any scaling.
pub(crate) fn translate(shape: &Shape, d: Point) -> Shape {
    let f = move |p: Point| p + d;
    ShapeMap { point: &f, geo: 1.0, style: 1.0, xscale: 1.0 }.shape(shape, (1.0, 1.0))
}
