use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use kidd_client::MonitorMode;

fn main() {
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        println!("Exit requested");
        r.store(false, Ordering::SeqCst);
    })
    .expect("Error setting Ctrl-C handler");

    println!("Plug monitor");

    let modes = [
        MonitorMode {
            width: 1920,
            height: 1080,
            vsync: 60,
        },
        MonitorMode {
            width: 3840,
            height: 2160,
            vsync: 60,
        },
        MonitorMode {
            width: 2560,
            height: 1440,
            vsync: 60,
        },
        MonitorMode {
            width: 800,
            height: 600,
            vsync: 60,
        },
    ];

    let device_interface = kidd_client::Driver::open().unwrap();
    device_interface.plug_monitor(&modes, 0).unwrap();

    println!("Monitor plugged");

    while running.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(100));
    }

    println!("Exit sample");
}
