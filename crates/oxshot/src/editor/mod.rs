//! The annotation editor window: a vector document over the screenshot, edited with the
//! Screenpresso tool set. `Ctrl+C` copies the flattened result.

mod canvas;
mod ops_ui;
mod panel;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use rust_i18n::t;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use oxshot_core::edit::{self, Handle, Style, Tool};
use oxshot_core::render::Renderer;
use oxshot_core::{Document, Object, ObjectId, Point as DPoint, Shape};

use crate::gfx::{TiledImage, ViewMap};

/// Fixed height of the property bar (it must not change between tools).
const PROPS_HEIGHT: f32 = 44.0;

/// Renderer shared by all editor windows (font system is expensive to create).
pub struct SharedRenderer(pub Rc<RefCell<Renderer>>);
impl Global for SharedRenderer {}

/// All open editors, so quitting can save them.
#[derive(Default)]
pub struct OpenEditors(pub Vec<WeakEntity<Editor>>);
impl Global for OpenEditors {}

/// Saves every open editor (called before the app quits).
pub fn save_all(cx: &mut App) {
    let editors = cx.try_global::<OpenEditors>().map(|e| e.0.clone()).unwrap_or_default();
    for e in editors {
        e.update(cx, |e, cx| {
            e.finish_pending(cx);
            e.autosave();
        })
        .ok();
    }
}

pub fn renderer(cx: &App) -> Rc<RefCell<Renderer>> {
    cx.global::<SharedRenderer>().0.clone()
}

/// Everything undo/redo needs to restore.
#[derive(Clone)]
struct DocState {
    objects: Vec<Object>,
    canvas: oxshot_core::Canvas,
    base: oxshot_core::SharedImage,
    resources: std::collections::BTreeMap<String, oxshot_core::SharedImage>,
}

#[derive(Debug, Clone)]
enum Interaction {
    None,
    /// A shape is being drawn by dragging.
    Creating { id: ObjectId, start: DPoint },
    /// Selected object(s) are being moved. Non-backdrop sprites are painted with
    /// `offset` and only committed on release, so moving never re-renders.
    Moving { id: ObjectId, start: DPoint, offset: DPoint, body_only: bool, live: bool },
    Handle { id: ObjectId, handle: Handle },
    Panning { start: Point<Pixels>, scroll: DPoint },
    /// Polygon being built click by click.
    Polygon { id: ObjectId },
    /// Crop mode: resizing / moving / drawing the crop rectangle.
    CropHandle(u8),
    CropMove { start: DPoint, orig: oxshot_core::Rect },
    CropNew { start: DPoint },
    /// Dragging the band for "cut out".
    CutBand,
}

#[derive(Debug, Clone, Copy)]
struct TextEdit {
    id: ObjectId,
    /// Caret as char index.
    caret: usize,
}

/// GPU-side cache of the base image and object sprites.
#[derive(Default)]
pub struct Gfx {
    base: Option<TiledImage>,
    base_key: usize,
    sprites: HashMap<ObjectId, (u64, Option<TiledImage>)>,
    garbage: Vec<TiledImage>,
    /// Effects preview (image, scale relative to the document).
    preview: Option<(TiledImage, f32)>,
}

pub struct Editor {
    doc: Document,
    /// Where the flattened PNG is (auto)saved.
    pub file: Option<PathBuf>,
    renderer: Rc<RefCell<Renderer>>,
    undo: Vec<DocState>,
    redo: Vec<DocState>,
    tool: Tool,
    styles: HashMap<Tool, Style>,
    selection: Option<ObjectId>,
    interaction: Interaction,
    text_edit: Option<TextEdit>,
    /// Document point shown at the center of the viewport.
    scroll: DPoint,
    zoom: f32,
    fit: bool,
    revs: HashMap<ObjectId, u64>,
    rev_counter: u64,
    gfx: Rc<RefCell<Gfx>>,
    bounds_cell: Rc<std::cell::Cell<Bounds<Pixels>>>,
    focus: FocusHandle,
    status: SharedString,
    modified: bool,
    /// Active crop rectangle (crop mode), in document coordinates.
    crop: Option<oxshot_core::Rect>,
    /// Resize / cut-out / effects mode.
    mode: Option<ops_ui::ImageMode>,
    preview_dirty: bool,
    /// Horizontal scroll position of the property bar.
    props_scroll: ScrollHandle,
    /// Typing into a text object records only one undo step.
    undo_coalesce_text: bool,
    /// Backdrop composite cache: (objects-below key, layer).
    backdrop: Option<(u64, Rc<oxshot_core::render::Layer>)>,
}

impl Editor {
    pub fn new(doc: Document, file: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        // Closing the window keeps the work (Screenpresso saves into the workspace).
        let weak = cx.entity().downgrade();
        let mut list = cx.try_global::<OpenEditors>().map(|e| e.0.clone()).unwrap_or_default();
        list.retain(|e| e.upgrade().is_some());
        list.push(weak.clone());
        cx.set_global(OpenEditors(list));
        window.on_window_should_close(cx, move |_, cx| {
            weak.update(cx, |e, _| e.autosave()).ok();
            true
        });
        let styles = Tool::ALL.iter().map(|t| (*t, crate::settings::style_for(*t, cx))).collect();
        let center = doc.canvas_rect().center();
        Self {
            doc,
            file,
            renderer: renderer(cx),
            undo: Vec::new(),
            redo: Vec::new(),
            tool: Tool::Arrow,
            styles,
            selection: None,
            interaction: Interaction::None,
            text_edit: None,
            scroll: center,
            zoom: 1.0,
            fit: true,
            revs: HashMap::new(),
            rev_counter: 1,
            gfx: Rc::new(RefCell::new(Gfx::default())),
            bounds_cell: Rc::new(std::cell::Cell::new(Bounds::default())),
            focus,
            status: "".into(),
            modified: false,
            undo_coalesce_text: false,
            props_scroll: ScrollHandle::new(),
            crop: None,
            mode: None,
            preview_dirty: false,
            backdrop: None,
        }
    }

    // ------------------------------------------------------------------ document changes

    fn state(&self) -> DocState {
        DocState {
            objects: self.doc.objects.clone(),
            canvas: self.doc.canvas.clone(),
            base: self.doc.base.clone(),
            resources: self.doc.resources.clone(),
        }
    }

    /// Records an undo checkpoint. Call *before* mutating.
    fn checkpoint(&mut self) {
        self.undo.push(self.state());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.modified = true;
    }

    fn restore(&mut self, st: DocState) {
        let ids: Vec<ObjectId> = st.objects.iter().map(|o| o.id).collect();
        self.doc = Document::from_parts(st.base, st.canvas, st.objects, st.resources);
        // Objects may have changed arbitrarily: invalidate everything.
        for id in ids {
            self.touch(id);
        }
        if self.selection.is_some_and(|id| self.doc.get(id).is_none()) {
            self.selection = None;
        }
        self.text_edit = None;
        self.interaction = Interaction::None;
        self.modified = true;
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        self.finish_pending(cx);
        if let Some(st) = self.undo.pop() {
            self.redo.push(self.state());
            self.restore(st);
            cx.notify();
        }
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        if let Some(st) = self.redo.pop() {
            self.undo.push(self.state());
            self.restore(st);
            cx.notify();
        }
    }

    /// Marks an object as changed (its sprite will be re-rendered).
    fn touch(&mut self, id: ObjectId) {
        self.rev_counter += 1;
        self.revs.insert(id, self.rev_counter);
    }

    fn mutate(&mut self, id: ObjectId, f: impl FnOnce(&mut Object)) {
        if let Some(o) = self.doc.get_mut(id) {
            f(o);
            self.touch(id);
        }
    }

    fn remove(&mut self, id: ObjectId) {
        self.doc.objects.retain(|o| o.id != id);
        if self.selection == Some(id) {
            self.selection = None;
        }
        if self.text_edit.is_some_and(|t| t.id == id) {
            self.text_edit = None;
        }
    }

    fn style(&self) -> Style {
        let tool = self.style_tool();
        self.styles.get(&tool).cloned().unwrap_or_else(|| Style::default_for(tool))
    }

    /// The tool whose style the property bar shows: the selected object's, else the active tool.
    fn style_tool(&self) -> Tool {
        self.selection.and_then(|id| self.doc.get(id)).map_or(self.tool, |o| Tool::of_shape(&o.shape))
    }

    /// Style shown in the property bar.
    fn current_style(&self) -> Style {
        match self.selection.and_then(|id| self.doc.get(id)) {
            Some(o) => edit::style_of(o, &self.style()),
            None => self.style(),
        }
    }

    /// Updates the style of the selected object (or the tool default) and remembers it as
    /// the new default for that tool, like Screenpresso's "Set default".
    fn set_style(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Style)) {
        let mut st = self.current_style();
        f(&mut st);
        let tool = self.style_tool();
        self.styles.insert(tool, st.clone());
        crate::settings::remember_style(tool, &st, cx);
        if let Some(id) = self.selection {
            self.checkpoint();
            self.mutate(id, |o| edit::apply_style(o, &st));
        }
        cx.notify();
    }

    fn set_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.finish_pending(cx);
        self.tool = tool;
        // Every tool's options start at the left edge of the property bar.
        self.props_scroll.set_offset(Point::default());
        if tool != Tool::Select {
            self.selection = None;
        }
        if tool == Tool::Stamp {
            self.insert_stamp_from_file(cx);
        }
        cx.notify();
    }

    /// Completes in-progress polygon/text editing.
    fn finish_pending(&mut self, cx: &mut Context<Self>) {
        if let Interaction::Polygon { id } = self.interaction {
            self.interaction = Interaction::None;
            // The last point follows the mouse; drop it.
            self.mutate(id, |o| {
                if let Shape::Polygon(p) = &mut o.shape
                    && p.points.len() > 2
                {
                    p.points.pop();
                }
            });
            if self.doc.get(id).is_some_and(|o| edit::is_degenerate(&o.shape)) {
                self.remove(id);
            }
        }
        self.undo_coalesce_text = false;
        if let Some(te) = self.text_edit.take() {
            let empty = self.doc.get(te.id).is_some_and(|o| match &o.shape {
                Shape::Text(t) => t.text.trim().is_empty(),
                _ => false,
            });
            if empty {
                self.remove(te.id);
            }
        }
        cx.notify();
    }

    // ------------------------------------------------------------------ output

    pub fn flatten(&self) -> image::RgbaImage {
        self.renderer.borrow_mut().flatten(&self.doc)
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        self.finish_pending(cx);
        let img = self.flatten();
        // Save first, so the clipboard can offer the file as well as the image.
        self.autosave_image(&img);
        match crate::clipboard::set_capture(&img, self.file.as_deref()) {
            Ok(()) => self.status = t!("status.copied", w = img.width(), h = img.height()).to_string().into(),
            Err(e) => self.status = t!("status.copy_failed", error = e).to_string().into(),
        }
        cx.notify();
    }

    fn copy_and_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.copy(cx);
        window.remove_window();
    }

    /// Saves the flattened PNG and the editable project next to it.
    fn autosave(&mut self) {
        if !self.modified && self.file.as_ref().is_some_and(|f| f.exists()) {
            return;
        }
        let img = self.flatten();
        self.autosave_image(&img);
    }

    /// Like [`Self::autosave`] with an already flattened image.
    fn autosave_image(&mut self, img: &image::RgbaImage) {
        if !self.modified && self.file.as_ref().is_some_and(|f| f.exists()) {
            return;
        }
        match crate::storage::save_capture(&self.doc, img, self.file.as_deref()) {
            Ok(path) => {
                self.file = Some(path);
                self.modified = false;
            }
            Err(e) => log::error!("autosave failed: {e:#}"),
        }
    }

    fn save_as(&mut self, cx: &mut Context<Self>) {
        self.finish_pending(cx);
        let dir = crate::storage::captures_dir();
        let name = self
            .file
            .as_ref()
            .and_then(|f| f.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "oxshot.png".into());
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        let img = self.flatten();
        let doc = self.doc.clone();
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                let res = crate::storage::export(&doc, &img, &path);
                this.update(cx, |this, cx| {
                    this.status = match res {
                        Ok(()) => t!("status.saved", path = path.display()).to_string().into(),
                        Err(e) => t!("status.save_failed", error = format!("{e:#}")).to_string().into(),
                    };
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn insert_stamp_from_file(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("dialog.insert_image").to_string().into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.first()
            {
                match image::open(path) {
                    Ok(img) => {
                        this.update(cx, |this, cx| this.insert_image(img.to_rgba8(), cx)).ok();
                    }
                    Err(e) => log::warn!("cannot open {}: {e}", path.display()),
                }
            }
            this.update(cx, |this, cx| {
                this.tool = Tool::Select;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Inserts an image as a stamp object centered in the view.
    fn insert_image(&mut self, img: image::RgbaImage, cx: &mut Context<Self>) {
        self.checkpoint();
        let key = format!("img{}", self.doc.alloc_id());
        let (w, h) = (img.width() as f32, img.height() as f32);
        self.doc.resources.insert(key.clone(), std::sync::Arc::new(img));
        let c = self.scroll;
        let max = self.doc.canvas_rect().w.max(self.doc.canvas_rect().h) * 0.8;
        let s = (max / w.max(h)).min(1.0);
        let rect = oxshot_core::Rect::new(c.x - w * s / 2.0, c.y - h * s / 2.0, w * s, h * s);
        let id = self.doc.push(Shape::Stamp(oxshot_core::StampShape { rect, resource: key }));
        self.touch(id);
        self.selection = Some(id);
        self.tool = Tool::Select;
        cx.notify();
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(img) = crate::clipboard::get_image() {
            self.insert_image(img, cx);
        }
    }

    // ------------------------------------------------------------------ view

    fn view_map(&self) -> ViewMap {
        let b = self.bounds_cell.get();
        let center = b.center();
        ViewMap {
            origin: point(center.x - px(self.scroll.x * self.zoom), center.y - px(self.scroll.y * self.zoom)),
            zoom: self.zoom,
        }
    }

    fn update_fit(&mut self) {
        if !self.fit {
            return;
        }
        let b = self.bounds_cell.get();
        let (vw, vh) = (f32::from(b.size.width) - 48.0, f32::from(b.size.height) - 48.0);
        if vw <= 0.0 || vh <= 0.0 {
            return;
        }
        let r = self.view_rect();
        self.zoom = (vw / r.w).min(vh / r.h).min(1.0);
        self.scroll = r.center();
    }

    /// Area the view is fitted to: the canvas, or canvas ∪ base image while cropping.
    fn view_rect(&self) -> oxshot_core::Rect {
        let c = self.doc.canvas_rect();
        if self.crop.is_some() {
            c.union(&oxshot_core::Rect::new(0.0, 0.0, self.doc.base.width() as f32, self.doc.base.height() as f32))
        } else {
            c
        }
    }

    // ------------------------------------------------------------------ navigation

    /// Replaces the document (PageUp/PageDown through the capture folder).
    fn load(&mut self, doc: Document, file: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_pending(cx);
        self.autosave();
        {
            let mut gfx = self.gfx.borrow_mut();
            let old: Vec<_> = gfx.sprites.drain().filter_map(|(_, (_, t))| t).collect();
            gfx.garbage.extend(old);
        }
        self.scroll = doc.canvas_rect().center();
        self.doc = doc;
        self.file = file;
        self.undo.clear();
        self.redo.clear();
        self.selection = None;
        self.interaction = Interaction::None;
        self.crop = None;
        self.mode = None;
        self.revs.clear();
        self.backdrop = None;
        self.modified = false;
        self.fit = true;
        let title = self.file.as_ref().and_then(|f| f.file_name()).map(|n| format!("{} – OxShot", n.to_string_lossy()));
        window.set_window_title(title.as_deref().unwrap_or("OxShot"));
        cx.notify();
    }

    fn navigate(&mut self, dir: i32, window: &mut Window, cx: &mut Context<Self>) {
        let files = crate::storage::list_captures();
        if files.is_empty() {
            return;
        }
        let idx = self.file.as_ref().and_then(|f| files.iter().position(|p| p == f));
        let next = match idx {
            Some(i) => i as i32 + dir,
            None if dir < 0 => files.len() as i32 - 1,
            None => return,
        };
        let Some(path) = usize::try_from(next).ok().and_then(|i| files.get(i)) else {
            self.status = if dir < 0 { t!("status.first_image") } else { t!("status.last_image") }.to_string().into();
            cx.notify();
            return;
        };
        match crate::storage::open(path) {
            Ok((doc, file)) => self.load(doc, file, window, cx),
            Err(e) => self.status = t!("status.open_failed", error = format!("{e:#}")).to_string().into(),
        }
    }

    // ------------------------------------------------------------------ crop

    fn start_crop(&mut self, cx: &mut Context<Self>) {
        self.finish_pending(cx);
        self.selection = None;
        self.crop = Some(self.doc.canvas_rect());
        self.fit = true;
        cx.notify();
    }

    fn apply_crop(&mut self, cx: &mut Context<Self>) {
        if let Some(r) = self.crop.take() {
            let ir = oxshot_core::IRect::covering(&r.normalized());
            if ir.w >= 2 && ir.h >= 2 && ir != self.doc.canvas.rect {
                self.checkpoint();
                self.doc.canvas.rect = ir;
            }
            self.interaction = Interaction::None;
            self.fit = true;
            cx.notify();
        }
    }

    fn cancel_crop(&mut self, cx: &mut Context<Self>) {
        self.crop = None;
        self.interaction = Interaction::None;
        self.fit = true;
        cx.notify();
    }

    fn set_zoom(&mut self, zoom: f32, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let zoom = zoom.clamp(0.05, 16.0);
        let map = self.view_map();
        let anchor = anchor.unwrap_or_else(|| self.bounds_cell.get().center());
        let doc_pt = map.to_doc(anchor);
        self.zoom = zoom;
        self.fit = false;
        // Keep the document point under the anchor fixed.
        let b = self.bounds_cell.get();
        let c = b.center();
        self.scroll = DPoint::new(
            doc_pt.x - f32::from(anchor.x - c.x) / zoom,
            doc_pt.y - f32::from(anchor.y - c.y) / zoom,
        );
        cx.notify();
    }

    fn zoom_step(&mut self, dir: f32, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        const STEPS: [f32; 15] = [0.1, 0.15, 0.25, 0.33, 0.5, 0.67, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0];
        let z = self.zoom;
        let next = if dir > 0.0 {
            STEPS.iter().copied().find(|s| *s > z + 1e-3).unwrap_or(16.0)
        } else {
            STEPS.iter().rev().copied().find(|s| *s < z - 1e-3).unwrap_or(0.05)
        };
        self.set_zoom(next, anchor, cx);
    }
}

impl Editor {
    /// The property bar in a fixed-height strip that scrolls sideways (also with the
    /// normal mouse wheel) and fades out at the right edge while more options follow.
    fn render_props_strip(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = crate::theme::Colors::get();
        let max = f32::from(self.props_scroll.max_offset().x);
        let at_end = f32::from(-self.props_scroll.offset().x) >= max - 1.0;
        div()
            .relative()
            .flex_none()
            .h(px(PROPS_HEIGHT))
            .bg(theme.panel_alt)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .id("props")
                    .size_full()
                    // Flex row: the bar takes its content width, so overflow is measurable.
                    .flex()
                    .flex_row()
                    .overflow_x_scroll()
                    .track_scroll(&self.props_scroll)
                    .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                        let d = ev.delta.pixel_delta(px(24.0));
                        let (dx, dy) = (f32::from(d.x), f32::from(d.y));
                        // Horizontal deltas are handled by the scroll container itself.
                        if dy.abs() <= dx.abs() {
                            return;
                        }
                        let max = f32::from(this.props_scroll.max_offset().x);
                        let mut o = this.props_scroll.offset();
                        o.x = px((f32::from(o.x) + dy).clamp(-max, 0.0));
                        this.props_scroll.set_offset(o);
                        cx.notify();
                    }))
                    .child(self.render_props(window, cx)),
            )
            .when(max > 1.0 && !at_end, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .right_0()
                        .h_full()
                        .w(px(48.0))
                        .bg(linear_gradient(90.0, linear_color_stop(theme.panel_alt.opacity(0.0), 0.0), linear_color_stop(theme.panel_alt, 1.0))),
                )
            })
    }
}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Opens an editor window for a document.
pub fn open(doc: Document, file: Option<PathBuf>, cx: &mut App) -> anyhow::Result<()> {
    let (w, h) = (doc.canvas.rect.w as f32, doc.canvas.rect.h as f32);
    // Initial size: image + chrome, at most 90% of the primary display.
    let display = cx.primary_display().map(|d| d.bounds().size).unwrap_or(size(px(1920.0), px(1080.0)));
    let chrome = (56.0 + 48.0, 96.0 + 28.0);
    let ww = (w + chrome.0 + 48.0).clamp(900.0, f32::from(display.width) * 0.9);
    let wh = (h + chrome.1 + 48.0).clamp(600.0, f32::from(display.height) * 0.9);
    let title = file
        .as_ref()
        .and_then(|f| f.file_name())
        .map(|n| format!("{} – OxShot", n.to_string_lossy()))
        .unwrap_or_else(|| "OxShot".into());
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(ww), px(wh)), cx))),
        titlebar: Some(TitlebarOptions { title: Some(title.into()), ..Default::default() }),
        focus: true,
        show: true,
        app_id: Some("oxshot".into()),
        window_min_size: Some(size(px(640.0), px(420.0))),
        ..Default::default()
    };
    let (handle, _) = gpui_kit::open_window(opts, cx, move |window, cx| cx.new(|cx| Editor::new(doc, file, window, cx)))?;
    handle
        .update(cx, |_, window, _| {
            crate::icon::apply(window);
            crate::theme::follow_system(window);
            window.activate_window();
        })
        .ok();
    Ok(())
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.update_fit();
        self.sync_gfx(window);
        self.sync_preview(window);
        let theme = crate::theme::Colors::get();
        div()
            .id("editor")
            .key_context("Editor")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg)
            .text_color(theme.fg)
            .text_sm()
            .child(self.render_topbar(window, cx))
            .child(self.render_props_strip(window, cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_toolbar(window, cx))
                    .child(self.render_canvas(window, cx)),
            )
            .child(self.render_statusbar(window, cx))
    }
}
