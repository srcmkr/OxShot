//! Editor chrome: action bar, tool bar, contextual property bar, status bar.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use oxshot_core::edit::{Style, Tool};
use oxshot_core::{Align, ArrowHeads, ArrowStyle, BlurMode, Color, Stroke};
use rust_i18n::t;

use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::popover::Popover;

use super::Editor;

/// Called with the color chosen in a palette popover (`None` = no color).
type PickColor = Rc<dyn Fn(Option<Color>, &mut App)>;
use crate::assets::icon;
use crate::theme::Colors;

/// Palette like Screenpresso's quick colors.
pub const PALETTE: [Color; 12] = [
    Color::rgb(0xE5, 0x1C, 0x23),
    Color::rgb(0xFF, 0x6D, 0x00),
    Color::rgb(0xFF, 0xE1, 0x00),
    Color::rgb(0x2E, 0xB8, 0x4B),
    Color::rgb(0x00, 0x9C, 0xDE),
    Color::rgb(0x2F, 0x4D, 0xE0),
    Color::rgb(0x8E, 0x3C, 0xD8),
    Color::rgb(0xE8, 0x3E, 0x8C),
    Color::rgb(0x00, 0x00, 0x00),
    Color::rgb(0x60, 0x60, 0x60),
    Color::rgb(0xC8, 0xC8, 0xC8),
    Color::rgb(0xFF, 0xFF, 0xFF),
];

pub fn to_hsla(c: Color) -> Hsla {
    Rgba { r: c.r as f32 / 255.0, g: c.g as f32 / 255.0, b: c.b as f32 / 255.0, a: c.a as f32 / 255.0 }.into()
}

fn tool_icon(t: Tool) -> &'static str {
    match t {
        Tool::Select => "mouse-pointer-2",
        Tool::Arrow => "move-up-right",
        Tool::Rect => "square",
        Tool::Text => "type",
        Tool::Callout => "message-square",
        Tool::Step => "step",
        Tool::Highlight => "highlighter",
        Tool::Ellipse => "circle",
        Tool::Polygon => "pentagon",
        Tool::Freehand => "pencil",
        Tool::Blur => "droplet",
        Tool::Stamp => "stamp",
        Tool::Magnifier => "zoom-in",
        Tool::Brace => "braces",
    }
}

fn icon_el(name: &str, color: Hsla, s: f32) -> Svg {
    svg().path(icon(name)).size(px(s)).text_color(color)
}

/// Square icon button with tooltip.
fn icon_button(
    id: impl Into<ElementId>,
    name: &str,
    tip: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let c = Colors::get();
    let tip: SharedString = tip.into();
    div()
        .id(id.into())
        .size(px(34.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .cursor_pointer()
        .when(active, |d| d.bg(c.accent_bg))
        .when(!active, |d| d.hover(|d| d.bg(c.hover)))
        .child(icon_el(name, if active { c.accent } else { c.fg }, 18.0))
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
        .on_click(on_click)
}

pub(super) fn text_button(
    id: impl Into<ElementId>,
    icon_name: &str,
    label: &str,
    primary: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let c = Colors::get();
    div()
        .id(id.into())
        .h(px(32.0))
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(6.0))
        .cursor_pointer()
        .when(primary, |d| d.bg(c.primary).text_color(c.on_primary).hover(|d| d.bg(c.primary_hover)))
        .when(!primary, |d| d.hover(|d| d.bg(c.hover)))
        .child(icon_el(icon_name, if primary { c.on_primary } else { c.fg }, 16.0))
        .child(label.to_string())
        .on_click(on_click)
}

pub(super) fn separator() -> Div {
    let c = Colors::get();
    div().w(px(1.0)).h(px(22.0)).mx_1().bg(c.border)
}

impl Editor {
    pub(super) fn render_topbar(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = Colors::get();
        let this = cx.entity().downgrade();
        let act = move |f: fn(&mut Editor, &mut Window, &mut Context<Editor>)| {
            let this = this.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                this.update(cx, |e, cx| f(e, window, cx)).ok();
            }
        };
        let zoom = format!("{:.0}%", self.zoom * 100.0);
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h(px(48.0))
            .px_2()
            .bg(c.panel)
            .border_b_1()
            .border_color(c.border)
            .child(text_button("copy-close", "clipboard-check", &t!("editor.copy_close"), true, act(|e, w, cx| e.copy_and_close(w, cx))))
            .child(icon_button("copy", "copy", t!("editor.copy"), false, act(|e, _, cx| e.copy(cx))))
            .child(icon_button("save", "save", t!("editor.save"), false, act(|e, _, cx| {
                e.modified = true;
                e.autosave();
                e.status = e.file.as_ref().map_or("".into(), |f| t!("status.saved", path = f.display()).to_string().into());
                cx.notify();
            })))
            .child(icon_button("save-as", "folder-open", t!("editor.save_as"), false, act(|e, _, cx| e.save_as(cx))))
            .child(separator())
            .child(icon_button("undo", "undo-2", t!("editor.undo"), false, act(|e, _, cx| e.undo(cx))))
            .child(icon_button("redo", "redo-2", t!("editor.redo"), false, act(|e, _, cx| e.redo(cx))))
            .child(separator())
            .child(icon_button("crop", "crop", t!("editor.crop"), self.crop.is_some(), act(|e, _, cx| {
                if e.crop.is_some() { e.cancel_crop(cx) } else { e.start_crop(cx) }
            })))
            .child(icon_button("resize", "scaling", t!("editor.resize"), matches!(self.mode, Some(super::ops_ui::ImageMode::Resize { .. })), act(|e, _, cx| {
                e.start_mode(super::ops_ui::ImageMode::Resize { percent: 50.0, scale_styles: true }, cx)
            })))
            .child(icon_button("cut", "scissors", t!("editor.cut_out"), matches!(self.mode, Some(super::ops_ui::ImageMode::Cut { .. })), act(|e, _, cx| {
                e.start_mode(super::ops_ui::ImageMode::Cut { vertical: false, band: None, edge: oxshot_core::ops::CutEdge::Zigzag }, cx)
            })))
            .child(icon_button("effects", "frame", t!("editor.effects"), matches!(self.mode, Some(super::ops_ui::ImageMode::Effects { .. })), act(|e, _, cx| {
                let fx = oxshot_core::ops::BorderEffect { shadow: 1.0, corner_radius: 8.0, ..Default::default() };
                e.start_mode(super::ops_ui::ImageMode::Effects { fx }, cx)
            })))
            .child(icon_button("image-plus", "image-plus", t!("editor.insert_image"), false, act(|e, _, cx| e.insert_stamp_from_file(cx))))
            .child(div().flex_1())
            .child(icon_button("zoom-out", "minus", t!("editor.zoom_out"), false, act(|e, _, cx| e.zoom_step(-1.0, None, cx))))
            .child(
                div()
                    .id("zoom-label")
                    .w(px(56.0))
                    .text_center()
                    .cursor_pointer()
                    .child(zoom)
                    .on_click(act(|e, _, cx| e.set_zoom(1.0, None, cx))),
            )
            .child(icon_button("zoom-in", "plus", t!("editor.zoom_in"), false, act(|e, _, cx| e.zoom_step(1.0, None, cx))))
            .child(icon_button("fit", "maximize", t!("editor.fit"), self.fit, act(|e, _, cx| {
                e.fit = true;
                cx.notify();
            })))
    }

    pub(super) fn render_toolbar(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = Colors::get();
        let mut col = div()
            .flex()
            .flex_col()
            .items_center()
            .gap_0p5()
            .w(px(48.0))
            .py_2()
            .bg(c.panel)
            .border_r_1()
            .border_color(c.border);
        for t in Tool::ALL {
            let this = cx.entity().downgrade();
            let tip = format!("{} ({})", crate::i18n::tool_label(t), t.shortcut().to_uppercase());
            col = col.child(icon_button(SharedString::from(format!("tool-{t:?}")), tool_icon(t), tip, self.tool == t, move |_, _, cx| {
                this.update(cx, |e, cx| e.set_tool(t, cx)).ok();
            }));
            if t == Tool::Select {
                col = col.child(div().h(px(1.0)).w(px(28.0)).my_1().bg(c.border));
            }
        }
        col
    }

    pub(super) fn render_statusbar(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let c = Colors::get();
        let r = self.doc.canvas.rect;
        let hint = match self.tool {
            Tool::Select => t!("hint.select"),
            Tool::Polygon => t!("hint.polygon"),
            Tool::Text => t!("hint.text"),
            Tool::Callout => t!("hint.callout"),
            Tool::Step => t!("hint.step"),
            Tool::Magnifier => t!("hint.magnifier"),
            _ => t!("hint.draw"),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .h(px(28.0))
            .px_3()
            .bg(c.panel)
            .border_t_1()
            .border_color(c.border)
            .text_xs()
            .text_color(c.muted)
            .child(format!("{} × {} px", r.w, r.h))
            .child(hint)
            .child(div().flex_1())
            .child(self.status.clone())
    }

    pub(super) fn render_props(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = Colors::get();
        let st = self.current_style();
        let tool = self.style_tool();
        let this = cx.entity().downgrade();
        // Helper producing a click handler that edits the style.
        let set = move |f: Box<dyn Fn(&mut Style)>| {
            let this = this.clone();
            let f = std::rc::Rc::new(f);
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                let f = f.clone();
                this.update(cx, |e, cx| e.set_style(cx, |st| f(st))).ok();
            }
        };

        // One row, never wrapping: the bar keeps its height for every tool, so the canvas
        // below (and its fit-to-window zoom) does not jump. Overflow scrolls sideways.
        let mut bar = div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap_2()
            .h_full()
            .min_w_full()
            .px_3()
            .whitespace_nowrap()
            .child(div().flex_none().text_color(c.muted).min_w(px(84.0)).child(crate::i18n::tool_label(tool)));

        if self.mode.is_some() {
            return self.render_mode_bar(bar, cx);
        }
        if let Some(r) = self.crop {
            let r = r.normalized();
            let this = cx.entity().downgrade();
            let (a, b, c2) = (this.clone(), this.clone(), this);
            let base = (self.doc.base.width() as f32, self.doc.base.height() as f32);
            return bar
                .child(div().child(t!("crop.size", w = format!("{:.0}", r.w), h = format!("{:.0}", r.h)).to_string()))
                .child(div().text_color(c.muted).child(t!("crop.hint").to_string()))
                .child(div().flex_1())
                .child(toggle("crop-reset", t!("crop.whole"), false, move |_, _, cx| {
                    a.update(cx, |e, cx| {
                        e.crop = Some(oxshot_core::Rect::new(0.0, 0.0, base.0, base.1));
                        cx.notify();
                    })
                    .ok();
                }))
                .child(toggle("crop-cancel", t!("common.cancel"), false, move |_, _, cx| {
                    b.update(cx, |e, cx| e.cancel_crop(cx)).ok();
                }))
                .child(text_button("crop-apply", "check", &t!("common.apply"), true, move |_, _, cx| {
                    c2.update(cx, |e, cx| e.apply_crop(cx)).ok();
                }));
        }
        if tool == Tool::Select {
            return bar.child(div().text_color(c.muted).child(t!("props.click_object").to_string()));
        }

        // Colors and fills are compact buttons that open a palette popover, so the bar
        // stays short enough for the window.
        let uses_color = !matches!(tool, Tool::Blur | Tool::Stamp);
        if uses_color {
            let is_text = matches!(tool, Tool::Text | Tool::Callout);
            let cur = if is_text { st.text.color } else { st.color };
            let this = cx.entity().downgrade();
            let pick: PickColor = Rc::new(move |col, cx| {
                let Some(col) = col else { return };
                this.update(cx, |e, cx| {
                    e.set_style(cx, |st| if is_text { st.text.color = col } else { st.color = col })
                })
                .ok();
            });
            let options = PALETTE.iter().map(|c| Some(*c)).collect();
            let recent = crate::settings::get(cx).recent_colors.clone();
            bar = bar.child(color_button("color", t!("props.color").to_string(), Some(cur), options, recent, pick));
        }

        if matches!(tool, Tool::Arrow | Tool::Rect | Tool::Ellipse | Tool::Polygon | Tool::Freehand | Tool::Brace | Tool::Magnifier | Tool::Callout) {
            let min = if matches!(tool, Tool::Rect | Tool::Ellipse | Tool::Polygon | Tool::Callout) { 0.0 } else { 1.0 };
            bar = bar.child(stepper("width", &t!("props.width"), format!("{:.0} px", st.width), set(Box::new(move |st| st.width = (st.width - 1.0).max(min))), set(Box::new(|st| st.width = (st.width + 1.0).min(40.0)))));
        }

        if matches!(tool, Tool::Rect | Tool::Ellipse | Tool::Polygon | Tool::Callout) {
            let callout = tool == Tool::Callout;
            let mut options: Vec<Option<Color>> = if callout { Vec::new() } else { vec![None] };
            for col in [PALETTE[2], PALETTE[11], PALETTE[0], PALETTE[1], PALETTE[3], PALETTE[4], PALETTE[6], Color::rgb(0xFF, 0xF7, 0xC2), PALETTE[10], PALETTE[8]] {
                options.push(Some(if callout { col } else { col.with_alpha(110) }));
            }
            let this = cx.entity().downgrade();
            let pick: PickColor = Rc::new(move |col, cx| {
                this.update(cx, |e, cx| e.set_style(cx, |st| st.fill = col)).ok();
            });
            bar = bar.child(color_button("fill", t!("props.fill").to_string(), st.fill, options, Vec::new(), pick));
        }
        if uses_color {
            bar = bar.child(separator());
        }

        if matches!(tool, Tool::Rect | Tool::Callout) {
            bar = bar.child(stepper("radius", &t!("props.corners"), format!("{:.0}", st.corner_radius), set(Box::new(|st| st.corner_radius = (st.corner_radius - 2.0).max(0.0))), set(Box::new(|st| st.corner_radius = (st.corner_radius + 2.0).min(60.0)))));
        }

        match tool {
            Tool::Arrow => {
                bar = bar
                    .child(toggle("arrow-tapered", t!("props.tapered"), st.arrow_style == ArrowStyle::Tapered, set(Box::new(|st| {
                        st.arrow_style = if st.arrow_style == ArrowStyle::Tapered { ArrowStyle::Line } else { ArrowStyle::Tapered };
                    }))))
                    .child(toggle("arrow-both", t!("props.double_head"), st.arrow_heads == ArrowHeads::Both, set(Box::new(|st| {
                        st.arrow_heads = if st.arrow_heads == ArrowHeads::Both { ArrowHeads::End } else { ArrowHeads::Both };
                    }))))
                    .child(toggle("arrow-line", t!("props.line"), st.arrow_heads == ArrowHeads::None, set(Box::new(|st| {
                        st.arrow_heads = if st.arrow_heads == ArrowHeads::None { ArrowHeads::End } else { ArrowHeads::None };
                    }))))
                    .child(toggle("arrow-dashed", t!("props.dashed"), st.dashed, set(Box::new(|st| st.dashed = !st.dashed))));
            }
            Tool::Text | Tool::Callout => {
                bar = bar
                    .child(stepper("font", &t!("props.size"), format!("{:.0}", st.text.size), set(Box::new(|st| st.text.size = (st.text.size - 2.0).max(6.0))), set(Box::new(|st| st.text.size = (st.text.size + 2.0).min(200.0)))))
                    .child(icon_toggle("bold", "bold", t!("props.bold"), st.text.bold, set(Box::new(|st| st.text.bold = !st.text.bold))))
                    .child(icon_toggle("italic", "italic", t!("props.italic"), st.text.italic, set(Box::new(|st| st.text.italic = !st.text.italic))))
                    .child(icon_toggle("underline", "underline", t!("props.underline"), st.text.underline, set(Box::new(|st| st.text.underline = !st.text.underline))))
                    .child(icon_toggle("al", "text-align-start", t!("props.left"), st.text.align == Align::Left, set(Box::new(|st| st.text.align = Align::Left))))
                    .child(icon_toggle("ac", "text-align-center", t!("props.center"), st.text.align == Align::Center, set(Box::new(|st| st.text.align = Align::Center))))
                    .child(icon_toggle("ar", "text-align-end", t!("props.right"), st.text.align == Align::Right, set(Box::new(|st| st.text.align = Align::Right))));
                if tool == Tool::Text {
                    bar = bar.child(toggle("outline", t!("props.outline"), st.text.outline.is_some(), set(Box::new(|st| {
                        st.text.outline = match st.text.outline {
                            Some(_) => None,
                            None => Some(Stroke { color: Color::WHITE, width: 3.0 }),
                        };
                    }))));
                }
            }
            Tool::Step => {
                bar = bar.child(stepper("size", &t!("props.size"), format!("{:.0}", st.size), set(Box::new(|st| st.size = (st.size - 2.0).max(8.0))), set(Box::new(|st| st.size = (st.size + 2.0).min(120.0)))));
                let next = self.doc.next_step_number();
                let sel_number = self.selection.and_then(|id| self.doc.get(id)).and_then(|o| match &o.shape {
                    oxshot_core::Shape::Step(s) => Some(s.number),
                    _ => None,
                });
                let this = cx.entity().downgrade();
                let this2 = this.clone();
                let label = match sel_number {
                    Some(n) => t!("props.number", n = n).to_string(),
                    None => t!("props.next_number", n = next).to_string(),
                };
                bar = bar.child(div().flex().items_center().gap_1().child(label).when(sel_number.is_some(), |d| {
                    d.child(small_btn("num-dec", "minus", move |_, _, cx| {
                        this.update(cx, |e, cx| e.bump_step(-1, cx)).ok();
                    }))
                    .child(small_btn("num-inc", "plus", move |_, _, cx| {
                        this2.update(cx, |e, cx| e.bump_step(1, cx)).ok();
                    }))
                }));
            }
            Tool::Blur => {
                bar = bar
                    .child(toggle("blur-gauss", t!("props.soft"), st.blur_mode == BlurMode::Gaussian, set(Box::new(|st| st.blur_mode = BlurMode::Gaussian))))
                    .child(toggle("blur-pix", t!("props.pixelate"), st.blur_mode == BlurMode::Pixelate, set(Box::new(|st| st.blur_mode = BlurMode::Pixelate))))
                    .child(stepper("strength", &t!("props.strength"), format!("{:.0}", st.strength), set(Box::new(|st| st.strength = (st.strength - 1.0).max(1.0))), set(Box::new(|st| st.strength = (st.strength + 1.0).min(20.0)))));
            }
            Tool::Magnifier => {
                bar = bar.child(stepper("zoom", &t!("props.zoom"), format!("{:.1}×", st.zoom), set(Box::new(|st| st.zoom = (st.zoom - 0.5).max(1.0))), set(Box::new(|st| st.zoom = (st.zoom + 0.5).min(8.0)))));
            }
            _ => {}
        }

        if !matches!(tool, Tool::Blur | Tool::Highlight) {
            bar = bar
                .child(separator())
                .child(stepper("opacity", &t!("props.opacity"), format!("{:.0}%", st.opacity * 100.0), set(Box::new(|st| st.opacity = (st.opacity - 0.1).max(0.1))), set(Box::new(|st| st.opacity = (st.opacity + 0.1).min(1.0)))))
                .child(toggle("shadow", t!("props.shadow"), st.shadow > 0.0, set(Box::new(|st| st.shadow = if st.shadow > 0.0 { 0.0 } else { 0.6 }))));
        }
        if self.selection.is_some() {
            let this = cx.entity().downgrade();
            bar = bar.child(div().flex_1()).child(icon_button("delete", "trash", t!("editor.delete"), false, move |_, _, cx| {
                this.update(cx, |e, cx| {
                    if let Some(id) = e.selection {
                        e.checkpoint();
                        e.remove(id);
                        cx.notify();
                    }
                })
                .ok();
            }));
        }
        bar
    }

    fn bump_step(&mut self, d: i32, cx: &mut Context<Self>) {
        if let Some(id) = self.selection {
            self.checkpoint();
            self.mutate(id, |o| {
                if let oxshot_core::Shape::Step(s) = &mut o.shape {
                    s.number = (s.number as i32 + d).max(0) as u32;
                }
            });
            cx.notify();
        }
    }
}

fn swatch(id: impl Into<ElementId>, col: Color, active: bool, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Stateful<Div> {
    let c = Colors::get();
    div()
        .id(id.into())
        .size(px(22.0))
        .rounded_full()
        .border_2()
        .border_color(if active { c.accent } else { c.border })
        .p(px(2.0))
        .cursor_pointer()
        .child(div().size_full().rounded_full().bg(to_hsla(col)).border_1().border_color(hsla(0.0, 0.0, 0.0, 0.25)))
        .on_click(on_click)
}

fn small_btn(id: &'static str, name: &str, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Stateful<Div> {
    let c = Colors::get();
    div()
        .id(id)
        .size(px(22.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .cursor_pointer()
        .hover(|d| d.bg(c.hover))
        .child(icon_el(name, c.fg, 14.0))
        .on_click(on_click)
}

fn stepper(
    id: &'static str,
    label: &str,
    value: String,
    dec: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    inc: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let c = Colors::get();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .child(div().text_color(c.muted).child(label.to_string()))
        .child(small_btn(Box::leak(format!("{id}-dec").into_boxed_str()), "minus", dec))
        .child(div().min_w(px(34.0)).text_center().child(value))
        .child(small_btn(Box::leak(format!("{id}-inc").into_boxed_str()), "plus", inc))
}

fn toggle(id: &'static str, label: impl std::fmt::Display, active: bool, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Stateful<Div> {
    let c = Colors::get();
    div()
        .id(id)
        .h(px(26.0))
        .px_2()
        .flex()
        .items_center()
        .rounded(px(5.0))
        .border_1()
        .cursor_pointer()
        .border_color(if active { c.accent } else { c.border })
        .when(active, |d| d.bg(c.accent_bg).text_color(c.accent))
        .hover(|d| d.bg(c.hover))
        .child(label.to_string())
        .on_click(on_click)
}

fn icon_toggle(id: &'static str, name: &str, tip: impl std::fmt::Display, active: bool, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Stateful<Div> {
    let c = Colors::get();
    let tip = SharedString::from(tip.to_string());
    div()
        .id(id)
        .size(px(26.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.0))
        .cursor_pointer()
        .when(active, |d| d.bg(c.accent_bg))
        .hover(|d| d.bg(c.hover))
        .child(icon_el(name, if active { c.accent } else { c.fg }, 15.0))
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
        .on_click(on_click)
}

pub(super) fn swatch_pub(id: impl Into<ElementId>, col: Color, active: bool, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Stateful<Div> {
    swatch(id, col, active, on_click)
}

/// Toggle chip with a dynamic id and label.
pub(super) fn chip_toggle(id: impl Into<ElementId>, label: impl Into<SharedString>, active: bool, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Stateful<Div> {
    let c = Colors::get();
    div()
        .id(id.into())
        .h(px(26.0))
        .px_2()
        .flex()
        .items_center()
        .rounded(px(5.0))
        .border_1()
        .cursor_pointer()
        .border_color(if active { c.accent } else { c.border })
        .when(active, |d| d.bg(c.accent_bg).text_color(c.accent))
        .hover(|d| d.bg(c.hover))
        .child(label.into())
        .on_click(on_click)
}

pub(super) fn stepper_ctl(
    id: &'static str,
    label: &str,
    value: String,
    dec: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    inc: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    stepper(id, label, value, dec, inc)
}

/// A compact color button (current color + chevron) that opens a palette popover.
/// `None` in `options` is "no color" (e.g. no fill).
fn color_button(
    id: &'static str,
    tip: String,
    current: Option<Color>,
    options: Vec<Option<Color>>,
    recent: Vec<Color>,
    pick: PickColor,
) -> impl IntoElement {
    // Only the line/text color offers the screen picker (a fill keeps its transparency).
    let with_picker = id == "color";
    let c = Colors::get();
    // Fill shows as a square, the line/text color as a circle.
    let radius = if id == "fill" { px(4.0) } else { px(8.0) };
    let dot = match current {
        Some(col) => div().size(px(16.0)).rounded(radius).bg(to_hsla(col)).border_1().border_color(hsla(0.0, 0.0, 0.0, 0.3)),
        None => div().size(px(16.0)).rounded(radius).border_1().border_color(c.muted).flex().items_center().justify_center().child(icon_el("slash", c.muted, 11.0)),
    };
    let trigger = Button::new(id)
        .ghost()
        .tooltip(tip)
        .child(div().flex().items_center().gap_1().child(dot).child(icon_el("chevron-down", c.muted, 12.0)));
    Popover::new(SharedString::from(format!("{id}-popover"))).trigger(trigger).content(move |_, _, cx| {
        let pop = cx.entity().downgrade();
        let mut grid = div().flex().flex_row().flex_wrap().gap_1p5().w(px(6.0 * 22.0 + 5.0 * 6.0));
        for (i, col) in options.iter().copied().enumerate() {
            let (pick, pop) = (pick.clone(), pop.clone());
            let on_click = move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                pick(col, cx);
                pop.update(cx, |s, cx| s.dismiss(window, cx)).ok();
            };
            let active = col == current;
            grid = grid.child(match col {
                Some(col) => swatch((id, i), col, active, on_click),
                None => swatch_none((id, i), active, on_click),
            });
        }
        let c = Colors::get();
        let mut panel = div().flex().flex_col().gap_2().child(grid);
        if !recent.is_empty() {
            let mut row = div().flex().flex_row().flex_wrap().gap_1p5().w(px(6.0 * 22.0 + 5.0 * 6.0));
            for (i, col) in recent.iter().copied().enumerate() {
                let (pick, pop) = (pick.clone(), pop.clone());
                row = row.child(swatch(("recent", i), col, Some(col) == current, move |_, window, cx| {
                    pick(Some(col), cx);
                    pop.update(cx, |s, cx| s.dismiss(window, cx)).ok();
                }));
            }
            panel = panel.child(div().text_xs().text_color(c.muted).child(t!("props.recent").to_string())).child(row);
        }
        if with_picker {
            let (pick, pop) = (pick.clone(), pop.clone());
            panel = panel.child(
                div()
                    .id("pick-screen")
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(c.hover))
                    .child(icon_el("pipette", c.fg, 15.0))
                    .child(t!("props.pick_screen").to_string())
                    .on_click(move |_, window, cx| {
                        pop.update(cx, |s, cx| s.dismiss(window, cx)).ok();
                        let pick = pick.clone();
                        crate::app::pick_color(Some(Box::new(move |[r, g, b], cx| pick(Some(Color::rgb(r, g, b)), cx))), cx);
                    }),
            );
        }
        panel
    })
}

fn swatch_none(id: impl Into<ElementId>, active: bool, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Stateful<Div> {
    let c = Colors::get();
    div()
        .id(id.into())
        .size(px(22.0))
        .rounded_full()
        .border_2()
        .border_color(if active { c.accent } else { c.border })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .child(icon_el("slash", c.muted, 14.0))
        .on_click(on_click)
}
