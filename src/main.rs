#![windows_subsystem = "windows"]

//! GatiMeter: network speed and system load on the Windows taskbar.
//! Uses only user-mode Windows APIs; there is no kernel driver.

mod app;
mod floating;
mod format;
mod histwin;
mod history;
mod menu;
mod net;
mod pdh;
mod render;
mod sampler;
mod settings;
mod sys;
mod taskbar;
mod temps;
mod tray;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--probe") {
        probe();
        return;
    }
    let Some(_lock) = sys::single_instance() else {
        sys::message_box(
            std::ptr::null_mut(),
            "GatiMeter is already running.\n\nRight-click its readout on the taskbar, \
             or its icon in the notification area, for the menu.",
            "GatiMeter",
        );
        return;
    };
    app::run();
}

/// `GatiMeter.exe --probe > probe.txt` prints three readings, for bug reports.
fn probe() {
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(u32::MAX);
    }
    for (i, s) in sampler::probe(3, std::time::Duration::from_secs(1)).iter().enumerate() {
        println!("--- reading {}", i + 1);
        println!("{s:#?}");
    }
}
