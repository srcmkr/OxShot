//! Global hotkeys via passive key grabs (`XGrabKey`) on the root window.

use std::collections::HashSet;
use std::fmt;
use std::ops::BitOr;
use std::str::FromStr;
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::{Context as _, Result, anyhow, bail};
use x11rb::connection::{Connection, RequestConnection as _};
use x11rb::errors::ReplyError;
use x11rb::protocol::xproto::{
    ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, GrabMode, Keycode, ModMask, Window,
    WindowClass,
};
use x11rb::protocol::{ErrorKind, Event, xkb};
use x11rb::rust_connection::RustConnection;

/// Modifier set. Combine with `|`, e.g. `Mods::CTRL | Mods::SHIFT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mods(u8);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const CTRL: Mods = Mods(1);
    pub const SHIFT: Mods = Mods(2);
    pub const ALT: Mods = Mods(4);
    pub const SUPER: Mods = Mods(8);

    pub fn contains(self, other: Mods) -> bool {
        self.0 & other.0 == other.0
    }

    fn x_mask(self) -> u16 {
        let mut m = 0u16;
        for (flag, x) in [
            (Mods::CTRL, ModMask::CONTROL),
            (Mods::SHIFT, ModMask::SHIFT),
            (Mods::ALT, ModMask::M1),
            (Mods::SUPER, ModMask::M4),
        ] {
            if self.contains(flag) {
                m |= u16::from(x);
            }
        }
        m
    }
}

impl BitOr for Mods {
    type Output = Mods;
    fn bitor(self, rhs: Mods) -> Mods {
        Mods(self.0 | rhs.0)
    }
}

/// A non-modifier key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Print,
    Pause,
    ScrollLock,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Space,
    Escape,
    /// Lowercase ASCII letter `a..=z`.
    Letter(char),
    /// `0..=9`.
    Digit(u8),
    /// Function key `F1..=F24`.
    F(u8),
}

impl Key {
    fn keysym(self) -> u32 {
        match self {
            Key::Print => 0xff61,
            Key::Pause => 0xff13,
            Key::ScrollLock => 0xff14,
            Key::Insert => 0xff63,
            Key::Delete => 0xffff,
            Key::Home => 0xff50,
            Key::End => 0xff57,
            Key::PageUp => 0xff55,
            Key::PageDown => 0xff56,
            Key::Space => 0x20,
            Key::Escape => 0xff1b,
            Key::Letter(c) => c as u32,
            Key::Digit(d) => 0x30 + d as u32,
            Key::F(n) => 0xffbe + (n as u32 - 1),
        }
    }

    const NAMED: [(Key, &'static [&'static str]); 11] = [
        (Key::Print, &["print", "printscreen", "prtsc", "prtscr", "sysrq"]),
        (Key::Pause, &["pause", "break"]),
        (Key::ScrollLock, &["scrolllock", "scroll_lock"]),
        (Key::Insert, &["insert", "ins"]),
        (Key::Delete, &["delete", "del"]),
        (Key::Home, &["home"]),
        (Key::End, &["end"]),
        (Key::PageUp, &["pageup", "page_up", "pgup", "prior"]),
        (Key::PageDown, &["pagedown", "page_down", "pgdn", "next"]),
        (Key::Space, &["space"]),
        (Key::Escape, &["escape", "esc"]),
    ];
}

impl FromStr for Key {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Key> {
        let l = s.trim().to_ascii_lowercase();
        if let Some((k, _)) = Key::NAMED.iter().find(|(_, names)| names.contains(&l.as_str())) {
            return Ok(*k);
        }
        let mut chars = l.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            if c.is_ascii_lowercase() {
                return Ok(Key::Letter(c));
            }
            if let Some(d) = c.to_digit(10) {
                return Ok(Key::Digit(d as u8));
            }
        }
        if let Some(n) = l.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())
            && (1..=24).contains(&n)
        {
            return Ok(Key::F(n));
        }
        bail!("unknown key {s:?}")
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Letter(c) => write!(f, "{}", c.to_ascii_uppercase()),
            Key::Digit(d) => write!(f, "{d}"),
            Key::F(n) => write!(f, "F{n}"),
            Key::Print => f.write_str("Print"),
            Key::Pause => f.write_str("Pause"),
            Key::ScrollLock => f.write_str("ScrollLock"),
            Key::Insert => f.write_str("Insert"),
            Key::Delete => f.write_str("Delete"),
            Key::Home => f.write_str("Home"),
            Key::End => f.write_str("End"),
            Key::PageUp => f.write_str("PageUp"),
            Key::PageDown => f.write_str("PageDown"),
            Key::Space => f.write_str("Space"),
            Key::Escape => f.write_str("Escape"),
        }
    }
}

/// A key plus modifiers, parsed from strings like `"Ctrl+Alt+Shift+Print"`.
///
/// Modifier names (case-insensitive): `Ctrl`/`Control`, `Shift`, `Alt`/`Mod1`,
/// `Super`/`Win`/`Meta`/`Mod4`. `Display` produces the canonical form
/// `Ctrl+Shift+Alt+Super+Key`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub key: Key,
    pub mods: Mods,
}

impl FromStr for Hotkey {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Hotkey> {
        let parts: Vec<&str> = s.split('+').map(str::trim).collect();
        let (key, mods) = parts.split_last().filter(|(k, _)| !k.is_empty()).ok_or_else(|| anyhow!("empty hotkey {s:?}"))?;
        let mut m = Mods::NONE;
        for part in mods {
            let flag = match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => Mods::CTRL,
                "shift" => Mods::SHIFT,
                "alt" | "mod1" => Mods::ALT,
                "super" | "win" | "meta" | "mod4" | "logo" => Mods::SUPER,
                _ => bail!("unknown modifier {part:?} in {s:?}"),
            };
            m = m | flag;
        }
        Ok(Hotkey { key: key.parse().with_context(|| format!("in hotkey {s:?}"))?, mods: m })
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (flag, name) in [(Mods::CTRL, "Ctrl"), (Mods::SHIFT, "Shift"), (Mods::ALT, "Alt"), (Mods::SUPER, "Super")] {
            if self.mods.contains(flag) {
                write!(f, "{name}+")?;
            }
        }
        write!(f, "{}", self.key)
    }
}

struct Grabbed {
    id: u32,
    keycodes: Vec<Keycode>,
    mask: u16,
}

/// Running global hotkey listener. Dropping it (or calling [`stop`](Self::stop)) releases the
/// grabs and joins the thread.
pub struct HotkeyService {
    conn: Arc<RustConnection>,
    wake_window: Window,
    wake_atom: u32,
    thread: Option<JoinHandle<()>>,
}

impl HotkeyService {
    /// Opens a dedicated X connection, grabs each binding on the root window (in every
    /// combination of CapsLock/NumLock/ScrollLock) and spawns a thread that calls
    /// `on_press(id)` for each non-autorepeat key press.
    ///
    /// Bindings that cannot be grabbed (typically `BadAccess`: another client such as the
    /// desktop environment or another screenshot tool owns the key) are returned as errors
    /// without affecting the others.
    pub fn start(
        bindings: Vec<(Hotkey, u32)>,
        on_press: impl Fn(u32) + Send + 'static,
    ) -> Result<(HotkeyService, Vec<(Hotkey, anyhow::Error)>)> {
        let (conn, screen_num) = x11rb::connect(None).context("cannot connect to X server")?;
        let conn = Arc::new(conn);
        let root = conn.setup().roots[screen_num].root;

        // Detectable autorepeat: repeats then arrive as press-press-...-release.
        let detectable = enable_detectable_autorepeat(&conn);

        let lock_masks = lock_masks(&conn)?;
        let keymap = keymap(&conn)?;

        let mut grabbed = Vec::new();
        let mut errors = Vec::new();
        for (hk, id) in bindings {
            match grab(&conn, root, &keymap, &lock_masks, hk) {
                Ok(keycodes) => grabbed.push(Grabbed { id, keycodes, mask: hk.mods.x_mask() }),
                Err(e) => errors.push((hk, e)),
            }
        }

        let wake_window = conn.generate_id()?;
        conn.create_window(0, wake_window, root, 0, 0, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new())?;
        let wake_atom = conn.intern_atom(false, b"_OXSHOT_HOTKEY_STOP")?.reply()?.atom;
        conn.flush()?;

        let thread_conn = conn.clone();
        let thread = std::thread::Builder::new()
            .name("oxshot-hotkeys".into())
            .spawn(move || event_loop(&thread_conn, grabbed, wake_atom, detectable, on_press))?;

        Ok((HotkeyService { conn, wake_window, wake_atom, thread: Some(thread) }, errors))
    }

    /// Releases all grabs and stops the listener thread.
    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for HotkeyService {
    fn drop(&mut self) {
        let ev = ClientMessageEvent::new(32, self.wake_window, self.wake_atom, [0u32; 5]);
        let sent = self.conn.send_event(false, self.wake_window, EventMask::NO_EVENT, ev).is_ok()
            && self.conn.flush().is_ok();
        if let Some(t) = self.thread.take()
            && sent
        {
            let _ = t.join();
        }
        // Grabs are released when the connection closes with the last Arc.
    }
}

fn enable_detectable_autorepeat(conn: &RustConnection) -> bool {
    let ok = (|| -> Result<bool> {
        if conn.extension_information(xkb::X11_EXTENSION_NAME)?.is_none() {
            return Ok(false);
        }
        if !xkb::use_extension(conn, 1, 0)?.reply()?.supported {
            return Ok(false);
        }
        let flag = xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT;
        let r = xkb::per_client_flags(
            conn,
            xkb::ID::USE_CORE_KBD.into(),
            flag,
            flag,
            xkb::BoolCtrl::from(0u32),
            xkb::BoolCtrl::from(0u32),
            xkb::BoolCtrl::from(0u32),
        )?
        .reply()?;
        Ok(r.value.contains(flag))
    })();
    ok.unwrap_or(false)
}

/// All combinations of the lock modifiers (CapsLock, NumLock, ScrollLock) to grab with.
fn lock_masks(conn: &RustConnection) -> Result<Vec<u16>> {
    let modmap = conn.get_modifier_mapping()?.reply()?;
    let per = modmap.keycodes_per_modifier() as usize;
    let mapping = conn
        .get_keyboard_mapping(conn.setup().min_keycode, conn.setup().max_keycode - conn.setup().min_keycode + 1)?
        .reply()?;
    let min = conn.setup().min_keycode;
    let kpk = mapping.keysyms_per_keycode as usize;
    let mod_of_keysym = |sym: u32| -> u16 {
        let codes: HashSet<u8> = mapping
            .keysyms
            .chunks(kpk.max(1))
            .enumerate()
            .filter(|(_, syms)| syms.contains(&sym))
            .map(|(i, _)| min + i as u8)
            .collect();
        for (m, kcs) in modmap.keycodes.chunks(per.max(1)).enumerate() {
            if kcs.iter().any(|k| *k != 0 && codes.contains(k)) {
                return 1 << m;
            }
        }
        0
    };
    let mut locks = vec![u16::from(ModMask::LOCK)];
    for sym in [0xff7f /* Num_Lock */, 0xff14 /* Scroll_Lock */] {
        let m = mod_of_keysym(sym);
        if m != 0 && !locks.contains(&m) && m & 0x0f == 0 {
            // ignore if it collides with Shift/Lock/Control/Mod1
            locks.push(m);
        }
    }
    let mut out = Vec::new();
    for subset in 0..(1u32 << locks.len()) {
        let mut m = 0;
        for (i, l) in locks.iter().enumerate() {
            if subset & (1 << i) != 0 {
                m |= l;
            }
        }
        out.push(m);
    }
    Ok(out)
}

struct Keymap {
    min: u8,
    per: usize,
    syms: Vec<u32>,
}

fn keymap(conn: &RustConnection) -> Result<Keymap> {
    let s = conn.setup();
    let r = conn.get_keyboard_mapping(s.min_keycode, s.max_keycode - s.min_keycode + 1)?.reply()?;
    Ok(Keymap { min: s.min_keycode, per: (r.keysyms_per_keycode as usize).max(1), syms: r.keysyms })
}

impl Keymap {
    /// Keycodes producing `sym`, preferring unshifted (column 0) matches.
    fn keycodes(&self, sym: u32) -> Vec<Keycode> {
        let find = |cols: usize| -> Vec<Keycode> {
            self.syms
                .chunks(self.per)
                .enumerate()
                .filter(|(_, s)| s[..cols.min(s.len())].contains(&sym))
                .map(|(i, _)| self.min + i as u8)
                .collect()
        };
        let primary = find(1);
        if primary.is_empty() { find(self.per) } else { primary }
    }
}

fn grab(conn: &RustConnection, root: Window, keymap: &Keymap, locks: &[u16], hk: Hotkey) -> Result<Vec<Keycode>> {
    let keycodes = keymap.keycodes(hk.key.keysym());
    if keycodes.is_empty() {
        bail!("key {} is not on the keyboard map", hk.key);
    }
    let mask = hk.mods.x_mask();
    let mut cookies = Vec::new();
    for &kc in &keycodes {
        for &l in locks {
            cookies.push((kc, l, conn.grab_key(false, root, ModMask::from(mask | l), kc, GrabMode::ASYNC, GrabMode::ASYNC)?));
        }
    }
    let mut failure = None;
    for (_, _, c) in cookies {
        if let Err(e) = c.check()
            && failure.is_none()
        {
            failure = Some(e);
        }
    }
    if let Some(e) = failure {
        // Roll back the partial grab so we don't hold half a binding.
        for &kc in &keycodes {
            for &l in locks {
                let _ = conn.ungrab_key(kc, root, ModMask::from(mask | l));
            }
        }
        let _ = conn.flush();
        return Err(match e {
            ReplyError::X11Error(x) if x.error_kind == ErrorKind::Access => {
                anyhow!("{hk} is already grabbed by another program (BadAccess)")
            }
            other => anyhow!("grabbing {hk} failed: {other}"),
        });
    }
    Ok(keycodes)
}

fn event_loop(conn: &RustConnection, grabbed: Vec<Grabbed>, wake_atom: u32, detectable: bool, on_press: impl Fn(u32)) {
    let relevant = u16::from(ModMask::SHIFT | ModMask::CONTROL | ModMask::M1 | ModMask::M4);
    let mut down: HashSet<Keycode> = HashSet::new();
    let mut pending: Option<Event> = None;
    loop {
        let ev = match pending.take().map(Ok).unwrap_or_else(|| conn.wait_for_event()) {
            Ok(ev) => ev,
            Err(e) => {
                log::warn!("hotkey connection closed: {e}");
                return;
            }
        };
        match ev {
            Event::ClientMessage(m) if m.type_ == wake_atom => return,
            Event::KeyPress(k) => {
                if !down.insert(k.detail) {
                    continue; // detectable autorepeat
                }
                let state = u16::from(k.state) & relevant;
                if let Some(g) = grabbed.iter().find(|g| g.mask == state && g.keycodes.contains(&k.detail)) {
                    on_press(g.id);
                }
            }
            Event::KeyRelease(k) => {
                if !detectable {
                    // Classic autorepeat sends Release+Press with the same timestamp.
                    let _ = conn.flush();
                    if let Ok(Some(next)) = conn.poll_for_event() {
                        if let Event::KeyPress(p) = &next
                            && p.detail == k.detail
                            && p.time == k.time
                        {
                            continue; // swallow the repeat pair, key stays down
                        }
                        pending = Some(next);
                    }
                }
                down.remove(&k.detail);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hotkeys() {
        let h: Hotkey = "Print".parse().unwrap();
        assert_eq!(h, Hotkey { key: Key::Print, mods: Mods::NONE });
        let h: Hotkey = "Shift+Print".parse().unwrap();
        assert_eq!(h.mods, Mods::SHIFT);
        let h: Hotkey = "ctrl + alt + shift + prtsc".parse().unwrap();
        assert_eq!(h, Hotkey { key: Key::Print, mods: Mods::CTRL | Mods::ALT | Mods::SHIFT });
        let h: Hotkey = "Super+Print".parse().unwrap();
        assert_eq!(h.mods, Mods::SUPER);
        assert_eq!("Ctrl+A".parse::<Hotkey>().unwrap().key, Key::Letter('a'));
        assert_eq!("Alt+7".parse::<Hotkey>().unwrap().key, Key::Digit(7));
        assert_eq!("Ctrl+Alt+Shift+F9".parse::<Hotkey>().unwrap().key, Key::F(9));
        assert_eq!("F12".parse::<Hotkey>().unwrap().key, Key::F(12));
        for bad in ["", "Ctrl+", "Hyper+Print", "F0", "F25", "Ctrl+Shift", "ab"] {
            assert!(bad.parse::<Hotkey>().is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn display_roundtrip() {
        for s in ["Print", "Ctrl+Shift+Alt+Super+Print", "Ctrl+A", "Alt+F4", "Shift+5"] {
            let h: Hotkey = s.parse().unwrap();
            assert_eq!(h.to_string(), s);
            assert_eq!(h.to_string().parse::<Hotkey>().unwrap(), h);
        }
    }

    #[test]
    fn keysyms() {
        assert_eq!(Key::F(1).keysym(), 0xffbe);
        assert_eq!(Key::F(12).keysym(), 0xffc9);
        assert_eq!(Key::Letter('q').keysym(), 'q' as u32);
        assert_eq!(Key::Digit(0).keysym(), 0x30);
        assert_eq!(Mods::CTRL.x_mask() | Mods::SUPER.x_mask(), 4 | 64);
    }
}
