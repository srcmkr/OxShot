//! Delayed capture: a small countdown pill at the top of the monitor under the pointer.
//! It never takes focus (open menus of other programs stay open), disappears before the
//! screen is frozen, and a click on it — or the hotkey again — cancels.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::*;
use rust_i18n::t;

use crate::theme::Colors;

const WIDTH: f32 = 300.0;
const HEIGHT: f32 = 64.0;

pub struct Countdown {
    started: Instant,
    total: Duration,
}

impl Render for Countdown {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let c = Colors::get();
        let elapsed = self.started.elapsed().min(self.total);
        let left = (self.total - elapsed).as_secs_f32().ceil().max(1.0) as u32;
        let progress = elapsed.as_secs_f32() / self.total.as_secs_f32().max(0.001);
        div()
            .id("countdown")
            .size_full()
            .cursor_pointer()
            // Close this very window directly (it is the one being updated right now).
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.remove_window();
                stop(cx);
            })
            .child(
                div()
                    .size_full()
                    .rounded(px(HEIGHT / 2.0))
                    .bg(hsla(0.0, 0.0, 0.08, 0.92))
                    .border_1()
                    .border_color(hsla(0.0, 0.0, 1.0, 0.12))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .px_5()
                            .child(
                                div()
                                    .size(px(36.0))
                                    .flex_none()
                                    .rounded_full()
                                    .bg(c.accent)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(white())
                                    .text_lg()
                                    .font_weight(FontWeight::BOLD)
                                    .child(left.to_string()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .text_color(white())
                                    .child(div().text_sm().child(t!("countdown.text", s = left).to_string()))
                                    .child(div().text_xs().text_color(hsla(0.0, 0.0, 1.0, 0.6)).child(t!("countdown.cancel").to_string())),
                            ),
                    )
                    // Progress along the bottom edge.
                    .child(div().h(px(3.0)).w(relative(progress)).bg(c.accent)),
            )
    }
}

/// The running countdown. The timer task itself is detached; it watches `cancelled`
/// (a task must not drop its own handle, or it would stop before capturing).
struct Running {
    window: WindowHandle<Countdown>,
    cancelled: Rc<Cell<bool>>,
}
impl Global for Running {}

/// Marks the countdown as cancelled and forgets it (the window is closed by the caller).
fn stop(cx: &mut App) {
    if let Some(r) = cx.try_global::<Running>() {
        r.cancelled.set(true);
    }
    cx.remove_global::<Running>();
}

pub fn is_running(cx: &App) -> bool {
    cx.has_global::<Running>()
}

/// Stops a running countdown without capturing.
pub fn cancel(cx: &mut App) {
    if let Some(w) = cx.try_global::<Running>().map(|r| r.window) {
        stop(cx);
        w.update(cx, |_, window, _| window.remove_window()).ok();
    }
}

/// Starts (or, if one is running, cancels) a delayed region capture.
pub fn start(seconds: u32, cx: &mut App) {
    if is_running(cx) {
        cancel(cx);
        return;
    }
    let total = Duration::from_secs(seconds.clamp(1, 60) as u64);
    // Top center of the monitor under the pointer.
    let (mx, my, mw) = oxshot_x11::pointer_monitor().map_or((0, 0, 1920), |m| (m.x, m.y, m.w as i32));
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px((mx + (mw - WIDTH as i32) / 2) as f32), px((my + 28) as f32)),
            size: size(px(WIDTH), px(HEIGHT)),
        })),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        app_id: Some("oxshot-countdown".into()),
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };
    let started = Instant::now();
    let window = match cx.open_window(opts, |_, cx| cx.new(|_| Countdown { started, total })) {
        Ok(w) => w,
        Err(e) => {
            log::warn!("countdown window: {e:#}");
            // Still honour the delay, just without the pill.
            cx.spawn(async move |cx| {
                cx.background_executor().timer(total).await;
                cx.update(|cx| crate::app::capture(crate::app::CaptureMode::Region, cx));
            })
            .detach();
            return;
        }
    };
    let cancelled = Rc::new(Cell::new(false));
    let flag = cancelled.clone();
    cx.spawn(async move |cx| {
        while started.elapsed() < total {
            cx.background_executor().timer(Duration::from_millis(50)).await;
            if flag.get() || window.update(cx, |_, window, _| window.refresh()).is_err() {
                return;
            }
        }
        cx.update(|cx| {
            cx.remove_global::<Running>();
            window.update(cx, |_, window, _| window.remove_window()).ok();
        });
        // Give the X server and compositor time to take the pill off the screen, so it
        // is not in the screenshot.
        cx.background_executor().timer(Duration::from_millis(150)).await;
        if !flag.get() {
            cx.update(|cx| crate::app::capture(crate::app::CaptureMode::Region, cx));
        }
    })
    .detach();
    cx.set_global(Running { window, cancelled });
}
