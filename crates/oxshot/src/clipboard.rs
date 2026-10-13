//! System clipboard. Copying goes through `oxshot_x11::clipboard` (image + file + path);
//! reading pasted images uses arboard. The `Clipboard` instance is
//! kept alive for the whole process: on X11 the owner must stay around to serve pastes,
//! which is why OxShot runs as a background process.

use std::sync::Mutex;

use image::RgbaImage;

static CLIPBOARD: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut arboard::Clipboard) -> anyhow::Result<R>) -> anyhow::Result<R> {
    let mut guard = CLIPBOARD.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(arboard::Clipboard::new()?);
    }
    f(guard.as_mut().expect("initialized above"))
}

/// Copies a screenshot: as image, and — if it is saved — as file and path too.
pub fn set_capture(img: &RgbaImage, file: Option<&std::path::Path>) -> anyhow::Result<()> {
    oxshot_x11::clipboard::set(Some(img.clone()), file.map(std::path::Path::to_path_buf))
}

#[allow(dead_code)]
pub fn set_image(img: &RgbaImage) -> anyhow::Result<()> {
    with(|c| {
        c.set_image(arboard::ImageData {
            width: img.width() as usize,
            height: img.height() as usize,
            bytes: std::borrow::Cow::Borrowed(img.as_raw()),
        })?;
        Ok(())
    })
}

/// Puts plain text (e.g. a picked color `#RRGGBB`) on the clipboard.
pub fn set_text(text: &str) -> anyhow::Result<()> {
    with(|c| {
        c.set_text(text)?;
        Ok(())
    })
}

pub fn get_image() -> Option<RgbaImage> {
    with(|c| {
        let d = c.get_image()?;
        RgbaImage::from_raw(d.width as u32, d.height as u32, d.bytes.into_owned()).ok_or_else(|| anyhow::anyhow!("bad image"))
    })
    .ok()
}
