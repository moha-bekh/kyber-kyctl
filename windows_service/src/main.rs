#[cfg(target_os = "windows")]
fn main() {
    #[path = "windows/mod.rs"]
    mod windows;

    windows::run().unwrap();
}

#[cfg(not(any(target_os = "windows")))]
fn main() {}
