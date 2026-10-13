//! Cached X connection shared by snapshot/capture/activate calls.

use std::sync::Mutex;

use anyhow::{Context as _, Result};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageOrder, Window};
use x11rb::rust_connection::RustConnection;

use crate::capture::ShmBuf;
use crate::pixels::PixelFormat;

x11rb::atom_manager! {
    pub Atoms: AtomsCookie {
        _NET_CLIENT_LIST_STACKING,
        _NET_CURRENT_DESKTOP,
        _NET_WM_DESKTOP,
        _NET_WM_STATE,
        _NET_WM_STATE_HIDDEN,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_DOCK,
        _NET_WM_WINDOW_TYPE_DESKTOP,
        _NET_FRAME_EXTENTS,
        _GTK_FRAME_EXTENTS,
        _NET_WM_NAME,
        _NET_ACTIVE_WINDOW,
        UTF8_STRING,
    }
}

pub struct Ctx {
    pub conn: RustConnection,
    pub root: Window,
    pub atoms: Atoms,
    /// Pixel format of the root visual (`None` = unsupported depth/bpp).
    pub format: Option<PixelFormat>,
    pub has_shm: bool,
    pub has_xfixes: bool,
    pub has_randr: bool,
    /// Lazily created, grown on demand.
    pub shm: Option<ShmBuf>,
}

impl Ctx {
    fn connect() -> Result<Ctx> {
        let (conn, screen_num) = x11rb::connect(None).context("cannot connect to X server")?;
        let setup = conn.setup();
        let screen = &setup.roots[screen_num];
        let root = screen.root;
        let depth = screen.root_depth;
        let bpp = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == depth)
            .map(|f| f.bits_per_pixel)
            .unwrap_or(0);
        let visual = screen
            .allowed_depths
            .iter()
            .flat_map(|d| d.visuals.iter())
            .find(|v| v.visual_id == screen.root_visual);
        let format = visual.and_then(|v| {
            PixelFormat::new(
                bpp,
                v.red_mask,
                v.green_mask,
                v.blue_mask,
                setup.image_byte_order == ImageOrder::MSB_FIRST,
            )
        });
        let atoms = Atoms::new(&conn)?.reply()?;

        let has_shm = x11rb::protocol::shm::query_version(&conn)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some();
        let has_xfixes = x11rb::protocol::xfixes::query_version(&conn, 4, 0)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|r| r.major_version >= 1);
        let has_randr = x11rb::protocol::randr::query_version(&conn, 1, 5)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|r| r.major_version > 1 || r.minor_version >= 5);
        log::debug!(
            "X11 connected: depth={depth} bpp={bpp} format={format:?} shm={has_shm} xfixes={has_xfixes} randr1.5={has_randr}"
        );
        Ok(Ctx { conn, root, atoms, format, has_shm, has_xfixes, has_randr, shm: None })
    }

    /// Current root size (may change on monitor hotplug).
    pub fn root_size(&self) -> Result<(u16, u16)> {
        let g = self.conn.get_geometry(self.root)?.reply()?;
        Ok((g.width, g.height))
    }
}

static CTX: Mutex<Option<Ctx>> = Mutex::new(None);

/// Runs `f` with the cached connection, (re)connecting if needed. On a connection-level
/// error the cached context is dropped so the next call reconnects.
pub fn with_ctx<T>(f: impl FnOnce(&mut Ctx) -> Result<T>) -> Result<T> {
    let mut guard = CTX.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(Ctx::connect()?);
    }
    let ctx = guard.as_mut().expect("just set");
    let res = f(ctx);
    if res.is_err() {
        // Cheap liveness check: if the connection is broken, reconnect next time.
        let alive = ctx.conn.get_input_focus().ok().and_then(|c| c.reply().ok()).is_some();
        if !alive {
            *guard = None;
        }
    }
    res
}
