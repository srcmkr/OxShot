//! Interface language: follows the desktop unless the settings say otherwise. Texts live
//! in `locales/*.yml` (same setup as the Anything project; gpui-component's own texts are
//! translated in `locales/components.yml`).

use gpui_kit::{App, Global};

use crate::settings::{self, Language};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    De,
    Es,
    Fr,
    PtBr,
    Ru,
    ZhCn,
}

impl Lang {
    pub const ALL: [Lang; 7] = [Lang::En, Lang::De, Lang::Es, Lang::Fr, Lang::PtBr, Lang::Ru, Lang::ZhCn];

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::De => "de",
            Lang::Es => "es",
            Lang::Fr => "fr",
            Lang::PtBr => "pt-BR",
            Lang::Ru => "ru",
            Lang::ZhCn => "zh-CN",
        }
    }

    /// The language's own name, for the settings.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::De => "Deutsch",
            Lang::Es => "Español",
            Lang::Fr => "Français",
            Lang::PtBr => "Português (Brasil)",
            Lang::Ru => "Русский",
            Lang::ZhCn => "简体中文",
        }
    }

    pub fn setting(self) -> Language {
        match self {
            Lang::En => Language::En,
            Lang::De => Language::De,
            Lang::Es => Language::Es,
            Lang::Fr => Language::Fr,
            Lang::PtBr => Language::PtBr,
            Lang::Ru => Language::Ru,
            Lang::ZhCn => Language::ZhCn,
        }
    }
}

/// Translated name of a drawing tool.
pub fn tool_label(tool: oxshot_core::edit::Tool) -> String {
    rust_i18n::t!(format!("tool.{}", tool.key())).to_string()
}

/// The desktop's language if we have it, English otherwise.
pub fn system() -> Lang {
    sys_locale::get_locale().map_or(Lang::En, |l| from_locale(&l))
}

/// `de-AT`, `pt_BR.UTF-8`, `zh-Hans-CN`, … Traditional Chinese stays English (simplified
/// characters would be the wrong script).
fn from_locale(locale: &str) -> Lang {
    let locale = locale.to_ascii_lowercase().replace('_', "-");
    match locale.split(['-', '.', '@']).next().unwrap_or_default() {
        "de" => Lang::De,
        "es" => Lang::Es,
        "fr" => Lang::Fr,
        "pt" => Lang::PtBr,
        "ru" => Lang::Ru,
        "zh" if !["-tw", "-hk", "-mo", "-hant"].iter().any(|t| locale.contains(t)) => Lang::ZhCn,
        _ => Lang::En,
    }
}

pub fn resolve(setting: Language) -> Lang {
    match setting {
        Language::System => system(),
        Language::En => Lang::En,
        Language::De => Lang::De,
        Language::Es => Lang::Es,
        Language::Fr => Lang::Fr,
        Language::PtBr => Lang::PtBr,
        Language::Ru => Lang::Ru,
        Language::ZhCn => Lang::ZhCn,
    }
}

/// Sets the language early (before the GPUI app exists), e.g. for the CLI usage text.
pub fn init_from(setting: Language) {
    let lang = resolve(setting);
    rust_i18n::set_locale(lang.code());
}

struct Applied;
impl Global for Applied {}

/// Applies the language from the settings to OxShot and gpui-component and redraws.
pub fn apply(cx: &mut App) {
    if !cx.has_global::<Applied>() {
        // Our translations of gpui-component's texts (namespace "ui").
        gpui_kit::component::_rust_i18n_extend(crate::_rust_i18n_backend(), "ui");
        cx.set_global(Applied);
    }
    let lang = resolve(settings::get(cx).language);
    rust_i18n::set_locale(lang.code());
    gpui_kit::component::set_locale(lang.code());
    crate::tray::refresh();
    cx.refresh_windows();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_languages() {
        assert_eq!(from_locale("de-AT"), Lang::De);
        assert_eq!(from_locale("de_DE.UTF-8"), Lang::De);
        assert_eq!(from_locale("pt_BR.UTF-8"), Lang::PtBr);
        assert_eq!(from_locale("zh-Hans-SG"), Lang::ZhCn);
        assert_eq!(from_locale("zh-TW"), Lang::En);
        assert_eq!(from_locale("C"), Lang::En);
    }

    /// Every key in `locales/*.yml` has a non-empty text in every language.
    #[test]
    fn complete_translations() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/locales");
        let mut keys = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            let mut current: Option<(String, Vec<String>)> = None;
            let check = |k: Option<(String, Vec<String>)>| {
                if let Some((key, langs)) = k {
                    for lang in Lang::ALL {
                        assert!(langs.iter().any(|l| l == lang.code()), "{}: {key} has no {}", path.display(), lang.code());
                    }
                }
            };
            for line in text.lines() {
                if line.trim().is_empty() || line.trim_start().starts_with('#') || line.starts_with("_version") {
                    continue;
                }
                if !line.starts_with(' ') {
                    check(current.take());
                    current = Some((line.trim_end_matches(':').to_string(), Vec::new()));
                    keys += 1;
                } else if let Some((_, langs)) = &mut current {
                    let (lang, value) = line.trim().split_once(':').expect("lang: text");
                    assert!(!value.trim().trim_matches('"').is_empty(), "empty text in {}", path.display());
                    langs.push(lang.to_string());
                }
            }
            check(current.take());
        }
        assert!(keys > 0);
    }
}
