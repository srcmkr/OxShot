//! Renders every annotation shape in several variants onto a synthetic screenshot and
//! measures renderer performance.
//!
//! `cargo run -p oxshot-core --example gallery --release [-- out.png]`

use std::sync::Arc;
use std::time::Instant;

use image::{Rgba, RgbaImage};
use oxshot_core::render::Renderer;
use oxshot_core::*;

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}

fn fill(img: &mut RgbaImage, x0: u32, y0: u32, w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) {
    for y in y0..(y0 + h).min(img.height()) {
        for x in x0..(x0 + w).min(img.width()) {
            let c = f(x - x0, y - y0);
            img.put_pixel(x, y, Rgba([c[0], c[1], c[2], 255]));
        }
    }
}

fn plain(size: f32, color: Color, bold: bool) -> TextStyle {
    TextStyle { size, color, bold, outline: None, ..TextStyle::default() }
}

/// A fake application window: title bar, sidebar, toolbar, text and a photo-like panel.
fn fake_ui(r: &mut Renderer, w: u32, h: u32) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(w, h, Rgba([246, 247, 249, 255]));
    fill(&mut img, 0, 0, w, 44, |x, _| {
        let t = x as f32 / w as f32;
        [lerp(32, 70, t), lerp(56, 40, t), lerp(110, 140, t)]
    });
    fill(&mut img, 0, 44, 210, h - 44, |_, y| {
        let v = lerp(236, 226, y as f32 / h as f32);
        [v, v, lerp(v, 240, 0.3)]
    });
    fill(&mut img, 210, 44, w - 210, 40, |_, _| [255, 255, 255]);
    fill(&mut img, 210, 84, w - 210, 1, |_, _| [220, 222, 228]);
    // Photo-like panel with gradients and stripes.
    fill(&mut img, 640, 120, 320, 220, |x, y| {
        let (fx, fy) = (x as f32 / 320.0, y as f32 / 220.0);
        let ring = (((fx - 0.6).powi(2) + (fy - 0.4).powi(2)).sqrt() * 30.0).sin() * 0.5 + 0.5;
        [lerp(255, 40, fy), lerp(120, 200, fx * ring), lerp(60, 230, fx)]
    });
    let mut doc = Document::new(img);
    let text = |doc: &mut Document, x: f32, y: f32, t: &str, style: TextStyle| {
        doc.push(Shape::Text(TextShape { origin: Point::new(x, y), text: t.into(), style, max_width: None }));
    };
    text(&mut doc, 16.0, 12.0, "OxShot Demo — Settings", plain(15.0, Color::WHITE, true));
    for (i, item) in ["General", "Capture", "Editor", "Hotkeys", "Upload", "Advanced"].iter().enumerate() {
        text(&mut doc, 24.0, 70.0 + i as f32 * 34.0, item, plain(14.0, Color::rgb(50, 55, 65), i == 2));
    }
    for (i, b) in ["New", "Open", "Save", "Export"].iter().enumerate() {
        let x = 226.0 + i as f32 * 82.0;
        doc.push(Shape::Rect(RectShape {
            rect: Rect::new(x, 52.0, 72.0, 24.0),
            stroke: Some(Stroke { color: Color::rgb(200, 204, 212), width: 1.0 }),
            fill: Some(Color::rgb(248, 249, 251)),
            corner_radius: 4.0,
        }));
        text(&mut doc, x + 14.0, 56.0, b, plain(13.0, Color::rgb(40, 44, 52), false));
    }
    let para = [
        "Screenshots are saved to ~/Pictures/OxShot automatically.",
        "Contact: jane.doe@example.com for licensing questions.",
        "The editor keeps every annotation as an editable object.",
        "Press Ctrl+Shift+S to capture a region of the screen.",
        "Use the magnifier to point out tiny details like this: 42px.",
        "Blur sensitive data such as API keys: sk-9f8a7b6c5d4e3f2a1b.",
        "Highlight important sentences so readers find them quickly.",
        "Numbered steps make tutorials easy to follow.",
    ];
    for (i, line) in para.iter().enumerate() {
        text(&mut doc, 236.0, 110.0 + i as f32 * 30.0, line, plain(14.0, Color::rgb(45, 48, 56), false));
    }
    text(&mut doc, 236.0, 380.0, "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor.", plain(13.0, Color::rgb(90, 94, 104), false));
    for i in 0..6 {
        let y = 410.0 + i as f32 * 44.0;
        doc.push(Shape::Rect(RectShape {
            rect: Rect::new(236.0, y, 720.0, 36.0),
            stroke: Some(Stroke { color: Color::rgb(226, 228, 234), width: 1.0 }),
            fill: Some(Color::WHITE),
            corner_radius: 6.0,
        }));
        text(&mut doc, 250.0, y + 9.0, &format!("Row {} · Item description text", i + 1), plain(13.0, Color::rgb(60, 64, 72), false));
    }
    r.flatten(&doc)
}

/// A mouse-pointer stamp image (rendered with the renderer itself).
fn cursor_image(r: &mut Renderer) -> RgbaImage {
    let mut doc = Document::new(RgbaImage::new(48, 72));
    let pts = [(4.0, 4.0), (4.0, 56.0), (16.0, 45.0), (24.0, 64.0), (32.0, 60.0), (24.0, 42.0), (40.0, 42.0)];
    doc.push(Shape::Polygon(PolygonShape {
        points: pts.iter().map(|&(x, y)| Point::new(x, y)).collect(),
        closed: true,
        stroke: Some(Stroke { color: Color::BLACK, width: 3.0 }),
        fill: Some(Color::WHITE),
    }));
    r.flatten(&doc)
}

fn arrow(s: (f32, f32), e: (f32, f32), c: Option<(f32, f32)>, w: f32, style: ArrowStyle, heads: ArrowHeads, dashed: bool) -> Shape {
    Shape::Arrow(ArrowShape {
        start: Point::new(s.0, s.1),
        end: Point::new(e.0, e.1),
        curve: c.map(|c| Point::new(c.0, c.1)),
        color: Color::RED,
        width: w,
        style,
        heads,
        dashed,
    })
}

fn build_gallery(r: &mut Renderer) -> Document {
    let base = fake_ui(r, 1000, 700);
    let mut doc = Document::new(base);
    doc.resources.insert("cursor".into(), Arc::new(cursor_image(r)));
    let red = Color::RED;
    let push = |doc: &mut Document, s: Shape, shadow: f32| {
        let id = doc.push(s);
        doc.get_mut(id).unwrap().shadow = shadow;
    };
    // Arrows (sidebar + header area).
    push(&mut doc, arrow((20.0, 300.0), (190.0, 300.0), None, 2.0, ArrowStyle::Line, ArrowHeads::End, false), 0.0);
    push(&mut doc, arrow((20.0, 330.0), (190.0, 330.0), None, 5.0, ArrowStyle::Line, ArrowHeads::End, false), 1.0);
    push(&mut doc, arrow((20.0, 360.0), (190.0, 360.0), None, 4.0, ArrowStyle::Line, ArrowHeads::Both, false), 0.0);
    push(&mut doc, arrow((20.0, 390.0), (190.0, 390.0), None, 3.0, ArrowStyle::Line, ArrowHeads::None, true), 0.0);
    push(&mut doc, arrow((20.0, 430.0), (190.0, 430.0), None, 8.0, ArrowStyle::Tapered, ArrowHeads::End, false), 1.0);
    push(&mut doc, arrow((20.0, 470.0), (190.0, 470.0), None, 5.0, ArrowStyle::Tapered, ArrowHeads::Both, false), 0.0);
    push(&mut doc, arrow((20.0, 560.0), (180.0, 520.0), Some((60.0, 480.0)), 5.0, ArrowStyle::Line, ArrowHeads::End, false), 0.6);
    push(&mut doc, arrow((20.0, 680.0), (190.0, 600.0), Some((140.0, 690.0)), 9.0, ArrowStyle::Tapered, ArrowHeads::End, false), 1.0);
    push(&mut doc, arrow((300.0, 660.0), (420.0, 560.0), Some((300.0, 580.0)), 3.0, ArrowStyle::Line, ArrowHeads::Both, true), 0.0);

    // Rectangles and ellipses.
    push(&mut doc, Shape::Rect(RectShape { rect: Rect::new(232.0, 140.0, 330.0, 26.0), stroke: Some(Stroke { color: red, width: 3.0 }), fill: None, corner_radius: 0.0 }), 0.0);
    push(&mut doc, Shape::Rect(RectShape { rect: Rect::new(222.0, 48.0, 336.0, 32.0), stroke: Some(Stroke { color: red, width: 4.0 }), fill: None, corner_radius: 10.0 }), 1.0);
    push(&mut doc, Shape::Rect(RectShape { rect: Rect::new(600.0, 600.0, 120.0, 70.0), stroke: Some(Stroke { color: Color::rgb(20, 90, 200), width: 3.0 }), fill: Some(Color::rgba(20, 90, 200, 60)), corner_radius: 14.0 }), 0.0);
    push(&mut doc, Shape::Ellipse(EllipseShape { rect: Rect::new(14.0, 130.0, 110.0, 44.0), stroke: Some(Stroke { color: red, width: 3.0 }), fill: None }), 0.0);
    push(&mut doc, Shape::Ellipse(EllipseShape { rect: Rect::new(740.0, 600.0, 70.0, 70.0), stroke: Some(Stroke { color: Color::WHITE, width: 4.0 }), fill: Some(Color::rgb(30, 160, 90)) }), 1.0);

    // Highlights.
    push(&mut doc, Shape::Highlight(HighlightShape { rect: Rect::new(232.0, 286.0, 420.0, 24.0), color: Color::YELLOW }), 0.0);
    push(&mut doc, Shape::Highlight(HighlightShape { rect: Rect::new(232.0, 196.0, 200.0, 24.0), color: Color::rgb(140, 255, 140) }), 0.0);

    // Blur / pixelate.
    push(&mut doc, Shape::Blur(BlurShape { rect: Rect::new(296.0, 143.0, 165.0, 20.0), mode: BlurMode::Gaussian, strength: 5.0 }), 0.0);
    push(&mut doc, Shape::Blur(BlurShape { rect: Rect::new(470.0, 254.0, 160.0, 26.0), mode: BlurMode::Pixelate, strength: 3.0 }), 0.0);

    // Text.
    push(&mut doc, Shape::Text(TextShape { origin: Point::new(660.0, 360.0), text: "Bold red text\nwith white halo".into(), style: TextStyle { size: 24.0, ..TextStyle::default() }, max_width: None }), 0.0);
    push(&mut doc, Shape::Text(TextShape { origin: Point::new(660.0, 140.0), text: "Over a photo".into(), style: TextStyle { size: 30.0, color: Color::rgb(20, 20, 20), outline: Some(Stroke { color: Color::WHITE, width: 4.0 }), ..TextStyle::default() }, max_width: None }), 1.0);
    push(&mut doc, Shape::Text(TextShape { origin: Point::new(820.0, 200.0), text: "Italic, centered and underlined".into(), style: TextStyle { size: 18.0, italic: true, underline: true, align: Align::Center, color: Color::WHITE, outline: Some(Stroke { color: Color::rgb(0, 0, 0), width: 2.0 }), ..TextStyle::default() }, max_width: Some(130.0) }), 0.0);

    // Callouts.
    push(&mut doc, Shape::Callout(CalloutShape { rect: Rect::new(560.0, 40.0, 200.0, 56.0), tail: Some(Point::new(500.0, 130.0)), text: "Speech bubble with a smooth pointer".into(), style: TextStyle { size: 15.0, color: Color::BLACK, outline: None, ..TextStyle::default() }, fill: Color::YELLOW, border: Some(Stroke { color: Color::rgb(60, 50, 0), width: 2.0 }), corner_radius: 10.0, padding: 9.0 }), 1.0);
    push(&mut doc, Shape::Callout(CalloutShape { rect: Rect::new(780.0, 420.0, 170.0, 60.0), tail: Some(Point::new(700.0, 520.0)), text: "Red callout, white text, no border".into(), style: TextStyle { size: 15.0, color: Color::WHITE, outline: None, ..TextStyle::default() }, fill: red, border: None, corner_radius: 6.0, padding: 10.0 }), 1.0);
    push(&mut doc, Shape::Callout(CalloutShape { rect: Rect::new(450.0, 590.0, 130.0, 60.0), tail: Some(Point::new(600.0, 560.0)), text: "Right-side tail".into(), style: TextStyle { size: 14.0, bold: false, color: Color::BLACK, outline: None, align: Align::Center, ..TextStyle::default() }, fill: Color::WHITE, border: Some(Stroke { color: red, width: 3.0 }), corner_radius: 12.0, padding: 8.0 }), 0.0);
    push(&mut doc, Shape::Callout(CalloutShape { rect: Rect::new(830.0, 520.0, 150.0, 40.0), tail: None, text: "Plain text box".into(), style: TextStyle { size: 15.0, color: Color::BLACK, outline: None, ..TextStyle::default() }, fill: Color::rgb(230, 240, 255), border: Some(Stroke { color: Color::rgb(20, 90, 200), width: 2.0 }), corner_radius: 4.0, padding: 10.0 }), 0.0);

    // Steps.
    let step = |n: u32, c: (f32, f32), r: f32, conn: Option<(f32, f32, ConnectorStyle)>, fill: Color| {
        Shape::Step(StepShape {
            center: Point::new(c.0, c.1),
            radius: r,
            number: n,
            fill,
            text_color: Color::WHITE,
            border: Some(Stroke { color: Color::WHITE, width: 2.0 }),
            connector: conn.map(|(x, y, style)| Connector { target: Point::new(x, y), style, width: 3.0 }),
        })
    };
    push(&mut doc, step(1, (600.0, 160.0), 15.0, None, red), 1.0);
    push(&mut doc, step(2, (240.0, 520.0), 15.0, Some((330.0, 470.0, ConnectorStyle::Arrow)), red), 1.0);
    push(&mut doc, step(12, (520.0, 520.0), 18.0, Some((440.0, 430.0, ConnectorStyle::Line)), Color::rgb(20, 90, 200)), 0.0);
    push(&mut doc, step(123, (580.0, 470.0), 20.0, None, Color::rgb(30, 140, 70)), 0.0);

    // Polygon and freehand.
    push(&mut doc, Shape::Polygon(PolygonShape { points: vec![Point::new(860.0, 590.0), Point::new(980.0, 600.0), Point::new(960.0, 680.0), Point::new(880.0, 660.0)], closed: true, stroke: Some(Stroke { color: red, width: 3.0 }), fill: Some(Color::rgba(255, 220, 0, 110)) }), 0.0);
    push(&mut doc, Shape::Polygon(PolygonShape { points: vec![Point::new(440.0, 660.0), Point::new(480.0, 690.0), Point::new(520.0, 660.0), Point::new(560.0, 690.0)], closed: false, stroke: Some(Stroke { color: Color::rgb(20, 90, 200), width: 4.0 }), fill: None }), 0.0);
    let squiggle: Vec<Point> = (0..60)
        .map(|i| {
            let t = i as f32 / 59.0;
            Point::new(240.0 + t * 380.0, 357.0 + (t * 26.0).sin() * 5.0 + (t * 7.0).cos() * 2.0)
        })
        .collect();
    push(&mut doc, Shape::Freehand(FreehandShape { points: squiggle, color: red, width: 3.0 }), 0.0);

    // Stamp.
    push(&mut doc, Shape::Stamp(StampShape { rect: Rect::new(405.0, 64.0, 24.0, 36.0), resource: "cursor".into() }), 1.0);
    push(&mut doc, Shape::Stamp(StampShape { rect: Rect::new(150.0, 196.0, 48.0, 72.0), resource: "cursor".into() }), 0.0);

    // Magnifier on "42px".
    push(&mut doc, Shape::Magnifier(MagnifierShape { source: Point::new(592.0, 239.0), glass: Point::new(900.0, 290.0), radius: 62.0, zoom: 2.5, border: Stroke { color: red, width: 3.0 }, show_source: true }), 1.0);

    // Braces.
    push(&mut doc, Shape::Brace(BraceShape { start: Point::new(224.0, 106.0), end: Point::new(224.0, 342.0), depth: -14.0, color: red, width: 3.0 }), 0.0);
    push(&mut doc, Shape::Brace(BraceShape { start: Point::new(236.0, 404.0), end: Point::new(560.0, 404.0), depth: -18.0, color: Color::rgb(20, 90, 200), width: 2.5 }), 0.0);
    push(&mut doc, Shape::Brace(BraceShape { start: Point::new(965.0, 410.0), end: Point::new(965.0, 500.0), depth: 20.0, color: red, width: 4.0 }), 0.0);
    doc
}

/// A 2560x1440 document with ~20 typical objects.
fn perf_doc(r: &mut Renderer) -> Document {
    let small = fake_ui(r, 1280, 720);
    let base = image::imageops::resize(&small, 2560, 1440, image::imageops::FilterType::Triangle);
    let mut doc = Document::new(base);
    for i in 0..20 {
        let x = 100.0 + (i % 5) as f32 * 480.0;
        let y = 100.0 + (i / 5) as f32 * 320.0;
        let shape = match i % 10 {
            0 => arrow((x, y), (x + 300.0, y + 150.0), None, 6.0, ArrowStyle::Line, ArrowHeads::End, false),
            1 => arrow((x, y), (x + 300.0, y + 150.0), Some((x + 250.0, y)), 9.0, ArrowStyle::Tapered, ArrowHeads::End, false),
            2 => Shape::Rect(RectShape { rect: Rect::new(x, y, 300.0, 180.0), stroke: Some(Stroke { color: Color::RED, width: 4.0 }), fill: None, corner_radius: 8.0 }),
            3 => Shape::Text(TextShape { origin: Point::new(x, y), text: "Annotated text\nsecond line".into(), style: TextStyle { size: 32.0, ..TextStyle::default() }, max_width: None }),
            4 => Shape::Callout(CalloutShape { rect: Rect::new(x, y, 280.0, 90.0), tail: Some(Point::new(x - 40.0, y + 200.0)), text: "Callout with some wrapped text inside".into(), style: TextStyle { size: 22.0, color: Color::BLACK, outline: None, ..TextStyle::default() }, fill: Color::YELLOW, border: Some(Stroke { color: Color::BLACK, width: 2.0 }), corner_radius: 10.0, padding: 10.0 }),
            5 => Shape::Step(StepShape { center: Point::new(x, y), radius: 24.0, number: i, fill: Color::RED, text_color: Color::WHITE, border: Some(Stroke { color: Color::WHITE, width: 3.0 }), connector: Some(Connector { target: Point::new(x + 200.0, y + 100.0), style: ConnectorStyle::Arrow, width: 4.0 }) }),
            6 => Shape::Highlight(HighlightShape { rect: Rect::new(x, y, 400.0, 40.0), color: Color::YELLOW }),
            7 => Shape::Blur(BlurShape { rect: Rect::new(x, y, 300.0, 60.0), mode: BlurMode::Gaussian, strength: 10.0 }),
            8 => Shape::Magnifier(MagnifierShape { source: Point::new(x, y), glass: Point::new(x + 250.0, y + 120.0), radius: 90.0, zoom: 2.0, border: Stroke { color: Color::RED, width: 4.0 }, show_source: true }),
            _ => Shape::Brace(BraceShape { start: Point::new(x, y), end: Point::new(x, y + 250.0), depth: 24.0, color: Color::RED, width: 4.0 }),
        };
        let id = doc.push(shape);
        doc.get_mut(id).unwrap().shadow = if i % 3 == 0 { 1.0 } else { 0.0 };
    }
    doc
}

fn time<T>(label: &str, runs: u32, mut f: impl FnMut() -> T) -> T {
    let mut out = f(); // warm-up (font loading, caches)
    let t = Instant::now();
    for _ in 0..runs {
        out = f();
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0 / runs as f64;
    println!("{label:<40} {ms:8.2} ms");
    out
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "gallery.png".into());
    let mut r = Renderer::new();
    let t = Instant::now();
    let doc = build_gallery(&mut r);
    println!("build gallery (incl. font load)          {:8.2} ms", t.elapsed().as_secs_f64() * 1000.0);
    let img = time("flatten gallery 1000x700", 5, || r.flatten(&doc));
    img.save(&out).expect("write png");
    println!("wrote {out}");

    let pdoc = perf_doc(&mut r);
    time("flatten 2560x1440, 20 objects", 10, || r.flatten(&pdoc));
    let backdrop = r.composite(&pdoc, 0);
    for (i, label) in [(0, "arrow"), (1, "tapered arrow"), (2, "rect"), (3, "text (cached layout)"), (4, "callout"), (5, "step+connector"), (6, "highlight"), (7, "blur s=10"), (8, "magnifier"), (9, "brace")] {
        let obj = &pdoc.objects[i];
        time(&format!("render_object {label}"), 20, || r.render_object(&pdoc, obj, Some(&backdrop)));
    }
    let style = TextStyle::default();
    let mut n = 0;
    time("text_metrics (uncached)", 20, || {
        n += 1;
        r.text_metrics(&format!("Some editable text {n}\nwith two lines"), &style, Some(300.0))
    });
}
