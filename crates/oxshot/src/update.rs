//! New versions (same design as rustpad++ and Anything): a check against GitHub's latest
//! release 30 seconds after the start and every 6 hours after that, and installing one,
//! depending on how OxShot is installed: from the Debian package or under `/usr/local`,
//! `oxshot --update` does it as root through pkexec (downloading and checking the release
//! itself, see `manage`); an AppImage replaces its own file. A build run from its own
//! folder can be installed for all users. Then OxShot restarts: open editors are saved,
//! the new copy starts as soon as this process has ended.

use std::{
    io::Read as _,
    path::Path,
    process::{Command, Stdio},
    sync::Mutex,
    time::Duration,
};

use gpui_kit::{App, AppContext as _, Global, Hsla};
use oxshot_update::{
    Release,
    layout::{Install, LOCAL},
};
use rust_i18n::t;

use crate::settings;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const FIRST_CHECK: Duration = Duration::from_secs(30);
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Default)]
pub struct Updates {
    pub latest: Option<Release>,
    /// The last check got no answer.
    pub failed: bool,
    pub checking: bool,
    pub busy: bool,
    pub outcome: Option<Outcome>,
    /// Version a notification was already shown for.
    notified: Option<String>,
}

impl Global for Updates {}

/// The version the tray menu offers (read on the tray's thread).
pub static TRAY_OFFER: Mutex<Option<String>> = Mutex::new(None);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Replaces the installed OxShot with the latest signed release.
    Update,
    /// Installs this build for all users.
    InstallApp,
}

impl Action {
    fn argument(self) -> &'static str {
        match self {
            Action::Update => "--update",
            Action::InstallApp => "--install-app",
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Outcome {
    Done,
    /// The password dialog was closed.
    Cancelled,
    /// Wrong password or not allowed.
    Refused,
    /// No way to ask for the password here; the command to run by hand.
    NoPrompt(String),
    Failed(String),
}

pub fn init(cx: &mut App) {
    cx.set_global(Updates::default());
    // OXSHOT_UPDATE_DELAY=<seconds> shortens the first wait for testing.
    let first = std::env::var("OXSHOT_UPDATE_DELAY").ok().and_then(|s| s.parse().ok()).map_or(FIRST_CHECK, Duration::from_secs);
    cx.spawn(async move |cx| {
        let mut wait = first;
        loop {
            cx.background_executor().timer(wait).await;
            wait = EVERY;
            cx.update(|cx| {
                if settings::get(cx).check_updates {
                    check(cx);
                }
            });
        }
    })
    .detach();
}

/// Whether this version runs for the first time after an update (not after a new
/// installation). Remembers this version, so it is true only once.
pub fn first_start_after_update(cx: &mut App) -> bool {
    let previous = settings::get(cx).last_version.clone();
    if previous.as_deref() != Some(VERSION) {
        settings::update(cx, |s| s.last_version = Some(VERSION.to_string()));
    }
    previous.is_some_and(|p| oxshot_update::is_newer(VERSION, &p))
}

/// Asks GitHub now, in the background.
pub fn check(cx: &mut App) {
    if cx.global::<Updates>().checking {
        return;
    }
    // Development aid: OXSHOT_UPDATE_FAKE=<version> pretends GitHub offers that version,
    // without asking it.
    let fake = std::env::var("OXSHOT_UPDATE_FAKE").ok();
    let task = cx.background_spawn(async move {
        match fake {
            Some(version) => Some(Release { page: releases_page(), version, assets: Vec::new() }),
            None => oxshot_update::latest(),
        }
    });
    cx.global_mut::<Updates>().checking = true;
    cx.refresh_windows();
    cx.spawn(async move |cx| {
        let latest = task.await;
        cx.update(|cx| {
            let updates = cx.global_mut::<Updates>();
            updates.checking = false;
            updates.failed = latest.is_none();
            if latest.is_some() {
                updates.latest = latest;
            }
            announce(cx);
            cx.refresh_windows();
        });
    })
    .detach();
}

/// Shows a found update in the tray and, once per version, as a notification.
fn announce(cx: &mut App) {
    let offer = available(cx).map(|r| r.version);
    *TRAY_OFFER.lock().unwrap_or_else(|e| e.into_inner()) = offer.clone();
    crate::tray::refresh();
    let updates = cx.global_mut::<Updates>();
    if let Some(version) = offer
        && updates.notified.as_deref() != Some(version.as_str())
    {
        updates.notified = Some(version.clone());
        crate::notify::update_available(&version);
    }
}

pub fn releases_page() -> String {
    format!("https://github.com/{}/releases", oxshot_update::REPOSITORY)
}

/// A newer release than this one, skipped or not.
pub fn newer(cx: &App) -> Option<Release> {
    let latest = cx.try_global::<Updates>()?.latest.clone()?;
    oxshot_update::is_newer(&latest.version, VERSION).then_some(latest)
}

/// A newer release the user has not skipped.
pub fn available(cx: &App) -> Option<Release> {
    let skipped = settings::get(cx).skipped_version.clone();
    newer(cx).filter(|r| skipped.as_deref() != Some(r.version.as_str()))
}

/// Never offers this version again; a later one is offered as usual.
pub fn skip(cx: &mut App) {
    let Some(release) = newer(cx) else { return };
    settings::update(cx, |s| s.skipped_version = Some(release.version));
    announce(cx);
    cx.refresh_windows();
}

/// Whether this OxShot can install an update: from the package, under `/usr/local` or
/// as an AppImage. A build run from its own folder only points to the release page.
pub fn can_update() -> bool {
    Install::current() != Install::Build
}

/// Whether this is a build run from its own folder, which can be installed for all users.
pub fn is_build() -> bool {
    Install::current() == Install::Build
}

/// One line on where things stand, and the color of the dot in front of it.
pub fn status(cx: &App) -> (String, Hsla) {
    let c = crate::theme::Colors::get();
    let Some(state) = cx.try_global::<Updates>() else { return (String::new(), c.muted) };
    match newer(cx) {
        Some(release) => (t!("update.available", version = release.version).to_string(), c.danger),
        None if state.checking => (t!("update.checking").to_string(), c.muted),
        None if state.latest.is_some() => (t!("update.up_to_date").to_string(), c.primary),
        None if state.failed => (t!("update.check_failed").to_string(), c.muted),
        None if !settings::get(cx).check_updates => (t!("update.check_off").to_string(), c.muted),
        None => (t!("update.check_soon").to_string(), c.muted),
    }
}

/// What happened to the last install, if anything is worth saying.
pub fn note(cx: &App) -> Option<String> {
    let state = cx.try_global::<Updates>()?;
    match &state.outcome {
        _ if state.busy => Some(t!("update.installing").to_string()),
        Some(Outcome::Refused) => Some(t!("update.refused").to_string()),
        Some(Outcome::Failed(error)) => Some(t!("update.failed", error = error).to_string()),
        Some(Outcome::NoPrompt(command)) => Some(t!("update.no_prompt", command = command).to_string()),
        _ => None,
    }
}

/// Installs the latest release (`Action::Update`) or this build for all users
/// (`Action::InstallApp`), then restarts from the installed copy.
pub fn install(action: Action, cx: &mut App) {
    if cx.global::<Updates>().busy {
        return;
    }
    let updates = cx.global_mut::<Updates>();
    updates.busy = true;
    updates.outcome = None;
    cx.refresh_windows();
    let task = cx.background_spawn(async move {
        match (action, Install::current()) {
            (Action::Update, Install::AppImage(image)) => match update_appimage(&image) {
                Ok(()) => Outcome::Done,
                Err(error) => Outcome::Failed(error),
            },
            _ => run_privileged(action),
        }
    });
    cx.spawn(async move |cx| {
        let outcome = task.await;
        cx.update(|cx| {
            let updates = cx.global_mut::<Updates>();
            updates.busy = false;
            match outcome {
                Outcome::Done => restart(cx),
                Outcome::Cancelled => {}
                other => updates.outcome = Some(other),
            }
            cx.refresh_windows();
        });
    })
    .detach();
}

/// Replaces an AppImage with the latest release's, if that is newer and signed. The file
/// belongs to the user, so no password is needed.
fn update_appimage(image: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    let release = oxshot_update::latest().ok_or("no release information from GitHub")?;
    if !oxshot_update::is_newer(&release.version, VERSION) {
        return Err(format!("{} is not newer than {VERSION}", release.version));
    }
    let data = oxshot_update::fetch_verified(&release, &oxshot_update::appimage_name(&release.version))?;
    let new = image.with_extension("new");
    std::fs::write(&new, data).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    std::fs::rename(&new, image).map_err(|e| e.to_string())
}

/// Runs `oxshot <action>` as root through pkexec: the installed program for an update
/// (the polkit rules name it), this build to install it. Blocks until the password was
/// given and the action is done.
fn run_privileged(action: Action) -> Outcome {
    let program = match action {
        Action::Update => Install::current().app(),
        Action::InstallApp => std::env::current_exe().ok(),
    };
    let Some(program) = program.filter(|p| p.is_file()) else {
        return Outcome::Failed("oxshot not found".into());
    };
    let by_hand = format!("sudo {} {}", program.display(), action.argument());
    let child = Command::new("pkexec")
        .arg(&program)
        .arg(action.argument())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else { return Outcome::NoPrompt(by_hand) };
    let mut errors = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        stderr.read_to_string(&mut errors).ok();
    }
    let Ok(status) = child.wait() else { return Outcome::Failed("pkexec".into()) };
    match status.code() {
        Some(0) => Outcome::Done,
        Some(126) => Outcome::Cancelled,
        Some(127) if errors.contains("No authentication agent") => Outcome::NoPrompt(by_hand),
        Some(127) => Outcome::Refused,
        _ => Outcome::Failed(errors.lines().last().unwrap_or_default().trim_start_matches("oxshot: ").to_owned()),
    }
}

/// Starts the installed copy once this process has ended, then quits the normal way
/// (open editors are saved first).
fn restart(cx: &mut App) {
    // The updated copy; after "Install for all users", the new one.
    let app = Install::current().app().unwrap_or_else(|| LOCAL.app.into());
    let waiter = Command::new("sh")
        .args(["-c", r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; exec "$0""#])
        .arg(&app)
        .arg(std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match waiter {
        Ok(_) => crate::app::quit(cx),
        Err(err) => log::error!("cannot start {}: {err}", app.display()),
    }
}
