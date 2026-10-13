//! Registers a few hotkeys, prints conflicts, listens for ~2 s.
//! Usage: cargo run -p oxshot-x11 --example hotkey [HOTKEY...]
use std::time::Duration;

use oxshot_x11::{Hotkey, HotkeyService};

fn main() -> anyhow::Result<()> {
    let mut specs: Vec<String> = std::env::args().skip(1).collect();
    if specs.is_empty() {
        specs = ["Ctrl+Alt+Shift+F9", "Print", "Shift+Print", "Ctrl+Print", "Alt+Print", "Super+Print"]
            .map(String::from)
            .into();
    }
    let bindings = specs
        .iter()
        .enumerate()
        .map(|(i, s)| Ok((s.parse::<Hotkey>()?, i as u32)))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let all: Vec<Hotkey> = bindings.iter().map(|b| b.0).collect();
    let (svc, errors) = HotkeyService::start(bindings, move |id| println!("pressed: {}", all[id as usize]))?;
    for spec in &specs {
        let hk: Hotkey = spec.parse()?;
        match errors.iter().find(|(h, _)| *h == hk) {
            Some((_, e)) => println!("CONFLICT {hk}: {e}"),
            None => println!("ok       {hk}"),
        }
    }
    std::thread::sleep(Duration::from_secs(2));
    svc.stop();
    println!("stopped");
    Ok(())
}
