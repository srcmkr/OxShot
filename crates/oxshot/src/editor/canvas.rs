//! Canvas: sprite cache, painting, pointer and keyboard interaction.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use rust_i18n::t;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use oxshot_core::edit::{self, Handle, Tool};
use oxshot_core::{Point as DPoint, Rect as DRect, Shape};

use super::{Editor, Interaction, TextEdit};
use crate::gfx::{TiledImage, ViewMap};

/// Handle size in window pixels.
const HANDLE: f32 = 9.0;

/// Selected object: visual bounds, handles, and the live move offset.
type SelectionChrome = (DRect, Vec<(Handle, DPoint)>, (f32, f32));

/// Everything the paint closure needs, captured by value.
struct PaintData {
    map: ViewMap,
    canvas: DRect,
    base_rect: DRect,
    canvas_bg: Hsla,
    /// (object id, offset) in z-order.
    order: Vec<(u64, (f32, f32))>,
    selection: Option<SelectionChrome>,
    caret: Option<(DPoint, f32)>,
    polygon_hint: bool,
    crop: Option<DRect>,
    /// Band to cut out (vertical?, from, to) in document coordinates.
    cut: Option<(bool, f32, f32)>,
    /// Show the effects preview instead of the document.
    preview: bool,
}

impl Editor {
    /// Brings GPU textures in sync with the document: re-renders changed sprites only.
    pub(super) fn sync_gfx(&mut self, window: &mut Window) {
        let mut gfx = self.gfx.borrow_mut();
        for g in gfx.garbage.drain(..) {
            g.release(window);
        }
        let base_key = std::sync::Arc::as_ptr(&self.doc.base) as usize;
        if gfx.base.is_none() || gfx.base_key != base_key {
            if let Some(old) = gfx.base.take() {
                old.release(window);
            }
            gfx.base = Some(TiledImage::new(&self.doc.base, (0, 0)));
            gfx.base_key = base_key;
            gfx.sprites.values_mut().for_each(|s| s.0 = 0);
        }

        let live_ids: std::collections::HashSet<u64> = self.doc.objects.iter().map(|o| o.id).collect();
        let stale: Vec<u64> = gfx.sprites.keys().filter(|id| !live_ids.contains(id)).copied().collect();
        for id in stale {
            if let Some((_, Some(img))) = gfx.sprites.remove(&id) {
                img.release(window);
            }
        }

        // Key of everything below the current object (for backdrop-reading shapes).
        let mut below = DefaultHasher::new();
        base_key.hash(&mut below);
        let mut renderer = self.renderer.borrow_mut();
        for (idx, obj) in self.doc.objects.iter().enumerate() {
            let rev = self.revs.get(&obj.id).copied().unwrap_or(0);
            let reads = obj.shape.reads_backdrop();
            let key = if reads {
                let mut h = below.clone();
                rev.hash(&mut h);
                h.finish() | 1
            } else {
                rev.wrapping_mul(2) + 2
            };
            (obj.id, rev).hash(&mut below);
            if gfx.sprites.get(&obj.id).is_some_and(|s| s.0 == key) {
                continue;
            }
            let layer = if reads {
                let bkey = {
                    let mut h = DefaultHasher::new();
                    for o in &self.doc.objects[..idx] {
                        (o.id, self.revs.get(&o.id).copied().unwrap_or(0)).hash(&mut h);
                    }
                    base_key.hash(&mut h);
                    idx.hash(&mut h);
                    h.finish()
                };
                let bd = match &self.backdrop {
                    Some((k, l)) if *k == bkey => l.clone(),
                    _ => {
                        let l = Rc::new(renderer.composite(&self.doc, idx));
                        self.backdrop = Some((bkey, l.clone()));
                        l
                    }
                };
                renderer.render_object(&self.doc, obj, Some(&bd))
            } else {
                renderer.render_object(&self.doc, obj, None)
            };
            let tiled = layer.map(|l| TiledImage::new(&l.image, l.origin));
            if let Some((_, Some(old))) = gfx.sprites.insert(obj.id, (key, tiled)) {
                old.release(window);
            }
        }
    }

    pub(super) fn render_canvas(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let map = self.view_map();
        let theme = crate::theme::Colors::get();
        let mut renderer = self.renderer.borrow_mut();

        let moving = match &self.interaction {
            Interaction::Moving { id, offset, live: false, .. } => Some((*id, (offset.x, offset.y))),
            _ => None,
        };
        let order = self
            .doc
            .objects
            .iter()
            .map(|o| (o.id, moving.filter(|m| m.0 == o.id).map_or((0.0, 0.0), |m| m.1)))
            .collect();

        let selection = self.selection.and_then(|id| self.doc.get(id)).map(|o| {
            let off = moving.filter(|m| m.0 == o.id).map_or((0.0, 0.0), |m| m.1);
            let b = renderer.bounds(&self.doc, o);
            let hs = if self.text_edit.is_some() { Vec::new() } else { edit::handles(&o.shape) };
            (b, hs, off)
        });

        let caret = self.text_edit.and_then(|te| {
            let o = self.doc.get(te.id)?;
            let (origin, text, style, maxw) = match &o.shape {
                Shape::Text(t) => (t.origin, &t.text, &t.style, t.max_width),
                Shape::Callout(c) => {
                    let (origin, width) = renderer.callout_text_frame(c);
                    (origin, &c.text, &c.style, Some(width))
                }
                _ => return None,
            };
            let m = renderer.text_metrics(text, style, maxw);
            let p = m.carets.get(te.caret).copied().unwrap_or_default();
            Some((origin + p, m.line_height))
        });
        drop(renderer);

        let bg = self.doc.canvas.background;
        let data = PaintData {
            map,
            canvas: self.doc.canvas_rect(),
            base_rect: DRect::new(0.0, 0.0, self.doc.base.width() as f32, self.doc.base.height() as f32),
            canvas_bg: Rgba { r: bg.r as f32 / 255.0, g: bg.g as f32 / 255.0, b: bg.b as f32 / 255.0, a: bg.a as f32 / 255.0 }.into(),
            order,
            selection,
            caret,
            polygon_hint: matches!(self.interaction, Interaction::Polygon { .. }),
            crop: self.crop.map(|r| r.normalized()),
            cut: match &self.mode {
                Some(super::ops_ui::ImageMode::Cut { vertical, band: Some((a, b)), .. }) => Some((*vertical, a.min(*b), a.max(*b))),
                _ => None,
            },
            preview: matches!(self.mode, Some(super::ops_ui::ImageMode::Effects { .. })),
        };
        let gfx = self.gfx.clone();
        let cell = self.bounds_cell.clone();
        let accent = theme.accent;
        let crosshair = self.tool != Tool::Select || self.crop.is_some();

        div()
            .id("canvas")
            .relative()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .bg(theme.canvas_bg)
            .when(crosshair, |d| d.cursor_crosshair())
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_left_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                this.interaction = Interaction::Panning { start: ev.position, scroll: this.scroll };
                cx.notify();
            }))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_left_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_left_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(|this, _: &MouseUpEvent, _, cx| {
                this.interaction = Interaction::None;
                cx.notify();
            }))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(
                canvas(
                    move |bounds, _, _| {
                        cell.set(bounds);
                    },
                    move |bounds, _, window, _| paint(&data, &gfx.borrow(), bounds, accent, window),
                )
                .absolute()
                .size_full(),
            )
    }

    // ------------------------------------------------------------------ pointer

    fn hit(&self, p: DPoint) -> Option<u64> {
        let tol = 5.0 / self.zoom;
        let mut r = self.renderer.borrow_mut();
        self.doc
            .objects
            .iter()
            .rev()
            .find(|o| {
                let b = r.bounds(&self.doc, o);
                edit::hit_test(&o.shape, p, tol, b)
            })
            .map(|o| o.id)
    }

    fn hit_handle(&self, screen: Point<Pixels>) -> Option<Handle> {
        let id = self.selection?;
        let o = self.doc.get(id)?;
        let map = self.view_map();
        edit::handles(&o.shape)
            .into_iter()
            .rev()
            .find(|(_, p)| {
                let s = map.to_screen(p.x, p.y);
                (f32::from(s.x - screen.x)).abs() <= HANDLE && (f32::from(s.y - screen.y)).abs() <= HANDLE
            })
            .map(|(h, _)| h)
    }

    fn on_left_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let p = self.view_map().to_doc(ev.position);
        let ctrl = ev.modifiers.control;

        if let Some(super::ops_ui::ImageMode::Cut { vertical, band, .. }) = &mut self.mode {
            let v = if *vertical { p.x } else { p.y };
            *band = Some((v.round(), v.round()));
            self.interaction = Interaction::CutBand;
            cx.notify();
            return;
        }
        if self.mode.is_some() {
            return;
        }
        if let Some(r) = self.crop {
            let r = r.normalized();
            if ev.click_count >= 2 && r.contains(p) {
                self.apply_crop(cx);
                return;
            }
            let map = self.view_map();
            let near = edit::rect_handles(r).iter().position(|h| {
                let s = map.to_screen(h.x, h.y);
                f32::from(s.x - ev.position.x).abs() <= HANDLE && f32::from(s.y - ev.position.y).abs() <= HANDLE
            });
            self.interaction = match near {
                Some(i) => Interaction::CropHandle(i as u8),
                None if r.contains(p) => Interaction::CropMove { start: p, orig: r },
                None => Interaction::CropNew { start: p },
            };
            cx.notify();
            return;
        }

        // Polygon: every click adds a vertex, double click finishes.
        if let Interaction::Polygon { id } = self.interaction {
            if ev.click_count >= 2 {
                self.finish_pending(cx);
            } else {
                self.mutate(id, |o| {
                    if let Shape::Polygon(poly) = &mut o.shape {
                        poly.points.push(p);
                    }
                });
            }
            cx.notify();
            return;
        }

        // Clicking while editing text: inside the same object moves the caret, else commit.
        if let Some(te) = self.text_edit {
            if self.hit(p) == Some(te.id) {
                return;
            }
            self.finish_pending(cx);
        }

        // Handles of the selected object always win.
        if let Some(h) = self.hit_handle(ev.position) {
            let id = self.selection.unwrap();
            self.checkpoint();
            self.interaction = Interaction::Handle { id, handle: h };
            cx.notify();
            return;
        }

        let hit = self.hit(p);
        let editing_tool_on_text = matches!(self.tool, Tool::Text | Tool::Callout | Tool::Select)
            && ev.click_count >= 2
            && hit.is_some_and(|id| self.doc.get(id).is_some_and(|o| matches!(o.shape, Shape::Text(_) | Shape::Callout(_))));
        if editing_tool_on_text {
            let id = hit.unwrap();
            self.selection = Some(id);
            self.start_text_edit(id);
            cx.notify();
            return;
        }

        if self.tool == Tool::Select || (hit.is_some() && hit == self.selection) {
            match hit {
                Some(mut id) => {
                    self.checkpoint();
                    if ctrl {
                        // Ctrl+drag duplicates (Screenpresso).
                        if let Some(o) = self.doc.get(id).cloned() {
                            id = self.doc.push(o.shape);
                            let (op, sh) = (o.opacity, o.shadow);
                            self.mutate(id, |n| {
                                n.opacity = op;
                                n.shadow = sh;
                            });
                        }
                    }
                    self.selection = Some(id);
                    let live = self.doc.get(id).is_some_and(|o| o.shape.reads_backdrop()) || ev.modifiers.alt;
                    self.interaction =
                        Interaction::Moving { id, start: p, offset: DPoint::default(), body_only: ev.modifiers.alt, live };
                }
                None => self.selection = None,
            }
            cx.notify();
            return;
        }

        // Drawing tools. Alt = draw with the color under the pointer (Screenpresso).
        let mut style = self.style();
        if ev.modifiers.alt
            && let Some(c) = self.color_at(p)
        {
            style.color = c;
            style.text.color = c;
        }
        self.selection = None;
        match self.tool {
            Tool::Text => {
                self.checkpoint();
                let shape = edit::create_shape(Tool::Text, &style, p, p, false, 0).unwrap();
                let id = self.push_styled(shape, &style);
                self.selection = Some(id);
                self.start_text_edit(id);
            }
            Tool::Stamp => self.insert_stamp_from_file(cx),
            tool => {
                let n = self.doc.next_step_number();
                if let Some(shape) = edit::create_shape(tool, &style, p, p, ctrl, n) {
                    self.checkpoint();
                    let id = self.push_styled(shape, &style);
                    self.selection = Some(id);
                    self.interaction = if tool == Tool::Polygon {
                        Interaction::Polygon { id }
                    } else {
                        Interaction::Creating { id, start: p }
                    };
                }
            }
        }
        cx.notify();
    }

    fn color_at(&self, p: DPoint) -> Option<oxshot_core::Color> {
        let (x, y) = (p.x.floor() as i64, p.y.floor() as i64);
        let b = &self.doc.base;
        (x >= 0 && y >= 0 && (x as u32) < b.width() && (y as u32) < b.height()).then(|| {
            let px = b.get_pixel(x as u32, y as u32);
            oxshot_core::Color::rgb(px[0], px[1], px[2])
        })
    }

    fn push_styled(&mut self, shape: Shape, style: &oxshot_core::edit::Style) -> u64 {
        let id = self.doc.push(shape);
        let (op, sh) = (style.opacity, style.shadow);
        self.mutate(id, |o| {
            o.opacity = op;
            o.shadow = sh;
        });
        id
    }

    fn start_text_edit(&mut self, id: u64) {
        let len = self.doc.get(id).map_or(0, |o| match &o.shape {
            Shape::Text(t) => t.text.chars().count(),
            Shape::Callout(c) => c.text.chars().count(),
            _ => 0,
        });
        self.undo_coalesce_text = false;
        self.text_edit = Some(TextEdit { id, caret: len });
    }

    fn on_right_down(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        // Right click ends the polygon / unselects the drawing tool (Screenpresso).
        if matches!(self.interaction, Interaction::Polygon { .. }) || self.text_edit.is_some() {
            self.finish_pending(cx);
        } else {
            self.tool = Tool::Select;
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.view_map().to_doc(ev.position);
        let ctrl = ev.modifiers.control;
        let round = |r: DRect| {
            let r = r.normalized();
            DRect::new(r.x.round(), r.y.round(), r.w.round(), r.h.round())
        };
        match self.interaction.clone() {
            Interaction::None => {}
            Interaction::CropHandle(i) => {
                if let Some(r) = self.crop {
                    let mut tmp = Shape::Rect(oxshot_core::RectShape { rect: r.normalized(), stroke: None, fill: None, corner_radius: 0.0 });
                    edit::drag_handle(&mut tmp, Handle::Resize(i), p, ctrl);
                    if let Some(nr) = edit::shape_rect(&tmp) {
                        self.crop = Some(round(nr));
                    }
                    cx.notify();
                }
            }
            Interaction::CutBand => {
                if let Some(super::ops_ui::ImageMode::Cut { vertical, band: Some((_, b)), .. }) = &mut self.mode {
                    *b = if *vertical { p.x.round() } else { p.y.round() };
                    cx.notify();
                }
            }
            Interaction::CropMove { start, orig } => {
                self.crop = Some(round(orig.translate(p - start)));
                cx.notify();
            }
            Interaction::CropNew { start } => {
                let b = if ctrl { edit::square_corner(start, p) } else { p };
                self.crop = Some(round(DRect::from_points(start, b)));
                cx.notify();
            }
            Interaction::Panning { start, scroll } => {
                self.fit = false;
                self.scroll = DPoint::new(
                    scroll.x - f32::from(ev.position.x - start.x) / self.zoom,
                    scroll.y - f32::from(ev.position.y - start.y) / self.zoom,
                );
                cx.notify();
            }
            Interaction::Creating { id, start } => {
                if ev.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let style = self.style();
                let tool = self.doc.get(id).map(|o| Tool::of_shape(&o.shape));
                self.mutate(id, |o| match (&mut o.shape, tool) {
                    (Shape::Freehand(f), _) => {
                        if f.points.last().is_none_or(|l| l.distance(p) >= 1.5) {
                            f.points.push(p);
                        }
                    }
                    (shape, Some(tool)) => {
                        let number = match shape {
                            Shape::Step(s) => s.number,
                            _ => 0,
                        };
                        if let Some(new) = edit::create_shape(tool, &style, start, p, ctrl, number) {
                            *shape = new;
                        }
                    }
                    _ => {}
                });
                cx.notify();
            }
            Interaction::Moving { id, start, body_only, live, .. } => {
                if ev.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let mut d = p - start;
                if ev.modifiers.shift {
                    // Constrain to the dominant axis.
                    if d.x.abs() > d.y.abs() { d.y = 0.0 } else { d.x = 0.0 }
                }
                if live {
                    // Apply incrementally; `start` follows so we only add the delta.
                    self.mutate(id, |o| {
                        if body_only { edit::translate_body(&mut o.shape, d) } else { edit::translate(&mut o.shape, d) }
                    });
                    self.interaction = Interaction::Moving { id, start: start + d, offset: DPoint::default(), body_only, live };
                } else {
                    self.interaction = Interaction::Moving { id, start, offset: d, body_only, live };
                }
                cx.notify();
            }
            Interaction::Handle { id, handle } => {
                if ev.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                self.mutate(id, |o| edit::drag_handle(&mut o.shape, handle, p, ctrl));
                cx.notify();
            }
            Interaction::Polygon { id } => {
                // The last vertex follows the pointer as a preview.
                self.mutate(id, |o| {
                    if let Shape::Polygon(poly) = &mut o.shape
                        && let Some(last) = poly.points.last_mut()
                    {
                        *last = p;
                    }
                });
                cx.notify();
            }
        }
    }

    fn on_left_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        match self.interaction.clone() {
            Interaction::Creating { id, .. } => {
                self.interaction = Interaction::None;
                let shape = self.doc.get(id).map(|o| o.shape.clone());
                match shape {
                    Some(s) if edit::is_degenerate(&s) => {
                        self.remove(id);
                        // Discard the checkpoint of the aborted creation.
                        self.undo.pop();
                    }
                    Some(Shape::Callout(_)) => self.start_text_edit(id),
                    _ => {}
                }
            }
            Interaction::Moving { id, offset, live: false, .. } => {
                self.interaction = Interaction::None;
                if offset.length() > 0.0 {
                    self.mutate(id, |o| edit::translate(&mut o.shape, offset));
                } else {
                    self.undo.pop();
                }
            }
            Interaction::Polygon { .. } => return,
            Interaction::Panning { .. } => return,
            _ => self.interaction = Interaction::None,
        }
        cx.notify();
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = ev.delta.pixel_delta(px(40.0));
        if ev.modifiers.control {
            let dy = f32::from(delta.y);
            if dy.abs() > 0.1 {
                self.zoom_step(dy.signum(), Some(ev.position), cx);
            }
            return;
        }
        let (mut dx, mut dy) = (f32::from(delta.x), f32::from(delta.y));
        if ev.modifiers.shift {
            std::mem::swap(&mut dx, &mut dy);
        }
        self.fit = false;
        self.scroll = DPoint::new(self.scroll.x - dx / self.zoom, self.scroll.y - dy / self.zoom);
        cx.notify();
    }

    // ------------------------------------------------------------------ keyboard

    pub(super) fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        let key = ks.key.as_str();

        if self.text_edit.is_some() && !m.control {
            self.text_key(ks, cx);
            return;
        }
        if self.mode_key(key, cx) {
            return;
        }
        if self.crop.is_some() {
            match key {
                "enter" => self.apply_crop(cx),
                "escape" | "c" => self.cancel_crop(cx),
                _ => {}
            }
            return;
        }
        if !m.control && !m.alt && key == "c" {
            self.start_crop(cx);
            return;
        }

        match (m.control, m.shift, key) {
            (true, false, "c") => self.copy(cx),
            (true, _, "enter") => self.copy_and_close(window, cx),
            (true, false, "z") => self.undo(cx),
            (true, true, "z") | (true, false, "y") => self.redo(cx),
            (true, false, "s") => {
                self.finish_pending(cx);
                self.modified = true;
                self.autosave();
                self.status = match &self.file {
                    Some(f) => t!("status.saved", path = f.display()).to_string().into(),
                    None => t!("status.save_failed_plain").to_string().into(),
                };
            }
            (true, true, "s") => self.save_as(cx),
            (true, false, "v") => self.paste(cx),
            (true, false, "w") => {
                self.autosave();
                window.remove_window();
            }
            (true, false, "d") => self.duplicate(cx),
            (true, false, "a") => {
                self.tool = Tool::Select;
                self.selection = self.doc.objects.last().map(|o| o.id);
            }
            (true, _, "=" | "+") => self.size_or_zoom(1.0, cx),
            (true, false, "-") => self.size_or_zoom(-1.0, cx),
            (true, false, "0") => {
                self.fit = true;
                cx.notify();
            }
            (true, false, "1") => self.set_zoom(1.0, None, cx),
            (_, false, "pageup") => self.navigate(-1, window, cx),
            (_, false, "pagedown") => self.navigate(1, window, cx),
            (false, false, "f11") => window.toggle_fullscreen(),
            (false, _, "delete" | "backspace") => {
                if let Some(id) = self.selection {
                    self.checkpoint();
                    self.remove(id);
                }
            }
            (false, _, "escape") => {
                if matches!(self.interaction, Interaction::Polygon { .. }) {
                    self.finish_pending(cx);
                } else if self.selection.is_some() {
                    self.selection = None;
                } else {
                    self.tool = Tool::Select;
                }
            }
            (false, false, "enter") => {
                if matches!(self.interaction, Interaction::Polygon { .. }) {
                    self.finish_pending(cx);
                } else if let Some(id) = self.selection
                    && self.doc.get(id).is_some_and(|o| matches!(o.shape, Shape::Text(_) | Shape::Callout(_)))
                {
                    self.start_text_edit(id);
                }
            }
            (false, shift, "left" | "right" | "up" | "down") => {
                if let Some(id) = self.selection {
                    let s = if shift { 10.0 } else { 1.0 };
                    let d = match key {
                        "left" => DPoint::new(-s, 0.0),
                        "right" => DPoint::new(s, 0.0),
                        "up" => DPoint::new(0.0, -s),
                        _ => DPoint::new(0.0, s),
                    };
                    self.checkpoint();
                    self.mutate(id, |o| edit::translate(&mut o.shape, d));
                }
            }
            (false, false, k) => {
                if let Some(t) = Tool::from_shortcut(k) {
                    self.set_tool(t, cx);
                }
            }
            _ => return,
        }
        cx.notify();
    }

    fn size_or_zoom(&mut self, dir: f32, cx: &mut Context<Self>) {
        let sized = self.selection.and_then(|id| self.doc.get(id)).is_some_and(|o| {
            matches!(o.shape, Shape::Text(_) | Shape::Callout(_) | Shape::Step(_))
        });
        if sized {
            self.set_style(cx, |st| {
                st.text.size = (st.text.size + dir * 2.0).clamp(6.0, 200.0);
                st.size = (st.size + dir * 2.0).clamp(6.0, 120.0);
            });
        } else {
            self.zoom_step(dir, None, cx);
        }
    }

    fn duplicate(&mut self, cx: &mut Context<Self>) {
        let Some(o) = self.selection.and_then(|id| self.doc.get(id)).cloned() else { return };
        self.checkpoint();
        let mut shape = o.shape;
        edit::translate(&mut shape, DPoint::new(16.0, 16.0));
        let id = self.doc.push(shape);
        self.mutate(id, |n| {
            n.opacity = o.opacity;
            n.shadow = o.shadow;
        });
        self.selection = Some(id);
        cx.notify();
    }

    fn text_key(&mut self, ks: &Keystroke, cx: &mut Context<Self>) {
        let Some(mut te) = self.text_edit else { return };
        let Some(text) = self.doc.get(te.id).and_then(|o| match &o.shape {
            Shape::Text(t) => Some(t.text.clone()),
            Shape::Callout(c) => Some(c.text.clone()),
            _ => None,
        }) else {
            return;
        };
        let mut chars: Vec<char> = text.chars().collect();
        te.caret = te.caret.min(chars.len());
        match ks.key.as_str() {
            "escape" => {
                self.finish_pending(cx);
                return;
            }
            "enter" if ks.modifiers.shift || self.doc.get(te.id).is_some_and(|o| matches!(o.shape, Shape::Text(_) | Shape::Callout(_))) => {
                chars.insert(te.caret, '\n');
                te.caret += 1;
            }
            "backspace" => {
                if te.caret > 0 {
                    chars.remove(te.caret - 1);
                    te.caret -= 1;
                }
            }
            "delete" => {
                if te.caret < chars.len() {
                    chars.remove(te.caret);
                }
            }
            "left" => te.caret = te.caret.saturating_sub(1),
            "right" => te.caret = (te.caret + 1).min(chars.len()),
            "home" => te.caret = chars[..te.caret].iter().rposition(|c| *c == '\n').map_or(0, |i| i + 1),
            "end" => te.caret += chars[te.caret..].iter().position(|c| *c == '\n').unwrap_or(chars.len() - te.caret),
            _ => {
                let Some(s) = ks.key_char.as_ref().filter(|s| !s.chars().any(|c| c.is_control())) else { return };
                for c in s.chars() {
                    chars.insert(te.caret, c);
                    te.caret += 1;
                }
            }
        }
        let new: String = chars.into_iter().collect();
        if new != text && !self.undo_coalesce_text {
            self.checkpoint();
            self.undo_coalesce_text = true;
        }
        let renderer = self.renderer.clone();
        self.mutate(te.id, |o| match &mut o.shape {
            Shape::Text(t) => t.text = new,
            Shape::Callout(c) => {
                c.text = new;
                // Grow the bubble to fit its text.
                let mut r = renderer.borrow_mut();
                let (_, width) = r.callout_text_frame(c);
                let m = r.text_metrics(&c.text, &c.style, Some(width));
                c.rect.h = c.rect.h.max(m.height + 2.0 * c.padding);
            }
            _ => {}
        });
        self.text_edit = Some(te);
        cx.notify();
    }
}

fn hsla_white(a: f32) -> Hsla {
    hsla(0.0, 0.0, 1.0, a)
}

fn paint(d: &PaintData, gfx: &super::Gfx, bounds: Bounds<Pixels>, accent: Hsla, window: &mut Window) {
    let map = d.map;
    let rect_screen = |r: &DRect| {
        let o = map.to_screen(r.x, r.y);
        Bounds { origin: o, size: size(px(r.w * map.zoom), px(r.h * map.zoom)) }
    };
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        // Canvas: drop shadow, checkerboard for transparency, background color.
        let cb = rect_screen(&d.canvas);
        window.paint_quad(fill(cb.dilate(px(1.0)), hsla(0.0, 0.0, 0.0, 0.5)));
        paint_checker(window, cb);
        window.paint_quad(fill(cb, d.canvas_bg));

        // Base image and objects. Outside crop mode everything is clipped to the canvas,
        // exactly like the exported image; while cropping the whole base is visible.
        if d.preview
            && let Some((img, s)) = &gfx.preview
        {
            // Effects preview: the downscaled result, centered on the canvas.
            let (w, h) = (img.width as f32 / s, img.height as f32 / s);
            let c = d.canvas.center();
            let pmap = ViewMap { origin: map.to_screen(c.x - w / 2.0, c.y - h / 2.0), zoom: map.zoom / s };
            let pb = Bounds { origin: pmap.origin, size: size(px(w * map.zoom), px(h * map.zoom)) };
            window.paint_quad(fill(bounds, crate::theme::Colors::get().canvas_bg));
            paint_checker(window, pb);
            img.paint(window, bounds, &pmap, (0.0, 0.0));
            return;
        }
        let clip = if d.crop.is_some() { bounds } else { bounds.intersect(&cb) };
        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
            if let Some(base) = &gfx.base {
                base.paint(window, clip, &map, (0.0, 0.0));
            }
            for (id, off) in &d.order {
                if let Some((_, Some(img))) = gfx.sprites.get(id) {
                    img.paint(window, clip, &map, *off);
                }
            }
        });
        let _ = d.base_rect;

        // Selection chrome.
        if let Some((b, hs, off)) = &d.selection {
            let b = b.translate(DPoint::new(off.0, off.1));
            let sb = rect_screen(&b);
            window.paint_quad(quad(sb.dilate(px(2.0)), px(2.0), transparent_black(), px(1.0), accent, BorderStyle::Dashed));
            for (h, p) in hs {
                let s = map.to_screen(p.x + off.0, p.y + off.1);
                let special = !matches!(h, Handle::Resize(_) | Handle::Vertex(_));
                let r = px(HANDLE / 2.0);
                let hb = Bounds { origin: point(s.x - r, s.y - r), size: size(r * 2.0, r * 2.0) };
                let (bg, radius) = if special { (hsla(0.08, 0.95, 0.55, 1.0), r) } else { (hsla_white(1.0), px(2.0)) };
                window.paint_quad(quad(hb, radius, bg, px(1.5), accent, BorderStyle::Solid));
            }
        }
        if let Some((vertical, a, b)) = d.cut {
            let r = if vertical {
                DRect::new(a, d.canvas.y, b - a, d.canvas.h)
            } else {
                DRect::new(d.canvas.x, a, d.canvas.w, b - a)
            };
            let sb = rect_screen(&r);
            window.paint_quad(quad(sb, px(0.0), hsla(0.0, 0.85, 0.55, 0.35), px(1.0), hsla(0.0, 0.85, 0.6, 1.0), BorderStyle::Dashed));
        }
        if let Some(r) = &d.crop {
            let cr = rect_screen(r);
            dim_outside(window, bounds, cr);
            window.paint_quad(quad(cr, px(0.0), transparent_black(), px(1.0), hsla_white(0.9), BorderStyle::Solid));
            // Rule-of-thirds guides.
            let guide = hsla_white(0.25);
            for k in 1..3 {
                let fx = cr.origin.x + cr.size.width * (k as f32 / 3.0);
                let fy = cr.origin.y + cr.size.height * (k as f32 / 3.0);
                window.paint_quad(fill(Bounds { origin: point(fx, cr.top()), size: size(px(1.0), cr.size.height) }, guide));
                window.paint_quad(fill(Bounds { origin: point(cr.left(), fy), size: size(cr.size.width, px(1.0)) }, guide));
            }
            for h in edit::rect_handles(*r) {
                let s = map.to_screen(h.x, h.y);
                let rr = px(HANDLE / 2.0);
                let hb = Bounds { origin: point(s.x - rr, s.y - rr), size: size(rr * 2.0, rr * 2.0) };
                window.paint_quad(quad(hb, px(2.0), hsla_white(1.0), px(1.5), accent, BorderStyle::Solid));
            }
        }
        if let Some((p, h)) = d.caret {
            let s = map.to_screen(p.x, p.y);
            let cb = Bounds { origin: s, size: size(px(2.0), px(h * map.zoom)) };
            window.paint_quad(fill(cb, accent));
        }
        let _ = d.polygon_hint;
    });
}

fn paint_checker(window: &mut Window, b: Bounds<Pixels>) {
    window.paint_quad(fill(b, hsla(0.0, 0.0, 0.85, 1.0)));
    let s = 12.0;
    let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
    if w * h / (s * s) > 40_000.0 {
        return;
    }
    let mut y = 0.0;
    let mut row = 0;
    while y < h {
        let mut x = if row % 2 == 0 { 0.0 } else { s };
        while x < w {
            let cell = Bounds {
                origin: point(b.origin.x + px(x), b.origin.y + px(y)),
                size: size(px(s.min(w - x)), px(s.min(h - y))),
            };
            window.paint_quad(fill(cell, hsla(0.0, 0.0, 0.7, 1.0)));
            x += 2.0 * s;
        }
        y += s;
        row += 1;
    }
}

/// Darkens parts of `inner` (the base image) that lie outside the canvas `keep`.
fn dim_outside(window: &mut Window, inner: Bounds<Pixels>, keep: Bounds<Pixels>) {
    let dim = hsla(0.0, 0.0, 0.0, 0.55);
    let (il, it, ir, ib) = (inner.left(), inner.top(), inner.right(), inner.bottom());
    let (kl, kt, kr, kb) = (keep.left().max(il), keep.top().max(it), keep.right().min(ir), keep.bottom().min(ib));
    let band = |window: &mut Window, l: Pixels, t: Pixels, r: Pixels, b: Pixels| {
        if r > l && b > t {
            window.paint_quad(fill(Bounds::from_corners(point(l, t), point(r, b)), dim));
        }
    };
    band(window, il, it, ir, kt);
    band(window, il, kb, ir, ib);
    band(window, il, kt, kl, kb);
    band(window, kr, kt, ir, kb);
}
