//! OxShot's color schemes for the window chrome (light and dark) and the switch between
//! them. "System" follows the desktop's preference (xdg-desktop-portal `color-scheme`).

use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Hsla, Window, WindowAppearance, hsla, rgb};

use crate::settings::{self, Appearance};

#[derive(Clone, Copy)]
pub struct Colors {
    pub bg: Hsla,
    pub panel: Hsla,
    pub panel_alt: Hsla,
    pub canvas_bg: Hsla,
    pub border: Hsla,
    pub fg: Hsla,
    pub muted: Hsla,
    pub hover: Hsla,
    pub accent: Hsla,
    pub accent_bg: Hsla,
    pub primary: Hsla,
    pub primary_hover: Hsla,
    pub on_primary: Hsla,
    /// Errors and warnings.
    pub danger: Hsla,
    /// Barely visible decoration (watermarks, empty-state icons).
    pub faint: Hsla,
}

/// The mode currently shown; read without an `App` from every render function.
static DARK: AtomicBool = AtomicBool::new(true);

impl Colors {
    pub fn get() -> Colors {
        if DARK.load(Ordering::Relaxed) { Self::dark() } else { Self::light() }
    }

    fn dark() -> Colors {
        Colors {
            bg: rgb(0x1b1d22).into(),
            panel: rgb(0x23262d).into(),
            panel_alt: rgb(0x1f2228).into(),
            canvas_bg: rgb(0x15171b).into(),
            border: rgb(0x343842).into(),
            fg: rgb(0xe6e8ec).into(),
            muted: rgb(0x9aa1ad).into(),
            hover: hsla(0.0, 0.0, 1.0, 0.07),
            accent: rgb(0x4da3ff).into(),
            accent_bg: hsla(0.59, 1.0, 0.65, 0.16),
            primary: rgb(0x22a35a).into(),
            primary_hover: rgb(0x29b866).into(),
            on_primary: rgb(0xffffff).into(),
            danger: rgb(0xff7b6b).into(),
            faint: hsla(0.0, 0.0, 1.0, 0.16),
        }
    }

    /// Soft cool-gray light scheme; the canvas stays a mid gray so screenshots with white
    /// edges remain distinguishable from the surroundings.
    fn light() -> Colors {
        Colors {
            bg: rgb(0xf3f4f7).into(),
            panel: rgb(0xffffff).into(),
            panel_alt: rgb(0xf7f8fa).into(),
            canvas_bg: rgb(0xdfe2e8).into(),
            border: rgb(0xd7dbe2).into(),
            fg: rgb(0x1d2026).into(),
            muted: rgb(0x5f6875).into(),
            hover: hsla(0.0, 0.0, 0.0, 0.05),
            accent: rgb(0x1f6fd6).into(),
            accent_bg: hsla(0.59, 0.75, 0.48, 0.12),
            primary: rgb(0x1e9650).into(),
            primary_hover: rgb(0x23a95b).into(),
            on_primary: rgb(0xffffff).into(),
            danger: rgb(0xd23b2f).into(),
            faint: hsla(0.0, 0.0, 0.0, 0.14),
        }
    }
}

/// Dark or light for the current settings and the desktop's appearance.
fn wants_dark(cx: &App) -> bool {
    match settings::get(cx).appearance {
        Appearance::Dark => true,
        Appearance::Light => false,
        Appearance::System => matches!(cx.window_appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark),
    }
}

/// Applies the chosen appearance to our chrome and gpui-component, redrawing all windows.
pub fn apply(cx: &mut App) {
    let dark = wants_dark(cx);
    let changed = DARK.swap(dark, Ordering::Relaxed) != dark;
    let mode = if dark { ThemeMode::Dark } else { ThemeMode::Light };
    if changed || gpui_kit::component::ActiveTheme::theme(cx).mode != mode {
        Theme::change(mode, None, cx);
    }
    cx.refresh_windows();
}

/// Lets a window follow the desktop when it switches between light and dark.
pub fn follow_system(window: &mut Window) {
    window.observe_window_appearance(|_, cx| apply(cx)).detach();
}
