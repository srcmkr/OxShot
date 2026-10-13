//! Vector geometry for the annotation shapes: arrows, rounded boxes, speech bubbles,
//! braces, smoothed freehand strokes and magnifier cones. Everything here produces
//! tiny-skia paths in base-image coordinates.

use tiny_skia::{Path, PathBuilder};

use crate::geom::{Point, Rect};
use crate::model::{ArrowHeads, ArrowShape, ArrowStyle};

/// Cubic Bézier constant for quarter-circle arcs.
const KAPPA: f32 = 0.552_284_8;

trait PbExt {
    fn mv(&mut self, p: Point);
    fn ln(&mut self, p: Point);
    fn qd(&mut self, c: Point, p: Point);
    fn cb(&mut self, c1: Point, c2: Point, p: Point);
}

impl PbExt for PathBuilder {
    fn mv(&mut self, p: Point) {
        self.move_to(p.x, p.y);
    }
    fn ln(&mut self, p: Point) {
        self.line_to(p.x, p.y);
    }
    fn qd(&mut self, c: Point, p: Point) {
        self.quad_to(c.x, c.y, p.x, p.y);
    }
    fn cb(&mut self, c1: Point, c2: Point, p: Point) {
        self.cubic_to(c1.x, c1.y, c2.x, c2.y, p.x, p.y);
    }
}

// ---------------------------------------------------------------------------------------------
// Quadratic Bézier helpers

#[derive(Clone, Copy, Debug)]
pub(crate) struct Quad {
    pub p0: Point,
    pub c: Point,
    pub p1: Point,
}

impl Quad {
    pub fn at(&self, t: f32) -> Point {
        let u = 1.0 - t;
        self.p0.scale(u * u) + self.c.scale(2.0 * u * t) + self.p1.scale(t * t)
    }
    pub fn tangent(&self, t: f32) -> Point {
        let d = (self.c - self.p0).scale(2.0 * (1.0 - t)) + (self.p1 - self.c).scale(2.0 * t);
        if d.length() < 1e-4 { (self.p1 - self.p0).normalized() } else { d.normalized() }
    }
    /// Control point of the sub-curve between `t0` and `t1` (polar form / blossom).
    pub fn sub(&self, t0: f32, t1: f32) -> Quad {
        let c = self.p0.scale((1.0 - t0) * (1.0 - t1))
            + self.c.scale((1.0 - t0) * t1 + t0 * (1.0 - t1))
            + self.p1.scale(t0 * t1);
        Quad { p0: self.at(t0), c, p1: self.at(t1) }
    }
    /// Approximate arc length.
    pub fn length(&self) -> f32 {
        let n = 16;
        (0..n).map(|i| self.at(i as f32 / n as f32).distance(self.at((i + 1) as f32 / n as f32))).sum()
    }
    /// Parameter `t` where the curve is `dist` away (straight line) from `p1`, searching
    /// back from the end.
    pub fn t_at_dist_from_end(&self, dist: f32) -> f32 {
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..24 {
            let mid = 0.5 * (lo + hi);
            if self.at(mid).distance(self.p1) > dist { lo = mid } else { hi = mid }
        }
        lo
    }
    pub fn t_at_dist_from_start(&self, dist: f32) -> f32 {
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..24 {
            let mid = 0.5 * (lo + hi);
            if self.at(mid).distance(self.p0) < dist { lo = mid } else { hi = mid }
        }
        hi
    }
    pub fn push(&self, pb: &mut PathBuilder) {
        pb.mv(self.p0);
        pb.qd(self.c, self.p1);
    }
}

// ---------------------------------------------------------------------------------------------
// Arrows

/// Head length and full head width for a stroke width.
pub(crate) fn arrow_head_size(width: f32) -> (f32, f32) {
    (3.5 * width + 6.0, 3.0 * width + 6.0)
}

/// Geometry of an arrow: an optional stroked shaft and filled parts.
pub(crate) struct ArrowGeom {
    /// Shaft to stroke with round caps (`Line` style only).
    pub shaft: Option<Path>,
    /// Filled heads / tapered body.
    pub fill: Option<Path>,
}

fn arrow_quad(a: &ArrowShape) -> Quad {
    Quad { p0: a.start, c: a.curve.unwrap_or(a.start.lerp(a.end, 0.5)), p1: a.end }
}

/// Filled arrow head with a slight swept-back notch. `dir` points towards the tip.
fn push_head(pb: &mut PathBuilder, tip: Point, dir: Point, len: f32, width: f32) {
    let n = dir.perp();
    let base = tip - dir.scale(len);
    let notch = tip - dir.scale(len * 0.86);
    pb.mv(tip);
    pb.ln(base + n.scale(width * 0.5));
    pb.ln(notch);
    pb.ln(base - n.scale(width * 0.5));
    pb.close();
}

pub(crate) fn arrow_geometry(a: &ArrowShape) -> ArrowGeom {
    let q = arrow_quad(a);
    let w = a.width.max(0.5);
    let total = q.length();
    let (mut hl, mut hw) = arrow_head_size(w);
    let n_heads = match a.heads {
        ArrowHeads::End => 1.0,
        ArrowHeads::Both => 2.0,
        ArrowHeads::None => 0.0,
    };
    // Shrink heads on short arrows so they never eat the whole shaft.
    if n_heads > 0.0 {
        let max_len = total * if n_heads > 1.0 { 0.4 } else { 0.6 };
        if hl > max_len && hl > 0.0 {
            let k = (max_len / hl).max(0.0);
            hl *= k;
            hw = (hw * k).max(w * 1.6);
        }
    }
    let has_end = n_heads > 0.0 && hl > 0.5;
    let has_start = n_heads > 1.0 && hl > 0.5;
    let t_end_base = if has_end { q.t_at_dist_from_end(hl) } else { 1.0 };
    let t_start_base = if has_start { q.t_at_dist_from_start(hl) } else { 0.0 };
    let end_dir = (q.p1 - q.at(t_end_base)).normalized();
    let start_dir = (q.p0 - q.at(t_start_base)).normalized();

    match a.style {
        ArrowStyle::Line => {
            let mut heads = PathBuilder::new();
            if has_end {
                push_head(&mut heads, q.p1, end_dir, hl, hw);
            }
            if has_start {
                push_head(&mut heads, q.p0, start_dir, hl, hw);
            }
            // The shaft ends inside the head so its round cap is hidden.
            let t1 = if has_end { q.t_at_dist_from_end(hl * 0.6) } else { 1.0 };
            let t0 = if has_start { q.t_at_dist_from_start(hl * 0.6) } else { 0.0 };
            let mut shaft = PathBuilder::new();
            if total < 1e-3 {
                // Degenerate: a dot (round cap of a zero-length line needs a tiny segment).
                shaft.mv(q.p0);
                shaft.ln(q.p0 + Point::new(0.01, 0.0));
            } else if a.curve.is_some() {
                q.sub(t0, t1).push(&mut shaft);
            } else {
                shaft.mv(q.at(t0));
                shaft.ln(q.at(t1));
            }
            ArrowGeom { shaft: shaft.finish(), fill: heads.finish() }
        }
        ArrowStyle::Tapered => {
            let samples = if a.curve.is_some() { 32 } else { 1 };
            let mut left = Vec::with_capacity(samples + 1);
            let mut right = Vec::with_capacity(samples + 1);
            let tail_hw = (w * 0.08).max(0.4);
            let max_hw = w * 0.75;
            for i in 0..=samples {
                let f = i as f32 / samples as f32;
                let t = t_start_base + (t_end_base - t_start_base) * f;
                let p = q.at(t);
                let mut n = q.tangent(t).perp();
                if i == samples && has_end {
                    n = end_dir.perp();
                }
                if i == 0 && has_start {
                    n = start_dir.scale(-1.0).perp();
                }
                let hw_i = match a.heads {
                    ArrowHeads::End => tail_hw + (max_hw - tail_hw) * f,
                    ArrowHeads::Both => w * 0.6,
                    ArrowHeads::None => tail_hw + (w * 0.6 - tail_hw) * f,
                };
                left.push(p + n.scale(hw_i));
                right.push(p - n.scale(hw_i));
            }
            let mut pb = PathBuilder::new();
            if has_start {
                let base = q.p0 - start_dir.scale(hl);
                let n = start_dir.scale(-1.0).perp();
                pb.mv(q.p0);
                pb.ln(base - n.scale(hw * 0.5));
                pb.ln(right[0]);
                pb.ln(left[0]);
                pb.ln(base + n.scale(hw * 0.5));
                pb.close();
                pb.mv(left[0]);
            } else {
                pb.mv(left[0]);
            }
            for p in &left[1..] {
                pb.ln(*p);
            }
            if has_end {
                let base = q.p1 - end_dir.scale(hl);
                let n = end_dir.perp();
                pb.ln(base + n.scale(hw * 0.5));
                pb.ln(q.p1);
                pb.ln(base - n.scale(hw * 0.5));
            }
            for p in right.iter().rev() {
                pb.ln(*p);
            }
            pb.close();
            ArrowGeom { shaft: None, fill: pb.finish() }
        }
    }
}

/// Conservative bounds of an arrow (control-point hull inflated by the widest part).
pub(crate) fn arrow_bounds(a: &ArrowShape) -> Rect {
    let mut pts = vec![a.start, a.end];
    if let Some(c) = a.curve {
        pts.push(c);
    }
    let w = a.width.max(0.5);
    let (_, hw) = arrow_head_size(w);
    let r = Rect::bounding(&pts).unwrap_or_default();
    let pad = match a.heads {
        ArrowHeads::None => w,
        _ => hw * 0.5 + w,
    };
    r.inflate(pad + 1.0)
}

// ---------------------------------------------------------------------------------------------
// Boxes

/// Rounded rectangle (clockwise). Radius is clamped to half the shorter side.
pub(crate) fn rounded_rect(r: Rect, radius: f32) -> Option<Path> {
    let r = r.normalized();
    let rad = radius.clamp(0.0, r.w.min(r.h) * 0.5);
    if rad < 0.01 {
        return Some(PathBuilder::from_rect(tiny_skia::Rect::from_xywh(r.x, r.y, r.w, r.h)?));
    }
    let mut pb = PathBuilder::new();
    push_rounded_box(&mut pb, r, rad, None);
    pb.finish()
}

/// A speech-bubble tail inserted into one edge of a rounded box.
#[derive(Clone, Copy)]
pub(crate) struct Tail {
    /// Edge index: 0 = top, 1 = right, 2 = bottom, 3 = left.
    edge: usize,
    /// Base start and end along the clockwise edge direction.
    a: Point,
    b: Point,
    tip: Point,
}

/// Decides where a bubble pointer leaves the box. `None` if the tail lies inside.
pub(crate) fn bubble_tail(r: Rect, radius: f32, tip: Point) -> Option<Tail> {
    let r = r.normalized();
    if r.w < 1.0 || r.h < 1.0 || r.contains(tip) {
        return None;
    }
    let rad = radius.clamp(0.0, r.w.min(r.h) * 0.5);
    let c = r.center();
    let dx = (tip.x - c.x) / (r.w * 0.5);
    let dy = (tip.y - c.y) / (r.h * 0.5);
    let edge = if dx.abs() > dy.abs() {
        if dx > 0.0 { 1 } else { 3 }
    } else if dy > 0.0 {
        2
    } else {
        0
    };
    let (s, e) = edge_points(r, edge);
    let dir = (e - s).normalized();
    let len = s.distance(e);
    let lo = rad;
    let hi = len - rad;
    let avail = (hi - lo).max(0.0);
    let bw = (r.w.min(r.h) / 3.0).min(avail).max(avail.min(4.0));
    let proj = (tip - s).dot(dir);
    let center = if avail <= bw {
        len * 0.5
    } else {
        proj.clamp(lo + bw * 0.5, hi - bw * 0.5)
    };
    Some(Tail {
        edge,
        a: s + dir.scale(center - bw * 0.5),
        b: s + dir.scale(center + bw * 0.5),
        tip,
    })
}

/// Clockwise start/end corners of an edge.
fn edge_points(r: Rect, edge: usize) -> (Point, Point) {
    let tl = Point::new(r.x, r.y);
    let tr = Point::new(r.right(), r.y);
    let br = Point::new(r.right(), r.bottom());
    let bl = Point::new(r.x, r.bottom());
    match edge {
        0 => (tl, tr),
        1 => (tr, br),
        2 => (br, bl),
        _ => (bl, tl),
    }
}

/// Pushes a closed clockwise rounded box, optionally with a speech-bubble tail merged in.
fn push_rounded_box(pb: &mut PathBuilder, r: Rect, rad: f32, tail: Option<Tail>) {
    for edge in 0..4 {
        let (s, e) = edge_points(r, edge);
        let dir = (e - s).normalized();
        let start = s + dir.scale(rad);
        let end = e - dir.scale(rad);
        if edge == 0 {
            pb.mv(start);
        } else {
            pb.ln(start);
        }
        if let Some(t) = tail.filter(|t| t.edge == edge) {
            let mid = t.a.lerp(t.b, 0.5);
            // Concave flanks: the control points are pulled towards the tail's axis.
            let ca = t.a.lerp(t.tip, 0.5).lerp(mid.lerp(t.tip, 0.5), 0.3);
            let cb = t.b.lerp(t.tip, 0.5).lerp(mid.lerp(t.tip, 0.5), 0.3);
            pb.ln(t.a);
            pb.qd(ca, t.tip);
            pb.qd(cb, t.b);
        }
        pb.ln(end);
        if rad > 0.01 {
            // Corner arc at `e` turning into the next edge.
            let (_, e2) = edge_points(r, (edge + 1) % 4);
            let ndir = (e2 - e).normalized();
            let next = e + ndir.scale(rad);
            pb.cb(end + dir.scale(rad * KAPPA), next - ndir.scale(rad * KAPPA), next);
        }
    }
    pb.close();
}

/// Rounded box with an optional pointer, as one closed outline.
pub(crate) fn bubble_path(r: Rect, radius: f32, tip: Option<Point>) -> Option<Path> {
    let r = r.normalized();
    if r.w <= 0.0 || r.h <= 0.0 {
        return None;
    }
    let rad = radius.clamp(0.0, r.w.min(r.h) * 0.5);
    let tail = tip.and_then(|t| bubble_tail(r, rad, t));
    let mut pb = PathBuilder::new();
    push_rounded_box(&mut pb, r, rad, tail);
    pb.finish()
}

// ---------------------------------------------------------------------------------------------
// Brace

/// A curly brace from `start` to `end` whose middle tip sticks out by `depth`
/// along `(end - start).perp()`.
pub(crate) fn brace_path(start: Point, end: Point, depth: f32) -> Option<Path> {
    let len = start.distance(end);
    if len < 0.5 {
        return None;
    }
    let u = (end - start).normalized();
    let n = u.perp();
    let h = len * 0.5;
    let d = depth;
    // Local -> world: x along the brace, y outward (signed by depth).
    let w = |x: f32, y: f32| start + u.scale(x) + n.scale(y);
    // Horizontal extent of each curl: roughly circular, limited by the half length.
    let r = (d.abs() * 0.5 * 1.2).min(h * 0.5);
    let y1 = d * 0.5;
    let mut pb = PathBuilder::new();
    pb.mv(w(0.0, 0.0));
    // First curl: leaves the end perpendicular, turns parallel to the brace.
    pb.cb(w(0.0, y1 * KAPPA), w(r * (1.0 - KAPPA), y1), w(r, y1));
    pb.ln(w(h - r, y1));
    // Into the tip, which points straight outwards.
    pb.cb(w(h - r + r * KAPPA, y1), w(h, y1 + (d - y1) * (1.0 - KAPPA)), w(h, d));
    pb.cb(w(h, y1 + (d - y1) * (1.0 - KAPPA)), w(h + r - r * KAPPA, y1), w(h + r, y1));
    pb.ln(w(len - r, y1));
    pb.cb(w(len - r * (1.0 - KAPPA), y1), w(len, y1 * KAPPA), w(len, 0.0));
    pb.finish()
}

// ---------------------------------------------------------------------------------------------
// Polylines

/// Smooth curve through freehand samples (quadratic Béziers between midpoints).
pub(crate) fn smooth_path(points: &[Point]) -> Option<Path> {
    let mut pb = PathBuilder::new();
    match points {
        [] => return None,
        [p] => {
            pb.mv(*p);
            pb.ln(*p + Point::new(0.01, 0.0));
        }
        [a, b] => {
            pb.mv(*a);
            pb.ln(*b);
        }
        _ => {
            pb.mv(points[0]);
            pb.ln(points[0].lerp(points[1], 0.5));
            for i in 1..points.len() - 1 {
                pb.qd(points[i], points[i].lerp(points[i + 1], 0.5));
            }
            pb.ln(points[points.len() - 1]);
        }
    }
    pb.finish()
}

pub(crate) fn polyline_path(points: &[Point], closed: bool) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let (first, rest) = points.split_first()?;
    pb.mv(*first);
    if rest.is_empty() {
        pb.ln(*first + Point::new(0.01, 0.0));
    }
    for p in rest {
        pb.ln(*p);
    }
    if closed && points.len() > 2 {
        pb.close();
    }
    pb.finish()
}

// ---------------------------------------------------------------------------------------------
// Magnifier

/// External tangent segments between two circles (`None` if one contains the other).
pub(crate) fn outer_tangents(
    c1: Point,
    r1: f32,
    c2: Point,
    r2: f32,
) -> Option<[(Point, Point); 2]> {
    let d = c1.distance(c2);
    if d <= (r1 - r2).abs() + 0.5 {
        return None;
    }
    let u = (c2 - c1).normalized();
    let cos = (r1 - r2) / d;
    let sin = (1.0 - cos * cos).max(0.0).sqrt();
    let rot = |s: f32| Point::new(u.x * cos - u.y * s, u.x * s + u.y * cos);
    let n1 = rot(sin);
    let n2 = rot(-sin);
    Some([
        (c1 + n1.scale(r1), c2 + n1.scale(r2)),
        (c1 + n2.scale(r1), c2 + n2.scale(r2)),
    ])
}

pub(crate) fn circle(c: Point, r: f32) -> Option<Path> {
    PathBuilder::from_circle(c.x, c.y, r.max(0.01))
}

/// Closed polygon through `points`.
pub(crate) fn polygon(points: &[Point]) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let (first, rest) = points.split_first()?;
    pb.mv(*first);
    for p in rest {
        pb.ln(*p);
    }
    pb.close();
    pb.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tangents_touch_both_circles() {
        let (c1, r1, c2, r2) = (Point::new(0.0, 0.0), 10.0, Point::new(100.0, 20.0), 40.0);
        let t = outer_tangents(c1, r1, c2, r2).unwrap();
        for (a, b) in t {
            assert!((a.distance(c1) - r1).abs() < 1e-3);
            assert!((b.distance(c2) - r2).abs() < 1e-3);
            // The segment is perpendicular to both radii.
            let dir = (b - a).normalized();
            assert!((a - c1).normalized().dot(dir).abs() < 1e-3);
            assert!((b - c2).normalized().dot(dir).abs() < 1e-3);
        }
    }

    #[test]
    fn quad_sub_endpoints() {
        let q = Quad { p0: Point::new(0.0, 0.0), c: Point::new(50.0, 80.0), p1: Point::new(100.0, 0.0) };
        let s = q.sub(0.25, 0.75);
        assert!(s.p0.distance(q.at(0.25)) < 1e-4);
        assert!(s.p1.distance(q.at(0.75)) < 1e-4);
        assert!(s.at(0.5).distance(q.at(0.5)) < 1e-3);
    }
}
