//! File side of the workspace: scanning the captures folder, the on-disk thumbnail cache
//! and the file operations (import, rename/convert, trash, show in folder).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use gpui_kit::{RenderImage, SharedString};

/// Image types shown in the workspace.
pub const EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "webp", "bmp", "gif"];

/// Longest side of a cached thumbnail.
const THUMB_SIZE: u32 = 256;

#[derive(Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub name: SharedString,
    /// Lower-case name for filtering.
    pub lname: String,
    pub mtime: SystemTime,
    pub len: u64,
}

pub fn is_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

pub fn dir_mtime(dir: &Path) -> Option<SystemTime> {
    std::fs::metadata(dir).and_then(|m| m.modified()).ok()
}

/// All images directly inside `dir` (hidden files and the `.oxshot` folder are skipped).
pub fn scan(dir: &Path) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = e.path();
        if !is_image(&path) {
            continue;
        }
        let Ok(md) = e.metadata() else { continue };
        if !md.is_file() {
            continue;
        }
        out.push(Entry {
            lname: name.to_lowercase(),
            name: name.into(),
            path,
            mtime: md.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            len: md.len(),
        });
    }
    out
}

/// `<dir>/.oxshot/thumbs/<file name>.png`
pub fn thumb_path(image: &Path) -> PathBuf {
    let dir = image.parent().unwrap_or(Path::new(".")).join(".oxshot").join("thumbs");
    let name = image.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    dir.join(format!("{name}.png"))
}

pub struct Thumb {
    pub image: Arc<RenderImage>,
    /// Size of the original image.
    pub dims: (u32, u32),
}

/// Loads the cached thumbnail, (re)generating it when the source is newer. Runs on a
/// background thread.
pub fn load_thumb(path: &Path, src_mtime: SystemTime) -> anyhow::Result<Thumb> {
    let dims = image::image_dimensions(path)?;
    let cache = thumb_path(path);
    let fresh = std::fs::metadata(&cache).and_then(|m| m.modified()).is_ok_and(|m| m >= src_mtime);
    let cached = if fresh { image::open(&cache).ok().map(|i| i.to_rgba8()) } else { None };
    let small = match cached {
        Some(i) => i,
        None => {
            let img = image::open(path)?;
            let small = if img.width() <= THUMB_SIZE && img.height() <= THUMB_SIZE {
                img.to_rgba8()
            } else {
                img.thumbnail(THUMB_SIZE, THUMB_SIZE).to_rgba8()
            };
            if let Err(e) = write_thumb(&small, &cache) {
                log::debug!("cannot cache thumbnail {}: {e:#}", cache.display());
            }
            small
        }
    };
    let (w, h) = small.dimensions();
    Ok(Thumb { image: crate::gfx::to_render_image(&small, 0, 0, w, h), dims })
}

fn write_thumb(img: &image::RgbaImage, path: &Path) -> anyhow::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("png.tmp");
    let f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
    let enc = image::codecs::png::PngEncoder::new_with_quality(f, image::codecs::png::CompressionType::Fast, image::codecs::png::FilterType::Adaptive);
    image::ImageEncoder::write_image(enc, img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// A free path `dir/stem.ext`, adding `_2`, `_3` … if taken.
fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut p = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{stem}_{n}.{ext}"));
        n += 1;
    }
    p
}

/// Copies image files into the workspace. Returns the new paths.
pub fn import(paths: &[PathBuf], dir: &Path) -> (Vec<PathBuf>, Vec<String>) {
    let mut done = Vec::new();
    let mut errors = Vec::new();
    if let Err(e) = std::fs::create_dir_all(dir) {
        errors.push(e.to_string());
        return (done, errors);
    }
    for src in paths {
        if !src.is_file() || !is_image(src) {
            errors.push(rust_i18n::t!("workspace.not_an_image", path = src.display()).to_string());
            continue;
        }
        // Already in the workspace: nothing to do.
        if src.parent().and_then(|p| p.canonicalize().ok()) == dir.canonicalize().ok() {
            continue;
        }
        let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| rust_i18n::t!("workspace.default_stem").to_string());
        let ext = src.extension().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_else(|| "png".into());
        let dst = unique_path(dir, &stem, &ext);
        match std::fs::copy(src, &dst) {
            Ok(_) => done.push(dst),
            Err(e) => errors.push(format!("{}: {e}", src.display())),
        }
    }
    (done, errors)
}

/// Checks a new file name typed by the user.
pub fn validate_name(old: &Path, new_name: &str) -> Result<PathBuf, String> {
    let new_name = new_name.trim();
    if new_name.is_empty() || new_name.starts_with('.') || new_name.contains('/') || new_name == ".." {
        return Err(rust_i18n::t!("workspace.invalid_name").to_string());
    }
    let mut new_path = old.with_file_name(new_name);
    if new_path.extension().is_none() {
        // No extension typed: keep the old one.
        if let Some(ext) = old.extension() {
            new_path = old.with_file_name(format!("{new_name}.{}", ext.to_string_lossy()));
        }
    }
    if !is_image(&new_path) {
        return Err(rust_i18n::t!("workspace.unknown_format", list = EXTENSIONS.join(", ")).to_string());
    }
    if new_path != old && new_path.exists() {
        return Err(rust_i18n::t!("workspace.name_taken").to_string());
    }
    Ok(new_path)
}

fn same_format(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let e = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if e == "jpeg" { "jpg".to_string() } else { e }
    };
    norm(a) == norm(b)
}

/// Renames an image (and its project file). A different extension converts the image,
/// like Screenpresso; the original then goes to the trash.
pub fn rename(old: &Path, new: &Path) -> anyhow::Result<()> {
    if old == new {
        return Ok(());
    }
    if same_format(old, new) {
        std::fs::rename(old, new)?;
    } else {
        let img = image::open(old)?.to_rgba8();
        let doc = oxshot_core::Document::new(img);
        crate::storage::export(&doc, &doc.base, new)?;
        if let Err(e) = trash::delete(old) {
            log::warn!("cannot trash {}: {e}", old.display());
            std::fs::remove_file(old)?;
        }
    }
    let (op, np) = (crate::storage::project_path(old), crate::storage::project_path(new));
    if op.exists()
        && op != np
        && let Err(e) = std::fs::rename(&op, &np)
    {
        log::warn!("cannot rename project {}: {e}", op.display());
    }
    let _ = std::fs::remove_file(thumb_path(old));
    Ok(())
}

/// Moves images and their project files to the freedesktop trash.
pub fn trash_all(paths: &[PathBuf]) -> anyhow::Result<()> {
    let mut items: Vec<PathBuf> = paths.to_vec();
    for p in paths {
        let proj = crate::storage::project_path(p);
        if proj.exists() {
            items.push(proj);
        }
    }
    trash::delete_all(&items)?;
    for p in paths {
        let _ = std::fs::remove_file(thumb_path(p));
    }
    Ok(())
}

fn file_uri(p: &Path) -> String {
    let mut s = String::from("file://");
    for b in p.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => s.push(b as char),
            _ => s.push_str(&format!("%{b:02X}")),
        }
    }
    s
}

/// "Open with…": the desktop's program chooser (xdg-desktop-portal `OpenURI.OpenFile`
/// with `ask`), parented to `window` (X11 id). Falls back to the default application.
/// Runs in the background (the portal call blocks until the chooser is shown).
pub fn open_with(p: &Path, window: Option<u32>) {
    let p = p.to_path_buf();
    std::thread::spawn(move || {
        if let Err(e) = portal_open_with(&p, window) {
            log::warn!("portal open-with failed ({e:#}), using xdg-open");
            xdg_open(&p);
        }
    });
}

fn portal_open_with(p: &Path, window: Option<u32>) -> anyhow::Result<()> {
    use std::collections::HashMap;
    use zbus::zvariant::{Fd, Value};
    let file = std::fs::File::open(p)?;
    let parent = window.map_or_else(String::new, |id| format!("x11:{id:x}"));
    let options: HashMap<&str, Value> = HashMap::from([("ask", Value::from(true))]);
    zbus::blocking::Connection::session()?.call_method(
        Some("org.freedesktop.portal.Desktop"),
        "/org/freedesktop/portal/desktop",
        Some("org.freedesktop.portal.OpenURI"),
        "OpenFile",
        &(parent, Fd::from(&file), options),
    )?;
    Ok(())
}

/// Opens a path with the default application.
pub fn xdg_open(p: &Path) {
    if let Err(e) = std::process::Command::new("xdg-open").arg(p).spawn() {
        log::warn!("xdg-open failed: {e}");
    }
}

/// Shows the file selected in the file manager (FileManager1 D-Bus interface), falling
/// back to opening the folder.
pub fn show_in_folder(p: &Path) {
    let ok = std::process::Command::new("dbus-send")
        .args([
            "--session",
            "--dest=org.freedesktop.FileManager1",
            "--type=method_call",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
        ])
        .arg(format!("array:string:{}", file_uri(p)))
        .arg("string:")
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        xdg_open(p.parent().unwrap_or(Path::new(".")));
    }
}

/// The current language uses a decimal comma.
fn decimal_comma() -> bool {
    !matches!(&*rust_i18n::locale(), "en" | "zh-CN")
}

/// "1.2 MB" / "1,2 MB" / "1,2 Mo"
pub fn human_size(n: u64) -> String {
    if n < 1024 {
        rust_i18n::t!("workspace.size_b", n = n).to_string()
    } else if n < 1024 * 1024 {
        rust_i18n::t!("workspace.size_kb", n = format!("{:.0}", n as f64 / 1024.0)).to_string()
    } else {
        let mut v = format!("{:.1}", n as f64 / (1024.0 * 1024.0));
        if decimal_comma() {
            v = v.replace('.', ",");
        }
        rust_i18n::t!("workspace.size_mb", n = v).to_string()
    }
}

/// Modification date in the current language's usual format.
pub fn human_date(t: SystemTime) -> String {
    let fmt = match &*rust_i18n::locale() {
        "de" | "ru" => "%d.%m.%Y %H:%M",
        "es" | "fr" | "pt-BR" => "%d/%m/%Y %H:%M",
        _ => "%Y-%m-%d %H:%M",
    };
    chrono::DateTime::<chrono::Local>::from(t).format(fmt).to_string()
}
