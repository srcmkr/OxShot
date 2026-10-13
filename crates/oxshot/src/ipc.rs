//! Single-instance control: the running OxShot listens on a Unix socket; `oxshot capture`
//! etc. just send a one-line command, so a hotkey bound in the desktop settings triggers a
//! capture without any start-up cost.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use crate::app::Command;

pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let display = std::env::var("DISPLAY").unwrap_or_default().replace([':', '/'], "_");
    dir.join(format!("oxshot{display}.sock"))
}

/// Tries to hand a command to a running instance. Returns false if none is running.
pub fn send(cmd: &Command) -> bool {
    let Ok(mut s) = UnixStream::connect(socket_path()) else { return false };
    let line = format!("{}\n", cmd.to_line());
    s.write_all(line.as_bytes()).is_ok()
}

/// Binds the socket (removing a stale one) and forwards commands to `tx`.
pub fn listen(tx: futures::channel::mpsc::UnboundedSender<Command>) -> anyhow::Result<()> {
    let path = socket_path();
    if path.exists() {
        if UnixStream::connect(&path).is_ok() {
            anyhow::bail!("another OxShot instance is running");
        }
        let _ = std::fs::remove_file(&path);
    }
    let listener = UnixListener::bind(&path)?;
    std::thread::Builder::new().name("oxshot-ipc".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).is_ok()
                && let Some(cmd) = Command::from_line(line.trim())
            {
                let _ = tx.unbounded_send(cmd);
            }
        }
    })?;
    Ok(())
}

pub fn cleanup() {
    let _ = std::fs::remove_file(socket_path());
}
