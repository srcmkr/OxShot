use image::{Rgba, RgbaImage};

use super::*;
use crate::geom::{IRect, Point, Rect};
use crate::model::*;

fn gradient(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255]))
}

fn arrow(start: Point, end: Point) -> Shape {
    Shape::Arrow(ArrowShape {
        start,
        end,
        curve: None,
        color: Color::RED,
        width: 4.0,
        style: ArrowStyle::Line,
        heads: ArrowHeads::End,
        dashed: false,
    })
}

fn rect(r: Rect) -> Shape {
    Shape::Rect(RectShape { rect: r, stroke: Some(Stroke { color: Color::RED, width: 2.0 }), fill: None, corner_radius: 4.0 })
}

#[test]
fn crop_sets_canvas_only() {
    let doc = Document::new(gradient(100, 80));
    let c = crop(&doc, IRect::new(10, 20, 0, 30));
    assert_eq!(c.canvas.rect, IRect::new(10, 20, 1, 30));
    assert!(Arc::ptr_eq(&c.base, &doc.base));
}

#[test]
fn trim_bakes_canvas() {
    let mut doc = Document::new(gradient(100, 80));
    let id = doc.push(arrow(Point::new(30.0, 30.0), Point::new(60.0, 50.0)));
    doc.canvas = Canvas { rect: IRect::new(20, -10, 50, 60), background: Color::WHITE };
    let t = trim_to_canvas(&doc);
    assert_eq!(t.base.dimensions(), (50, 60));
    assert_eq!(t.canvas.rect, IRect::new(0, 0, 50, 60));
    // Row 0..10 is padding (white), then base pixels shifted by (20, -10).
    assert_eq!(t.base.get_pixel(5, 5).0, [255, 255, 255, 255]);
    assert_eq!(t.base.get_pixel(0, 10).0, doc.base.get_pixel(20, 0).0);
    let Shape::Arrow(a) = &t.get(id).unwrap().shape else { panic!() };
    assert_eq!(a.start, Point::new(10.0, 40.0));
    // Flattened output is unchanged by trimming.
    let mut r = crate::render::Renderer::new();
    assert_eq!(r.flatten(&doc), r.flatten(&t));
}

#[test]
fn resize_scales_everything() {
    let mut doc = Document::new(gradient(200, 100));
    let id = doc.push(arrow(Point::new(10.0, 20.0), Point::new(100.0, 50.0)));
    let tid = doc.push(Shape::Text(TextShape {
        origin: Point::new(4.0, 4.0),
        text: "x".into(),
        style: TextStyle::default(),
        max_width: Some(50.0),
    }));
    let r = resize(&doc, 2.0, 0.5, true);
    assert_eq!(r.base.dimensions(), (400, 50));
    assert_eq!(r.canvas.rect, IRect::new(0, 0, 400, 50));
    let Shape::Arrow(a) = &r.get(id).unwrap().shape else { panic!() };
    assert_eq!(a.start, Point::new(20.0, 10.0));
    assert_eq!(a.end, Point::new(200.0, 25.0));
    assert!((a.width - 4.0).abs() < 1e-4, "sqrt(2*0.5) = 1");
    let r2 = resize(&doc, 2.0, 2.0, true);
    let Shape::Arrow(a2) = &r2.get(id).unwrap().shape else { panic!() };
    assert!((a2.width - 8.0).abs() < 1e-4);
    let Shape::Text(t) = &r2.get(tid).unwrap().shape else { panic!() };
    assert_eq!(t.style.size, 40.0);
    assert_eq!(t.max_width, Some(100.0));
    let r3 = resize(&doc, 2.0, 2.0, false);
    let Shape::Arrow(a3) = &r3.get(id).unwrap().shape else { panic!() };
    assert_eq!(a3.width, 4.0);
    assert_eq!(a3.end, Point::new(200.0, 100.0));
}

#[test]
fn cut_out_moves_objects() {
    for edge in [CutEdge::Straight, CutEdge::Zigzag, CutEdge::Wave] {
        let mut doc = Document::new(gradient(120, 300));
        let before = doc.push(rect(Rect::new(10.0, 10.0, 50.0, 40.0)));
        let after = doc.push(rect(Rect::new(10.0, 200.0, 50.0, 40.0)));
        let across = doc.push(arrow(Point::new(20.0, 50.0), Point::new(80.0, 250.0)));
        let spanning = doc.push(rect(Rect::new(5.0, 80.0, 30.0, 100.0)));
        let inside = doc.push(rect(Rect::new(5.0, 120.0, 30.0, 20.0)));
        let c = cut_out(&doc, false, 100.0, 160.0, edge);
        assert_eq!(c.base.dimensions(), (120, 240), "{edge:?}");
        assert_eq!(c.canvas.rect, IRect::new(0, 0, 120, 240));
        let r = |id| match &c.get(id).unwrap().shape {
            Shape::Rect(r) => r.rect,
            _ => panic!(),
        };
        assert_eq!(r(before), Rect::new(10.0, 10.0, 50.0, 40.0));
        assert_eq!(r(after), Rect::new(10.0, 140.0, 50.0, 40.0));
        assert_eq!(r(spanning), Rect::new(5.0, 80.0, 30.0, 40.0));
        assert_eq!(r(inside), Rect::new(5.0, 100.0, 30.0, 20.0));
        let Shape::Arrow(a) = &c.get(across).unwrap().shape else { panic!() };
        assert_eq!(a.start, Point::new(20.0, 50.0));
        assert_eq!(a.end, Point::new(80.0, 190.0));
        // Far away from the seam the pixels are untouched.
        assert_eq!(c.base.get_pixel(7, 20), doc.base.get_pixel(7, 20));
        assert_eq!(c.base.get_pixel(7, 220), doc.base.get_pixel(7, 280));
        if edge != CutEdge::Straight {
            // There is a (partially) transparent gap at the seam.
            assert!((0..120).any(|x| c.base.get_pixel(x, 100).0[3] < 128));
        }
    }
}

#[test]
fn vertical_cut_out() {
    let mut doc = Document::new(gradient(300, 50));
    let id = doc.push(arrow(Point::new(250.0, 10.0), Point::new(280.0, 40.0)));
    let c = cut_out(&doc, true, 50.0, 150.0, CutEdge::Zigzag);
    assert_eq!(c.base.dimensions(), (200, 50));
    assert_eq!(c.base.get_pixel(190, 25), doc.base.get_pixel(290, 25));
    let Shape::Arrow(a) = &c.get(id).unwrap().shape else { panic!() };
    assert_eq!(a.start, Point::new(150.0, 10.0));
}

#[test]
fn border_grows_with_shadow_and_reflection() {
    let img = gradient(100, 60);
    let plain = preview_border(&img, &BorderEffect::default());
    assert_eq!(plain, img);
    let sh = preview_border(&img, &BorderEffect { shadow: 1.0, ..Default::default() });
    assert!(sh.width() > 100 && sh.height() > 60);
    let rf = preview_border(&img, &BorderEffect { reflection: 1.0, ..Default::default() });
    assert_eq!(rf.dimensions(), (100, 60 + 18));
    let rounded = preview_border(&img, &BorderEffect { corner_radius: 12.0, ..Default::default() });
    assert_eq!(rounded.get_pixel(0, 0).0[3], 0);
    assert_eq!(rounded.get_pixel(50, 30).0[3], 255);
    let torn = preview_border(&img, &BorderEffect { torn: TornEdges { bottom: true, ..Default::default() }, ..Default::default() });
    assert!((0..100).all(|x| torn.get_pixel(x, 59).0[3] < 255));
    assert_eq!(torn.get_pixel(50, 5).0[3], 255);
}

#[test]
fn rotation_90_swaps_dimensions() {
    let img = gradient(100, 60);
    let r = preview_border(&img, &BorderEffect { rotation_deg: 90.0, ..Default::default() });
    assert_eq!(r.dimensions(), (60, 100));
    // Clockwise: the top-left pixel ends up at the top-right.
    assert_eq!(r.get_pixel(59, 0), img.get_pixel(0, 0));
    let r45 = preview_border(&img, &BorderEffect { rotation_deg: 45.0, ..Default::default() });
    assert!(r45.width() > 110 && r45.height() > 110);
}

#[test]
fn apply_border_bakes_objects() {
    let mut doc = Document::new(gradient(80, 80));
    doc.push(rect(Rect::new(10.0, 10.0, 20.0, 20.0)));
    let out = apply_border(&doc, &BorderEffect { grayscale: true, border_width: 2.0, ..Default::default() });
    assert!(out.objects.is_empty());
    assert_eq!(out.canvas.rect, IRect::new(0, 0, 80, 80));
    let p = out.base.get_pixel(40, 40).0;
    assert!(p[0] == p[1] && p[1] == p[2]);
}
