//! X11 platform layer for OxShot.
//!
//! * [`snapshot`] grabs the whole root window (all monitors) plus everything needed to build
//!   the region-selection overlay: monitor layout, visible top-level windows (topmost first),
//!   the cursor image and pointer position.
//! * [`capture_rect`] grabs one region (e.g. "repeat last region").
//! * [`activate_window`] forces focus onto a window (e.g. our own override-redirect overlay).
//! * [`HotkeyService`] registers global hotkeys via passive key grabs on the root window.
//!
//! Everything uses pure-Rust `x11rb`; screen capture uses MIT-SHM when available.

mod capture;
mod conn;
mod hotkey;
mod pixels;
mod windows;

pub mod clipboard;
pub use hotkey::{Hotkey, HotkeyService, Key, Mods};
pub use oxshot_core::IRect;

/// One RandR monitor (logical output area) in root coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    /// RandR monitor name, e.g. `"DP-1"`.
    pub name: String,
    pub rect: IRect,
    pub primary: bool,
}

/// A visible top-level window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    /// Client window XID.
    pub id: u32,
    /// Visible rect in root coordinates, clipped to the root: includes the WM frame/title bar,
    /// excludes invisible client-side-decoration shadows (`_GTK_FRAME_EXTENTS`).
    pub rect: IRect,
    pub title: String,
    /// `WM_CLASS` class part (e.g. `"Firefox"`).
    pub class: String,
    /// Visible sub-areas worth snapping to (mapped child X windows larger than 16 px, root
    /// coordinates, clipped to `rect`). Often empty: GTK/Qt apps rarely use child windows.
    pub children: Vec<IRect>,
}

/// Current cursor image. `x`/`y` is the top-left of the image in root coordinates
/// (pointer position minus hotspot).
#[derive(Debug, Clone)]
pub struct CursorImage {
    /// Straight (non-premultiplied) RGBA.
    pub image: image::RgbaImage,
    pub x: i32,
    pub y: i32,
}

/// Everything captured at one moment.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// The whole root window (all monitors), straight RGBA, alpha = 255.
    pub image: image::RgbaImage,
    pub monitors: Vec<Monitor>,
    /// Visible top-level windows on the current desktop, topmost first. Docks/panels and the
    /// desktop window come last.
    pub windows: Vec<WindowInfo>,
    pub cursor: Option<CursorImage>,
    /// Pointer position in root coordinates.
    pub pointer: (i32, i32),
}

/// Captures the root window plus monitors, windows, cursor and pointer.
///
/// Uses a cached X connection and MIT-SHM segment, so repeated calls are cheap.
pub fn snapshot() -> anyhow::Result<Snapshot> {
    conn::with_ctx(capture::snapshot)
}

/// Captures `rect` (root coordinates, clipped to the root window) as straight RGBA.
pub fn capture_rect(rect: IRect) -> anyhow::Result<image::RgbaImage> {
    conn::with_ctx(|ctx| capture::capture_rect(ctx, rect))
}

/// Raises `xid` and gives it keyboard focus.
///
/// For managed windows `_NET_ACTIVE_WINDOW` is sent to the WM as well. `SetInputFocus` is
/// retried for about 100 ms until `GetInputFocus` reports `xid` (a freshly mapped window may
/// not be viewable yet). Returns an error if focus could not be obtained.
pub fn activate_window(xid: u32) -> anyhow::Result<()> {
    conn::with_ctx(|ctx| windows::activate_window(ctx, xid))
}

/// Sets `_NET_WM_ICON` (width, height, then ARGB pixels, for every size). GPUI sends a
/// single icon in one request, which silently fails for large images; small sizes from
/// our own connection work reliably.
pub fn set_window_icon(xid: u32, data: &[u32]) -> anyhow::Result<()> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode};
    use x11rb::wrapper::ConnectionExt as _;
    conn::with_ctx(|ctx| {
        let atom = ctx.conn.intern_atom(false, b"_NET_WM_ICON")?.reply()?.atom;
        ctx.conn.change_property32(PropMode::REPLACE, xid, atom, AtomEnum::CARDINAL, data)?.check()?;
        ctx.conn.flush()?;
        Ok(())
    })
}

/// Maps (shows) a window, e.g. the pre-created capture overlay.
pub fn map_window(xid: u32) -> anyhow::Result<()> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::ConnectionExt as _;
    conn::with_ctx(|ctx| {
        ctx.conn.map_window(xid)?.check()?;
        ctx.conn.flush()?;
        Ok(())
    })
}

/// Unmaps (hides) a window without destroying it.
pub fn unmap_window(xid: u32) -> anyhow::Result<()> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::ConnectionExt as _;
    conn::with_ctx(|ctx| {
        ctx.conn.unmap_window(xid)?.check()?;
        ctx.conn.flush()?;
        Ok(())
    })
}

/// Moves a window to a root position (used for the override-redirect overlay).
pub fn move_window(xid: u32, x: i32, y: i32) -> anyhow::Result<()> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{ConfigureWindowAux, ConnectionExt as _};
    conn::with_ctx(|ctx| {
        ctx.conn.configure_window(xid, &ConfigureWindowAux::new().x(x).y(y))?.check()?;
        ctx.conn.flush()?;
        Ok(())
    })
}

/// Size of the root window (all monitors).
pub fn root_size() -> anyhow::Result<(u32, u32)> {
    conn::with_ctx(|ctx| ctx.root_size().map(|(w, h)| (w as u32, h as u32)))
}

/// The monitor under the mouse pointer, in root coordinates.
pub fn pointer_monitor() -> anyhow::Result<IRect> {
    conn::with_ctx(capture::pointer_monitor)
}
