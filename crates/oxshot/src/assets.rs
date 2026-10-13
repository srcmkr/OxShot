//! Embedded SVG icons (Lucide, ISC license) with fallback to the GPUI Kit component icons.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

#[derive(rust_embed::RustEmbed)]
#[folder = "assets"]
#[include = "icons/*.svg"]
struct Embedded;

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(file) = Embedded::get(path) {
            return Ok(Some(file.data));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut out: Vec<SharedString> =
            Embedded::iter().filter(|p| p.starts_with(path)).map(|p| SharedString::from(p.into_owned())).collect();
        out.extend(gpui_kit::assets::Assets.list(path)?);
        Ok(out)
    }
}

/// Path of one of our icons, for `Icon::default().path(..)` / `svg().path(..)`.
pub fn icon(name: &str) -> SharedString {
    SharedString::from(format!("icons/{name}.svg"))
}

const MARK_256: &[u8] = include_bytes!("../../../assets/brand/png/oxshot-mark-256.png");

/// Installs the icon into the user's icon theme dir so `.desktop` files can use `Icon=oxshot`.
pub fn install_desktop_icon() -> anyhow::Result<()> {
    let dir = dirs::data_dir().unwrap_or_default().join("icons/hicolor/256x256/apps");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("oxshot.png"), MARK_256)?;
    Ok(())
}
