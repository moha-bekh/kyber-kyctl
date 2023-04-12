#[cfg(target_os = "windows")]
mod windows;

fn main() {
    #[cfg(target_os = "windows")]
    windows::run().unwrap();
}
