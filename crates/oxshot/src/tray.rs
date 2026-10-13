//! Tray icon (StatusNotifierItem, shown by Cinnamon's xapp status applet).

use futures::channel::mpsc::UnboundedSender;
use rust_i18n::t;

use crate::app::{CaptureMode, Command};

struct Tray {
    tx: UnboundedSender<Command>,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "oxshot".into()
    }
    fn title(&self) -> String {
        "OxShot".into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        crate::icon::tray_pixmaps().into_iter().map(|(width, height, data)| ksni::Icon { width, height, data }).collect()
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.unbounded_send(Command::Capture(CaptureMode::Region));
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: "OxShot".into(), description: t!("tray.tooltip").to_string(), ..Default::default() }
    }
    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        // Keep waiting: at login the panel's tray often starts after us (Anything does the same).
        log::info!("tray not available (yet), waiting for it: {reason:?}");
        true
    }
    fn watcher_online(&self) {
        log::info!("tray available, icon registered");
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        let item = |label: &str, cmd: Command| -> ksni::MenuItem<Self> {
            StandardItem {
                label: label.into(),
                activate: Box::new(move |t: &mut Self| {
                    let _ = t.tx.unbounded_send(cmd.clone());
                }),
                ..Default::default()
            }
            .into()
        };
        let mut items = Vec::new();
        if let Some(version) = crate::update::TRAY_OFFER.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            items.push(item(&t!("update.tray", version = version), Command::Settings));
            items.push(MenuItem::Separator);
        }
        items.extend([
            item(&t!("capture.region"), Command::Capture(CaptureMode::Region)),
            item(&t!("capture.last_region"), Command::Capture(CaptureMode::LastRegion)),
            item(&t!("capture.screen"), Command::Capture(CaptureMode::Fullscreen)),
            item(&t!("capture.delayed_menu"), Command::Delayed(None)),
            item(&t!("capture.pick_color"), Command::PickColor),
            MenuItem::Separator,
            item(&t!("tray.workspace"), Command::Workspace),
            item(&t!("tray.settings"), Command::Settings),
            item(&t!("tray.quit"), Command::Quit),
        ]);
        items
    }
}

static HANDLE: std::sync::OnceLock<ksni::blocking::Handle<Tray>> = std::sync::OnceLock::new();

/// Rebuilds the tray menu (after a language change).
pub fn refresh() {
    if let Some(h) = HANDLE.get() {
        h.update(|_| {});
    }
}

pub fn spawn(tx: UnboundedSender<Command>) {
    std::thread::Builder::new()
        .name("oxshot-tray".into())
        .spawn(move || {
            use ksni::blocking::TrayMethods;
            match (Tray { tx }).assume_sni_available(true).spawn() {
                Ok(handle) => {
                    // Kept for the life of the process (dropping it removes the icon).
                    let _ = HANDLE.set(handle);
                }
                Err(e) => log::warn!("tray icon unavailable: {e}"),
            }
        })
        .ok();
}
