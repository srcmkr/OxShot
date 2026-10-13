//! Image operations in the editor: resize, cut out, border & effects. Each runs as a mode
//! with its options in the property bar; Enter applies, Esc cancels.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use oxshot_core::ops::{self, BorderEffect, CutEdge};
use oxshot_core::{Color, Document, IRect};
use rust_i18n::t;

use super::Editor;
use super::panel::{chip_toggle, separator, stepper_ctl, text_button};
use crate::gfx::TiledImage;
use crate::theme::Colors;

#[derive(Debug, Clone)]
pub enum ImageMode {
    Resize { percent: f32, scale_styles: bool },
    Cut { vertical: bool, band: Option<(f32, f32)>, edge: CutEdge },
    Effects { fx: BorderEffect },
}

/// Longest side of the effects preview image.
const PREVIEW_MAX: u32 = 1400;

impl Editor {
    pub(super) fn start_mode(&mut self, mode: ImageMode, cx: &mut Context<Self>) {
        self.finish_pending(cx);
        self.crop = None;
        self.selection = None;
        self.mode = Some(mode);
        self.preview_dirty = true;
        cx.notify();
    }

    pub(super) fn cancel_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = None;
        self.drop_preview();
        cx.notify();
    }

    fn drop_preview(&mut self) {
        let mut gfx = self.gfx.borrow_mut();
        if let Some(p) = gfx.preview.take() {
            gfx.garbage.push(p.0);
        }
    }

    /// Document with the canvas baked in, so operations work in simple coordinates.
    fn trimmed(&self) -> Document {
        let full = IRect::new(0, 0, self.doc.base.width(), self.doc.base.height());
        if self.doc.canvas.rect == full { self.doc.clone() } else { ops::trim_to_canvas(&self.doc) }
    }

    pub(super) fn apply_mode(&mut self, cx: &mut Context<Self>) {
        let Some(mode) = self.mode.take() else { return };
        self.drop_preview();
        let new = match mode {
            ImageMode::Resize { percent, scale_styles } if (percent - 100.0).abs() > 0.01 => {
                let s = percent / 100.0;
                Some(ops::resize(&self.trimmed(), s, s, scale_styles))
            }
            ImageMode::Cut { vertical, band: Some((a, b)), edge } => {
                let c = self.doc.canvas.rect;
                let off = if vertical { c.x as f32 } else { c.y as f32 };
                let (from, to) = (a.min(b) - off, a.max(b) - off);
                (to - from >= 2.0).then(|| ops::cut_out(&self.trimmed(), vertical, from, to, edge))
            }
            ImageMode::Effects { fx } => Some(ops::apply_border(&self.doc, &fx)),
            _ => None,
        };
        if let Some(doc) = new {
            self.replace_document(doc);
        }
        self.fit = true;
        cx.notify();
    }

    /// Swaps in the result of an operation (undoable).
    fn replace_document(&mut self, doc: Document) {
        self.checkpoint();
        let ids: Vec<_> = doc.objects.iter().map(|o| o.id).collect();
        self.doc = doc;
        for id in ids {
            self.touch(id);
        }
        self.backdrop = None;
        self.scroll = self.doc.canvas_rect().center();
    }

    /// Recomputes the effects preview (a downscaled flattened image with the effects).
    pub(super) fn sync_preview(&mut self, window: &mut Window) {
        let Some(ImageMode::Effects { fx }) = &self.mode else { return };
        if !self.preview_dirty {
            return;
        }
        self.preview_dirty = false;
        let flat = self.flatten();
        let longest = flat.width().max(flat.height());
        let s = (PREVIEW_MAX as f32 / longest as f32).min(1.0);
        let small = if s < 1.0 {
            image::imageops::resize(&flat, (flat.width() as f32 * s) as u32, (flat.height() as f32 * s) as u32, image::imageops::FilterType::Triangle)
        } else {
            flat
        };
        let img = ops::preview_border(&small, &fx.scaled(s));
        let mut gfx = self.gfx.borrow_mut();
        if let Some(old) = gfx.preview.take() {
            old.0.release(window);
        }
        gfx.preview = Some((TiledImage::new(&img, (0, 0)), s));
    }

    pub(super) fn mode_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        if self.mode.is_none() {
            return false;
        }
        match key {
            "enter" => self.apply_mode(cx),
            "escape" => self.cancel_mode(cx),
            _ => return true,
        }
        true
    }

    fn update_mode(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut ImageMode)) {
        if let Some(m) = &mut self.mode {
            f(m);
            self.preview_dirty = true;
            cx.notify();
        }
    }

    pub(super) fn render_mode_bar(&mut self, bar: Div, cx: &mut Context<Self>) -> Div {
        let c = Colors::get();
        let Some(mode) = self.mode.clone() else { return bar };
        let this = cx.entity().downgrade();
        let upd = {
            let this = this.clone();
            move |f: Box<dyn Fn(&mut ImageMode)>| {
                let this = this.clone();
                let f = std::rc::Rc::new(f);
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    let f = f.clone();
                    this.update(cx, |e, cx| e.update_mode(cx, |m| f(m))).ok();
                }
            }
        };
        let r = self.doc.canvas.rect;
        let bar = match mode {
            ImageMode::Resize { percent, scale_styles } => {
                let mut b = bar.child(div().child(t!(
                    "ops.resize_info",
                    w = r.w,
                    h = r.h,
                    nw = format!("{:.0}", r.w as f32 * percent / 100.0),
                    nh = format!("{:.0}", r.h as f32 * percent / 100.0)
                )
                .to_string()));
                for p in [25.0f32, 50.0, 75.0, 100.0, 150.0, 200.0] {
                    b = b.child(chip_toggle(SharedString::from(format!("pct{p}")), format!("{p:.0}%"), (percent - p).abs() < 0.01, upd(Box::new(move |m| {
                        if let ImageMode::Resize { percent, .. } = m {
                            *percent = p;
                        }
                    }))));
                }
                b.child(stepper_ctl("pct", "", format!("{percent:.0}%"), upd(Box::new(|m| {
                    if let ImageMode::Resize { percent, .. } = m {
                        *percent = (*percent - 5.0).max(5.0);
                    }
                })), upd(Box::new(|m| {
                    if let ImageMode::Resize { percent, .. } = m {
                        *percent = (*percent + 5.0).min(800.0);
                    }
                }))))
                .child(separator())
                .child(chip_toggle("scale-styles", t!("ops.scale_styles"), scale_styles, upd(Box::new(|m| {
                    if let ImageMode::Resize { scale_styles, .. } = m {
                        *scale_styles = !*scale_styles;
                    }
                }))))
            }
            ImageMode::Cut { vertical, band, edge } => {
                let info = match band {
                    Some((a, b)) => t!("ops.cut_info", px = format!("{:.0}", (b - a).abs())).to_string(),
                    None => if vertical { t!("ops.cut_drag_column").to_string() } else { t!("ops.cut_drag_strip").to_string() },
                };
                let mut b = bar
                    .child(div().child(info))
                    .child(chip_toggle("cut-h", t!("ops.horizontal"), !vertical, upd(Box::new(|m| {
                        if let ImageMode::Cut { vertical, band, .. } = m {
                            *vertical = false;
                            *band = None;
                        }
                    }))))
                    .child(chip_toggle("cut-v", t!("ops.vertical"), vertical, upd(Box::new(|m| {
                        if let ImageMode::Cut { vertical, band, .. } = m {
                            *vertical = true;
                            *band = None;
                        }
                    }))))
                    .child(separator());
                for (i, (e, label)) in [(CutEdge::Straight, t!("ops.straight")), (CutEdge::Zigzag, t!("ops.zigzag")), (CutEdge::Wave, t!("ops.wave"))].into_iter().enumerate() {
                    b = b.child(chip_toggle(("cut-edge", i), label, edge == e, upd(Box::new(move |m| {
                        if let ImageMode::Cut { edge, .. } = m {
                            *edge = e;
                        }
                    }))));
                }
                b
            }
            ImageMode::Effects { fx } => {
                let set = |f: fn(&mut BorderEffect)| {
                    upd(Box::new(move |m| {
                        if let ImageMode::Effects { fx } = m {
                            f(fx);
                        }
                    }))
                };
                bar.child(chip_toggle("fx-shadow", t!("props.shadow"), fx.shadow > 0.0, set(|fx| fx.shadow = if fx.shadow > 0.0 { 0.0 } else { 1.0 })))
                    .child(stepper_ctl("fx-radius", &t!("props.corners"), format!("{:.0}", fx.corner_radius), set(|fx| fx.corner_radius = (fx.corner_radius - 4.0).max(0.0)), set(|fx| fx.corner_radius = (fx.corner_radius + 4.0).min(80.0))))
                    .child(stepper_ctl("fx-border", &t!("ops.border"), format!("{:.0}", fx.border_width), set(|fx| fx.border_width = (fx.border_width - 1.0).max(0.0)), set(|fx| fx.border_width = (fx.border_width + 1.0).min(40.0))))
                    .when(fx.border_width > 0.0, |b| {
                        let mut sw = div().flex().gap_1();
                        for (i, col) in [Color::BLACK, Color::rgb(0x60, 0x60, 0x60), Color::WHITE, Color::RED, Color::rgb(0x00, 0x9C, 0xDE)].into_iter().enumerate() {
                            let up = upd(Box::new(move |m| {
                                if let ImageMode::Effects { fx } = m {
                                    fx.border_color = col;
                                }
                            }));
                            sw = sw.child(super::panel::swatch_pub(("fx-col", i), col, fx.border_color == col, up));
                        }
                        b.child(sw)
                    })
                    .child(separator())
                    .child(div().text_color(c.muted).child(t!("ops.torn").to_string()))
                    .child(chip_toggle("torn-t", t!("ops.top"), fx.torn.top, set(|fx| fx.torn.top = !fx.torn.top)))
                    .child(chip_toggle("torn-r", t!("ops.right"), fx.torn.right, set(|fx| fx.torn.right = !fx.torn.right)))
                    .child(chip_toggle("torn-b", t!("ops.bottom"), fx.torn.bottom, set(|fx| fx.torn.bottom = !fx.torn.bottom)))
                    .child(chip_toggle("torn-l", t!("ops.left"), fx.torn.left, set(|fx| fx.torn.left = !fx.torn.left)))
                    .child(separator())
                    .child(chip_toggle("fx-refl", t!("ops.reflection"), fx.reflection > 0.0, set(|fx| fx.reflection = if fx.reflection > 0.0 { 0.0 } else { 1.0 })))
                    .child(chip_toggle("fx-gray", t!("ops.grayscale"), fx.grayscale, set(|fx| fx.grayscale = !fx.grayscale)))
                    .child(stepper_ctl("fx-rot", &t!("ops.rotate"), format!("{:.0}°", fx.rotation_deg), set(|fx| fx.rotation_deg = (fx.rotation_deg - 90.0).rem_euclid(360.0)), set(|fx| fx.rotation_deg = (fx.rotation_deg + 90.0).rem_euclid(360.0))))
            }
        };
        let (a, b) = (this.clone(), this);
        bar.child(div().flex_1())
            .child(chip_toggle("mode-cancel", t!("common.cancel"), false, move |_, _, cx| {
                a.update(cx, |e, cx| e.cancel_mode(cx)).ok();
            }))
            .child(text_button("mode-apply", "check", &t!("common.apply"), true, move |_, _, cx| {
                b.update(cx, |e, cx| e.apply_mode(cx)).ok();
            }))
    }
}
