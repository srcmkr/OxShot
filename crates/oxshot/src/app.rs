//! Application orchestration: background process, hotkeys, capture flow, windows.

use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt;
use gpui_kit::*;
use oxshot_core::{Document, IRect};

use crate::settings::{self, AfterCapture};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    /// Interactive all-in-one selection.
    Region,
    /// Same rectangle as the previous capture.
    LastRegion,
    /// Monitor under the pointer, immediately.
    Fullscreen,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Capture(CaptureMode),
    Open(PathBuf),
    PickColor,
    /// Region capture after a countdown (`None` = delay from the settings).
    Delayed(Option<u32>),
    Settings,
    Workspace,
    Quit,
}

impl Command {
    pub fn to_line(&self) -> String {
        match self {
            Command::Capture(CaptureMode::Region) => "capture".into(),
            Command::Capture(CaptureMode::LastRegion) => "capture-last".into(),
            Command::Capture(CaptureMode::Fullscreen) => "capture-full".into(),
            Command::Open(p) => format!("open {}", p.display()),
            Command::Settings => "settings".into(),
            Command::PickColor => "pick".into(),
            Command::Delayed(None) => "capture-delay".into(),
            Command::Delayed(Some(n)) => format!("capture-delay {n}"),
            Command::Workspace => "workspace".into(),
            Command::Quit => "quit".into(),
        }
    }
    pub fn from_line(s: &str) -> Option<Command> {
        Some(match s {
            "capture" => Command::Capture(CaptureMode::Region),
            "capture-last" => Command::Capture(CaptureMode::LastRegion),
            "capture-full" => Command::Capture(CaptureMode::Fullscreen),
            "quit" => Command::Quit,
            "settings" => Command::Settings,
            "pick" => Command::PickColor,
            "capture-delay" => Command::Delayed(None),
            s if s.starts_with("capture-delay ") => Command::Delayed(s["capture-delay ".len()..].trim().parse().ok()),
            "workspace" => Command::Workspace,
            _ => Command::Open(PathBuf::from(s.strip_prefix("open ")?)),
        })
    }
}

/// Keeps process-wide services alive.
pub struct Services {
    hotkeys: Option<oxshot_x11::HotkeyService>,
    /// Hotkeys that could not be grabbed (taken by another program), with the reason.
    pub hotkey_errors: Vec<(String, String)>,
    pub tx: futures::channel::mpsc::UnboundedSender<Command>,
    /// An overlay is open: ignore further capture requests.
    capturing: bool,
}
impl Global for Services {}

pub fn run(initial: Vec<Command>) {
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<Command>();
    if let Err(e) = crate::ipc::listen(tx.clone()) {
        log::error!("{e:#}");
        return;
    }

    gpui_kit::application().with_assets(crate::assets::Assets).with_quit_mode(QuitMode::Explicit).run(move |cx| {
        gpui_kit::init(cx);
        let s = settings::load();
        crate::storage::set_save_dir(s.save_dir.clone());
        cx.set_global(settings::SettingsGlobal(s));
        crate::i18n::apply(cx);
        crate::theme::apply(cx);
        cx.set_global(crate::editor::SharedRenderer(Default::default()));

        cx.set_global(Services { hotkeys: None, hotkey_errors: Vec::new(), tx: tx.clone(), capturing: false });
        set_hotkeys_paused(false, cx);
        prewarm_overlay(cx);
        crate::update::init(cx);
        if crate::update::first_start_after_update(cx) {
            crate::notify::updated(crate::update::VERSION);
        }
        // Keep autostart/menu entries pointing at this program (off the UI thread).
        std::thread::spawn(crate::desktop::sync);
        crate::tray::spawn(tx.clone());

        for c in initial {
            let _ = tx.unbounded_send(c);
        }
        cx.spawn(async move |cx| {
            while let Some(cmd) = rx.next().await {
                cx.update(|cx| handle(cmd, cx));
            }
        })
        .detach();
    });
    crate::storage::flush();
    crate::ipc::cleanup();
}

/// Stops the global hotkey grabs (while the settings window records a new hotkey) or
/// (re)starts them from the current settings.
pub fn set_hotkeys_paused(paused: bool, cx: &mut App) {
    if let Some(svc) = cx.global_mut::<Services>().hotkeys.take() {
        svc.stop();
    }
    if paused {
        return;
    }
    let tx = cx.global::<Services>().tx.clone();
    let hk = settings::get(cx).hotkeys.clone();
    let mut bindings = Vec::new();
    let mut failed = Vec::new();
    for (i, s) in [(0u32, &hk.region), (1, &hk.last_region), (2, &hk.fullscreen), (3, &hk.pick_color), (4, &hk.delayed)] {
        if s.is_empty() {
            continue;
        }
        match s.parse::<oxshot_x11::Hotkey>() {
            Ok(h) => bindings.push((h, i)),
            Err(e) => failed.push((s.clone(), format!("{e:#}"))),
        }
    }
    let on_press = move |id: u32| {
        if id == 3 {
            let _ = tx.unbounded_send(Command::PickColor);
            return;
        }
        if id == 4 {
            let _ = tx.unbounded_send(Command::Delayed(None));
            return;
        }
        let mode = match id {
            1 => CaptureMode::LastRegion,
            2 => CaptureMode::Fullscreen,
            _ => CaptureMode::Region,
        };
        let _ = tx.unbounded_send(Command::Capture(mode));
    };
    let svc = match oxshot_x11::HotkeyService::start(bindings, on_press) {
        Ok((svc, errors)) => {
            for (h, e) in errors {
                log::warn!("hotkey {h} not available: {e:#} – bind `oxshot capture` in the desktop's keyboard settings instead");
                failed.push((h.to_string(), format!("{e:#}")));
            }
            Some(svc)
        }
        Err(e) => {
            log::warn!("global hotkeys unavailable: {e:#}");
            failed.push(("*".into(), format!("{e:#}")));
            None
        }
    };
    let services = cx.global_mut::<Services>();
    services.hotkeys = svc;
    services.hotkey_errors = failed;
}

/// Quits the normal way: open editors are saved, pending writes finished.
pub fn quit(cx: &mut App) {
    crate::editor::save_all(cx);
    crate::storage::flush();
    cx.quit();
}

fn handle(cmd: Command, cx: &mut App) {
    match cmd {
        Command::Capture(mode) => capture(mode, cx),
        Command::Open(path) => match crate::storage::open(&path) {
            Ok((doc, file)) => {
                if let Err(e) = crate::editor::open(doc, file, cx) {
                    log::error!("cannot open editor: {e:#}");
                }
            }
            Err(e) => log::error!("cannot open {}: {e:#}", path.display()),
        },
        Command::Settings => crate::settings_window::open(cx),
        Command::PickColor => pick_color(None, cx),
        Command::Delayed(n) => {
            let n = n.unwrap_or(settings::get(cx).capture_delay);
            crate::countdown::start(n, cx);
        }
        Command::Workspace => crate::workspace::open(cx),
        Command::Quit => quit(cx),
    }
}

pub(crate) fn capture(mode: CaptureMode, cx: &mut App) {
    if cx.global::<Services>().capturing {
        return;
    }
    let t0 = std::time::Instant::now();
    let Some(snap) = take_snapshot() else { return };
    let direct = match mode {
        CaptureMode::Region => None,
        CaptureMode::LastRegion => Some(settings::get(cx).last_region.unwrap_or_else(|| monitor_at_pointer(&snap))),
        CaptureMode::Fullscreen => Some(monitor_at_pointer(&snap)),
    };
    if let Some(rect) = direct {
        let include_cursor = settings::get(cx).capture_cursor;
        finish_capture(rect, include_cursor, &Arc::new(snap), cx);
        return;
    }
    show_overlay(crate::overlay::Mode::Capture, snap, t0, Box::new(|outcome, snap, cx| {
        if let Some(crate::overlay::Outcome::Region(sel)) = outcome {
            finish_capture(sel.rect, sel.include_cursor, &snap, cx);
        }
    }), cx);
}

/// Called with the picked color when the picker was started by an editor.
pub type OnColor = Box<dyn FnOnce([u8; 3], &mut App)>;

/// Screen color picker: frozen screen with a big magnifier, a click picks the pixel.
/// The color goes to the clipboard as `#RRGGBB`, into the recent colors and, if given,
/// to `on_color` (e.g. the editor that asked for it).
pub(crate) fn pick_color(on_color: Option<OnColor>, cx: &mut App) {
    if cx.global::<Services>().capturing {
        return;
    }
    let t0 = std::time::Instant::now();
    let Some(snap) = take_snapshot() else { return };
    show_overlay(crate::overlay::Mode::PickColor, snap, t0, Box::new(move |outcome, _, cx| {
        if let Some(crate::overlay::Outcome::Color(rgb)) = outcome {
            picked(rgb, on_color, cx);
        }
    }), cx);
}

fn picked([r, g, b]: [u8; 3], on_color: Option<OnColor>, cx: &mut App) {
    let hex = format!("#{r:02X}{g:02X}{b:02X}");
    if let Err(e) = crate::clipboard::set_text(&hex) {
        log::warn!("clipboard: {e:#}");
    }
    settings::update(cx, |s| {
        let c = oxshot_core::Color::rgb(r, g, b);
        s.recent_colors.retain(|x| *x != c);
        s.recent_colors.insert(0, c);
        s.recent_colors.truncate(8);
    });
    match on_color {
        Some(f) => f([r, g, b], cx),
        None => crate::notify::color_picked([r, g, b], &hex),
    }
}

fn take_snapshot() -> Option<oxshot_x11::Snapshot> {
    let t0 = std::time::Instant::now();
    match oxshot_x11::snapshot() {
        Ok(s) => {
            log::info!("snapshot {}x{} in {:?}", s.image.width(), s.image.height(), t0.elapsed());
            Some(s)
        }
        Err(e) => {
            log::error!("screen capture failed: {e:#}");
            None
        }
    }
}

/// Shows the overlay: the pre-created window if it fits the screen, else a fresh one.
fn show_overlay(mode: crate::overlay::Mode, snap: oxshot_x11::Snapshot, t0: std::time::Instant, done: crate::overlay::OnDone, cx: &mut App) {
    cx.global_mut::<Services>().capturing = true;
    let on_done: crate::overlay::OnDone = Box::new(move |outcome, snap, cx| {
        cx.global_mut::<Services>().capturing = false;
        done(outcome, snap, cx);
    });

    // Fast path: the pre-created overlay window, if it matches the current screen size.
    let size = (snap.image.width(), snap.image.height());
    let warm = cx.try_global::<WarmOverlay>().filter(|w| w.size == size).map(|w| w.handle);
    let idle = warm.and_then(|h| h.read_with(cx, |o, _| o.is_idle()).ok()).unwrap_or(false);
    if let (Some(handle), true) = (warm, idle) {
        let include_cursor = settings::get(cx).capture_cursor;
        let mut args = Some((snap, on_done));
        let res = handle.update(cx, |o, window, cx| {
            let (snap, on_done) = args.take().expect("taken once");
            let req = crate::overlay::Request { snap, started: t0, include_cursor, mode, on_done };
            let xid = o.begin(req, window, cx);
            xid.map(|xid| oxshot_x11::move_window(xid, 0, 0).and_then(|_| oxshot_x11::map_window(xid)))
        });
        match res {
            Ok(Some(Ok(()))) => return,
            other => {
                // Should not happen; fall back to a fresh window next time.
                log::warn!("reusable overlay failed ({:?}), recreating it", other.map(|r| r.map(|r| r.err())));
                cx.remove_global::<WarmOverlay>();
                cx.global_mut::<Services>().capturing = false;
                prewarm_overlay(cx);
                return;
            }
        }
    }

    if let Err(e) = crate::overlay::open(snap, t0, mode, on_done, cx) {
        cx.global_mut::<Services>().capturing = false;
        log::error!("cannot open capture overlay: {e:#}");
    }
    // The screen size changed (or there was no warm window yet): prepare one for next time.
    prewarm_overlay(cx);
}

/// The pre-created overlay window and the root size it was made for.
struct WarmOverlay {
    handle: WindowHandle<crate::overlay::Overlay>,
    size: (u32, u32),
}
impl Global for WarmOverlay {}

/// Creates the reusable overlay window (again, if the screen size changed).
fn prewarm_overlay(cx: &mut App) {
    let size = match oxshot_x11::root_size() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("cannot pre-create the capture overlay: {e:#}");
            return;
        }
    };
    if cx.try_global::<WarmOverlay>().is_some_and(|w| w.size == size) {
        return;
    }
    if let Some(old) = cx.try_global::<WarmOverlay>().map(|w| w.handle) {
        old.update(cx, |_, window, _| window.remove_window()).ok();
    }
    match crate::overlay::prewarm(size, cx) {
        Ok(handle) => {
            log::debug!("capture overlay pre-created for {}x{}", size.0, size.1);
            cx.set_global(WarmOverlay { handle, size });
        }
        Err(e) => log::warn!("cannot pre-create the capture overlay: {e:#}"),
    }
}

fn monitor_at_pointer(snap: &oxshot_x11::Snapshot) -> IRect {
    let (x, y) = snap.pointer;
    snap.monitors
        .iter()
        .find(|m| m.rect.contains(x, y))
        .or_else(|| snap.monitors.iter().find(|m| m.primary))
        .map(|m| m.rect)
        .unwrap_or(IRect::new(0, 0, snap.image.width(), snap.image.height()))
}

fn finish_capture(rect: IRect, include_cursor: bool, snap: &oxshot_x11::Snapshot, cx: &mut App) {
    let root = IRect::new(0, 0, snap.image.width(), snap.image.height());
    let Some(rect) = rect.intersect(&root) else { return };
    let mut img = image::imageops::crop_imm(&snap.image, rect.x as u32, rect.y as u32, rect.w, rect.h).to_image();
    if include_cursor && let Some(c) = &snap.cursor {
        image::imageops::overlay(&mut img, &c.image, (c.x - rect.x) as i64, (c.y - rect.y) as i64);
    }
    settings::update(cx, |s| s.last_region = Some(rect));

    let doc = Document::new(img);
    let s = settings::get(cx).clone();
    let file = match crate::storage::save_capture(&doc, &doc.base, None) {
        Ok(p) => Some(p),
        Err(e) => {
            log::error!("saving capture failed: {e:#}");
            None
        }
    };
    if s.copy_on_capture
        && let Err(e) = crate::clipboard::set_capture(&doc.base, file.as_deref())
    {
        log::warn!("clipboard: {e:#}");
    }
    if s.after_capture == AfterCapture::Editor
        && let Err(e) = crate::editor::open(doc, file, cx)
    {
        log::error!("cannot open editor: {e:#}");
    }
}
