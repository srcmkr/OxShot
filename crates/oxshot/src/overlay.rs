//! The "all in one" capture overlay: the screen is frozen, the window under the pointer is
//! highlighted (click = capture it), dragging selects a region, a pixel magnifier shows
//! the exact position. Releasing the mouse captures immediately (Screenpresso workflow).
//!
//! The same overlay is the screen color picker: a big round magnifier follows the pointer
//! and a click picks the pixel under it.
//!
//! The window is created once at start-up and only mapped/unmapped per capture (idea from
//! the Anything project), so a capture does not pay for window and GPU surface creation.

use std::sync::Arc;
use rust_i18n::t;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use oxshot_core::IRect;
use oxshot_x11::Snapshot;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::gfx::{TiledImage, ViewMap};

/// Result handed to the app when the user finished selecting.
pub struct Selection {
    pub rect: IRect,
    pub include_cursor: bool,
}

/// What the overlay is used for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Capture,
    PickColor,
}

/// What the user chose.
pub enum Outcome {
    Region(Selection),
    Color([u8; 3]),
}

pub type OnDone = Box<dyn FnOnce(Option<Outcome>, Arc<Snapshot>, &mut App)>;

/// Everything a capture session in the pre-created window needs.
pub struct Request {
    pub snap: Snapshot,
    /// When the capture was requested (for the latency log).
    pub started: std::time::Instant,
    pub include_cursor: bool,
    pub mode: Mode,
    pub on_done: OnDone,
}

pub struct Overlay {
    snap: Arc<Snapshot>,
    /// Root position of the overlay window (normally 0,0).
    origin: (i32, i32),
    tiles: Option<TiledImage>,
    /// Pointer in root coordinates (sub-pixel precision ignored).
    pointer: (i32, i32),
    drag_start: Option<(i32, i32)>,
    include_cursor: bool,
    show_help: bool,
    mag_zoom: u32,
    mag: Option<Arc<RenderImage>>,
    focus: FocusHandle,
    on_done: Option<OnDone>,
    activated: bool,
    /// When the capture was requested (for the latency log).
    started: std::time::Instant,
    /// Pre-created window: hide it after a capture instead of closing it.
    reusable: bool,
    /// No capture in progress (the pre-created window waits unmapped).
    idle: bool,
    mode: Mode,
}

/// Source pixels shown by the magnifier (odd, so there is a center pixel).
const MAG_SRC: u32 = 21;
const PICK_SRC: u32 = 17;

impl Overlay {
    fn hovered(&self) -> IRect {
        let (x, y) = self.pointer;
        // Top edge of a monitor = whole monitor (Screenpresso).
        if let Some(m) = self.snap.monitors.iter().find(|m| m.rect.contains(x, y))
            && y <= m.rect.y + 1
        {
            return m.rect;
        }
        if let Some(w) = self.snap.windows.iter().find(|w| w.rect.contains(x, y)) {
            if let Some(c) = w.children.iter().filter(|c| c.contains(x, y)).min_by_key(|c| c.w * c.h) {
                return *c;
            }
            return clip_to_root(w.rect, &self.snap);
        }
        self.snap.monitors.iter().find(|m| m.rect.contains(x, y)).map(|m| m.rect).unwrap_or(IRect::new(
            0,
            0,
            self.snap.image.width(),
            self.snap.image.height(),
        ))
    }

    fn selection(&self) -> IRect {
        match self.drag_start {
            Some((sx, sy)) => {
                let (x, y) = self.pointer;
                let (x0, y0, x1, y1) = (sx.min(x), sy.min(y), sx.max(x) + 1, sy.max(y) + 1);
                IRect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
            }
            None => self.hovered(),
        }
    }

    fn mag_src(&self) -> u32 {
        if self.mode == Mode::PickColor { PICK_SRC } else { MAG_SRC }
    }

    fn pixel(&self) -> Option<[u8; 3]> {
        let (x, y) = self.pointer;
        let img = &self.snap.image;
        (x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height()).then(|| {
            let p = img.get_pixel(x as u32, y as u32);
            [p[0], p[1], p[2]]
        })
    }

    fn pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let color = self.pixel();
        self.finish_with(color.map(Outcome::Color), window, cx);
    }

    fn is_dragging(&self) -> bool {
        self.drag_start.is_some_and(|(sx, sy)| (sx - self.pointer.0).abs() > 3 || (sy - self.pointer.1).abs() > 3)
    }

    /// Starts a capture in the pre-created window. Returns the X window id to map.
    pub fn begin(&mut self, req: Request, window: &mut Window, cx: &mut Context<Self>) -> Option<u32> {
        let Request { snap, started, include_cursor, mode, on_done } = req;
        self.mode = mode;
        self.mag_zoom = if mode == Mode::PickColor { 12 } else { 8 };
        self.pointer = snap.pointer;
        self.snap = Arc::new(snap);
        self.drag_start = None;
        self.include_cursor = include_cursor;
        self.show_help = false;
        self.on_done = Some(on_done);
        self.activated = false;
        self.started = started;
        self.idle = false;
        window.focus(&self.focus, cx);
        cx.notify();
        crate::icon::xid(window)
    }

    pub fn is_idle(&self) -> bool {
        self.idle
    }

    fn finish(&mut self, rect: Option<IRect>, window: &mut Window, cx: &mut Context<Self>) {
        let sel = rect.and_then(|r| clip_to_root(r, &self.snap).w.gt(&0).then_some(r)).map(|r| {
            Outcome::Region(Selection { rect: clip_to_root(r, &self.snap), include_cursor: self.include_cursor })
        });
        self.finish_with(sel, window, cx);
    }

    fn finish_with(&mut self, sel: Option<Outcome>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.tiles.take() {
            t.release(window);
        }
        if let Some(m) = self.mag.take() {
            let _ = window.drop_image(m);
        }
        // The callback needs the real screenshot; the reusable window drops it afterwards.
        let snap = std::mem::replace(&mut self.snap, Arc::new(empty_snapshot()));
        if self.reusable {
            self.idle = true;
            if let Some(xid) = crate::icon::xid(window)
                && let Err(e) = oxshot_x11::unmap_window(xid)
            {
                log::warn!("overlay hide: {e:#}");
            }
        } else {
            window.remove_window();
        }
        if let Some(done) = self.on_done.take() {
            cx.defer(move |cx| done(sel, snap, cx));
        }
    }

    fn to_root(&self, p: Point<Pixels>, window: &Window) -> (i32, i32) {
        let s = window.scale_factor();
        (
            (f32::from(p.x) * s).floor() as i32 + self.origin.0,
            (f32::from(p.y) * s).floor() as i32 + self.origin.1,
        )
    }

    fn update_magnifier(&mut self, window: &mut Window) {
        if let Some(m) = self.mag.take() {
            let _ = window.drop_image(m);
        }
        let z = self.mag_zoom;
        let n = self.mag_src();
        let img = &self.snap.image;
        let mut out = image::RgbaImage::new(n * z, n * z);
        let (cx, cy) = self.pointer;
        let half = (n / 2) as i32;
        for sy in 0..n {
            for sx in 0..n {
                let (x, y) = (cx - half + sx as i32, cy - half + sy as i32);
                let p = if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
                    *img.get_pixel(x as u32, y as u32)
                } else {
                    image::Rgba([0, 0, 0, 255])
                };
                for dy in 0..z {
                    for dx in 0..z {
                        // Faint pixel grid.
                        let grid = z >= 6 && (dx == 0 || dy == 0);
                        let q = if grid { image::Rgba([p[0] / 8 * 7, p[1] / 8 * 7, p[2] / 8 * 7, 255]) } else { p };
                        out.put_pixel(sx * z + dx, sy * z + dy, q);
                    }
                }
            }
        }
        self.mag = Some(crate::gfx::to_render_image(&out, 0, 0, out.width(), out.height()));
    }
}

fn clip_to_root(r: IRect, snap: &Snapshot) -> IRect {
    r.intersect(&IRect::new(0, 0, snap.image.width(), snap.image.height())).unwrap_or_default()
}

fn empty_snapshot() -> Snapshot {
    Snapshot { image: image::RgbaImage::new(1, 1), monitors: Vec::new(), windows: Vec::new(), cursor: None, pointer: (0, 0) }
}

impl Render for Overlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.idle {
            // Paint a 1×1 image while idle so the first real capture finds the sprite
            // pipeline and atlas ready.
            let warm = self.tiles.get_or_insert_with(|| TiledImage::new(&image::RgbaImage::new(1, 1), (0, 0))).tiles[0].image.clone();
            return div()
                .id("overlay")
                .track_focus(&self.focus)
                .size_full()
                .child(
                    canvas(|_, _, _| {}, move |b, _, window, _| {
                        let r = Bounds { origin: b.origin, size: gpui_kit::size(px(1.0), px(1.0)) };
                        let _ = window.paint_image(r, r, Corners::default(), warm.clone(), 0, false);
                    })
                    .size_full(),
                )
                .into_any_element();
        }
        if self.tiles.as_ref().is_some_and(|t| t.width == 1 && t.height == 1)
            && let Some(t) = self.tiles.take()
        {
            t.release(window);
        }
        if !self.activated {
            self.activated = true;
            log::info!("overlay first frame {:?} after request", self.started.elapsed());
            // Override-redirect windows don't get keyboard focus by themselves.
            if let Ok(h) = HasWindowHandle::window_handle(window) {
                let xid = match h.as_raw() {
                    RawWindowHandle::Xlib(x) => Some(x.window as u32),
                    RawWindowHandle::Xcb(x) => Some(x.window.get()),
                    _ => None,
                };
                if let Some(xid) = xid {
                    std::thread::spawn(move || {
                        if let Err(e) = oxshot_x11::activate_window(xid) {
                            log::warn!("overlay focus: {e:#}");
                        }
                    });
                }
            }
            window.focus(&self.focus, cx);
        }
        if self.tiles.is_none() {
            self.tiles = Some(TiledImage::new(&self.snap.image, (0, 0)));
        }
        self.update_magnifier(window);

        let scale = window.scale_factor();
        let map = ViewMap {
            origin: point(px(-self.origin.0 as f32 / scale), px(-self.origin.1 as f32 / scale)),
            zoom: 1.0 / scale,
        };
        let picking = self.mode == Mode::PickColor;
        let sel = if picking { IRect::new(0, 0, self.snap.image.width(), self.snap.image.height()) } else { self.selection() };
        let dragging = self.is_dragging();
        let src = self.mag_src();
        let paint_tiles: Vec<(Bounds<Pixels>, Arc<RenderImage>)> = self
            .tiles
            .as_ref()
            .unwrap()
            .tiles
            .iter()
            .map(|t| {
                let o = map.to_screen(t.x as f32, t.y as f32);
                (Bounds { origin: o, size: size(px(t.w as f32 / scale), px(t.h as f32 / scale)) }, t.image.clone())
            })
            .collect();
        let to_s = move |r: IRect| Bounds {
            origin: map.to_screen(r.x as f32, r.y as f32),
            size: size(px(r.w as f32 / scale), px(r.h as f32 / scale)),
        };
        let sel_b = to_s(sel);
        let red: Hsla = rgb(0xff3b30).into();

        // Info label: size + position, placed above the selection.
        let label = format!("{} × {}", sel.w, sel.h);
        let label_pos = point(sel_b.origin.x, (sel_b.origin.y - px(26.0)).max(px(4.0)));

        // Magnifier placement: bottom-right of the pointer, flipped near edges.
        let (pxr, pyr) = self.pointer;
        let ptr = map.to_screen(pxr as f32, pyr as f32);
        let mag_size = px((src * self.mag_zoom) as f32 / scale);
        let vp = window.viewport_size();
        let mut mx = ptr.x + px(24.0);
        let mut my = ptr.y + px(24.0);
        if mx + mag_size > vp.width {
            mx = ptr.x - px(24.0) - mag_size;
        }
        if my + mag_size + px(40.0) > vp.height {
            my = ptr.y - px(24.0) - mag_size - px(40.0);
        }
        let px_color = self.pixel();
        let color = px_color.map(|[r, g, b]| format!("#{r:02X}{g:02X}{b:02X}")).unwrap_or_default();
        let ring: Hsla = px_color.map_or(white(), |[r, g, b]| Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 }.into());
        let mag = self.mag.clone();
        let help = self.show_help;
        let include_cursor = self.include_cursor;

        div()
            .id("overlay")
            .track_focus(&self.focus)
            .size_full()
            .cursor_crosshair()
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                this.pointer = this.to_root(ev.position, window);
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                this.pointer = this.to_root(ev.position, window);
                if this.mode == Mode::PickColor {
                    this.pick(window, cx);
                    return;
                }
                this.drag_start = Some(this.pointer);
                cx.notify();
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, ev: &MouseUpEvent, window, cx| {
                if this.mode == Mode::PickColor || this.idle {
                    return;
                }
                this.pointer = this.to_root(ev.position, window);
                let rect = if this.is_dragging() { this.selection() } else { this.hovered() };
                this.finish(Some(rect), window, cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _: &MouseDownEvent, window, cx| {
                if this.drag_start.take().is_none() {
                    this.finish(None, window, cx);
                }
                cx.notify();
            }))
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                let dy = f32::from(ev.delta.pixel_delta(px(20.0)).y);
                if dy > 0.0 {
                    this.mag_zoom = (this.mag_zoom + 2).min(24);
                } else if dy < 0.0 {
                    this.mag_zoom = this.mag_zoom.saturating_sub(2).max(4);
                }
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                let step = if ev.keystroke.modifiers.shift { 10 } else { 1 };
                match ev.keystroke.key.as_str() {
                    "escape" => {
                        if this.drag_start.take().is_none() {
                            this.finish(None, window, cx);
                        }
                    }
                    "left" => this.pointer.0 -= step,
                    "right" => this.pointer.0 += step,
                    "up" => this.pointer.1 -= step,
                    "down" => this.pointer.1 += step,
                    "enter" | "space" if this.mode == Mode::PickColor => this.pick(window, cx),
                    // Keyboard selection: Space starts/ends a region, Enter captures.
                    "enter" | "space" => {
                        if this.drag_start.is_some() {
                            let r = this.selection();
                            this.finish(Some(r), window, cx);
                        } else if ev.keystroke.key == "enter" {
                            let r = this.hovered();
                            this.finish(Some(r), window, cx);
                        } else {
                            this.drag_start = Some(this.pointer);
                        }
                    }
                    "f" | "a" | "m" if this.mode == Mode::PickColor => {}
                    "f" => {
                        let r = this.snap.monitors.iter().find(|m| m.rect.contains(this.pointer.0, this.pointer.1)).map(|m| m.rect);
                        this.finish(r, window, cx);
                    }
                    "a" => {
                        let r = IRect::new(0, 0, this.snap.image.width(), this.snap.image.height());
                        this.finish(Some(r), window, cx);
                    }
                    "m" => this.include_cursor = !this.include_cursor,
                    "h" => this.show_help = !this.show_help,
                    _ => {}
                }
                cx.notify();
            }))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        for (b, img) in &paint_tiles {
                            let _ = window.paint_image(bounds, *b, Corners::default(), img.clone(), 0, false);
                        }
                        // Dim everything outside the selection (not when picking a color).
                        let dim = hsla(0.0, 0.0, 0.0, if picking { 0.0 } else { 0.45 });
                        let s = sel_b;
                        let (l, t, r, btm) = (bounds.left(), bounds.top(), bounds.right(), bounds.bottom());
                        let band = |window: &mut Window, a: Point<Pixels>, b: Point<Pixels>| {
                            if b.x > a.x && b.y > a.y {
                                window.paint_quad(fill(Bounds::from_corners(a, b), dim));
                            }
                        };
                        band(window, point(l, t), point(r, s.top()));
                        band(window, point(l, s.bottom()), point(r, btm));
                        band(window, point(l, s.top()), point(s.left(), s.bottom()));
                        band(window, point(s.right(), s.top()), point(r, s.bottom()));
                        if !picking {
                            let border = if dragging { px(1.0) } else { px(2.0) };
                            window.paint_quad(quad(s, px(0.0), transparent_black(), border, red, BorderStyle::Solid));
                        }
                        // Crosshair lines through the pointer while not dragging.
                        if !dragging {
                            let line = hsla(0.0, 0.0, 1.0, 0.35);
                            window.paint_quad(fill(Bounds::from_corners(point(l, ptr.y), point(r, ptr.y + px(1.0))), line));
                            window.paint_quad(fill(Bounds::from_corners(point(ptr.x, t), point(ptr.x + px(1.0), btm)), line));
                        }
                        if let Some(m) = &mag {
                            let mb = Bounds { origin: point(mx, my), size: size(mag_size, mag_size) };
                            let cell = mag_size / src as f32;
                            let c = Bounds { origin: point(mx + cell * (src / 2) as f32, my + cell * (src / 2) as f32), size: size(cell, cell) };
                            if picking {
                                // Round loupe; its ring shows the color under the pointer.
                                let round = Corners::all(mag_size / 2.0);
                                window.paint_quad(quad(mb.dilate(px(8.0)), Corners::all(mag_size / 2.0 + px(8.0)), ring, px(2.0), hsla(0.0, 0.0, 0.0, 0.6), BorderStyle::Solid));
                                window.paint_quad(quad(mb.dilate(px(2.0)), Corners::all(mag_size / 2.0 + px(2.0)), hsla(0.0, 0.0, 1.0, 1.0), px(0.0), transparent_black(), BorderStyle::Solid));
                                let _ = window.paint_image(mb, mb, round, m.clone(), 0, false);
                                window.paint_quad(quad(c.dilate(px(1.0)), px(0.0), transparent_black(), px(1.0), hsla(0.0, 0.0, 0.0, 0.9), BorderStyle::Solid));
                                window.paint_quad(quad(c, px(0.0), transparent_black(), px(1.0), white(), BorderStyle::Solid));
                            } else {
                                window.paint_quad(fill(mb.dilate(px(2.0)), hsla(0.0, 0.0, 1.0, 1.0)));
                                let _ = window.paint_image(mb, mb, Corners::default(), m.clone(), 0, false);
                                window.paint_quad(quad(c, px(0.0), transparent_black(), px(1.0), red, BorderStyle::Solid));
                            }
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .when(!picking, |d| d.child(
                div()
                    .absolute()
                    .left(label_pos.x)
                    .top(label_pos.y)
                    .px_2()
                    .py_0p5()
                    .rounded(px(4.0))
                    .bg(hsla(0.0, 0.0, 0.0, 0.75))
                    .text_color(white())
                    .text_xs()
                    .child(label),
            ))
            .when(picking, |d| {
                let rgb_text = px_color.map(|[r, g, b]| format!("rgb({r}, {g}, {b})")).unwrap_or_default();
                d.child(
                    div()
                        .absolute()
                        .left(mx)
                        .top(my + mag_size + px(14.0))
                        .min_w(mag_size)
                        .p_2()
                        .rounded(px(8.0))
                        .bg(hsla(0.0, 0.0, 0.08, 0.9))
                        .text_color(white())
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(div().size(px(28.0)).rounded(px(6.0)).bg(ring).border_1().border_color(hsla(0.0, 0.0, 1.0, 0.4)))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(div().text_sm().font_weight(FontWeight::BOLD).child(color.clone()))
                                .child(div().text_xs().text_color(hsla(0.0, 0.0, 1.0, 0.7)).child(format!("{rgb_text} · {pxr}, {pyr}"))),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .bottom(px(24.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .px_3()
                                .py_1()
                                .rounded(px(6.0))
                                .bg(hsla(0.0, 0.0, 0.0, 0.75))
                                .text_color(white())
                                .text_sm()
                                .child(t!("picker.hint").to_string()),
                        ),
                )
            })
            .when(!picking, |d| d.child(
                div()
                    .absolute()
                    .left(mx)
                    .top(my + mag_size + px(4.0))
                    .w(mag_size)
                    .px_1()
                    .py_0p5()
                    .rounded(px(4.0))
                    .bg(hsla(0.0, 0.0, 0.0, 0.75))
                    .text_color(white())
                    .text_xs()
                    .child(format!("{}, {}  {}", pxr, pyr, color)),
            ))
            .when(help && !picking, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(40.0))
                        .left(px(40.0))
                        .p_4()
                        .rounded(px(8.0))
                        .bg(hsla(0.0, 0.0, 0.0, 0.8))
                        .text_color(white())
                        .text_sm()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(t!("overlay.help_click").to_string())
                        .child(t!("overlay.help_keys").to_string())
                        .child(t!("overlay.help_modes").to_string())
                        .child(t!("overlay.help_cursor", state = if include_cursor { t!("common.on") } else { t!("common.off") }).to_string())
                        .child(t!("overlay.help_exit").to_string()),
                )
            })
            .into_any_element()
    }
}

/// Opens the overlay over all monitors.
pub fn open(snap: Snapshot, started: std::time::Instant, mode: Mode, on_done: impl FnOnce(Option<Outcome>, Arc<Snapshot>, &mut App) + 'static, cx: &mut App) -> anyhow::Result<()> {
    let snap = Arc::new(snap);
    let (w, h) = (snap.image.width() as f32, snap.image.height() as f32);
    let pointer = snap.pointer;
    let opts = WindowOptions {
        // GPUI's X11 backend shifts every new window 2 px to the right (workaround for a
        // GNOME bug); compensate so the frozen image lines up with the real screen.
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(-2.0), px(0.0)), size: size(px(w), px(h)) })),
        titlebar: None,
        focus: true,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        app_id: Some("oxshot-overlay".into()),
        window_background: WindowBackgroundAppearance::Opaque,
        ..Default::default()
    };
    let include_cursor = crate::settings::get(cx).capture_cursor;
    let on_done: OnDone = Box::new(on_done);
    cx.open_window(opts, move |window, cx| {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        cx.new(|_| Overlay {
            snap,
            origin: (0, 0),
            tiles: None,
            pointer,
            drag_start: None,
            include_cursor,
            show_help: false,
            mag_zoom: if mode == Mode::PickColor { 12 } else { 8 },
            mag: None,
            focus,
            on_done: Some(on_done),
            activated: false,
            started,
            reusable: false,
            idle: false,
            mode,
        })
    })?;
    Ok(())
}

/// Creates the reusable overlay window for a root of `size`: GPUI maps new windows right
/// away, so it is created far off-screen, unmapped at once and then moved to (0, 0) where
/// it waits for [`Overlay::begin`].
pub fn prewarm(size: (u32, u32), cx: &mut App) -> anyhow::Result<WindowHandle<Overlay>> {
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(-30000.0), px(-30000.0)),
            size: gpui_kit::size(px(size.0 as f32), px(size.1 as f32)),
        })),
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        app_id: Some("oxshot-overlay".into()),
        window_background: WindowBackgroundAppearance::Opaque,
        ..Default::default()
    };
    let handle = cx.open_window(opts, |_, cx| {
        cx.new(|cx| Overlay {
            snap: Arc::new(empty_snapshot()),
            origin: (0, 0),
            tiles: None,
            pointer: (0, 0),
            drag_start: None,
            include_cursor: false,
            show_help: false,
            mag_zoom: 8,
            mag: None,
            focus: cx.focus_handle(),
            on_done: None,
            activated: true,
            started: std::time::Instant::now(),
            reusable: true,
            idle: true,
            mode: Mode::Capture,
        })
    })?;
    // Let it draw one (off-screen) frame to warm up the GPU side, then hide it. Only
    // while idle: should the frame come late, a capture may already be showing. A timer
    // covers the case that GPUI does not draw off-screen windows at all.
    let hide = move |cx: &mut App| {
        let idle = handle.read_with(cx, |o, _| o.idle).unwrap_or(false);
        let xid = handle.update(cx, |_, window, _| crate::icon::xid(window)).ok().flatten();
        if let (true, Some(xid)) = (idle, xid)
            && let Err(e) = oxshot_x11::unmap_window(xid).and_then(|_| oxshot_x11::move_window(xid, 0, 0))
        {
            log::warn!("overlay prewarm: {e:#}");
        }
    };
    handle.update(cx, |_, window, _| {
        window.on_next_frame(move |_, cx| cx.defer(hide));
        window.refresh();
    })?;
    cx.spawn(async move |cx| {
        cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
        cx.update(|cx| hide(cx));
    })
    .detach();
    Ok(handle)
}
