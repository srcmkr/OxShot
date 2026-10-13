//! Tests against the running X server. Ignored by default:
//! `cargo test -p oxshot-x11 -- --ignored`
use std::sync::mpsc;
use std::time::Duration;

use oxshot_x11::{Hotkey, HotkeyService};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
use x11rb::protocol::xtest::ConnectionExt as _;

/// Fakes key events via XTEST: press mods, press key `repeats` times (simulating autorepeat
/// with detectable autorepeat semantics), release all.
fn fake(conn: &impl Connection, codes: &[u8], key: u8, repeats: usize) {
    let root = conn.setup().roots[0].root;
    for &c in codes {
        conn.xtest_fake_input(KEY_PRESS_EVENT, c, 0, root, 0, 0, 0).unwrap();
    }
    for _ in 0..repeats {
        conn.xtest_fake_input(KEY_PRESS_EVENT, key, 0, root, 0, 0, 0).unwrap();
    }
    conn.xtest_fake_input(KEY_RELEASE_EVENT, key, 0, root, 0, 0, 0).unwrap();
    for &c in codes.iter().rev() {
        conn.xtest_fake_input(KEY_RELEASE_EVENT, c, 0, root, 0, 0, 0).unwrap();
    }
    conn.flush().unwrap();
}

fn keycode(conn: &impl Connection, sym: u32) -> u8 {
    let s = conn.setup();
    let m = conn.get_keyboard_mapping(s.min_keycode, s.max_keycode - s.min_keycode + 1).unwrap().reply().unwrap();
    let per = m.keysyms_per_keycode as usize;
    let i = m.keysyms.chunks(per).position(|k| k[0] == sym).expect("keysym on keymap");
    s.min_keycode + i as u8
}

#[test]
#[ignore]
fn hotkey_press_delivered_once() {
    let hk: Hotkey = "Ctrl+Alt+Shift+F9".parse().unwrap();
    let (tx, rx) = mpsc::channel();
    let (svc, errors) = HotkeyService::start(vec![(hk, 42)], move |id| tx.send(id).unwrap()).unwrap();
    assert!(errors.is_empty(), "{errors:?}");

    let (conn, _) = x11rb::connect(None).unwrap();
    let mods = [keycode(&conn, 0xffe3), keycode(&conn, 0xffe9), keycode(&conn, 0xffe1)]; // Control_L Alt_L Shift_L
    let f9 = keycode(&conn, 0xffc6);
    fake(&conn, &mods, f9, 3); // 1 real press + 2 "autorepeat" presses
    assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Ok(42));
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "autorepeat must be ignored");
    fake(&conn, &mods, f9, 1);
    assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Ok(42));
    svc.stop();
}

#[test]
#[ignore]
fn snapshot_sane() {
    let s = oxshot_x11::snapshot().unwrap();
    assert!(s.image.width() > 0 && !s.monitors.is_empty());
    assert!(s.image.pixels().all(|p| p.0[3] == 255));
    let r = oxshot_x11::capture_rect(oxshot_core::IRect::new(-50, -50, 100, 100)).unwrap();
    assert_eq!(r.dimensions(), (50, 50));
}

/// Maps a server-side-decorated window and a fake CSD window (red shadow margin declared via
/// `_GTK_FRAME_EXTENTS`, green content), then checks the reported rects against the pixels.
/// Set `OXSHOT_TEST_OUT=<dir>` to save the crops.
#[test]
#[ignore]
fn window_rects_match_pixels() {
    use x11rb::protocol::xproto::*;
    use x11rb::wrapper::ConnectionExt as _;
    let (conn, screen) = x11rb::connect(None).unwrap();
    let root = conn.setup().roots[screen].root;
    let atom = |n: &str| conn.intern_atom(false, n.as_bytes()).unwrap().reply().unwrap().atom;
    let (blue, red, green) = (0x0000ffu32, 0xff0000u32, 0x00ff00u32);

    let ssd = conn.generate_id().unwrap();
    conn.create_window(0, ssd, root, 300, 300, 400, 300, 0, WindowClass::INPUT_OUTPUT, 0,
        &CreateWindowAux::new().background_pixel(blue)).unwrap();
    conn.change_property8(PropMode::REPLACE, ssd, AtomEnum::WM_NAME, AtomEnum::STRING, b"oxshot-test-ssd").unwrap();

    let csd = conn.generate_id().unwrap();
    let m = 20u16;
    conn.create_window(0, csd, root, 800, 300, 400, 300, 0, WindowClass::INPUT_OUTPUT, 0,
        &CreateWindowAux::new().background_pixel(red)).unwrap();
    conn.change_property8(PropMode::REPLACE, csd, AtomEnum::WM_NAME, AtomEnum::STRING, b"oxshot-test-csd").unwrap();
    let motif = atom("_MOTIF_WM_HINTS");
    conn.change_property32(PropMode::REPLACE, csd, motif, motif, &[2, 0, 0, 0, 0]).unwrap();
    conn.change_property32(PropMode::REPLACE, csd, atom("_GTK_FRAME_EXTENTS"), AtomEnum::CARDINAL, &[m as u32; 4]).unwrap();
    let inner = conn.generate_id().unwrap();
    conn.create_window(0, inner, csd, m as i16, m as i16, 400 - 2 * m, 300 - 2 * m, 0, WindowClass::INPUT_OUTPUT, 0,
        &CreateWindowAux::new().background_pixel(green)).unwrap();
    for (w, x) in [(ssd, 300), (csd, 800)] {
        let hints = x11rb::properties::WmSizeHints {
            position: Some((x11rb::properties::WmSizeHintsSpecification::UserSpecified, x, 300)),
            ..Default::default()
        };
        hints.set_normal_hints(&conn, w).unwrap();
    }
    conn.map_window(inner).unwrap();
    conn.map_window(ssd).unwrap();
    conn.map_window(csd).unwrap();
    conn.flush().unwrap();
    std::thread::sleep(Duration::from_millis(700)); // WM framing + compositor

    let s = oxshot_x11::snapshot().unwrap();
    conn.destroy_window(ssd).unwrap();
    conn.destroy_window(csd).unwrap();
    conn.flush().unwrap();

    let find = |id| s.windows.iter().find(|w| w.id == id).unwrap_or_else(|| panic!("window {id:#x} not listed"));
    let (ws, wc) = (find(ssd), find(csd));
    println!("ssd {:?}\ncsd {:?}", ws, wc);
    let crop = |r: oxshot_core::IRect| image::imageops::crop_imm(&s.image, r.x as u32, r.y as u32, r.w, r.h).to_image();
    let (cs, cc) = (crop(ws.rect), crop(wc.rect));
    if let Ok(dir) = std::env::var("OXSHOT_TEST_OUT") {
        cs.save(format!("{dir}/test_ssd.png")).unwrap();
        cc.save(format!("{dir}/test_csd.png")).unwrap();
    }
    // SSD: bigger than the client (title bar), title bar is not blue, bottom-centre is blue.
    assert!(ws.rect.w >= 400 && ws.rect.h > 300, "{:?}", ws.rect);
    assert_ne!(cs.get_pixel(ws.rect.w / 2, 2).0[..3], [0, 0, 255]);
    assert_eq!(cs.get_pixel(ws.rect.w / 2, ws.rect.h - 10).0[..3], [0, 0, 255]);
    // CSD: exactly the green content, no red margin; inner child reported? (equal to rect -> skipped)
    assert_eq!((wc.rect.w, wc.rect.h), (360, 260), "{:?}", wc.rect);
    for (x, y) in [(0, 0), (359, 0), (0, 259), (359, 259), (180, 130)] {
        assert_eq!(cc.get_pixel(x, y).0[..3], [0, 255, 0], "pixel ({x},{y})");
    }
}
