//! `.oxshot` project files: JSON with the base image and stamp resources embedded as
//! base64 PNG. Lets the user re-open a screenshot with all annotations still editable.
//!
//! ```json
//! { "version": 1, "canvas": {...}, "objects": [...],
//!   "base_png": "<base64>", "resources": { "name": "<base64 png>" } }
//! ```

use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use image::{ImageFormat, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::model::{Canvas, DOCUMENT_VERSION, Document, Object};

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    version: u32,
    canvas: Canvas,
    objects: Vec<Object>,
    base_png: String,
    #[serde(default)]
    resources: BTreeMap<String, String>,
}

fn encode_png(img: &RgbaImage) -> anyhow::Result<String> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Png).context("encoding PNG")?;
    Ok(B64.encode(buf.into_inner()))
}

fn decode_png(data: &str, what: &str) -> anyhow::Result<RgbaImage> {
    let bytes = B64.decode(data.trim()).with_context(|| format!("{what}: invalid base64"))?;
    let img = image::load_from_memory_with_format(&bytes, ImageFormat::Png)
        .with_context(|| format!("{what}: invalid PNG"))?;
    Ok(img.to_rgba8())
}

/// Serializes a document to the project JSON format.
pub fn to_json(doc: &Document) -> anyhow::Result<String> {
    let file = ProjectFile {
        version: DOCUMENT_VERSION,
        canvas: doc.canvas.clone(),
        objects: doc.objects.clone(),
        base_png: encode_png(&doc.base)?,
        resources: doc
            .resources
            .iter()
            .map(|(k, v)| Ok((k.clone(), encode_png(v)?)))
            .collect::<anyhow::Result<_>>()?,
    };
    Ok(serde_json::to_string(&file)?)
}

/// Parses project JSON, validating the format version.
pub fn from_json(json: &str) -> anyhow::Result<Document> {
    let value: serde_json::Value = serde_json::from_str(json).context("not a valid project file (JSON)")?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .context("project file has no version")?;
    if version != DOCUMENT_VERSION as u64 {
        bail!("unsupported project file version {version} (expected {DOCUMENT_VERSION})");
    }
    let file: ProjectFile = serde_json::from_value(value).context("malformed project file")?;
    let base = decode_png(&file.base_png, "base image")?;
    let mut resources = BTreeMap::new();
    for (name, data) in &file.resources {
        resources.insert(name.clone(), Arc::new(decode_png(data, &format!("resource {name:?}"))?));
    }
    Ok(Document::from_parts(Arc::new(base), file.canvas, file.objects, resources))
}

/// Writes the project atomically: a temporary file next to `path`, then a rename.
pub fn save(doc: &Document, path: &Path) -> anyhow::Result<()> {
    let json = to_json(doc)?;
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().context("project path has no file name")?.to_string_lossy();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| -> anyhow::Result<()> {
        let mut f = std::fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path).with_context(|| format!("renaming to {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

pub fn load(path: &Path) -> anyhow::Result<Document> {
    let json = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    from_json(&json).with_context(|| format!("loading {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{IRect, Point, Rect};
    use crate::model::*;
    use image::Rgba;

    fn sample() -> Document {
        let base = RgbaImage::from_fn(30, 20, |x, y| Rgba([x as u8 * 8, y as u8 * 12, 77, 200 + (x % 50) as u8]));
        let mut doc = Document::new(base);
        doc.canvas = Canvas { rect: IRect::new(-5, -5, 40, 30), background: Color::rgba(1, 2, 3, 4) };
        doc.resources.insert("stamp".into(), Arc::new(RgbaImage::from_pixel(4, 4, Rgba([9, 8, 7, 6]))));
        doc.push(Shape::Arrow(ArrowShape {
            start: Point::new(1.0, 2.0),
            end: Point::new(20.5, 10.25),
            curve: Some(Point::new(5.0, 5.0)),
            color: Color::RED,
            width: 4.0,
            style: ArrowStyle::Tapered,
            heads: ArrowHeads::Both,
            dashed: true,
        }));
        let id = doc.push(Shape::Stamp(StampShape { rect: Rect::new(2.0, 2.0, 8.0, 8.0), resource: "stamp".into() }));
        let o = doc.get_mut(id).unwrap();
        o.opacity = 0.5;
        o.shadow = 0.75;
        doc.push(Shape::Text(TextShape {
            origin: Point::new(3.0, 4.0),
            text: "Hällo\n\"quoted\"".into(),
            style: TextStyle::default(),
            max_width: Some(100.0),
        }));
        doc
    }

    #[test]
    fn roundtrip_file() {
        let doc = sample();
        let dir = std::env::temp_dir().join(format!("oxshot-project-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("demo.oxshot");
        save(&doc, &path).unwrap();
        let back = load(&path).unwrap();
        assert_eq!(*back.base, *doc.base);
        assert_eq!(back.canvas, doc.canvas);
        assert_eq!(back.objects, doc.objects);
        assert_eq!(back.resources.len(), 1);
        assert_eq!(*back.resources["stamp"], *doc.resources["stamp"]);
        // Ids continue after the loaded objects.
        let mut back = back;
        let max = doc.objects.iter().map(|o| o.id).max().unwrap();
        assert!(back.alloc_id() > max);
        // No temp files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).collect();
        assert!(leftovers.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_bad_input() {
        assert!(from_json("not json").is_err());
        assert!(from_json("{}").is_err());
        let json = to_json(&sample()).unwrap();
        let wrong = json.replacen("\"version\":1", "\"version\":99", 1);
        let err = from_json(&wrong).unwrap_err().to_string();
        assert!(err.contains("version"), "{err}");
        let broken = json.replacen("\"base_png\":\"", "\"base_png\":\"!!", 1);
        assert!(from_json(&broken).is_err());
    }
}
