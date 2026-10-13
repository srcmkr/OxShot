//! The workspace window ("Verlauf"): a thumbnail grid of every capture in the captures
//! folder, like Screenpresso's workspace. Thumbnails are generated in the background,
//! cached on disk and only loaded for visible rows, so thousands of files stay fast.

mod files;

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use rust_i18n::t;

use crate::assets::icon;
use crate::theme::Colors;
use files::Entry;

const PAD: f32 = 16.0;
const GAP: f32 = 12.0;
const TILE_MIN_W: f32 = 196.0;
const THUMB_H: f32 = 140.0;
const ROW_H: f32 = THUMB_H + 84.0;
/// Parallel thumbnail jobs.
const MAX_INFLIGHT: usize = 4;
/// Thumbnails kept on the GPU before the least recently shown ones are released.
const MAX_THUMBS: usize = 240;

/// Handle of the open workspace window (only one at a time).
struct WorkspaceWindow(AnyWindowHandle);
impl Global for WorkspaceWindow {}

/// Opens the workspace window, or brings the existing one to the front.
pub fn open(cx: &mut App) {
    if let Some(h) = cx.try_global::<WorkspaceWindow>().map(|g| g.0)
        && h.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        return;
    }
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1120.0), px(760.0)), cx))),
        titlebar: Some(TitlebarOptions { title: Some(t!("workspace.title").to_string().into()), ..Default::default() }),
        focus: true,
        show: true,
        app_id: Some("oxshot".into()),
        window_min_size: Some(size(px(520.0), px(360.0))),
        ..Default::default()
    };
    match gpui_kit::open_window(opts, cx, |window, cx| cx.new(|cx| Workspace::new(window, cx))) {
        Ok((handle, _)) => {
            handle
                .update(cx, |_, window, _| {
                    crate::icon::apply(window);
                    crate::theme::follow_system(window);
                    window.activate_window();
                })
                .ok();
            cx.set_global(WorkspaceWindow(handle));
        }
        Err(e) => log::error!("cannot open workspace: {e:#}"),
    }
}

#[derive(Default)]
struct ThumbSlot {
    image: Option<Arc<RenderImage>>,
    dims: Option<(u32, u32)>,
    /// Source mtime the slot was loaded for.
    mtime: Option<SystemTime>,
    loading: bool,
    failed: bool,
    used: u64,
}

pub struct Workspace {
    focus: FocusHandle,
    search: Entity<InputState>,
    dir: PathBuf,
    dir_mtime: Option<SystemTime>,
    /// All images, sorted by the current order.
    entries: Vec<Entry>,
    /// Indices into `entries` that match the filter.
    view: Vec<usize>,
    filter: String,
    newest_first: bool,
    selected: HashSet<PathBuf>,
    anchor: Option<PathBuf>,
    cursor: Option<PathBuf>,
    thumbs: HashMap<PathBuf, ThumbSlot>,
    inflight: usize,
    tick: u64,
    garbage: Vec<Arc<RenderImage>>,
    scroll: UniformListScrollHandle,
    cols: usize,
    tile_w: f32,
    visible_rows: Range<usize>,
    status: SharedString,
    loaded: bool,
    _subs: Vec<Subscription>,
}

impl Workspace {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("workspace.search_placeholder").to_string()).clean_on_escape());
        let sub = cx.subscribe_in(&search, window, |this: &mut Self, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::Change => {
                this.filter = input.read(cx).value().to_string();
                this.apply_view(cx);
            }
            InputEvent::PressEnter { .. } => {
                window.focus(&this.focus, cx);
                if this.selected.is_empty()
                    && let Some(&i) = this.view.first()
                {
                    let p = this.entries[i].path.clone();
                    this.select_only(p);
                }
                cx.notify();
            }
            _ => {}
        });
        let mut this = Self {
            focus,
            search,
            dir: crate::storage::captures_dir(),
            dir_mtime: None,
            entries: Vec::new(),
            view: Vec::new(),
            filter: String::new(),
            newest_first: true,
            selected: HashSet::new(),
            anchor: None,
            cursor: None,
            thumbs: HashMap::new(),
            inflight: 0,
            tick: 0,
            garbage: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            cols: 1,
            tile_w: TILE_MIN_W,
            visible_rows: 0..0,
            status: "".into(),
            loaded: false,
            _subs: vec![sub],
        };
        this.rescan(cx);
        this.start_watch(cx);
        this
    }

    // ------------------------------------------------------------------ folder

    /// Re-reads the folder in the background.
    fn rescan(&mut self, cx: &mut Context<Self>) {
        let dir = crate::storage::captures_dir();
        cx.spawn(async move |this, cx| {
            let d = dir.clone();
            let (entries, mtime) = cx.background_spawn(async move { (files::scan(&d), files::dir_mtime(&d)) }).await;
            this.update(cx, |this, cx| {
                this.dir = dir;
                this.dir_mtime = mtime;
                this.set_entries(entries, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Polls the folder's mtime once a second; new, renamed or deleted files change it.
    fn start_watch(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(1000)).await;
                let Ok((dir, known)) = this.update(cx, |this, _| (this.dir.clone(), this.dir_mtime)) else { break };
                let current = crate::storage::captures_dir();
                let d = current.clone();
                let mtime = cx.background_spawn(async move { files::dir_mtime(&d) }).await;
                if (current != dir || mtime != known) && this.update(cx, |this, cx| this.rescan(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn set_entries(&mut self, mut entries: Vec<Entry>, cx: &mut Context<Self>) {
        entries.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| b.name.cmp(&a.name)));
        if !self.newest_first {
            entries.reverse();
        }
        let present: HashSet<&PathBuf> = entries.iter().map(|e| &e.path).collect();
        let gone: Vec<PathBuf> = self.thumbs.keys().filter(|p| !present.contains(p)).cloned().collect();
        for p in gone {
            if let Some(slot) = self.thumbs.remove(&p)
                && let Some(img) = slot.image
            {
                self.garbage.push(img);
            }
        }
        self.selected.retain(|p| present.contains(p));
        if self.cursor.as_ref().is_some_and(|c| !present.contains(c)) {
            self.cursor = None;
        }
        self.entries = entries;
        self.loaded = true;
        self.apply_view(cx);
    }

    fn apply_view(&mut self, cx: &mut Context<Self>) {
        let f = self.filter.trim().to_lowercase();
        self.view = (0..self.entries.len()).filter(|&i| f.is_empty() || self.entries[i].lname.contains(&f)).collect();
        // Never act on hidden items.
        let shown: HashSet<&PathBuf> = self.view.iter().map(|&i| &self.entries[i].path).collect();
        self.selected.retain(|p| shown.contains(p));
        cx.notify();
    }

    fn toggle_sort(&mut self, cx: &mut Context<Self>) {
        self.newest_first = !self.newest_first;
        self.entries.reverse();
        self.apply_view(cx);
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    // ------------------------------------------------------------------ thumbnails

    /// Requests thumbnails for the rows on screen (plus one row ahead).
    fn want_thumbs(&mut self, rows: Range<usize>, cx: &mut Context<Self>) {
        let cols = self.cols.max(1);
        let start = rows.start.saturating_sub(1) * cols;
        let end = ((rows.end + 1) * cols).min(self.view.len());
        for vi in start..end {
            if self.inflight >= MAX_INFLIGHT {
                break;
            }
            let e = &self.entries[self.view[vi]];
            let slot = self.thumbs.entry(e.path.clone()).or_default();
            if slot.loading || slot.mtime == Some(e.mtime) {
                continue;
            }
            slot.loading = true;
            self.inflight += 1;
            let (path, mtime) = (e.path.clone(), e.mtime);
            cx.spawn(async move |this, cx| {
                let p = path.clone();
                let res = cx.background_spawn(async move { files::load_thumb(&p, mtime) }).await;
                this.update(cx, |this, cx| {
                    this.inflight -= 1;
                    let slot = this.thumbs.entry(path.clone()).or_default();
                    slot.loading = false;
                    slot.mtime = Some(mtime);
                    match res {
                        Ok(t) => {
                            if let Some(old) = slot.image.replace(t.image) {
                                this.garbage.push(old);
                            }
                            slot.dims = Some(t.dims);
                            slot.failed = false;
                        }
                        Err(e) => {
                            log::debug!("thumbnail {}: {e:#}", path.display());
                            slot.failed = true;
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// Releases GPU textures of thumbnails that have not been shown for a while.
    fn evict(&mut self, window: &mut Window) {
        let loaded = self.thumbs.values().filter(|s| s.image.is_some()).count();
        if loaded > MAX_THUMBS {
            let mut by_age: Vec<(u64, PathBuf)> =
                self.thumbs.iter().filter(|(_, s)| s.image.is_some() && !s.loading).map(|(p, s)| (s.used, p.clone())).collect();
            by_age.sort();
            for (_, p) in by_age.into_iter().take(loaded - MAX_THUMBS * 3 / 4) {
                if let Some(slot) = self.thumbs.remove(&p)
                    && let Some(img) = slot.image
                {
                    self.garbage.push(img);
                }
            }
        }
        for img in self.garbage.drain(..) {
            let _ = window.drop_image(img);
        }
    }

    // ------------------------------------------------------------------ selection

    fn view_index(&self, path: &Path) -> Option<usize> {
        self.view.iter().position(|&i| self.entries[i].path == path)
    }

    fn path_at(&self, vi: usize) -> Option<PathBuf> {
        self.view.get(vi).map(|&i| self.entries[i].path.clone())
    }

    /// Selected paths in display order.
    fn selection(&self) -> Vec<PathBuf> {
        self.view.iter().map(|&i| &self.entries[i].path).filter(|p| self.selected.contains(*p)).cloned().collect()
    }

    fn select_only(&mut self, p: PathBuf) {
        self.selected.clear();
        self.selected.insert(p.clone());
        self.anchor = Some(p.clone());
        self.cursor = Some(p);
    }

    fn select_range(&mut self, to: usize, add: bool) {
        let from = self.anchor.as_deref().and_then(|a| self.view_index(a)).unwrap_or(to);
        if !add {
            self.selected.clear();
        }
        for vi in from.min(to)..=from.max(to) {
            if let Some(p) = self.path_at(vi) {
                self.selected.insert(p);
            }
        }
        self.cursor = self.path_at(to);
    }

    fn click_tile(&mut self, vi: usize, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some(p) = self.path_at(vi) else { return };
        let m = ev.modifiers;
        if ev.click_count >= 2 && !m.control && !m.shift {
            self.select_only(p.clone());
            self.open_paths(vec![p], cx);
            return;
        }
        if m.shift {
            self.select_range(vi, m.control);
        } else if m.control {
            if !self.selected.remove(&p) {
                self.selected.insert(p.clone());
            }
            self.anchor = Some(p.clone());
            self.cursor = Some(p);
        } else {
            self.select_only(p);
        }
        cx.notify();
    }

    fn move_cursor(&mut self, delta: isize, shift: bool, cx: &mut Context<Self>) {
        if self.view.is_empty() {
            return;
        }
        let cur = self.cursor.as_deref().and_then(|c| self.view_index(c));
        let next = match cur {
            None => 0,
            Some(c) => (c as isize + delta).clamp(0, self.view.len() as isize - 1) as usize,
        };
        if shift {
            if self.anchor.is_none() {
                self.anchor = cur.and_then(|c| self.path_at(c));
            }
            self.select_range(next, false);
        } else if let Some(p) = self.path_at(next) {
            self.select_only(p);
        }
        self.scroll.scroll_to_item(next / self.cols.max(1), ScrollStrategy::Nearest);
        cx.notify();
    }

    // ------------------------------------------------------------------ actions

    fn set_status(&mut self, s: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.status = s.into();
        cx.notify();
    }

    fn open_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for p in paths.into_iter().take(10) {
            match crate::storage::open(&p) {
                Ok((doc, file)) => {
                    if let Err(e) = crate::editor::open(doc, file, cx) {
                        log::error!("cannot open editor: {e:#}");
                    }
                }
                Err(e) => self.set_status(t!("workspace.open_failed", error = format!("{e:#}")).to_string(), cx),
            }
        }
    }

    fn open_selected(&mut self, cx: &mut Context<Self>) {
        let sel = self.selection();
        self.open_paths(sel, cx);
    }

    fn copy_selected(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.selection().into_iter().next() else { return };
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.set_status(t!("workspace.copying").to_string(), cx);
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    let img = image::open(&p)?.to_rgba8();
                    crate::clipboard::set_capture(&img, Some(&p))?;
                    anyhow::Ok((img.width(), img.height()))
                })
                .await;
            this.update(cx, |this, cx| {
                let msg = match res {
                    Ok((w, h)) => t!("workspace.copied", name = name, w = w, h = h).to_string(),
                    Err(e) => t!("workspace.copy_failed", error = format!("{e:#}")).to_string(),
                };
                this.set_status(msg, cx);
            })
            .ok();
        })
        .detach();
    }

    /// `Ctrl+V`: an image on the clipboard becomes a new capture.
    fn paste(&mut self, cx: &mut Context<Self>) {
        match crate::clipboard::get_image() {
            Some(img) => {
                let doc = oxshot_core::Document::new(img);
                match crate::storage::save_capture(&doc, &doc.base, None) {
                    Ok(p) => {
                        self.select_only(p);
                        self.set_status(t!("workspace.pasted").to_string(), cx);
                    }
                    Err(e) => self.set_status(t!("workspace.paste_failed", error = format!("{e:#}")).to_string(), cx),
                }
            }
            None => self.set_status(t!("workspace.no_clipboard_image").to_string(), cx),
        }
    }

    fn import(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let dir = crate::storage::captures_dir();
        cx.spawn(async move |this, cx| {
            let (done, errors) = cx.background_spawn(async move { files::import(&paths, &dir) }).await;
            this.update(cx, |this, cx| {
                this.selected = done.iter().cloned().collect();
                this.cursor = done.first().cloned();
                this.anchor = this.cursor.clone();
                let mut msg = match done.len() {
                    0 => String::new(),
                    1 => t!("workspace.imported_one").to_string(),
                    n => t!("workspace.imported_other", n = n).to_string(),
                };
                if let Some(e) = errors.first() {
                    if !msg.is_empty() {
                        msg.push_str(" · ");
                    }
                    msg.push_str(e);
                }
                this.set_status(msg, cx);
                this.rescan(cx);
            })
            .ok();
        })
        .detach();
    }

    fn import_dialog(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some(t!("workspace.import_prompt").to_string().into()) });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                this.update(cx, |this, cx| this.import(paths, cx)).ok();
            }
        })
        .detach();
    }

    fn request_delete(&mut self, confirm: bool, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.selection();
        if paths.is_empty() {
            return;
        }
        if !confirm {
            self.delete_now(paths, cx);
            return;
        }
        let weak = cx.entity().downgrade();
        let desc: SharedString = match paths.as_slice() {
            [one] => t!("workspace.delete_desc_one", name = one.file_name().unwrap_or_default().to_string_lossy()).to_string().into(),
            many => t!("workspace.delete_desc_other", n = many.len()).to_string().into(),
        };
        window.open_alert_dialog(cx, move |alert, _, _| {
            let weak = weak.clone();
            let paths = paths.clone();
            alert
                .confirm()
                .title(t!("workspace.delete_title").to_string())
                .description(desc.clone())
                .ok_text(t!("workspace.delete").to_string())
                .ok_variant(ButtonVariant::Danger)
                .cancel_text(t!("workspace.cancel").to_string())
                .on_ok(move |_, _, cx| {
                    weak.update(cx, |this, cx| this.delete_now(paths.clone(), cx)).ok();
                    true
                })
        });
    }

    fn delete_now(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        // Keep a selection: the item after the last deleted one.
        let last = paths.iter().filter_map(|p| self.view_index(p)).max();
        let gone: HashSet<&PathBuf> = paths.iter().collect();
        let next = last.and_then(|l| {
            (l + 1..self.view.len()).chain((0..l).rev()).filter_map(|vi| self.path_at(vi)).find(|p| !gone.contains(p))
        });
        cx.spawn(async move |this, cx| {
            let n = paths.len();
            let res = cx.background_spawn(async move { files::trash_all(&paths) }).await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        this.selected.clear();
                        if let Some(p) = next {
                            this.select_only(p);
                        }
                        this.set_status(if n == 1 { t!("workspace.trashed_one").to_string() } else { t!("workspace.trashed_other", n = n).to_string() }, cx);
                    }
                    Err(e) => this.set_status(t!("workspace.delete_failed", error = e.to_string()).to_string(), cx),
                }
                this.rescan(cx);
            })
            .ok();
        })
        .detach();
    }

    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.selection().into_iter().next() else { return };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let stem_len = path.file_stem().map(|s| s.len()).unwrap_or(name.len());
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name.clone()));
        input.update(cx, |s, cx| s.set_selected_range(0..stem_len, cx));
        let error: Entity<Option<SharedString>> = cx.new(|_| None);
        let weak = cx.entity().downgrade();

        // Returns true when the dialog may close.
        let submit = {
            let (input, error, path) = (input.clone(), error.clone(), path.clone());
            let done = std::cell::Cell::new(false);
            move |_: &mut Window, cx: &mut App| -> bool {
                if done.get() {
                    return true;
                }
                let typed = input.read(cx).value().to_string();
                match files::validate_name(&path, &typed) {
                    Ok(new) => {
                        done.set(true);
                        weak.update(cx, |this, cx| this.rename_now(path.clone(), new, cx)).ok();
                        true
                    }
                    Err(msg) => {
                        error.update(cx, |e, cx| {
                            *e = Some(msg.into());
                            cx.notify();
                        });
                        false
                    }
                }
            }
        };
        let submit = std::rc::Rc::new(submit);
        let s2 = submit.clone();
        let sub = cx.subscribe_in(&input, window, move |_, _, ev: &InputEvent, window, cx| {
            // Runs while the workspace is being updated: submit afterwards.
            if let InputEvent::PressEnter { .. } = ev {
                let s2 = s2.clone();
                window.defer(cx, move |window, cx| {
                    if s2(window, cx) {
                        window.close_dialog(cx);
                    }
                });
            }
        });
        self._subs.truncate(1);
        self._subs.push(sub);

        let first = std::cell::Cell::new(true);
        window.open_alert_dialog(cx, move |alert, window, cx| {
            if first.replace(false) {
                input.focus_handle(cx).focus(window, cx);
            }
            let submit = submit.clone();
            let err = error.read(cx).clone();
            alert
                .confirm()
                .title(t!("workspace.rename").to_string())
                .description(t!("workspace.rename_hint").to_string())
                .child(div().mt_2().child(Input::new(&input)))
                .when_some(err, |a, e| a.child(div().mt_1().text_sm().text_color(Colors::get().danger).child(e)))
                .ok_text(t!("workspace.rename").to_string())
                .cancel_text(t!("workspace.cancel").to_string())
                .on_ok(move |_, window, cx| submit(window, cx))
        });
    }

    fn rename_now(&mut self, old: PathBuf, new: PathBuf, cx: &mut Context<Self>) {
        if old == new {
            return;
        }
        let converting = !old.extension().zip(new.extension()).is_some_and(|(a, b)| a.eq_ignore_ascii_case(b));
        if converting {
            self.set_status(t!("workspace.converting").to_string(), cx);
        }
        cx.spawn(async move |this, cx| {
            let (o, n) = (old.clone(), new.clone());
            let res = cx.background_spawn(async move { files::rename(&o, &n) }).await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        this.select_only(new.clone());
                        let name = new.file_name().unwrap_or_default().to_string_lossy().into_owned();
                        this.set_status(if converting { t!("workspace.converted", name = name) } else { t!("workspace.renamed", name = name) }.to_string(), cx);
                    }
                    Err(e) => this.set_status(t!("workspace.rename_failed", error = format!("{e:#}")).to_string(), cx),
                }
                this.rescan(cx);
            })
            .ok();
        })
        .detach();
    }

    fn new_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Get out of the way first, like Screenpresso.
        window.minimize_window();
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(Duration::from_millis(350)).await;
            cx.update(|cx| crate::app::capture(crate::app::CaptureMode::Region, cx));
        })
        .detach();
    }

    fn delayed_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.minimize_window();
        let secs = crate::settings::get(cx).capture_delay;
        crate::countdown::start(secs, cx);
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        let cols = self.cols.max(1) as isize;
        let page = cols * (self.visible_rows.len().max(2) as isize - 1);
        match (m.control, ks.key.as_str()) {
            (true, "a") => {
                self.selected = self.view.iter().map(|&i| self.entries[i].path.clone()).collect();
                cx.notify();
            }
            (true, "c") => self.copy_selected(cx),
            (true, "v") => self.paste(cx),
            (true, "f") => self.search.update(cx, |s, cx| s.focus(window, cx)),
            (true, "o") => self.import_dialog(cx),
            (true, "n") => self.new_capture(window, cx),
            (true, "w") => window.remove_window(),
            (false, "delete") => self.request_delete(!m.shift, window, cx),
            (false, "f2") => self.start_rename(window, cx),
            (false, "f5") => self.rescan(cx),
            (false, "enter") => self.open_selected(cx),
            (false, "left") => self.move_cursor(-1, m.shift, cx),
            (false, "right") => self.move_cursor(1, m.shift, cx),
            (false, "up") => self.move_cursor(-cols, m.shift, cx),
            (false, "down") => self.move_cursor(cols, m.shift, cx),
            (false, "pageup") => self.move_cursor(-page, m.shift, cx),
            (false, "pagedown") => self.move_cursor(page, m.shift, cx),
            (false, "home") => self.move_cursor(isize::MIN / 2, m.shift, cx),
            (false, "end") => self.move_cursor(isize::MAX / 2, m.shift, cx),
            (false, "escape") => {
                if !self.filter.is_empty() {
                    self.search.update(cx, |s, cx| s.set_value("", window, cx));
                    self.filter.clear();
                    self.apply_view(cx);
                } else {
                    self.selected.clear();
                    cx.notify();
                }
            }
            (false, _) if !m.alt && !m.platform => {
                // Typing filters the view, like Screenpresso.
                if let Some(ch) = ks.key_char.as_deref().filter(|c| c.chars().all(|ch| !ch.is_control()) && !c.is_empty()) {
                    let ch = ch.to_string();
                    self.search.update(cx, |s, cx| {
                        let v = format!("{}{ch}", s.value());
                        let len = v.len();
                        s.set_value(v, window, cx);
                        s.set_selected_range(len..len, cx);
                        s.focus(window, cx);
                    });
                    self.filter = self.search.read(cx).value().to_string();
                    self.apply_view(cx);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ rendering

    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = Colors::get();
        let this = cx.entity().downgrade();
        let act = move |f: fn(&mut Workspace, &mut Window, &mut Context<Workspace>)| {
            let this = this.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                this.update(cx, |w, cx| f(w, window, cx)).ok();
            }
        };
        let (sort_icon, sort_tip) =
            if self.newest_first { ("arrow-down-wide-narrow", t!("workspace.sort_newest")) } else { ("arrow-up-narrow-wide", t!("workspace.sort_oldest")) };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h(px(48.0))
            .flex_none()
            .px_2()
            .bg(c.panel)
            .border_b_1()
            .border_color(c.border)
            .child(text_button("new-capture", "camera", &t!("workspace.new_capture"), true, act(|w, win, cx| w.new_capture(win, cx))))
            .child(icon_button("delayed-capture", "timer", t!("capture.delayed", s = crate::settings::get(cx).capture_delay).to_string(), act(|w, win, cx| w.delayed_capture(win, cx))))
            .child(icon_button("import", "image-plus", t!("workspace.import_tip").to_string(), act(|w, _, cx| w.import_dialog(cx))))
            .child(separator())
            .child(
                div().w(px(300.0)).child(
                    Input::new(&self.search)
                        .prefix(svg().path(icon("search")).size(px(15.0)).text_color(c.muted))
                        .cleanable(true),
                ),
            )
            .child(div().flex_1())
            .child(icon_button("sort", sort_icon, sort_tip.to_string(), act(|w, _, cx| w.toggle_sort(cx))))
            .child(text_button("open-folder", "folder-open", &t!("workspace.open_folder"), false, act(|w, _, _| files::xdg_open(&w.dir))))
    }

    fn render_statusbar(&self) -> impl IntoElement {
        let c = Colors::get();
        let count = if self.view.len() == self.entries.len() {
            match self.entries.len() {
                1 => t!("workspace.count_one").to_string(),
                n => t!("workspace.count_other", n = n).to_string(),
            }
        } else {
            t!("workspace.count_filtered", shown = self.view.len(), total = self.entries.len()).to_string()
        };
        let sel = match self.selected.len() {
            0 => String::new(),
            n => t!("workspace.selected", n = n).to_string(),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .h(px(28.0))
            .flex_none()
            .px_3()
            .bg(c.panel)
            .border_t_1()
            .border_color(c.border)
            .text_xs()
            .text_color(c.muted)
            .child(count)
            .child(sel)
            .child(t!("workspace.shortcuts").to_string())
            .child(div().flex_1())
            .child(div().text_color(c.fg).child(self.status.clone()))
    }

    fn render_rows(&mut self, rows: Range<usize>, _window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.visible_rows = rows.clone();
        self.tick += 1;
        self.want_thumbs(rows.clone(), cx);
        let cols = self.cols.max(1);
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let mut row = div().flex().flex_row().gap(px(GAP)).h(px(ROW_H)).px(px(PAD)).pb(px(GAP));
            for vi in r * cols..((r + 1) * cols).min(self.view.len()) {
                row = row.child(self.render_tile(vi, cx));
            }
            out.push(row.into_any_element());
        }
        out
    }

    fn render_tile(&mut self, vi: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let c = Colors::get();
        let e = &self.entries[self.view[vi]];
        let selected = self.selected.contains(&e.path);
        let is_cursor = self.cursor.as_ref() == Some(&e.path);
        let tick = self.tick;
        let slot = self.thumbs.get_mut(&e.path);
        let (image, dims, failed) = match slot {
            Some(s) => {
                s.used = tick;
                (s.image.clone(), s.dims, s.failed)
            }
            None => (None, None, false),
        };
        let date = files::human_date(e.mtime);
        let meta = match dims {
            Some((w, h)) => format!("{w} × {h} · {}", files::human_size(e.len)),
            None => files::human_size(e.len),
        };
        let thumb: AnyElement = match image {
            Some(img) => img_el(img).into_any_element(),
            None => svg()
                .path(icon(if failed { "image-off" } else { "image" }))
                .size(px(28.0))
                .text_color(Colors::get().faint)
                .into_any_element(),
        };
        let this = cx.entity().downgrade();
        let this_r = this.clone();
        let path = e.path.clone();
        div()
            .id(("tile", vi))
            .w(px(self.tile_w))
            .h(px(ROW_H - GAP))
            .flex()
            .flex_col()
            .p(px(5.0))
            .gap(px(4.0))
            .rounded(px(8.0))
            .border_2()
            .border_color(if selected { c.accent } else { transparent_black() })
            .when(selected, |d| d.bg(c.accent_bg))
            .when(!selected, |d| d.hover(|d| d.bg(c.hover)))
            .when(is_cursor && !selected, |d| d.border_color(c.border))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                cx.stop_propagation();
                this.update(cx, |w, cx| w.click_tile(vi, ev, window, cx)).ok();
            })
            .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                this_r
                    .update(cx, |w, cx| {
                        window.focus(&w.focus, cx);
                        if !w.selected.contains(&path) {
                            w.select_only(path.clone());
                        }
                        cx.notify();
                    })
                    .ok();
            })
            .child(
                div()
                    .h(px(THUMB_H))
                    .w_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.0))
                    .bg(c.canvas_bg)
                    .overflow_hidden()
                    .child(thumb),
            )
            .child(
                div()
                    .px_1()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(div().text_sm().text_color(c.fg).whitespace_nowrap().overflow_hidden().text_ellipsis().child(e.name.clone()))
                    .child(div().text_xs().text_color(c.muted).whitespace_nowrap().overflow_hidden().text_ellipsis().child(meta))
                    .child(div().text_xs().text_color(c.muted).whitespace_nowrap().child(date)),
            )
            .context_menu(self.menu_builder(cx))
    }

    fn menu_builder(&self, cx: &mut Context<Self>) -> impl Fn(gpui_kit::component::menu::PopupMenu, &mut Window, &mut Context<gpui_kit::component::menu::PopupMenu>) -> gpui_kit::component::menu::PopupMenu + 'static {
        let weak = cx.entity().downgrade();
        move |menu, _, _| {
            let item = |label: &str, ic: &str, f: fn(&mut Workspace, &mut Window, &mut Context<Workspace>)| {
                let weak = weak.clone();
                PopupMenuItem::new(label.to_string())
                    .icon(gpui_kit::component::Icon::default().path(icon(ic)))
                    .on_click(move |_, window, cx| {
                        weak.update(cx, |w, cx| f(w, window, cx)).ok();
                    })
            };
            menu.min_w(px(200.0))
                .item(item(&t!("workspace.open"), "pencil", |w, _, cx| w.open_selected(cx)))
                .item(item(&t!("workspace.copy"), "copy", |w, _, cx| w.copy_selected(cx)))
                .item(item(&t!("workspace.rename"), "file-pen-line", |w, win, cx| w.start_rename(win, cx)))
                .separator()
                .item(item(&t!("workspace.show_in_folder"), "folder-search", |w, _, _| {
                    if let Some(p) = w.selection().first() {
                        files::show_in_folder(p);
                    }
                }))
                .item(item(&t!("workspace.open_with"), "external-link", |w, win, _| {
                    // The chooser asks once per file; keep it to the first one.
                    if let Some(p) = w.selection().first() {
                        files::open_with(p, crate::icon::xid(win));
                    }
                }))
                .separator()
                .item(item(&t!("workspace.delete"), "trash", |w, win, cx| w.request_delete(true, win, cx)))
        }
    }

    fn render_empty(&self) -> impl IntoElement {
        let c = Colors::get();
        let (title, hint) = if !self.loaded {
            (t!("workspace.loading").to_string(), String::new())
        } else if self.entries.is_empty() {
            (t!("workspace.empty_title").to_string(), t!("workspace.empty_hint").to_string())
        } else {
            (t!("workspace.no_match", filter = self.filter.trim()).to_string(), t!("workspace.no_match_hint").to_string())
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .child(svg().path(icon("images")).size(px(48.0)).text_color(Colors::get().faint))
            .child(div().text_base().text_color(c.fg).child(title))
            .child(div().text_sm().text_color(c.muted).child(hint))
    }
}

fn img_el(image: Arc<RenderImage>) -> Img {
    img(ImageSource::Render(image)).size_full().object_fit(ObjectFit::ScaleDown)
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.evict(window);
        let c = Colors::get();
        let width = f32::from(window.viewport_size().width) - 2.0 * PAD;
        self.cols = (((width + GAP) / (TILE_MIN_W + GAP)).floor() as usize).max(1);
        self.tile_w = ((width - GAP * (self.cols as f32 - 1.0)) / self.cols as f32).floor();
        let rows = self.view.len().div_ceil(self.cols);
        let this = cx.entity().downgrade();
        let this_drop = this.clone();

        let grid: AnyElement = if self.view.is_empty() {
            self.render_empty().into_any_element()
        } else {
            uniform_list("grid", rows, cx.processor(|this, range, window, cx| this.render_rows(range, window, cx)))
                .track_scroll(&self.scroll)
                .pt(px(PAD))
                .size_full()
                .into_any_element()
        };

        div()
            .id("workspace")
            .key_context("Workspace")
            .size_full()
            .flex()
            .flex_col()
            .bg(c.bg)
            .text_color(c.fg)
            .text_sm()
            .child(self.render_header(cx))
            .child(
                div()
                    .id("grid-area")
                    .track_focus(&self.focus)
                    .on_key_down(cx.listener(Self::on_key_down))
                    .flex_1()
                    .min_h_0()
                    .border_2()
                    .border_color(transparent_black())
                    .drag_over::<ExternalPaths>(move |s, _, _, _| s.border_color(c.accent).bg(c.accent_bg))
                    .on_drop(move |paths: &ExternalPaths, _, cx| {
                        let paths = paths.paths().to_vec();
                        this_drop.update(cx, |w, cx| w.import(paths, cx)).ok();
                    })
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        this.update(cx, |w, cx| {
                            window.focus(&w.focus, cx);
                            w.selected.clear();
                            cx.notify();
                        })
                        .ok();
                    })
                    .child(grid),
            )
            .child(self.render_statusbar())
    }
}

// ---------------------------------------------------------------------- chrome helpers

fn icon_el(name: &str, color: Hsla, s: f32) -> Svg {
    svg().path(icon(name)).size(px(s)).text_color(color)
}

fn icon_button(
    id: &'static str,
    name: &str,
    tip: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let c = Colors::get();
    let tip: SharedString = tip.into();
    div()
        .id(id)
        .size(px(34.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .cursor_pointer()
        .hover(|d| d.bg(c.hover))
        .child(icon_el(name, c.fg, 18.0))
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
        .on_click(on_click)
}

fn text_button(id: &'static str, icon_name: &str, label: &str, primary: bool, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Stateful<Div> {
    let c = Colors::get();
    div()
        .id(id)
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

fn separator() -> Div {
    let c = Colors::get();
    div().w(px(1.0)).h(px(22.0)).mx_1().bg(c.border)
}
