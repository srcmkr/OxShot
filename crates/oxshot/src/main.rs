//! OxShot – fast screenshots with a Screenpresso-style editor for Linux.

mod app;
mod assets;
mod clipboard;
mod countdown;
mod desktop;
mod editor;
mod gfx;
mod i18n;
mod icon;
mod ipc;
mod manage;
mod notify;
mod overlay;
mod settings;
mod settings_window;
mod storage;
mod theme;
mod tray;
mod update;
mod workspace;

use app::{CaptureMode, Command};

rust_i18n::i18n!("locales", fallback = "en");


fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("oxshot=info,oxshot_core=info,oxshot_x11=info,warn")).init();
    i18n::init_from(settings::load().language);
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Packaging helpers and the privileged update actions run without the GUI.
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["--desktop-entry", exec] => {
            print!("{}", desktop::system_entry(exec));
            return;
        }
        [action] if action.starts_with("--") => {
            if let Some(code) = manage::run(action) {
                std::process::exit(code);
            }
        }
        _ => {}
    }
    let cmd = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => None,
        ["capture"] => Some(Command::Capture(CaptureMode::Region)),
        ["capture", "--last"] => Some(Command::Capture(CaptureMode::LastRegion)),
        ["capture", "--full"] => Some(Command::Capture(CaptureMode::Fullscreen)),
        ["capture", "--delay"] => Some(Command::Delayed(None)),
        ["capture", "--delay", n] if n.parse::<u32>().is_ok() => Some(Command::Delayed(n.parse().ok())),
        ["open", path] => Some(Command::Open(std::fs::canonicalize(path).unwrap_or_else(|_| path.into()))),
        ["quit"] => Some(Command::Quit),
        ["settings"] => Some(Command::Settings),
        ["pick"] => Some(Command::PickColor),
        ["workspace"] => Some(Command::Workspace),
        _ => {
            eprint!("{}", rust_i18n::t!("cli.usage"));
            std::process::exit(if args.iter().any(|a| a == "-h" || a == "--help") { 0 } else { 2 });
        }
    };

    // Hand over to a running instance if there is one.
    if let Some(c) = &cmd {
        if ipc::send(c) {
            return;
        }
        if *c == Command::Quit {
            return;
        }
    } else if std::os::unix::net::UnixStream::connect(ipc::socket_path()).is_ok() {
        eprintln!("{}", rust_i18n::t!("cli.running"));
        return;
    }
    app::run(cmd.into_iter().collect());
}
