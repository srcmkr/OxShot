//! The application icon, rendered from `assets/brand/oxshot-mark.svg` at the sizes the
//! tray and the window manager ask for (idea from the Anything project).

use gpui_kit::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use resvg::{tiny_skia, usvg};

const SVG: &[u8] = include_bytes!("../../../assets/brand/oxshot-mark.svg");

/// Sizes offered to the tray and the window manager.
const SIZES: [u32; 7] = [16, 22, 24, 32, 48, 64, 128];

/// Straight RGBA pixels, row by row.
fn rgba(size: u32) -> Vec<[u8; 4]> {
    let tree = usvg::Tree::from_data(SVG, &usvg::Options::default()).expect("bundled icon is valid");
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("icon size is not zero");
    let scale = size as f32 / tree.size().width();
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap
        .pixels()
        .iter()
        .map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}

/// Tray pixmaps: (width, height, ARGB in network byte order).
pub fn tray_pixmaps() -> Vec<(i32, i32, Vec<u8>)> {
    SIZES[..6]
        .iter()
        .map(|&s| (s as i32, s as i32, rgba(s).into_iter().flat_map(|[r, g, b, a]| [a, r, g, b]).collect()))
        .collect()
}

/// `_NET_WM_ICON` data for every size.
fn net_wm_icon() -> &'static [u32] {
    static DATA: std::sync::OnceLock<Vec<u32>> = std::sync::OnceLock::new();
    DATA.get_or_init(|| {
        let mut data = Vec::new();
        for size in SIZES {
            data.extend([size, size]);
            data.extend(rgba(size).into_iter().map(|[r, g, b, a]| u32::from_be_bytes([a, r, g, b])));
        }
        data
    })
}

/// X11 window id of a GPUI window.
pub fn xid(window: &Window) -> Option<u32> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Xlib(x) => Some(x.window as u32),
        RawWindowHandle::Xcb(x) => Some(x.window.get()),
        _ => None,
    }
}

/// Gives a window the OxShot icon.
pub fn apply(window: &Window) {
    if let Some(xid) = xid(window)
        && let Err(e) = oxshot_x11::set_window_icon(xid, net_wm_icon())
    {
        log::warn!("window icon: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders() {
        let px = super::rgba(32);
        assert_eq!(px.len(), 32 * 32);
        assert_eq!(px[0][3], 0, "corners are transparent");
        assert_eq!(px[16 * 32 + 16][3], 255, "the tile is opaque");
        assert_eq!(&super::net_wm_icon()[..2], &[16, 16]);
    }
}
