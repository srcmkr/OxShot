//! Desktop integration: the autostart entry, the menu entry and their icon. Both entries
//! repair themselves at every start, so they keep pointing at the program that runs now —
//! also after it was moved or updated (idea from the Anything project).

use std::path::{Path, PathBuf};

pub fn autostart_path() -> PathBuf {
    dirs::config_dir().unwrap_or_default().join("autostart").join("oxshot.desktop")
}

pub fn menu_entry_path() -> PathBuf {
    dirs::data_dir().unwrap_or_default().join("applications").join("oxshot.desktop")
}

/// The program to start: the installed copy (an AppImage runs from a new temporary mount
/// every time, so its own file is what has to be started), else this build.
fn program() -> String {
    oxshot_update::layout::Install::current()
        .app()
        .or_else(|| std::env::current_exe().ok())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "oxshot".into())
}

/// Quotes an `Exec` argument as the Desktop Entry spec wants it.
fn quote(arg: &str) -> String {
    if arg.chars().all(|c| c.is_ascii_alphanumeric() || "/-_.+,=:@%".contains(c)) {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

fn entry(args: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=OxShot\nComment={}\nExec={}{args}\nIcon=oxshot\nTerminal=false\nCategories=Graphics;2DGraphics;RasterGraphics;\nX-GNOME-Autostart-enabled=true\n",
        rust_i18n::t!("desktop.comment"),
        quote(&program())
    )
}

/// The menu entry for an installation (Debian package, AppImage, `/usr/local`), with the
/// comment in every language: `oxshot --desktop-entry /usr/bin/oxshot`.
pub fn system_entry(exec: &str) -> String {
    let mut out = format!(
        "[Desktop Entry]\nType=Application\nName=OxShot\nGenericName=Screenshot Tool\nComment={}\n",
        rust_i18n::t!("desktop.comment", locale = "en")
    );
    for lang in crate::i18n::Lang::ALL.into_iter().filter(|l| *l != crate::i18n::Lang::En) {
        let code = lang.code().replace('-', "_");
        out.push_str(&format!("Comment[{code}]={}\n", rust_i18n::t!("desktop.comment", locale = lang.code())));
    }
    out.push_str(&format!(
        "Exec={} capture\nIcon=oxshot\nTerminal=false\nCategories=Graphics;2DGraphics;RasterGraphics;\nKeywords=screenshot;capture;annotate;screen;\nStartupNotify=false\n",
        quote(exec)
    ));
    out
}

fn autostart_entry() -> String {
    entry("")
}

fn menu_entry() -> String {
    entry(" capture")
}

fn write(path: &Path, content: &str) -> anyhow::Result<()> {
    crate::assets::install_desktop_icon()?;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    // Temporary file + rename: never a half-written entry.
    let tmp = path.with_extension("desktop.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

pub fn set_autostart(on: bool) -> anyhow::Result<()> {
    let p = autostart_path();
    if on { write(&p, &autostart_entry()) } else { remove(&p) }
}

pub fn set_menu_entry(on: bool) -> anyhow::Result<()> {
    let p = menu_entry_path();
    if on { write(&p, &menu_entry()) } else { remove(&p) }
}

fn remove(p: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(p) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// Called at start: rewrites existing entries whose content is outdated (moved program,
/// other language) and reinstalls the icon. Entries the user switched off stay off.
pub fn sync() {
    // Installed for all users: the system's menu entry applies; ours in the user's folder
    // would hide it (and may point to an old build).
    if oxshot_update::layout::Install::current().is_system() && menu_entry_path().exists() {
        match std::fs::remove_file(menu_entry_path()) {
            Ok(()) => log::info!("removed the user menu entry (system install)"),
            Err(e) => log::warn!("cannot remove {}: {e}", menu_entry_path().display()),
        }
    }
    for (path, content) in [(autostart_path(), autostart_entry()), (menu_entry_path(), menu_entry())] {
        if !path.exists() {
            continue;
        }
        if std::fs::read_to_string(&path).is_ok_and(|old| old == content) {
            continue;
        }
        match write(&path, &content) {
            Ok(()) => log::info!("updated {}", path.display()),
            Err(e) => log::warn!("cannot update {}: {e:#}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::quote;

    #[test]
    fn exec_quoting() {
        assert_eq!(quote("/usr/bin/oxshot"), "/usr/bin/oxshot");
        assert_eq!(quote("/home/a b/oxshot"), "\"/home/a b/oxshot\"");
        assert_eq!(quote("/x/$y\"z"), "\"/x/\\$y\\\"z\"");
    }
}
