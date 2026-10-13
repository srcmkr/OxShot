//! Desktop notifications (freedesktop, via notify-rust).

use rust_i18n::t;

/// "Color copied" with a swatch of the color as notification image.
pub fn color_picked([r, g, b]: [u8; 3], hex: &str) {
    let body = format!("{hex} · rgb({r}, {g}, {b})");
    let summary = t!("notify.color_copied").to_string();
    std::thread::spawn(move || {
        let size = 64u32;
        let swatch: Vec<u8> = (0..size * size).flat_map(|_| [r, g, b]).collect();
        let mut n = notify_rust::Notification::new();
        n.appname("OxShot").summary(&summary).body(&body).icon("oxshot").timeout(notify_rust::Timeout::Milliseconds(4000));
        if let Ok(img) = notify_rust::Image::from_rgb(size as i32, size as i32, swatch) {
            n.image_data(img);
        }
        if let Err(e) = n.show() {
            log::warn!("notification: {e}");
        }
    });
}

/// "OxShot X is available" (once per version).
pub fn update_available(version: &str) {
    let summary = t!("update.notify_available", version = version).to_string();
    let body = t!("update.notify_body").to_string();
    simple(summary, body);
}

/// First start after an update.
pub fn updated(version: &str) {
    let summary = t!("update.notify_updated", version = version).to_string();
    simple(summary, String::new());
}

fn simple(summary: String, body: String) {
    std::thread::spawn(move || {
        let mut n = notify_rust::Notification::new();
        n.appname("OxShot").summary(&summary).body(&body).icon("oxshot").timeout(notify_rust::Timeout::Milliseconds(8000));
        if let Err(e) = n.show() {
            log::warn!("notification: {e}");
        }
    });
}
