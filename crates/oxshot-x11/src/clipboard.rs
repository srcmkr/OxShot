//! Clipboard owner that offers a screenshot the way every target app likes it: as
//! `image/png` (chat, mail, image editors), as the saved file (`text/uri-list` and
//! `x-special/gnome-copied-files`, so pasting into a file manager copies the file) and as
//! its path (`UTF8_STRING`, e.g. for a terminal). Large data goes out with the INCR
//! protocol. Each copy runs its own thread and connection until another program takes
//! the clipboard over (idea from the Anything project's `copy_files`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use x11rb::connection::{Connection, RequestConnection as _};
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Property,
    SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// Above this size, data is sent incrementally (ICCCM INCR).
const INCR_CHUNK: usize = 256 * 1024;

/// `file:///…` with every byte that needs it percent-escaped.
pub fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

struct Atoms {
    clipboard: u32,
    targets: u32,
    timestamp: u32,
    incr: u32,
    png: u32,
    uri_list: u32,
    gnome: u32,
    utf8: u32,
    text_plain: u32,
}

impl Atoms {
    fn new(conn: &RustConnection) -> Result<Atoms> {
        let a = |name: &[u8]| -> Result<u32> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
        Ok(Atoms {
            clipboard: a(b"CLIPBOARD")?,
            targets: a(b"TARGETS")?,
            timestamp: a(b"TIMESTAMP")?,
            incr: a(b"INCR")?,
            png: a(b"image/png")?,
            uri_list: a(b"text/uri-list")?,
            gnome: a(b"x-special/gnome-copied-files")?,
            utf8: a(b"UTF8_STRING")?,
            text_plain: a(b"text/plain;charset=utf-8")?,
        })
    }
}

/// A transfer in progress: remaining data for one (requestor, property).
struct Incr {
    target: u32,
    data: std::sync::Arc<Vec<u8>>,
    offset: usize,
}

/// Puts an image and/or a file on the clipboard. `image` is PNG-encoded on the clipboard
/// thread, so this returns immediately; pastes that arrive meanwhile wait for it.
pub fn set(image: Option<image::RgbaImage>, file: Option<PathBuf>) -> Result<()> {
    if image.is_none() && file.is_none() {
        bail!("nothing to copy");
    }
    let (conn, screen) = x11rb::connect(None).context("cannot connect to X server")?;
    let root = conn.setup().roots[screen].root;
    let atoms = Atoms::new(&conn)?;
    let owner = conn.generate_id()?;
    conn.create_window(0, owner, root, 0, 0, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new())?;
    // A real timestamp is required by ICCCM; get one from a zero-length property change.
    conn.change_window_attributes(owner, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE))?;
    conn.change_property8(PropMode::APPEND, owner, AtomEnum::WM_NAME, AtomEnum::STRING, &[])?;
    conn.flush()?;
    let time = loop {
        if let Event::PropertyNotify(e) = conn.wait_for_event()? {
            break e.time;
        }
    };
    conn.set_selection_owner(owner, atoms.clipboard, time)?;
    conn.flush()?;
    if conn.get_selection_owner(atoms.clipboard)?.reply()?.owner != owner {
        bail!("the clipboard is busy");
    }

    std::thread::Builder::new().name("oxshot-clipboard".into()).spawn(move || {
        if let Err(e) = serve(&conn, owner, time, &atoms, image, file) {
            log::warn!("clipboard: {e:#}");
        }
        let _ = conn.destroy_window(owner);
        let _ = conn.flush();
    })?;
    Ok(())
}

fn serve(
    conn: &RustConnection,
    owner: u32,
    time: u32,
    atoms: &Atoms,
    image: Option<image::RgbaImage>,
    file: Option<PathBuf>,
) -> Result<()> {
    let mut offers: Vec<(u32, std::sync::Arc<Vec<u8>>)> = Vec::new();
    if let Some(img) = image {
        let t0 = std::time::Instant::now();
        let mut png = Vec::new();
        let enc = image::codecs::png::PngEncoder::new_with_quality(
            &mut png,
            image::codecs::png::CompressionType::Fast,
            image::codecs::png::FilterType::Adaptive,
        );
        image::ImageEncoder::write_image(enc, img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)?;
        log::debug!("clipboard png {} KiB in {:?}", png.len() / 1024, t0.elapsed());
        offers.push((atoms.png, std::sync::Arc::new(png)));
    }
    if let Some(path) = &file {
        let uri = file_uri(path);
        let text = path.display().to_string();
        offers.push((atoms.uri_list, std::sync::Arc::new(format!("{uri}\r\n").into_bytes())));
        offers.push((atoms.gnome, std::sync::Arc::new(format!("copy\n{uri}").into_bytes())));
        offers.push((atoms.utf8, std::sync::Arc::new(text.clone().into_bytes())));
        offers.push((atoms.text_plain, std::sync::Arc::new(text.into_bytes())));
    }
    let max_direct = INCR_CHUNK.min(conn.maximum_request_bytes().saturating_sub(1024));
    let mut transfers: HashMap<(u32, u32), Incr> = HashMap::new();

    loop {
        match conn.wait_for_event()? {
            Event::SelectionRequest(req) => {
                let property = answer(conn, &req, time, atoms, &offers, max_direct, &mut transfers)?;
                let notify = SelectionNotifyEvent {
                    response_type: SELECTION_NOTIFY_EVENT,
                    sequence: 0,
                    time: req.time,
                    requestor: req.requestor,
                    selection: req.selection,
                    target: req.target,
                    property,
                };
                conn.send_event(false, req.requestor, EventMask::NO_EVENT, notify)?;
                conn.flush()?;
            }
            // The requestor deleted the property: send the next chunk.
            Event::PropertyNotify(e) if e.state == Property::DELETE => {
                let key = (e.window, e.atom);
                if let Some(t) = transfers.get_mut(&key) {
                    let end = (t.offset + INCR_CHUNK).min(t.data.len());
                    conn.change_property8(PropMode::REPLACE, e.window, e.atom, t.target, &t.data[t.offset..end])?;
                    if t.offset == end {
                        // The zero-length chunk ending the transfer has been written.
                        transfers.remove(&key);
                    } else {
                        t.offset = end;
                    }
                    conn.flush()?;
                }
            }
            // Someone else copied something.
            Event::SelectionClear(e) if e.owner == owner => return Ok(()),
            _ => {}
        }
    }
}

/// Fills the requested property; returns it, or NONE if the target is not offered.
fn answer(
    conn: &RustConnection,
    req: &SelectionRequestEvent,
    time: u32,
    atoms: &Atoms,
    offers: &[(u32, std::sync::Arc<Vec<u8>>)],
    max_direct: usize,
    transfers: &mut HashMap<(u32, u32), Incr>,
) -> Result<u32> {
    // Obsolete clients pass NONE: use the target as property name.
    let property = if req.property == x11rb::NONE { req.target } else { req.property };
    if req.target == atoms.targets {
        let mut list = vec![atoms.targets, atoms.timestamp];
        list.extend(offers.iter().map(|(a, _)| *a));
        conn.change_property32(PropMode::REPLACE, req.requestor, property, AtomEnum::ATOM, &list)?;
        return Ok(property);
    }
    if req.target == atoms.timestamp {
        conn.change_property32(PropMode::REPLACE, req.requestor, property, AtomEnum::INTEGER, &[time])?;
        return Ok(property);
    }
    let Some((_, data)) = offers.iter().find(|(a, _)| *a == req.target) else {
        return Ok(x11rb::NONE);
    };
    if data.len() <= max_direct {
        conn.change_property8(PropMode::REPLACE, req.requestor, property, req.target, data)?;
    } else {
        conn.change_window_attributes(req.requestor, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE))?;
        conn.change_property32(PropMode::REPLACE, req.requestor, property, atoms.incr, &[data.len() as u32])?;
        transfers.insert((req.requestor, property), Incr { target: req.target, data: data.clone(), offset: 0 });
    }
    Ok(property)
}

#[cfg(test)]
mod tests {
    #[test]
    fn uri_escaping() {
        assert_eq!(super::file_uri(std::path::Path::new("/a b/ü.png")), "file:///a%20b/%C3%BC.png");
    }
}
