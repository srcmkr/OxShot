//! Writes before/after PNGs for the document operations and times `apply_border`.
//!
//! `cargo run -p oxshot-core --example ops [-- out_dir]`

use std::path::{Path, PathBuf};
use std::time::Instant;

use image::{Rgba, RgbaImage};
use oxshot_core::ops::{self, BorderEffect, CutEdge, TornEdges};
use oxshot_core::render::Renderer;
use oxshot_core::*;

/// A long, list-like screenshot with a gradient header.
fn screenshot(r: &mut Renderer, w: u32, h: u32) -> RgbaImage {
    let mut img = RgbaImage::from_fn(w, h, |x, y| {
        if y < 50 {
            let t = x as f32 / w as f32;
            Rgba([(40.0 + 60.0 * t) as u8, 70, (150.0 - 30.0 * t) as u8, 255])
        } else if y % 48 < 2 {
            Rgba([226, 228, 234, 255])
        } else {
            Rgba([250, 250, 252, 255])
        }
    });
    for y in 0..h {
        for x in 0..w.min(6) {
            img.put_pixel(x, y, Rgba([90, 140, 230, 255]));
        }
    }
    let mut doc = Document::new(img);
    let style = TextStyle { size: 15.0, color: Color::rgb(45, 48, 56), outline: None, bold: false, ..TextStyle::default() };
    doc.push(Shape::Text(TextShape {
        origin: Point::new(16.0, 15.0),
        text: "Inbox — 128 messages".into(),
        style: TextStyle { color: Color::WHITE, bold: true, ..style.clone() },
        max_width: None,
    }));
    for i in 0..((h - 50) / 48) {
        doc.push(Shape::Text(TextShape {
            origin: Point::new(20.0, 64.0 + i as f32 * 48.0),
            text: format!("Message {:>3} · Subject line of the email number {}", i + 1, i + 1),
            style: style.clone(),
            max_width: None,
        }));
    }
    r.flatten(&doc)
}

/// Composites onto a light checkerboard so transparency is visible in the PNG.
fn on_checker(img: &RgbaImage) -> RgbaImage {
    RgbaImage::from_fn(img.width(), img.height(), |x, y| {
        let bg = if (x / 10 + y / 10) % 2 == 0 { 236.0 } else { 214.0 };
        let p = img.get_pixel(x, y).0;
        let a = p[3] as f32 / 255.0;
        let c = |i: usize| (p[i] as f32 * a + bg * (1.0 - a)).round() as u8;
        Rgba([c(0), c(1), c(2), 255])
    })
}

fn save(dir: &Path, name: &str, img: &RgbaImage) {
    let path = dir.join(format!("ops_{name}.png"));
    on_checker(img).save(&path).expect("write png");
    println!("wrote {}", path.display());
}

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let mut r = Renderer::new();
    let mut doc = Document::new(screenshot(&mut r, 520, 640));
    let id = doc.push(Shape::Arrow(ArrowShape {
        start: Point::new(420.0, 90.0),
        end: Point::new(360.0, 560.0),
        curve: None,
        color: Color::RED,
        width: 5.0,
        style: ArrowStyle::Line,
        heads: ArrowHeads::End,
        dashed: false,
    }));
    doc.get_mut(id).unwrap().shadow = 1.0;
    doc.push(Shape::Rect(RectShape {
        rect: Rect::new(14.0, 530.0, 380.0, 44.0),
        stroke: Some(Stroke { color: Color::RED, width: 3.0 }),
        fill: None,
        corner_radius: 6.0,
    }));
    save(&dir, "before", &r.flatten(&doc));

    for (edge, name) in [(CutEdge::Straight, "straight"), (CutEdge::Zigzag, "zigzag"), (CutEdge::Wave, "wave")] {
        let cut = ops::cut_out(&doc, false, 200.0, 420.0, edge);
        save(&dir, &format!("cut_{name}"), &r.flatten(&cut));
    }
    let vcut = ops::cut_out(&doc, true, 150.0, 300.0, CutEdge::Wave);
    save(&dir, "cut_vertical", &r.flatten(&vcut));
    save(&dir, "resize", &r.flatten(&ops::resize(&doc, 0.6, 0.6, true)));

    let small = ops::cut_out(&doc, false, 200.0, 420.0, CutEdge::Zigzag);
    let effects = [
        ("border_rounded", BorderEffect { corner_radius: 14.0, border_width: 2.0, border_color: Color::rgb(80, 80, 90), shadow: 1.0, ..Default::default() }),
        ("border_torn", BorderEffect { torn: TornEdges { bottom: true, right: true, ..Default::default() }, shadow: 0.8, ..Default::default() }),
        ("border_reflection", BorderEffect { corner_radius: 8.0, reflection: 1.0, ..Default::default() }),
        ("border_rotated", BorderEffect { rotation_deg: -6.0, shadow: 1.0, border_width: 6.0, border_color: Color::WHITE, ..Default::default() }),
        ("border_gray_bg", BorderEffect { grayscale: true, corner_radius: 10.0, shadow: 0.6, background: Color::rgb(60, 110, 200), ..Default::default() }),
    ];
    for (name, fx) in &effects {
        save(&dir, name, &ops::apply_border(&small, fx).base);
    }

    // Performance: all effects on a 1920x1080 image.
    let big = RgbaImage::from_fn(1920, 1080, |x, y| Rgba([(x % 256) as u8, (y % 256) as u8, 160, 255]));
    let fx = BorderEffect {
        corner_radius: 16.0,
        border_width: 3.0,
        torn: TornEdges { bottom: true, ..Default::default() },
        shadow: 1.0,
        reflection: 0.8,
        grayscale: true,
        rotation_deg: 5.0,
        ..Default::default()
    };
    let t = Instant::now();
    let out = ops::preview_border(&big, &fx);
    println!("preview_border 1920x1080 all effects: {:.1} ms -> {:?}", t.elapsed().as_secs_f64() * 1000.0, out.dimensions());
    let t = Instant::now();
    let _ = ops::preview_border(&big, &BorderEffect { shadow: 1.0, corner_radius: 12.0, ..Default::default() });
    println!("preview_border 1920x1080 shadow+round: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
    let bigdoc = Document::new(big);
    let t = Instant::now();
    let _ = ops::cut_out(&bigdoc, false, 300.0, 700.0, CutEdge::Zigzag);
    println!("cut_out 1920x1080 zigzag: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
    let t = Instant::now();
    let _ = ops::resize(&bigdoc, 0.5, 0.5, true);
    println!("resize 1920x1080 -> 50%: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
}
