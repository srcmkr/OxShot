//! Settings window: behaviour after capture, storage folder, global hotkeys, autostart.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use rust_i18n::t;

use crate::settings::{self, AfterCapture};
use crate::theme::Colors;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HotkeyField {
    Region,
    LastRegion,
    Fullscreen,
    PickColor,
    Delayed,
}

pub struct SettingsWindow {
    focus: FocusHandle,
    /// Hotkey currently being recorded.
    recording: Option<HotkeyField>,
}

struct Open(AnyWindowHandle);
impl Global for Open {}

pub fn open(cx: &mut App) {
    if let Some(Open(h)) = cx.try_global::<Open>()
        && h.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        return;
    }
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(620.0), px(560.0)), cx))),
        titlebar: Some(TitlebarOptions { title: Some(t!("settings.window_title").to_string().into()), ..Default::default() }),
        app_id: Some("oxshot".into()),
        focus: true,
        ..Default::default()
    };
    match gpui_kit::open_window(opts, cx, |window, cx| {
        cx.new(|cx| {
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            SettingsWindow { focus, recording: None }
        })
    }) {
        Ok((h, _)) => {
            h.update(cx, |_, window, _| {
                crate::icon::apply(window);
                crate::theme::follow_system(window);
            })
            .ok();
            cx.set_global(Open(h));
        }
        Err(e) => log::error!("cannot open settings: {e:#}"),
    }
}

impl SettingsWindow {
    fn record_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(field) = self.recording else { return };
        let ks = &ev.keystroke;
        let key = ks.key.as_str();
        if matches!(key, "shift" | "control" | "alt" | "super" | "platform" | "function" | "capslock") {
            return;
        }
        let combo = if key == "escape" && !ks.modifiers.modified() {
            None
        } else if key == "backspace" || key == "delete" {
            Some(String::new())
        } else {
            let mut s = String::new();
            if ks.modifiers.control {
                s.push_str("Ctrl+");
            }
            if ks.modifiers.shift {
                s.push_str("Shift+");
            }
            if ks.modifiers.alt {
                s.push_str("Alt+");
            }
            if ks.modifiers.platform {
                s.push_str("Super+");
            }
            s.push_str(&capitalize(key));
            Some(s)
        };
        self.recording = None;
        if let Some(combo) = combo {
            if !combo.is_empty() && combo.parse::<oxshot_x11::Hotkey>().is_err() {
                log::warn!("unsupported hotkey {combo}");
            } else {
                settings::update(cx, |s| match field {
                    HotkeyField::Region => s.hotkeys.region = combo,
                    HotkeyField::LastRegion => s.hotkeys.last_region = combo,
                    HotkeyField::Fullscreen => s.hotkeys.fullscreen = combo,
                    HotkeyField::PickColor => s.hotkeys.pick_color = combo,
                    HotkeyField::Delayed => s.hotkeys.delayed = combo,
                });
            }
        }
        crate::app::set_hotkeys_paused(false, cx);
        cx.notify();
    }
}

fn capitalize(k: &str) -> String {
    let mut c = k.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn section(title: &str) -> Div {
    let c = Colors::get();
    div().mt_4().mb_1().text_xs().text_color(c.muted).child(title.to_uppercase())
}

fn row(label: &str, control: impl IntoElement) -> Div {
    let c = Colors::get();
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_4()
        .py_2()
        .border_b_1()
        .border_color(c.border)
        .child(div().flex_none().child(label.to_string()))
        .child(control)
}

fn chip(id: impl Into<ElementId>, label: impl Into<SharedString>, active: bool, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Stateful<Div> {
    let c = Colors::get();
    div()
        .id(id.into())
        .flex_none()
        .h(px(28.0))
        .px_3()
        .flex()
        .items_center()
        .rounded(px(6.0))
        .border_1()
        .cursor_pointer()
        .border_color(if active { c.accent } else { c.border })
        .when(active, |d| d.bg(c.accent_bg).text_color(c.accent))
        .hover(|d| d.bg(c.hover))
        .child(label.into())
        .on_click(on_click)
}

fn switch(id: impl Into<ElementId>, on: bool, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Stateful<Div> {
    let c = Colors::get();
    div()
        .id(id.into())
        .w(px(40.0))
        .h(px(22.0))
        .rounded_full()
        .p(px(2.0))
        .cursor_pointer()
        .bg(if on { c.accent } else { c.border })
        .flex()
        .when(on, |d| d.justify_end())
        .child(div().size(px(18.0)).rounded_full().bg(white()))
        .on_click(on_click)
}

/// Version, update status and the actions that fit this installation.
fn updates_rows(s: &settings::Settings, cx: &App) -> Div {
    use crate::update::{self, Action};
    let c = Colors::get();
    let (status, dot) = update::status(cx);
    let busy = cx.try_global::<update::Updates>().is_some_and(|u| u.busy || u.checking);
    let newer = update::newer(cx);
    let mut actions = div().flex().flex_row().flex_wrap().gap_2();
    if !busy {
        actions = actions.child(chip("upd-check", t!("update.check_now").to_string(), false, |_, _, cx| crate::update::check(cx)));
    }
    if let Some(release) = newer.clone() {
        if update::can_update() && !busy {
            actions = actions.child(chip("upd-install", t!("update.install", version = release.version.clone()).to_string(), true, |_, _, cx| {
                crate::update::install(Action::Update, cx)
            }));
        }
        let page = release.page.clone();
        actions = actions.child(chip("upd-page", t!("update.release_page").to_string(), false, move |_, _, _| {
            let _ = std::process::Command::new("xdg-open").arg(&page).spawn();
        }));
        if s.skipped_version.as_deref() != Some(release.version.as_str()) {
            actions = actions.child(chip("upd-skip", t!("update.skip").to_string(), false, |_, _, cx| crate::update::skip(cx)));
        }
    }
    if update::is_build() && !busy {
        actions = actions.child(chip("upd-install-app", t!("update.install_app").to_string(), false, |_, _, cx| {
            crate::update::install(Action::InstallApp, cx)
        }));
    }
    let note = update::note(cx);
    div()
        .flex()
        .flex_col()
        .child(row(
            &t!("update.version", version = update::VERSION),
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .child(div().flex_none().size(px(8.0)).rounded_full().bg(dot))
                .child(div().min_w_0().text_color(c.muted).child(status)),
        ))
        .child(row(&t!("update.auto"), switch("upd-auto", s.check_updates, |_, _, cx| settings::update(cx, |s| s.check_updates = !s.check_updates))))
        .child(div().py_2().child(actions))
        .when_some(note, |d, n| d.child(div().pb_2().text_xs().text_color(c.danger).child(n)))
}

fn appearance(a: settings::Appearance) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |_, _, cx| {
        settings::update(cx, |s| s.appearance = a);
        crate::theme::apply(cx);
    }
}

/// "System (Deutsch)" plus one chip per language, in the language's own name.
fn language_row(current: settings::Language) -> Div {
    use crate::i18n::Lang;
    let choose = |lang: settings::Language| {
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            settings::update(cx, |s| s.language = lang);
            crate::i18n::apply(cx);
            // The menu/autostart entries carry a translated comment.
            std::thread::spawn(crate::desktop::sync);
        }
    };
    let system = t!("settings.language_system", lang = crate::i18n::system().native_name()).to_string();
    let mut chips = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap_2()
        .py_2()
        .child(chip("lang-system", system, current == settings::Language::System, choose(settings::Language::System)));
    for lang in Lang::ALL {
        let l = lang.setting();
        chips = chips.child(chip(SharedString::from(format!("lang-{}", lang.code())), lang.native_name(), current == l, choose(l)));
    }
    chips
}

impl Render for SettingsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = Colors::get();
        let s = settings::get(cx).clone();
        let errors = cx.try_global::<crate::app::Services>().map(|s| s.hotkey_errors.clone()).unwrap_or_default();
        let this = cx.entity().downgrade();
        let upd = move |f: fn(&mut settings::Settings)| move |_: &ClickEvent, _: &mut Window, cx: &mut App| settings::update(cx, f);

        let hotkey_row = |field: HotkeyField, label: &str, value: &str| {
            let recording = self.recording == Some(field);
            let failed = errors.iter().find(|(h, _)| !value.is_empty() && h.eq_ignore_ascii_case(value)).map(|(_, e)| e.clone());
            let this = this.clone();
            let shown: SharedString = if recording {
                t!("settings.press_key").to_string().into()
            } else if value.is_empty() {
                "—".into()
            } else {
                value.to_string().into()
            };
            row(
                label,
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when_some(failed, |d, e| d.child(div().text_xs().text_color(c.danger).child(t!("settings.taken", error = e).to_string())))
                    .child(chip(SharedString::from(format!("hk-{field:?}")), shown, recording, move |_, window, cx| {
                        this.update(cx, |w, cx| {
                            w.recording = Some(field);
                            window.focus(&w.focus, cx);
                            crate::app::set_hotkeys_paused(true, cx);
                            cx.notify();
                        })
                        .ok();
                    })),
            )
        };

        let autostart = crate::desktop::autostart_path().exists();
        let menu = crate::desktop::menu_entry_path().exists();
        let dir = crate::storage::captures_dir();

        div()
            .id("settings")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.record_key(ev, cx)))
            .size_full()
            .overflow_y_scroll()
            .bg(c.bg)
            .text_color(c.fg)
            .text_sm()
            .p_5()
            .flex()
            .flex_col()
            .child(div().text_lg().child(t!("settings.title").to_string()))
            .child(section(&t!("settings.after_capture")))
            .child(row(
                &t!("settings.action"),
                div()
                    .flex()
                    .gap_2()
                    .child(chip("after-editor", t!("settings.open_editor").to_string(), s.after_capture == AfterCapture::Editor, upd(|s| s.after_capture = AfterCapture::Editor)))
                    .child(chip("after-copy", t!("settings.copy_only").to_string(), s.after_capture == AfterCapture::CopyOnly, upd(|s| s.after_capture = AfterCapture::CopyOnly))),
            ))
            .child(row(&t!("settings.copy_now"), switch("copy-on-capture", s.copy_on_capture, upd(|s| s.copy_on_capture = !s.copy_on_capture))))
            .child(row(
                &t!("settings.delay"),
                div()
                    .flex()
                    .gap_2()
                    .child(chip("delay-3", "3 s", s.capture_delay == 3, upd(|s| s.capture_delay = 3)))
                    .child(chip("delay-5", "5 s", s.capture_delay == 5, upd(|s| s.capture_delay = 5)))
                    .child(chip("delay-10", "10 s", s.capture_delay == 10, upd(|s| s.capture_delay = 10))),
            ))
            .child(row(&t!("settings.capture_cursor"), switch("cursor", s.capture_cursor, upd(|s| s.capture_cursor = !s.capture_cursor))))
            .child(section(&t!("settings.storage")))
            .child(row(
                &t!("settings.folder"),
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .justify_end()
                    .items_center()
                    .gap_2()
                    // Long paths are shortened with "…" so the buttons stay visible.
                    .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_color(c.muted).text_xs().child(dir.display().to_string()))
                    .child(chip("dir-change", t!("settings.change").to_string(), false, |_, _, cx| {
                        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some(t!("settings.choose_folder").to_string().into()) });
                        cx.spawn(async move |cx| {
                            if let Ok(Ok(Some(paths))) = rx.await
                                && let Some(p) = paths.into_iter().next()
                            {
                                cx.update(|cx| {
                                    crate::storage::set_save_dir(Some(p.clone()));
                                    settings::update(cx, |s| s.save_dir = Some(p));
                                    cx.refresh_windows();
                                });
                            }
                        })
                        .detach();
                    }))
                    .child(chip("dir-open", t!("common.open").to_string(), false, move |_, _, _| {
                        let _ = std::fs::create_dir_all(crate::storage::captures_dir());
                        let _ = std::process::Command::new("xdg-open").arg(crate::storage::captures_dir()).spawn();
                    })),
            ))
            .child(section(&t!("settings.hotkeys")))
            .child(hotkey_row(HotkeyField::Region, &t!("capture.region"), &s.hotkeys.region))
            .child(hotkey_row(HotkeyField::LastRegion, &t!("capture.last_region"), &s.hotkeys.last_region))
            .child(hotkey_row(HotkeyField::Fullscreen, &t!("capture.screen"), &s.hotkeys.fullscreen))
            .child(hotkey_row(HotkeyField::Delayed, &t!("capture.delayed", s = s.capture_delay), &s.hotkeys.delayed))
            .child(hotkey_row(HotkeyField::PickColor, &t!("capture.pick_color"), &s.hotkeys.pick_color))
            .child(
                div()
                    .mt_1()
                    .text_xs()
                    .text_color(c.muted)
                    .child(t!("settings.print_hint").to_string()),
            )
            .child(section(&t!("settings.appearance")))
            .child(row(
                &t!("settings.appearance_mode"),
                div()
                    .flex()
                    .gap_2()
                    .child(chip("app-system", t!("appearance.system").to_string(), s.appearance == settings::Appearance::System, appearance(settings::Appearance::System)))
                    .child(chip("app-light", t!("appearance.light").to_string(), s.appearance == settings::Appearance::Light, appearance(settings::Appearance::Light)))
                    .child(chip("app-dark", t!("appearance.dark").to_string(), s.appearance == settings::Appearance::Dark, appearance(settings::Appearance::Dark))),
            ))
            .child(section(&t!("settings.language")))
            .child(language_row(s.language))
            .child(section(&t!("settings.updates")))
            .child(updates_rows(&s, cx))
            .child(section(&t!("settings.system")))
            .child(row(
                &t!("settings.autostart"),
                switch("autostart", autostart, move |_, window, _| {
                    if let Err(e) = crate::desktop::set_autostart(!autostart) {
                        log::error!("autostart: {e:#}");
                    }
                    window.refresh();
                }),
            ))
            .child(row(
                &t!("settings.menu_entry"),
                switch("menu-entry", menu, move |_, window, _| {
                    if let Err(e) = crate::desktop::set_menu_entry(!menu) {
                        log::error!("menu entry: {e:#}");
                    }
                    window.refresh();
                }),
            ))
    }
}
