//! Screen capture (MIT-SHM with plain GetImage fallback), cursor, pointer, monitors.

use anyhow::{Context as _, Result, anyhow, bail};
use image::RgbaImage;
use oxshot_core::IRect;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};
use x11rb::protocol::{randr, shm, xfixes};

use crate::conn::Ctx;
use crate::pixels::{self, PixelFormat};
use crate::{CursorImage, Monitor, Snapshot};

/// A SysV shared memory segment attached to both us and the X server.
pub struct ShmBuf {
    seg: shm::Seg,
    addr: *mut u8,
    size: usize,
}

// SAFETY: the mapping is process-wide; access is serialized by the Ctx mutex.
unsafe impl Send for ShmBuf {}

impl ShmBuf {
    fn new(ctx: &Ctx, size: usize) -> Result<ShmBuf> {
        // SAFETY: plain SysV shm calls; results are checked.
        unsafe {
            let id = libc::shmget(libc::IPC_PRIVATE, size, libc::IPC_CREAT | 0o600);
            if id < 0 {
                bail!("shmget failed: {}", std::io::Error::last_os_error());
            }
            let addr = libc::shmat(id, std::ptr::null(), 0);
            if addr as isize == -1 {
                let err = std::io::Error::last_os_error();
                libc::shmctl(id, libc::IPC_RMID, std::ptr::null_mut());
                bail!("shmat failed: {err}");
            }
            let seg = ctx.conn.generate_id()?;
            let attached = shm::attach(&ctx.conn, seg, id as u32, false)
                .map_err(anyhow::Error::from)
                .and_then(|c| c.check().map_err(anyhow::Error::from));
            // Mark for deletion now; it lives until both sides detach (Linux semantics).
            libc::shmctl(id, libc::IPC_RMID, std::ptr::null_mut());
            if let Err(e) = attached {
                libc::shmdt(addr);
                return Err(e.context("XShmAttach failed (remote X server?)"));
            }
            Ok(ShmBuf { seg, addr: addr as *mut u8, size })
        }
    }

    fn detach(self, ctx: &Ctx) {
        let _ = shm::detach(&ctx.conn, self.seg);
        let _ = ctx.conn.flush();
        // SAFETY: addr came from shmat and is not used afterwards.
        unsafe { libc::shmdt(self.addr as *const _) };
    }
}

/// Grabs a region of the root window and converts it to RGBA.
fn grab(ctx: &mut Ctx, r: IRect) -> Result<RgbaImage> {
    let format = ctx.format.ok_or_else(|| anyhow!("unsupported root visual (need 24/32-bit TrueColor)"))?;
    let (w, h) = (r.w as u16, r.h as u16);
    let (x, y) = (r.x as i16, r.y as i16);
    if w == 0 || h == 0 {
        bail!("empty capture rect");
    }
    let stride = w as usize * 4;
    let size = stride * h as usize;

    if ctx.has_shm {
        if ctx.shm.as_ref().is_some_and(|b| b.size < size) {
            ctx.shm.take().expect("checked").detach(ctx);
        }
        if ctx.shm.is_none() {
            match ShmBuf::new(ctx, size) {
                Ok(b) => ctx.shm = Some(b),
                Err(e) => {
                    log::warn!("MIT-SHM unavailable, falling back to GetImage: {e:#}");
                    ctx.has_shm = false;
                }
            }
        }
        if let Some(buf) = &ctx.shm {
            let reply = shm::get_image(&ctx.conn, ctx.root, x, y, w, h, !0, ImageFormat::Z_PIXMAP.into(), buf.seg, 0)?
                .reply()
                .context("XShmGetImage failed")?;
            if (reply.size as usize) < size {
                bail!("XShmGetImage returned {} bytes, expected {size}", reply.size);
            }
            // SAFETY: the server finished writing (we have the reply); buffer is >= size bytes.
            let data = unsafe { std::slice::from_raw_parts(buf.addr as *const u8, size) };
            return Ok(format.to_rgba(data, stride, w as u32, h as u32));
        }
    }

    let reply = ctx
        .conn
        .get_image(ImageFormat::Z_PIXMAP, ctx.root, x, y, w, h, !0)?
        .reply()
        .context("GetImage failed")?;
    to_rgba_checked(format, &reply.data, w, h)
}

fn to_rgba_checked(format: PixelFormat, data: &[u8], w: u16, h: u16) -> Result<RgbaImage> {
    let stride = w as usize * 4;
    if data.len() < stride * h as usize {
        bail!("GetImage returned {} bytes, expected {}", data.len(), stride * h as usize);
    }
    Ok(format.to_rgba(data, stride, w as u32, h as u32))
}

fn root_rect(ctx: &Ctx) -> Result<IRect> {
    let (w, h) = ctx.root_size()?;
    Ok(IRect::new(0, 0, w as u32, h as u32))
}

pub fn capture_rect(ctx: &mut Ctx, rect: IRect) -> Result<RgbaImage> {
    let r = rect.intersect(&root_rect(ctx)?).ok_or_else(|| anyhow!("rect {rect:?} is outside the screen"))?;
    grab(ctx, r)
}

pub fn snapshot(ctx: &mut Ctx) -> Result<Snapshot> {
    let root = root_rect(ctx)?;
    // Image first: it defines "the moment" of the screenshot.
    let image = grab(ctx, root)?;

    // Fire the small requests together, then collect.
    let pointer = ctx.conn.query_pointer(ctx.root)?;
    let cursor = ctx.has_xfixes.then(|| xfixes::get_cursor_image(&ctx.conn)).transpose()?;
    let monitors = ctx.has_randr.then(|| randr::get_monitors(&ctx.conn, ctx.root, true)).transpose()?;
    let windows = crate::windows::list_windows(ctx, root)?;

    let p = pointer.reply()?;
    let pointer = (p.root_x as i32, p.root_y as i32);

    let cursor = cursor.and_then(|c| c.reply().ok()).and_then(|c| {
        Some(CursorImage {
            image: pixels::cursor_to_rgba(c.width, c.height, &c.cursor_image)?,
            x: c.x as i32 - c.xhot as i32,
            y: c.y as i32 - c.yhot as i32,
        })
    });

    let monitors = match monitors.and_then(|c| c.reply().ok()) {
        Some(reply) if !reply.monitors.is_empty() => {
            let names: Vec<_> = reply
                .monitors
                .iter()
                .map(|m| ctx.conn.get_atom_name(m.name).ok())
                .collect();
            reply
                .monitors
                .iter()
                .zip(names)
                .map(|(m, name)| Monitor {
                    name: name
                        .and_then(|c| c.reply().ok())
                        .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                        .unwrap_or_default(),
                    rect: IRect::new(m.x as i32, m.y as i32, m.width as u32, m.height as u32),
                    primary: m.primary,
                })
                .collect()
        }
        _ => vec![Monitor { name: "screen".into(), rect: root, primary: true }],
    };

    Ok(Snapshot { image, monitors, windows, cursor, pointer })
}

/// The monitor under the pointer (the whole root without RandR).
pub fn pointer_monitor(ctx: &mut Ctx) -> Result<IRect> {
    let root = root_rect(ctx)?;
    let p = ctx.conn.query_pointer(ctx.root)?.reply()?;
    let (x, y) = (p.root_x as i32, p.root_y as i32);
    if !ctx.has_randr {
        return Ok(root);
    }
    let reply = randr::get_monitors(&ctx.conn, ctx.root, true)?.reply()?;
    Ok(reply
        .monitors
        .iter()
        .map(|m| IRect::new(m.x as i32, m.y as i32, m.width as u32, m.height as u32))
        .find(|r| r.contains(x, y))
        .unwrap_or(root))
}
