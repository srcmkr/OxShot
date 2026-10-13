use image::{Rgba, RgbaImage};

use super::*;
use crate::geom::{IRect, Point, Rect};
use crate::model::*;

fn checker(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| {
        if (x / 8 + y / 8) % 2 == 0 { Rgba([240, 240, 240, 255]) } else { Rgba([30, 60, 90, 255]) }
    })
}

fn stroke(c: Color, w: f32) -> Option<Stroke> {
    Some(Stroke { color: c, width: w })
}

/// One of every shape type.
fn sample_shapes() -> Vec<Shape> {
    let style = TextStyle::default();
    vec![
        Shape::Arrow(ArrowShape {
            start: Point::new(20.0, 20.0),
            end: Point::new(150.0, 90.0),
            curve: Some(Point::new(120.0, 10.0)),
            color: Color::RED,
            width: 5.0,
            style: ArrowStyle::Line,
            heads: ArrowHeads::Both,
            dashed: false,
        }),
        Shape::Arrow(ArrowShape {
            start: Point::new(30.0, 150.0),
            end: Point::new(170.0, 120.0),
            curve: None,
            color: Color::RED,
            width: 6.0,
            style: ArrowStyle::Tapered,
            heads: ArrowHeads::End,
            dashed: false,
        }),
        Shape::Rect(RectShape {
            rect: Rect::new(40.5, 40.5, 80.0, 50.0),
            stroke: stroke(Color::RED, 4.0),
            fill: None,
            corner_radius: 8.0,
        }),
        Shape::Ellipse(EllipseShape {
            rect: Rect::new(60.0, 20.0, 70.0, 40.0),
            stroke: stroke(Color::BLACK, 3.0),
            fill: Some(Color::YELLOW),
        }),
        Shape::Text(TextShape {
            origin: Point::new(10.0, 100.0),
            text: "Hello\nWorld gjpq".into(),
            style: TextStyle { italic: true, underline: true, ..style.clone() },
            max_width: None,
        }),
        Shape::Callout(CalloutShape {
            rect: Rect::new(80.0, 100.0, 100.0, 50.0),
            tail: Some(Point::new(40.0, 190.0)),
            text: "Click here to continue".into(),
            style: TextStyle { outline: None, color: Color::BLACK, ..style.clone() },
            fill: Color::YELLOW,
            border: stroke(Color::BLACK, 2.0),
            corner_radius: 8.0,
            padding: 8.0,
        }),
        Shape::Step(StepShape {
            center: Point::new(100.0, 100.0),
            radius: 16.0,
            number: 12,
            fill: Color::RED,
            text_color: Color::WHITE,
            border: stroke(Color::WHITE, 2.0),
            connector: Some(Connector {
                target: Point::new(180.0, 40.0),
                style: ConnectorStyle::Arrow,
                width: 3.0,
            }),
        }),
        Shape::Highlight(HighlightShape { rect: Rect::new(10.0, 10.0, 60.0, 20.0), color: Color::YELLOW }),
        Shape::Polygon(PolygonShape {
            points: vec![Point::new(10.0, 10.0), Point::new(80.0, 30.0), Point::new(40.0, 90.0)],
            closed: true,
            stroke: stroke(Color::RED, 3.0),
            fill: Some(Color::rgba(0, 0, 255, 80)),
        }),
        Shape::Freehand(FreehandShape {
            points: (0..30).map(|i| Point::new(10.0 + i as f32 * 5.0, 50.0 + (i as f32 * 0.5).sin() * 20.0)).collect(),
            color: Color::RED,
            width: 4.0,
        }),
        Shape::Blur(BlurShape { rect: Rect::new(20.0, 20.0, 60.0, 40.0), mode: BlurMode::Gaussian, strength: 6.0 }),
        Shape::Blur(BlurShape { rect: Rect::new(20.0, 70.0, 60.0, 40.0), mode: BlurMode::Pixelate, strength: 4.0 }),
        Shape::Stamp(StampShape { rect: Rect::new(30.0, 30.0, 40.0, 40.0), resource: "dot".into() }),
        Shape::Magnifier(MagnifierShape {
            source: Point::new(40.0, 40.0),
            glass: Point::new(130.0, 120.0),
            radius: 40.0,
            zoom: 2.0,
            border: Stroke { color: Color::RED, width: 3.0 },
            show_source: true,
        }),
        Shape::Brace(BraceShape {
            start: Point::new(150.0, 20.0),
            end: Point::new(150.0, 180.0),
            depth: 18.0,
            color: Color::RED,
            width: 3.0,
        }),
    ]
}

fn doc_with_all() -> Document {
    let mut doc = Document::new(checker(200, 200));
    let dot = RgbaImage::from_fn(16, 16, |x, y| {
        let d = ((x as f32 - 7.5).powi(2) + (y as f32 - 7.5).powi(2)).sqrt();
        if d < 7.0 { Rgba([0, 160, 0, 255]) } else { Rgba([0, 0, 0, 0]) }
    });
    doc.resources.insert("dot".into(), std::sync::Arc::new(dot));
    for s in sample_shapes() {
        doc.push(s);
    }
    doc
}

#[test]
fn bounds_contain_all_sprite_pixels() {
    let mut doc = doc_with_all();
    // Exercise shadows on half of the objects.
    for (i, o) in doc.objects.iter_mut().enumerate() {
        o.shadow = if i % 2 == 0 { 1.0 } else { 0.0 };
    }
    let mut r = Renderer::new();
    for obj in &doc.objects {
        let b = r.bounds(&doc, obj);
        let sprite = r.render_object(&doc, obj, None).expect("visible");
        let mut any = false;
        for (x, y, p) in sprite.image.enumerate_pixels() {
            if p.0[3] == 0 {
                continue;
            }
            any = true;
            let (px, py) = (sprite.origin.0 as f32 + x as f32, sprite.origin.1 as f32 + y as f32);
            assert!(
                px + 1.0 >= b.x && px <= b.right() && py + 1.0 >= b.y && py <= b.bottom(),
                "pixel ({px},{py}) outside bounds {b:?} for {:?}",
                obj.shape
            );
        }
        assert!(any, "no visible pixels for {:?}", obj.shape);
        // Visible ink must not touch the sprite border (the sprite would clip it), except
        // for shapes that fill their rect exactly.
        let fills_rect = matches!(obj.shape, Shape::Highlight(_) | Shape::Blur(_) | Shape::Stamp(_));
        if fills_rect && obj.shadow == 0.0 {
            continue;
        }
        let (w, h) = sprite.image.dimensions();
        for (x, y, p) in sprite.image.enumerate_pixels() {
            if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                assert!(p.0[3] < 40, "ink at sprite edge ({x},{y}) a={} for {:?}", p.0[3], obj.shape);
            }
        }
    }
}

#[test]
fn flatten_size_matches_canvas() {
    let mut doc = doc_with_all();
    let mut r = Renderer::new();
    assert_eq!(r.flatten(&doc).dimensions(), (200, 200));
    doc.canvas = Canvas { rect: IRect::new(-20, -10, 260, 230), background: Color::WHITE };
    let img = r.flatten(&doc);
    assert_eq!(img.dimensions(), (260, 230));
    // Padding is background, base starts at (20, 10).
    assert_eq!(img.get_pixel(2, 2).0, [255, 255, 255, 255]);
    doc.canvas = Canvas { rect: IRect::new(50, 50, 30, 20), background: Color::TRANSPARENT };
    assert_eq!(r.flatten(&doc).dimensions(), (30, 20));
}

#[test]
fn flatten_matches_composited_sprites() {
    let doc = doc_with_all();
    let mut r = Renderer::new();
    let flat = r.flatten(&doc);
    // Re-composite by hand through the public API (straight alpha, CPU "over").
    let mut acc = r.composite(&doc, 0);
    for (i, obj) in doc.objects.iter().enumerate() {
        let backdrop = r.composite(&doc, i);
        if let Some(s) = r.render_object(&doc, obj, Some(&backdrop)) {
            over(&mut acc, &s);
        }
    }
    let mut max_diff = 0;
    for (a, b) in flat.pixels().zip(acc.image.pixels()) {
        for c in 0..4 {
            max_diff = max_diff.max((a.0[c] as i32 - b.0[c] as i32).abs());
        }
    }
    assert!(max_diff <= 4, "max channel diff {max_diff}");
}

fn over(dst: &mut Layer, src: &Layer) {
    for (x, y, p) in src.image.enumerate_pixels() {
        let dx = src.origin.0 + x as i32 - dst.origin.0;
        let dy = src.origin.1 + y as i32 - dst.origin.1;
        if dx < 0 || dy < 0 || dx >= dst.image.width() as i32 || dy >= dst.image.height() as i32 {
            continue;
        }
        let d = dst.image.get_pixel_mut(dx as u32, dy as u32);
        let sa = p.0[3] as f32 / 255.0;
        let da = d.0[3] as f32 / 255.0;
        let oa = sa + da * (1.0 - sa);
        if oa <= 0.0 {
            continue;
        }
        for c in 0..3 {
            let v = (p.0[c] as f32 * sa + d.0[c] as f32 * da * (1.0 - sa)) / oa;
            d.0[c] = v.round() as u8;
        }
        d.0[3] = (oa * 255.0).round() as u8;
    }
}

#[test]
fn text_metrics_caret_count() {
    let mut r = Renderer::new();
    let style = TextStyle::default();
    for t in ["", "a", "Hello world", "two\nlines", "trail\n", "crlf\r\nx", "ümlaut ✓ 😀", "a b c d e f g h"] {
        let m = r.text_metrics(t, &style, Some(40.0));
        assert_eq!(m.carets.len(), t.chars().count() + 1, "{t:?}");
        assert!(m.line_height > 0.0 && m.height >= m.line_height);
    }
    let m = r.text_metrics("ab\ncd", &style, None);
    // Carets advance along a line and the second line is lower.
    assert!(m.carets[1].x > m.carets[0].x);
    assert!(m.carets[3].y > m.carets[0].y);
    assert_eq!(m.carets[3].x, 0.0);
    let trail = r.text_metrics("ab\n", &style, None);
    assert!(trail.carets[3].y > trail.carets[0].y);
}

#[test]
fn highlight_multiplies() {
    let mut base = RgbaImage::from_pixel(40, 20, Rgba([255, 255, 255, 255]));
    for x in 20..40 {
        for y in 0..20 {
            base.put_pixel(x, y, Rgba([100, 100, 100, 255]));
        }
    }
    let mut doc = Document::new(base);
    doc.push(Shape::Highlight(HighlightShape { rect: Rect::new(0.0, 0.0, 40.0, 20.0), color: Color::rgb(255, 225, 0) }));
    let img = Renderer::new().flatten(&doc);
    assert_eq!(img.get_pixel(5, 5).0, [255, 225, 0, 255]);
    let p = img.get_pixel(30, 5).0;
    assert_eq!(p[3], 255);
    assert!((p[0] as i32 - 100).abs() <= 1 && (p[1] as i32 - 88).abs() <= 1 && p[2] <= 1, "{p:?}");
}

#[test]
fn blur_changes_pixels_inside_only() {
    for mode in [BlurMode::Gaussian, BlurMode::Pixelate] {
        let mut doc = Document::new(checker(100, 100));
        doc.push(Shape::Blur(BlurShape { rect: Rect::new(20.0, 20.0, 40.0, 40.0), mode, strength: 5.0 }));
        let img = Renderer::new().flatten(&doc);
        let orig = checker(100, 100);
        let changed = (20..60).flat_map(|y| (20..60).map(move |x| (x, y))).filter(|&(x, y)| img.get_pixel(x, y) != orig.get_pixel(x, y)).count();
        assert!(changed > 800, "{mode:?}: only {changed} pixels changed");
        for (x, y) in [(5, 5), (80, 80), (10, 50), (70, 30)] {
            assert_eq!(img.get_pixel(x, y), orig.get_pixel(x, y), "{mode:?} changed outside at {x},{y}");
        }
    }
}

#[test]
fn opacity_and_invisible() {
    let mut doc = Document::new(checker(50, 50));
    let id = doc.push(Shape::Rect(RectShape {
        rect: Rect::new(5.0, 5.0, 20.0, 20.0),
        stroke: None,
        fill: Some(Color::RED),
        corner_radius: 0.0,
    }));
    let mut r = Renderer::new();
    let full = r.render_object(&doc, doc.get(id).unwrap(), None).unwrap();
    doc.get_mut(id).unwrap().opacity = 0.5;
    let half = r.render_object(&doc, doc.get(id).unwrap(), None).unwrap();
    let a_full = full.image.get_pixel(10, 10).0[3];
    let a_half = half.image.get_pixel(10, 10).0[3];
    assert_eq!(a_full, 255);
    assert!((a_half as i32 - 128).abs() <= 2);
    doc.get_mut(id).unwrap().opacity = 0.0;
    assert!(r.render_object(&doc, doc.get(id).unwrap(), None).is_none());
}

#[test]
fn renderer_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Renderer>();
}

#[test]
fn emoji_and_empty_text_render() {
    let mut doc = Document::new(checker(120, 60));
    let id = doc.push(Shape::Text(TextShape {
        origin: Point::new(4.0, 4.0),
        text: "ok 😀 ✓".into(),
        style: TextStyle::default(),
        max_width: None,
    }));
    let empty = doc.push(Shape::Text(TextShape {
        origin: Point::new(4.0, 4.0),
        text: String::new(),
        style: TextStyle::default(),
        max_width: None,
    }));
    let mut r = Renderer::new();
    let obj = doc.get(id).unwrap();
    let b = r.bounds(&doc, obj);
    let s = r.render_object(&doc, obj, None).expect("text visible");
    assert!(s.origin.0 as f32 >= b.x.floor() && s.image.width() as f32 <= b.w.ceil() + 1.0);
    assert!(r.render_object(&doc, doc.get(empty).unwrap(), None).is_none());
    let _ = r.flatten(&doc);
}
