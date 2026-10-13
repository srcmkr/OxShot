//! User settings (`~/.config/oxshot/settings.json`).

use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::{App, Global};
use oxshot_core::edit::{Style, Tool};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AfterCapture {
    /// Open the editor window (default, Screenpresso with the workspace step skipped).
    #[default]
    Editor,
    /// Only copy to the clipboard and save.
    CopyOnly,
}

/// Light or dark window chrome; `System` follows the desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

/// Interface language; `System` follows the desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Language {
    #[default]
    System,
    En,
    De,
    Es,
    Fr,
    PtBr,
    Ru,
    ZhCn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub region: String,
    pub last_region: String,
    pub fullscreen: String,
    pub pick_color: String,
    pub delayed: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self { region: "Print".into(), last_region: "Shift+Print".into(), fullscreen: "Ctrl+Alt+Shift+Print".into(), pick_color: "Ctrl+Print".into(), delayed: "Ctrl+Shift+Print".into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub after_capture: AfterCapture,
    pub language: Language,
    pub appearance: Appearance,
    /// Put every capture on the clipboard right away (before editing).
    pub copy_on_capture: bool,
    pub capture_cursor: bool,
    /// Where captures are stored; default `~/Pictures/OxShot`.
    pub save_dir: Option<PathBuf>,
    pub hotkeys: Hotkeys,
    /// Remembered per-tool styles.
    pub styles: HashMap<Tool, Style>,
    /// Look for new releases on GitHub.
    pub check_updates: bool,
    /// A release the user does not want to be offered.
    pub skipped_version: Option<String>,
    /// The version that ran last (to notice the first start after an update).
    pub last_version: Option<String>,
    /// Seconds before a delayed capture.
    pub capture_delay: u32,
    /// Colors picked from the screen, newest first (at most 8).
    pub recent_colors: Vec<oxshot_core::Color>,
    /// Last region capture (for "repeat last region").
    pub last_region: Option<oxshot_core::IRect>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            after_capture: AfterCapture::Editor,
            language: Language::System,
            appearance: Appearance::System,
            copy_on_capture: true,
            capture_cursor: false,
            save_dir: None,
            hotkeys: Hotkeys::default(),
            styles: HashMap::new(),
            last_region: None,
            recent_colors: Vec::new(),
            capture_delay: 5,
            check_updates: true,
            skipped_version: None,
            last_version: None,
        }
    }
}

pub struct SettingsGlobal(pub Settings);
impl Global for SettingsGlobal {}

pub fn path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("oxshot").join("settings.json")
}

pub fn load() -> Settings {
    match std::fs::read_to_string(path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            log::warn!("invalid settings, using defaults: {e}");
            Settings::default()
        }),
        Err(_) => Settings::default(),
    }
}

pub fn save(s: &Settings) {
    let p = path();
    let res = (|| -> anyhow::Result<()> {
        std::fs::create_dir_all(p.parent().unwrap())?;
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(s)?)?;
        std::fs::rename(tmp, &p)?;
        Ok(())
    })();
    if let Err(e) = res {
        log::warn!("cannot save settings: {e:#}");
    }
}

pub fn get(cx: &App) -> &Settings {
    &cx.global::<SettingsGlobal>().0
}

pub fn update(cx: &mut App, f: impl FnOnce(&mut Settings)) {
    let s = &mut cx.global_mut::<SettingsGlobal>().0;
    f(s);
    save(s);
}

pub fn style_for(tool: Tool, cx: &App) -> Style {
    get(cx).styles.get(&tool).cloned().unwrap_or_else(|| Style::default_for(tool))
}

pub fn remember_style(tool: Tool, st: &Style, cx: &mut App) {
    let st = st.clone();
    update(cx, |s| {
        s.styles.insert(tool, st);
    });
}
