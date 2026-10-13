//! Where captures live: `~/Pictures/OxShot/<timestamp>.png`, plus the editable project in
//! `~/Pictures/OxShot/.oxshot/<timestamp>.oxshot` (like Screenpresso's `Originals`).

use std::path::{Path, PathBuf};

use image::RgbaImage;
use oxshot_core::Document;

/// Background writes that must finish before the process exits.
static PENDING: std::sync::Mutex<Vec<std::thread::JoinHandle<()>>> = std::sync::Mutex::new(Vec::new());

/// Blocks until all background saves are written.
pub fn flush() {
    let handles: Vec<_> = std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()));
    for h in handles {
        let _ = h.join();
    }
}

static SAVE_DIR: std::sync::OnceLock<std::sync::Mutex<Option<PathBuf>>> = std::sync::OnceLock::new();

pub fn set_save_dir(dir: Option<PathBuf>) {
    *SAVE_DIR.get_or_init(Default::default).lock().unwrap() = dir;
}

pub fn captures_dir() -> PathBuf {
    if let Some(d) = SAVE_DIR.get_or_init(Default::default).lock().unwrap().clone() {
        return d;
    }
    dirs::picture_dir().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from(".")).join("OxShot")
}

/// Images in the capture folder, oldest first (names carry the timestamp; mtime breaks ties).
pub fn list_captures() -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(captures_dir()) else { return Vec::new() };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif" | "tif" | "tiff")
            })
        })
        .map(|p| (std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p))
        .collect();
    files.sort();
    files.into_iter().map(|(_, p)| p).collect()
}

/// Project file belonging to an exported image.
pub fn project_path(image: &Path) -> PathBuf {
    let dir = image.parent().unwrap_or(Path::new(".")).join(".oxshot");
    let stem = image.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "capture".into());
    dir.join(format!("{stem}.oxshot"))
}

pub fn new_capture_path() -> PathBuf {
    let dir = captures_dir();
    let base = chrono::Local::now().format("Screenshot_%Y-%m-%d_%H-%M-%S").to_string();
    let mut p = dir.join(format!("{base}.png"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{base}_{n}.png"));
        n += 1;
    }
    p
}

/// Saves the flattened image and the project file. The path is decided synchronously,
/// encoding and writing happen on a background thread so the UI never stalls.
pub fn save_capture(doc: &Document, img: &RgbaImage, existing: Option<&Path>) -> anyhow::Result<PathBuf> {
    let path = existing.map(Path::to_path_buf).unwrap_or_else(new_capture_path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let (doc, img, p) = (doc.clone(), img.clone(), path.clone());
    let handle = std::thread::spawn(move || {
        if let Err(e) = write_image(&img, &p) {
            log::error!("saving {} failed: {e:#}", p.display());
        }
        let proj = project_path(&p);
        let res = proj.parent().map_or(Ok(()), std::fs::create_dir_all).map_err(anyhow::Error::from).and_then(|_| oxshot_core::project::save(&doc, &proj));
        if let Err(e) = res {
            log::error!("saving project {} failed: {e:#}", proj.display());
        }
    });
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    pending.retain(|h| !h.is_finished());
    pending.push(handle);
    Ok(path)
}

/// "Save as": format by extension; `.oxshot` writes the editable project.
pub fn export(doc: &Document, img: &RgbaImage, path: &Path) -> anyhow::Result<()> {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("oxshot") => oxshot_core::project::save(doc, path),
        _ => write_image(img, path),
    }
}

fn write_image(img: &RgbaImage, path: &Path) -> anyhow::Result<()> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    let tmp = path.with_extension(format!("{}.tmp", ext.as_deref().unwrap_or("png")));
    let fmt = image::ImageFormat::from_extension(ext.as_deref().unwrap_or("png")).unwrap_or(image::ImageFormat::Png);
    match fmt {
        // No alpha channel in JPEG/BMP: composite on white.
        image::ImageFormat::Jpeg | image::ImageFormat::Bmp => {
            let mut rgb = image::RgbImage::new(img.width(), img.height());
            for (d, s) in rgb.pixels_mut().zip(img.pixels()) {
                let a = s[3] as u32;
                let mix = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
                *d = image::Rgb([mix(s[0]), mix(s[1]), mix(s[2])]);
            }
            rgb.save_with_format(&tmp, fmt)?;
        }
        image::ImageFormat::Png => {
            // Fast compression: screenshots compress well anyway and saving must be quick.
            let f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
            let enc = image::codecs::png::PngEncoder::new_with_quality(f, image::codecs::png::CompressionType::Fast, image::codecs::png::FilterType::Adaptive);
            image::ImageEncoder::write_image(enc, img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)?;
        }
        _ => img.save_with_format(&tmp, fmt)?,
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Opens an image or project file as a document. For images that have a project file
/// next to them, the editable project is loaded instead.
pub fn open(path: &Path) -> anyhow::Result<(Document, Option<PathBuf>)> {
    if path.extension().is_some_and(|e| e == "oxshot") {
        return Ok((oxshot_core::project::load(path)?, None));
    }
    let proj = project_path(path);
    if proj.exists() {
        match oxshot_core::project::load(&proj) {
            Ok(doc) => return Ok((doc, Some(path.to_path_buf()))),
            Err(e) => log::warn!("ignoring broken project {}: {e:#}", proj.display()),
        }
    }
    let img = image::open(path)?.to_rgba8();
    // Images from elsewhere are never overwritten: edits are saved as a new capture.
    let own = path.canonicalize().ok().zip(captures_dir().canonicalize().ok()).is_some_and(|(p, d)| p.starts_with(d));
    Ok((Document::new(img), own.then(|| path.to_path_buf())))
}
