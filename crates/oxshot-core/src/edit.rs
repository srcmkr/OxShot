//! UI-independent editing logic: creating shapes from a drag, handles, hit testing,
//! moving and resizing. The editor maps pointer input to these functions.

use crate::geom::{Point, Rect};
use crate::model::*;

/// The drawing tools, in toolbar order (same order as Screenpresso).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Tool {
    Select,
    Arrow,
    Rect,
    Text,
    Callout,
    Step,
    Highlight,
    Ellipse,
    Polygon,
    Freehand,
    Blur,
    Stamp,
    Magnifier,
    Brace,
}

impl Tool {
    pub const ALL: [Tool; 14] = [
        Tool::Select,
        Tool::Arrow,
        Tool::Rect,
        Tool::Text,
        Tool::Callout,
        Tool::Step,
        Tool::Highlight,
        Tool::Ellipse,
        Tool::Polygon,
        Tool::Freehand,
        Tool::Blur,
        Tool::Stamp,
        Tool::Magnifier,
        Tool::Brace,
    ];

    /// Stable identifier (used as translation key `tool.<key>` by the app).
    pub fn key(self) -> &'static str {
        match self {
            Tool::Select => "select",
            Tool::Arrow => "arrow",
            Tool::Rect => "rect",
            Tool::Text => "text",
            Tool::Callout => "callout",
            Tool::Step => "step",
            Tool::Highlight => "highlight",
            Tool::Ellipse => "ellipse",
            Tool::Polygon => "polygon",
            Tool::Freehand => "freehand",
            Tool::Blur => "blur",
            Tool::Stamp => "stamp",
            Tool::Magnifier => "magnifier",
            Tool::Brace => "brace",
        }
    }

    /// Single-key shortcut.
    pub fn shortcut(self) -> &'static str {
        match self {
            Tool::Select => "v",
            Tool::Arrow => "a",
            Tool::Rect => "r",
            Tool::Text => "t",
            Tool::Callout => "b",
            Tool::Step => "n",
            Tool::Highlight => "h",
            Tool::Ellipse => "e",
            Tool::Polygon => "p",
            Tool::Freehand => "f",
            Tool::Blur => "x",
            Tool::Stamp => "i",
            Tool::Magnifier => "m",
            Tool::Brace => "k",
        }
    }

    pub fn from_shortcut(key: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.shortcut() == key)
    }

    pub fn of_shape(shape: &Shape) -> Tool {
        match shape {
            Shape::Arrow(_) => Tool::Arrow,
            Shape::Rect(_) => Tool::Rect,
            Shape::Ellipse(_) => Tool::Ellipse,
            Shape::Text(_) => Tool::Text,
            Shape::Callout(_) => Tool::Callout,
            Shape::Step(_) => Tool::Step,
            Shape::Highlight(_) => Tool::Highlight,
            Shape::Polygon(_) => Tool::Polygon,
            Shape::Freehand(_) => Tool::Freehand,
            Shape::Blur(_) => Tool::Blur,
            Shape::Stamp(_) => Tool::Stamp,
            Shape::Magnifier(_) => Tool::Magnifier,
            Shape::Brace(_) => Tool::Brace,
        }
    }
}

/// Per-tool default style (what Screenpresso calls "Set default" / presets).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Style {
    pub color: Color,
    pub width: f32,
    pub fill: Option<Color>,
    pub opacity: f32,
    pub shadow: f32,
    pub corner_radius: f32,
    pub text: TextStyle,
    pub arrow_style: ArrowStyle,
    pub arrow_heads: ArrowHeads,
    pub dashed: bool,
    pub blur_mode: BlurMode,
    pub strength: f32,
    pub zoom: f32,
    pub size: f32,
}

impl Style {
    pub fn default_for(tool: Tool) -> Style {
        let base = Style {
            color: Color::RED,
            width: 4.0,
            fill: None,
            opacity: 1.0,
            shadow: 0.6,
            corner_radius: 4.0,
            text: TextStyle::default(),
            arrow_style: ArrowStyle::Tapered,
            arrow_heads: ArrowHeads::End,
            dashed: false,
            blur_mode: BlurMode::Gaussian,
            strength: 8.0,
            zoom: 2.0,
            size: 16.0,
        };
        match tool {
            Tool::Arrow => Style { width: 6.0, ..base },
            Tool::Text => Style { shadow: 0.0, ..base },
            Tool::Callout => Style {
                color: Color::rgb(0x33, 0x33, 0x33),
                width: 2.0,
                fill: Some(Color::rgb(0xFF, 0xF7, 0xC2)),
                corner_radius: 10.0,
                text: TextStyle {
                    color: Color::rgb(0x22, 0x22, 0x22),
                    outline: None,
                    bold: false,
                    size: 18.0,
                    ..TextStyle::default()
                },
                ..base
            },
            Tool::Step => Style { size: 16.0, shadow: 0.5, ..base },
            Tool::Highlight => Style { color: Color::YELLOW, shadow: 0.0, ..base },
            Tool::Blur => Style { shadow: 0.0, ..base },
            Tool::Magnifier => Style {
                color: Color::rgb(0x55, 0x55, 0x55),
                width: 3.0,
                ..base
            },
            Tool::Brace => Style { width: 4.0, shadow: 0.3, ..base },
            Tool::Freehand => Style { width: 5.0, shadow: 0.3, ..base },
            _ => base,
        }
    }
}

/// Stable handle identifiers. Rect-like shapes use the 8 resize handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Handle {
    /// 0=top-left, 1=top, 2=top-right, 3=right, 4=bottom-right, 5=bottom, 6=bottom-left, 7=left
    Resize(u8),
    Start,
    End,
    /// Arrow curve control point.
    Curve,
    /// Callout pointer / step connector target.
    Tail,
    /// Magnifier source center.
    Source,
    /// Magnifier glass radius.
    Radius,
    /// Step radius.
    Size,
    /// Brace depth.
    Depth,
    Vertex(usize),
}

/// Minimum extent of a freshly created shape before it counts as a drag (not a click).
pub const MIN_DRAG: f32 = 4.0;

/// Creates a shape for `tool` from a drag `a` → `b`. `constrain` = Ctrl held
/// (square/circle, 45° arrows). Returns `None` for tools that are not drag-created.
pub fn create_shape(tool: Tool, style: &Style, a: Point, b: Point, constrain: bool, step_number: u32) -> Option<Shape> {
    let stroke = (style.width > 0.0).then_some(Stroke { color: style.color, width: style.width });
    let rect = || {
        let b = if constrain { square_corner(a, b) } else { b };
        Rect::from_points(a, b)
    };
    Some(match tool {
        Tool::Arrow => {
            let b = if constrain { snap_angle(a, b) } else { b };
            Shape::Arrow(ArrowShape {
                start: a,
                end: b,
                curve: None,
                color: style.color,
                width: style.width,
                style: style.arrow_style,
                heads: style.arrow_heads,
                dashed: style.dashed,
            })
        }
        Tool::Rect => Shape::Rect(RectShape { rect: rect(), stroke, fill: style.fill, corner_radius: style.corner_radius }),
        Tool::Ellipse => Shape::Ellipse(EllipseShape { rect: rect(), stroke, fill: style.fill }),
        Tool::Highlight => Shape::Highlight(HighlightShape { rect: rect(), color: style.color }),
        Tool::Blur => Shape::Blur(BlurShape { rect: rect(), mode: style.blur_mode, strength: style.strength }),
        Tool::Callout => {
            // Dragging defines the pointer target → box placement: the box is created at
            // the drag end, the tail points to the drag start (where the user pressed).
            let size = Point::new(220.0, 64.0);
            let dir = b - a;
            let (bx, by) = (if dir.x >= 0.0 { b.x } else { b.x - size.x }, if dir.y >= 0.0 { b.y } else { b.y - size.y });
            let tail = (a.distance(b) > 24.0).then_some(a);
            Shape::Callout(CalloutShape {
                rect: Rect::new(bx, by, size.x, size.y),
                tail,
                text: String::new(),
                style: style.text.clone(),
                fill: style.fill.unwrap_or(Color::WHITE),
                border: stroke,
                corner_radius: style.corner_radius,
                padding: 10.0,
            })
        }
        Tool::Step => {
            let connector = (a.distance(b) > 12.0).then(|| Connector { target: b, style: ConnectorStyle::Arrow, width: (style.size / 4.0).max(2.0) });
            Shape::Step(StepShape {
                center: a,
                radius: style.size,
                number: step_number,
                fill: style.color,
                text_color: if style.color.luminance() > 0.6 { Color::BLACK } else { Color::WHITE },
                border: Some(Stroke { color: Color::WHITE, width: 2.0 }),
                connector,
            })
        }
        Tool::Magnifier => {
            // Press on the detail to magnify, drag to where the glass should go.
            let radius = 70.0f32;
            let glass = if a.distance(b) < MIN_DRAG { a + Point::new(radius * 1.6, -radius * 1.2) } else { b };
            Shape::Magnifier(MagnifierShape {
                source: a,
                glass,
                radius,
                zoom: style.zoom,
                border: Stroke { color: style.color, width: style.width },
                show_source: true,
            })
        }
        Tool::Brace => {
            let b = if constrain { snap_angle(a, b) } else { b };
            let len = a.distance(b);
            Shape::Brace(BraceShape { start: a, end: b, depth: (len * 0.12).clamp(8.0, 40.0), color: style.color, width: style.width })
        }
        Tool::Freehand => Shape::Freehand(FreehandShape { points: vec![a, b], color: style.color, width: style.width }),
        Tool::Polygon => Shape::Polygon(PolygonShape { points: vec![a, b], closed: false, stroke, fill: style.fill }),
        Tool::Text => Shape::Text(TextShape { origin: a, text: String::new(), style: style.text.clone(), max_width: None }),
        Tool::Select | Tool::Stamp => return None,
    })
}

/// Snaps `b` so that `a→b` is a multiple of 45°.
pub fn snap_angle(a: Point, b: Point) -> Point {
    let d = b - a;
    let len = d.length();
    let step = std::f32::consts::FRAC_PI_4;
    let angle = (d.y.atan2(d.x) / step).round() * step;
    a + Point::new(angle.cos() * len, angle.sin() * len)
}

/// Moves `b` so that the rect a→b is a square.
pub fn square_corner(a: Point, b: Point) -> Point {
    let d = b - a;
    let s = d.x.abs().max(d.y.abs());
    a + Point::new(s * d.x.signum(), s * d.y.signum())
}

/// The main geometric rect of rect-like shapes.
pub fn shape_rect(shape: &Shape) -> Option<Rect> {
    match shape {
        Shape::Rect(s) => Some(s.rect),
        Shape::Ellipse(s) => Some(s.rect),
        Shape::Highlight(s) => Some(s.rect),
        Shape::Blur(s) => Some(s.rect),
        Shape::Stamp(s) => Some(s.rect),
        Shape::Callout(s) => Some(s.rect),
        _ => None,
    }
}

fn set_shape_rect(shape: &mut Shape, r: Rect) {
    match shape {
        Shape::Rect(s) => s.rect = r,
        Shape::Ellipse(s) => s.rect = r,
        Shape::Highlight(s) => s.rect = r,
        Shape::Blur(s) => s.rect = r,
        Shape::Stamp(s) => s.rect = r,
        Shape::Callout(s) => s.rect = r,
        _ => {}
    }
}

pub fn rect_handles(r: Rect) -> [Point; 8] {
    let (x0, y0, x1, y1, cx, cy) = (r.x, r.y, r.right(), r.bottom(), r.x + r.w / 2.0, r.y + r.h / 2.0);
    [
        Point::new(x0, y0),
        Point::new(cx, y0),
        Point::new(x1, y0),
        Point::new(x1, cy),
        Point::new(x1, y1),
        Point::new(cx, y1),
        Point::new(x0, y1),
        Point::new(x0, cy),
    ]
}

/// Handles of a shape with their positions (image coordinates).
pub fn handles(shape: &Shape) -> Vec<(Handle, Point)> {
    let mut out = Vec::new();
    if let Some(r) = shape_rect(shape) {
        let hs = rect_handles(r);
        out.extend(hs.iter().enumerate().map(|(i, p)| (Handle::Resize(i as u8), *p)));
    }
    match shape {
        Shape::Arrow(s) => {
            out.push((Handle::Start, s.start));
            out.push((Handle::End, s.end));
            out.push((Handle::Curve, s.curve.unwrap_or_else(|| s.start.lerp(s.end, 0.5))));
        }
        Shape::Callout(s) => {
            let tail = s.tail.unwrap_or_else(|| Point::new(s.rect.x - 30.0, s.rect.bottom() + 30.0));
            out.push((Handle::Tail, tail));
        }
        Shape::Step(s) => {
            out.push((Handle::Size, s.center + Point::new(s.radius, s.radius).scale(std::f32::consts::FRAC_1_SQRT_2)));
            let t = s.connector.as_ref().map_or(s.center + Point::new(s.radius * 2.2, s.radius * 2.2), |c| c.target);
            out.push((Handle::Tail, t));
        }
        Shape::Magnifier(s) => {
            out.push((Handle::Source, s.source));
            out.push((Handle::Radius, s.glass + Point::new(s.radius, 0.0)));
        }
        Shape::Brace(s) => {
            out.push((Handle::Start, s.start));
            out.push((Handle::End, s.end));
            out.push((Handle::Depth, brace_tip(s)));
        }
        Shape::Polygon(s) => out.extend(s.points.iter().enumerate().map(|(i, p)| (Handle::Vertex(i), *p))),
        Shape::Text(_) | Shape::Freehand(_) => {}
        _ => {}
    }
    out
}

pub fn brace_tip(s: &BraceShape) -> Point {
    let n = (s.end - s.start).perp().normalized();
    s.start.lerp(s.end, 0.5) + n.scale(s.depth)
}

/// Applies dragging `handle` to `pos`. `constrain` = Ctrl (keep square / snap angle).
pub fn drag_handle(shape: &mut Shape, handle: Handle, pos: Point, constrain: bool) {
    if let (Handle::Resize(i), Some(r)) = (handle, shape_rect(shape)) {
        let mut x0 = r.x;
        let mut y0 = r.y;
        let mut x1 = r.right();
        let mut y1 = r.bottom();
        match i {
            0 => (x0, y0) = (pos.x, pos.y),
            1 => y0 = pos.y,
            2 => (x1, y0) = (pos.x, pos.y),
            3 => x1 = pos.x,
            4 => (x1, y1) = (pos.x, pos.y),
            5 => y1 = pos.y,
            6 => (x0, y1) = (pos.x, pos.y),
            _ => x0 = pos.x,
        }
        let mut nr = Rect::from_points(Point::new(x0, y0), Point::new(x1, y1));
        let keep_aspect = constrain || matches!(shape, Shape::Stamp(_));
        if keep_aspect && i % 2 == 0 && r.w > 0.0 && r.h > 0.0 {
            // Corner drag: keep aspect ratio anchored at the opposite corner.
            let aspect = if constrain && !matches!(shape, Shape::Stamp(_)) { 1.0 } else { r.w / r.h };
            let anchor = rect_handles(r)[((i + 4) % 8) as usize];
            let w = nr.w.max(nr.h * aspect);
            let h = w / aspect;
            let dx = if pos.x < anchor.x { -w } else { w };
            let dy = if pos.y < anchor.y { -h } else { h };
            nr = Rect::from_points(anchor, anchor + Point::new(dx, dy));
        }
        set_shape_rect(shape, nr);
        return;
    }
    match (shape, handle) {
        (Shape::Arrow(s), Handle::Start) => s.start = if constrain { snap_angle(s.end, pos) } else { pos },
        (Shape::Arrow(s), Handle::End) => s.end = if constrain { snap_angle(s.start, pos) } else { pos },
        (Shape::Arrow(s), Handle::Curve) => {
            // Straighten when the control point is dragged back near the chord.
            let mid = s.start.lerp(s.end, 0.5);
            s.curve = (pos.distance(mid) > 6.0).then_some(pos);
        }
        (Shape::Callout(s), Handle::Tail) => {
            s.tail = (!s.rect.inflate(4.0).contains(pos)).then_some(pos);
        }
        (Shape::Step(s), Handle::Size) => s.radius = (pos.distance(s.center) * std::f32::consts::FRAC_1_SQRT_2).clamp(6.0, 200.0),
        (Shape::Step(s), Handle::Tail) => {
            if pos.distance(s.center) <= s.radius * 1.2 {
                s.connector = None;
            } else if let Some(c) = &mut s.connector {
                c.target = pos;
            } else {
                s.connector = Some(Connector { target: pos, style: ConnectorStyle::Arrow, width: (s.radius / 4.0).max(2.0) });
            }
        }
        (Shape::Magnifier(s), Handle::Source) => s.source = pos,
        (Shape::Magnifier(s), Handle::Radius) => s.radius = pos.distance(s.glass).clamp(12.0, 600.0),
        (Shape::Brace(s), Handle::Start) => s.start = if constrain { snap_angle(s.end, pos) } else { pos },
        (Shape::Brace(s), Handle::End) => s.end = if constrain { snap_angle(s.start, pos) } else { pos },
        (Shape::Brace(s), Handle::Depth) => {
            let n = (s.end - s.start).perp().normalized();
            s.depth = (pos - s.start.lerp(s.end, 0.5)).dot(n);
        }
        (Shape::Polygon(s), Handle::Vertex(i)) => {
            if let Some(p) = s.points.get_mut(i) {
                *p = pos;
            }
        }
        _ => {}
    }
}

/// Moves the whole shape.
pub fn translate(shape: &mut Shape, d: Point) {
    match shape {
        Shape::Arrow(s) => {
            s.start += d;
            s.end += d;
            if let Some(c) = &mut s.curve {
                *c += d;
            }
        }
        Shape::Rect(s) => s.rect = s.rect.translate(d),
        Shape::Ellipse(s) => s.rect = s.rect.translate(d),
        Shape::Highlight(s) => s.rect = s.rect.translate(d),
        Shape::Blur(s) => s.rect = s.rect.translate(d),
        Shape::Stamp(s) => s.rect = s.rect.translate(d),
        Shape::Text(s) => s.origin += d,
        Shape::Callout(s) => {
            s.rect = s.rect.translate(d);
            if let Some(t) = &mut s.tail {
                *t += d;
            }
        }
        Shape::Step(s) => {
            s.center += d;
            if let Some(c) = &mut s.connector {
                c.target += d;
            }
        }
        Shape::Magnifier(s) => {
            s.glass += d;
            s.source += d;
        }
        Shape::Brace(s) => {
            s.start += d;
            s.end += d;
        }
        Shape::Polygon(s) => s.points.iter_mut().for_each(|p| *p += d),
        Shape::Freehand(s) => s.points.iter_mut().for_each(|p| *p += d),
    }
}

/// Alt-drag in Screenpresso moves only the "body" and leaves the anchor where it is:
/// callout box without its tail, step circle without its connector target, magnifier
/// glass without its source.
pub fn translate_body(shape: &mut Shape, d: Point) {
    match shape {
        Shape::Callout(s) => s.rect = s.rect.translate(d),
        Shape::Step(s) => s.center += d,
        Shape::Magnifier(s) => s.glass += d,
        other => translate(other, d),
    }
}

/// Geometric hit test in image coordinates. `tol` is the grab tolerance in image px.
/// `bounds` is the renderer's visual bounds (used for text-like shapes).
pub fn hit_test(shape: &Shape, p: Point, tol: f32, bounds: Rect) -> bool {
    if !bounds.inflate(tol).contains(p) {
        return false;
    }
    match shape {
        Shape::Arrow(s) => {
            let w = s.width / 2.0 + tol;
            match s.curve {
                Some(c) => polyline_distance(&quad_points(s.start, c, s.end, 24), p) <= w * 1.5,
                None => segment_distance(s.start, s.end, p) <= w * 1.5,
            }
        }
        Shape::Rect(s) => {
            if s.fill.is_some() || s.rect.w.min(s.rect.h) < 3.0 * tol {
                true
            } else {
                let w = s.stroke.map_or(0.0, |st| st.width) / 2.0 + tol;
                let inner = s.rect.inflate(-w);
                s.rect.inflate(w).contains(p) && !(inner.w > 0.0 && inner.h > 0.0 && inner.contains(p))
            }
        }
        Shape::Ellipse(s) => {
            let c = s.rect.center();
            let (rx, ry) = (s.rect.w / 2.0, s.rect.h / 2.0);
            if rx < 1.0 || ry < 1.0 {
                return true;
            }
            let d = ((p.x - c.x) / rx).powi(2) + ((p.y - c.y) / ry).powi(2);
            if s.fill.is_some() || rx.min(ry) < 3.0 * tol {
                d <= 1.0 + tol / rx.min(ry)
            } else {
                let w = s.stroke.map_or(0.0, |st| st.width) / 2.0 + tol;
                let k = w / rx.min(ry);
                d.sqrt() >= 1.0 - k && d.sqrt() <= 1.0 + k
            }
        }
        Shape::Step(s) => {
            p.distance(s.center) <= s.radius + tol
                || s.connector.as_ref().is_some_and(|c| segment_distance(s.center, c.target, p) <= c.width / 2.0 + tol)
        }
        Shape::Magnifier(s) => p.distance(s.glass) <= s.radius + tol || p.distance(s.source) <= s.source_radius() + tol,
        Shape::Brace(s) => {
            let tip = brace_tip(s);
            let w = s.width / 2.0 + tol * 1.5;
            segment_distance(s.start, tip, p) <= w + s.depth.abs() * 0.3 || segment_distance(tip, s.end, p) <= w + s.depth.abs() * 0.3
        }
        Shape::Polygon(s) => {
            let w = s.stroke.map_or(0.0, |st| st.width) / 2.0 + tol;
            let mut pts = s.points.clone();
            if s.closed && pts.len() > 2 {
                if s.fill.is_some() && point_in_polygon(&s.points, p) {
                    return true;
                }
                pts.push(pts[0]);
            }
            polyline_distance(&pts, p) <= w
        }
        Shape::Freehand(s) => polyline_distance(&s.points, p) <= s.width / 2.0 + tol,
        Shape::Callout(s) => s.rect.inflate(tol).contains(p) || s.tail.is_some_and(|t| t.distance(p) <= tol * 2.0),
        // Filled rect-like or text: the visual bounds are good enough.
        _ => true,
    }
}

pub fn segment_distance(a: Point, b: Point, p: Point) -> f32 {
    let ab = b - a;
    let len2 = ab.dot(ab);
    if len2 <= f32::EPSILON {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab.scale(t))
}

pub fn polyline_distance(points: &[Point], p: Point) -> f32 {
    match points {
        [] => f32::INFINITY,
        [only] => only.distance(p),
        _ => points.windows(2).map(|w| segment_distance(w[0], w[1], p)).fold(f32::INFINITY, f32::min),
    }
}

pub fn quad_points(a: Point, c: Point, b: Point, n: usize) -> Vec<Point> {
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            a.lerp(c, t).lerp(c.lerp(b, t), t)
        })
        .collect()
}

pub fn point_in_polygon(poly: &[Point], p: Point) -> bool {
    let mut inside = false;
    let mut j = poly.len().wrapping_sub(1);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Whether a freshly created shape is too small to keep (an accidental click).
pub fn is_degenerate(shape: &Shape) -> bool {
    match shape {
        Shape::Arrow(s) => s.start.distance(s.end) < MIN_DRAG * 2.0,
        Shape::Brace(s) => s.start.distance(s.end) < MIN_DRAG * 2.0,
        Shape::Rect(RectShape { rect, .. })
        | Shape::Ellipse(EllipseShape { rect, .. })
        | Shape::Highlight(HighlightShape { rect, .. })
        | Shape::Blur(BlurShape { rect, .. }) => rect.w < MIN_DRAG || rect.h < MIN_DRAG,
        Shape::Freehand(s) => s.points.len() < 2,
        Shape::Polygon(s) => s.points.len() < 2,
        _ => false,
    }
}

/// Applies a style to an existing shape (when the user changes a property with an
/// object selected). Only the properties meaningful for the shape are touched.
pub fn apply_style(obj: &mut Object, st: &Style) {
    obj.opacity = st.opacity;
    obj.shadow = st.shadow;
    let stroke = (st.width > 0.0).then_some(Stroke { color: st.color, width: st.width });
    match &mut obj.shape {
        Shape::Arrow(s) => {
            s.color = st.color;
            s.width = st.width.max(1.0);
            s.style = st.arrow_style;
            s.heads = st.arrow_heads;
            s.dashed = st.dashed;
        }
        Shape::Rect(s) => {
            s.stroke = stroke;
            s.fill = st.fill;
            s.corner_radius = st.corner_radius;
        }
        Shape::Ellipse(s) => {
            s.stroke = stroke;
            s.fill = st.fill;
        }
        Shape::Text(s) => s.style = st.text.clone(),
        Shape::Callout(s) => {
            s.style = st.text.clone();
            s.border = stroke;
            s.fill = st.fill.unwrap_or(Color::WHITE);
            s.corner_radius = st.corner_radius;
        }
        Shape::Step(s) => {
            s.fill = st.color;
            s.text_color = if st.color.luminance() > 0.6 { Color::BLACK } else { Color::WHITE };
            s.radius = st.size;
        }
        Shape::Highlight(s) => s.color = st.color,
        Shape::Polygon(s) => {
            s.stroke = stroke;
            s.fill = st.fill;
        }
        Shape::Freehand(s) => {
            s.color = st.color;
            s.width = st.width.max(1.0);
        }
        Shape::Blur(s) => {
            s.mode = st.blur_mode;
            s.strength = st.strength;
        }
        Shape::Stamp(_) => {}
        Shape::Magnifier(s) => {
            s.border = Stroke { color: st.color, width: st.width };
            s.zoom = st.zoom;
        }
        Shape::Brace(s) => {
            s.color = st.color;
            s.width = st.width.max(1.0);
        }
    }
}

/// Reads the style back from an object (to populate the property bar on selection).
pub fn style_of(obj: &Object, defaults: &Style) -> Style {
    let mut st = defaults.clone();
    st.opacity = obj.opacity;
    st.shadow = obj.shadow;
    let from_stroke = |st: &mut Style, s: &Option<Stroke>| match s {
        Some(s) => {
            st.color = s.color;
            st.width = s.width;
        }
        None => st.width = 0.0,
    };
    match &obj.shape {
        Shape::Arrow(s) => {
            st.color = s.color;
            st.width = s.width;
            st.arrow_style = s.style;
            st.arrow_heads = s.heads;
            st.dashed = s.dashed;
        }
        Shape::Rect(s) => {
            from_stroke(&mut st, &s.stroke);
            st.fill = s.fill;
            st.corner_radius = s.corner_radius;
        }
        Shape::Ellipse(s) => {
            from_stroke(&mut st, &s.stroke);
            st.fill = s.fill;
        }
        Shape::Text(s) => {
            st.text = s.style.clone();
            st.color = s.style.color;
        }
        Shape::Callout(s) => {
            st.text = s.style.clone();
            from_stroke(&mut st, &s.border);
            st.fill = Some(s.fill);
            st.corner_radius = s.corner_radius;
        }
        Shape::Step(s) => {
            st.color = s.fill;
            st.size = s.radius;
        }
        Shape::Highlight(s) => st.color = s.color,
        Shape::Polygon(s) => {
            from_stroke(&mut st, &s.stroke);
            st.fill = s.fill;
        }
        Shape::Freehand(s) => {
            st.color = s.color;
            st.width = s.width;
        }
        Shape::Blur(s) => {
            st.blur_mode = s.mode;
            st.strength = s.strength;
        }
        Shape::Stamp(_) => {}
        Shape::Magnifier(s) => {
            st.color = s.border.color;
            st.width = s.border.width;
            st.zoom = s.zoom;
        }
        Shape::Brace(s) => {
            st.color = s.color;
            st.width = s.width;
        }
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_angle_45() {
        let p = snap_angle(Point::new(0.0, 0.0), Point::new(10.0, 9.0));
        assert!((p.x - p.y).abs() < 1e-3);
    }

    #[test]
    fn rect_resize_handles() {
        let mut s = create_shape(Tool::Rect, &Style::default_for(Tool::Rect), Point::new(10.0, 10.0), Point::new(50.0, 30.0), false, 1).unwrap();
        drag_handle(&mut s, Handle::Resize(4), Point::new(100.0, 100.0), false);
        assert_eq!(shape_rect(&s), Some(Rect::new(10.0, 10.0, 90.0, 90.0)));
        // Dragging past the opposite edge flips instead of producing negative sizes.
        drag_handle(&mut s, Handle::Resize(4), Point::new(0.0, 0.0), false);
        assert_eq!(shape_rect(&s), Some(Rect::new(0.0, 0.0, 10.0, 10.0)));
    }

    #[test]
    fn unfilled_rect_hit_only_on_border() {
        let s = create_shape(Tool::Rect, &Style::default_for(Tool::Rect), Point::new(0.0, 0.0), Point::new(200.0, 200.0), false, 1).unwrap();
        let b = Rect::new(-5.0, -5.0, 210.0, 210.0);
        assert!(hit_test(&s, Point::new(1.0, 100.0), 4.0, b));
        assert!(!hit_test(&s, Point::new(100.0, 100.0), 4.0, b));
    }

    #[test]
    fn step_numbers_increment() {
        let mut d = Document::new(image::RgbaImage::new(10, 10));
        assert_eq!(d.next_step_number(), 1);
        let s = create_shape(Tool::Step, &Style::default_for(Tool::Step), Point::new(5.0, 5.0), Point::new(5.0, 5.0), false, 1).unwrap();
        d.push(s);
        assert_eq!(d.next_step_number(), 2);
    }

    #[test]
    fn brace_depth_handle_roundtrip() {
        let mut s = create_shape(Tool::Brace, &Style::default_for(Tool::Brace), Point::new(0.0, 0.0), Point::new(0.0, 100.0), false, 1).unwrap();
        drag_handle(&mut s, Handle::Depth, Point::new(-30.0, 50.0), false);
        let Shape::Brace(b) = &s else { panic!() };
        let tip = brace_tip(b);
        assert!(tip.distance(Point::new(-30.0, 50.0)) < 1e-3);
    }
}
