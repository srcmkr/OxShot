//! Top-level window enumeration and window activation.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use oxshot_core::IRect;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask, GetPropertyReply, InputFocus,
    MapState, StackMode, Window,
};
use x11rb::rust_connection::RustConnection;
use x11rb::CURRENT_TIME;

use crate::WindowInfo;
use crate::conn::Ctx;

const MIN_CHILD: u32 = 16;

type PropCookie<'a> = x11rb::cookie::Cookie<'a, RustConnection, GetPropertyReply>;

fn prop<'a>(conn: &'a RustConnection, win: Window, atom: u32) -> Result<PropCookie<'a>> {
    Ok(conn.get_property(false, win, atom, AtomEnum::ANY, 0, 4096)?)
}

fn u32s(c: PropCookie<'_>) -> Vec<u32> {
    c.reply().ok().and_then(|r| r.value32().map(|v| v.collect())).unwrap_or_default()
}

fn text(c: PropCookie<'_>) -> Option<String> {
    let r = c.reply().ok()?;
    (r.format == 8 && !r.value.is_empty()).then(|| String::from_utf8_lossy(&r.value).into_owned())
}

/// Extents property (left, right, top, bottom).
fn extents(c: PropCookie<'_>) -> Option<[u32; 4]> {
    let v = u32s(c);
    (v.len() >= 4).then(|| [v[0], v[1], v[2], v[3]])
}

/// Computes the visible window rect from the client area and frame/CSD extents.
pub(crate) fn visible_rect(
    client: (i32, i32, u32, u32),
    net_frame: Option<[u32; 4]>,
    gtk_frame: Option<[u32; 4]>,
) -> IRect {
    let (x, y, w, h) = client;
    if let Some([l, r, t, b]) = gtk_frame {
        // CSD: the client window includes an invisible shadow margin.
        let w = w.saturating_sub(l + r);
        let h = h.saturating_sub(t + b);
        IRect::new(x + l as i32, y + t as i32, w, h)
    } else if let Some([l, r, t, b]) = net_frame {
        // Server-side decorations around the client.
        IRect::new(x - l as i32, y - t as i32, w + l + r, h + t + b)
    } else {
        IRect::new(x, y, w, h)
    }
}

/// Visible top-level windows on the current desktop, topmost first; docks/desktop last.
pub fn list_windows(ctx: &Ctx, root: IRect) -> Result<Vec<WindowInfo>> {
    let conn = &ctx.conn;
    let a = &ctx.atoms;
    let stacking = prop(conn, ctx.root, a._NET_CLIENT_LIST_STACKING)?;
    let current = prop(conn, ctx.root, a._NET_CURRENT_DESKTOP)?;
    let mut clients = u32s(stacking);
    let current_desktop = u32s(current).first().copied();
    clients.reverse(); // stacking order is bottom-to-top

    struct Pending<'a> {
        id: Window,
        geom: x11rb::cookie::Cookie<'a, RustConnection, x11rb::protocol::xproto::GetGeometryReply>,
        trans: x11rb::cookie::Cookie<'a, RustConnection, x11rb::protocol::xproto::TranslateCoordinatesReply>,
        attrs: x11rb::cookie::Cookie<'a, RustConnection, x11rb::protocol::xproto::GetWindowAttributesReply>,
        tree: x11rb::cookie::Cookie<'a, RustConnection, x11rb::protocol::xproto::QueryTreeReply>,
        desktop: PropCookie<'a>,
        state: PropCookie<'a>,
        wtype: PropCookie<'a>,
        net_frame: PropCookie<'a>,
        gtk_frame: PropCookie<'a>,
        net_name: PropCookie<'a>,
        name: PropCookie<'a>,
        class: PropCookie<'a>,
    }

    let mut pending = Vec::with_capacity(clients.len());
    for &id in &clients {
        pending.push(Pending {
            id,
            geom: conn.get_geometry(id)?,
            trans: conn.translate_coordinates(id, ctx.root, 0, 0)?,
            attrs: conn.get_window_attributes(id)?,
            tree: conn.query_tree(id)?,
            desktop: prop(conn, id, a._NET_WM_DESKTOP)?,
            state: prop(conn, id, a._NET_WM_STATE)?,
            wtype: prop(conn, id, a._NET_WM_WINDOW_TYPE)?,
            net_frame: prop(conn, id, a._NET_FRAME_EXTENTS)?,
            gtk_frame: prop(conn, id, a._GTK_FRAME_EXTENTS)?,
            net_name: prop(conn, id, a._NET_WM_NAME)?,
            name: prop(conn, id, AtomEnum::WM_NAME.into())?,
            class: prop(conn, id, AtomEnum::WM_CLASS.into())?,
        });
    }

    struct Found {
        info: WindowInfo,
        bottom: bool,
        child_ids: Vec<Window>,
        origin: (i32, i32),
    }
    let mut found = Vec::new();
    for p in pending {
        let (Ok(geom), Ok(trans), Ok(attrs)) = (p.geom.reply(), p.trans.reply(), p.attrs.reply()) else {
            continue; // window vanished
        };
        let tree = p.tree.reply().ok();
        let desktop = u32s(p.desktop).first().copied();
        let state = u32s(p.state);
        let wtype = u32s(p.wtype);
        let net_frame = extents(p.net_frame);
        let gtk_frame = extents(p.gtk_frame);
        let title = text(p.net_name).or_else(|| text(p.name)).unwrap_or_default();
        let class = text(p.class)
            .map(|s| s.split('\0').nth(1).unwrap_or("").to_string())
            .unwrap_or_default();

        if attrs.map_state != MapState::VIEWABLE || state.contains(&a._NET_WM_STATE_HIDDEN) {
            continue;
        }
        if let (Some(d), Some(cur)) = (desktop, current_desktop)
            && d != cur
            && d != 0xFFFF_FFFF
        {
            continue;
        }
        let origin = (trans.dst_x as i32, trans.dst_y as i32);
        let rect = visible_rect((origin.0, origin.1, geom.width as u32, geom.height as u32), net_frame, gtk_frame);
        let Some(rect) = rect.intersect(&root) else { continue };
        let bottom = wtype.contains(&a._NET_WM_WINDOW_TYPE_DOCK) || wtype.contains(&a._NET_WM_WINDOW_TYPE_DESKTOP);
        found.push(Found {
            info: WindowInfo { id: p.id, rect, title, class, children: Vec::new() },
            bottom,
            child_ids: tree.map(|t| t.children).unwrap_or_default(),
            origin,
        });
    }

    // Second round-trip batch: mapped child windows.
    let child_cookies: Vec<Vec<_>> = found
        .iter()
        .map(|f| {
            f.child_ids
                .iter()
                .filter_map(|&c| Some((conn.get_window_attributes(c).ok()?, conn.get_geometry(c).ok()?)))
                .collect()
        })
        .collect();
    for (f, cookies) in found.iter_mut().zip(child_cookies) {
        // query_tree lists bottom-to-top; keep topmost first like the window list.
        for (attrs, geom) in cookies.into_iter().rev() {
            let (Ok(attrs), Ok(geom)) = (attrs.reply(), geom.reply()) else { continue };
            if attrs.map_state != MapState::VIEWABLE || attrs.class == x11rb::protocol::xproto::WindowClass::INPUT_ONLY {
                continue;
            }
            let bw = geom.border_width as i32;
            let r = IRect::new(
                f.origin.0 + geom.x as i32 + bw,
                f.origin.1 + geom.y as i32 + bw,
                geom.width as u32,
                geom.height as u32,
            );
            if r.w <= MIN_CHILD || r.h <= MIN_CHILD {
                continue;
            }
            if let Some(r) = r.intersect(&f.info.rect)
                && r != f.info.rect
            {
                f.info.children.push(r);
            }
        }
    }

    let (bottom, mut normal): (Vec<_>, Vec<_>) = found.into_iter().partition(|f| f.bottom);
    normal.extend(bottom);
    Ok(normal.into_iter().map(|f| f.info).collect())
}

pub fn activate_window(ctx: &Ctx, xid: u32) -> Result<()> {
    let conn = &ctx.conn;
    let attrs = conn.get_window_attributes(xid)?.reply()?;
    if !attrs.override_redirect {
        // Source indication 2 ("pager") bypasses focus-stealing prevention in most WMs.
        let ev = ClientMessageEvent::new(32, xid, ctx.atoms._NET_ACTIVE_WINDOW, [2, CURRENT_TIME, 0, 0, 0]);
        conn.send_event(
            false,
            ctx.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            ev,
        )?;
    }
    conn.configure_window(xid, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
    conn.flush()?;

    let deadline = Instant::now() + Duration::from_millis(120);
    loop {
        // BadMatch if not viewable yet: ignore and retry.
        let _ = conn.set_input_focus(InputFocus::PARENT, xid, CURRENT_TIME)?.check();
        let focus = conn.get_input_focus()?.reply()?.focus;
        if focus == xid {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("could not focus window {xid:#x} (focus is {focus:#x})");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_csd_and_ssd() {
        assert_eq!(visible_rect((100, 100, 800, 600), None, Some([20, 20, 10, 30])), IRect::new(120, 110, 760, 560));
        assert_eq!(visible_rect((100, 100, 800, 600), Some([1, 1, 30, 1]), None), IRect::new(99, 70, 802, 631));
        assert_eq!(visible_rect((5, 6, 7, 8), None, None), IRect::new(5, 6, 7, 8));
    }
}
