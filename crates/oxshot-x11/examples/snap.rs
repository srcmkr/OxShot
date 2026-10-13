//! Takes snapshots, prints timing and metadata, saves small previews.
//! Usage: cargo run --release -p oxshot-x11 --example snap [OUT_DIR]
use std::time::Instant;

use image::imageops::{self, FilterType};

fn main() -> anyhow::Result<()> {
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| std::env::temp_dir().display().to_string());

    let t = Instant::now();
    let snap = oxshot_x11::snapshot()?;
    println!("first snapshot (incl. connect + shm setup): {:?}", t.elapsed());
    for _ in 0..5 {
        let t = Instant::now();
        let s = oxshot_x11::snapshot()?;
        println!("snapshot {}x{}: {:?}", s.image.width(), s.image.height(), t.elapsed());
    }
    let t = Instant::now();
    let r = oxshot_x11::capture_rect(oxshot_core::IRect::new(100, 100, 800, 600))?;
    println!("capture_rect {}x{}: {:?}", r.width(), r.height(), t.elapsed());

    println!("\nmonitors:");
    for m in &snap.monitors {
        println!("  {:<10} {:?} primary={}", m.name, m.rect, m.primary);
    }
    println!("\nwindows ({} total, topmost first):", snap.windows.len());
    for w in snap.windows.iter().take(10) {
        let title: String = w.title.chars().take(40).collect();
        println!("  {:#010x} {:?} class={:?} title={:?} children={}", w.id, w.rect, w.class, title, w.children.len());
    }
    println!("\npointer: {:?}", snap.pointer);
    match &snap.cursor {
        Some(c) => println!("cursor: {}x{} at ({}, {})", c.image.width(), c.image.height(), c.x, c.y),
        None => println!("cursor: none"),
    }

    let (w, h) = snap.image.dimensions();
    imageops::resize(&snap.image, w / 4, h / 4, FilterType::Triangle).save(format!("{out_dir}/snap_small.png"))?;
    if let Some(win) = snap.windows.first() {
        let crop = imageops::crop_imm(&snap.image, win.rect.x as u32, win.rect.y as u32, win.rect.w, win.rect.h).to_image();
        let scale = (1000.0 / crop.width().max(crop.height()) as f32).min(1.0);
        let crop = if scale < 1.0 {
            imageops::resize(&crop, (crop.width() as f32 * scale) as u32, (crop.height() as f32 * scale) as u32, FilterType::Triangle)
        } else {
            crop
        };
        crop.save(format!("{out_dir}/snap_window.png"))?;
    }
    println!("saved previews to {out_dir}");
    Ok(())
}
