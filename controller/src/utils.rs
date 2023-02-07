use std::path::PathBuf;

pub(crate) fn get_resource_path(resource_name: &str) -> PathBuf {
    if let Ok(mut path) = std::env::current_exe() {
        path.pop();
        path.push(resource_name);

        path
    } else {
        PathBuf::from(resource_name)
    }
}

// Windows requires a fullpath.
// Unix platforms must have $PATH correctly configured.
#[cfg(target_os = "windows")]
pub fn get_executable_path(exe_name: &str) -> PathBuf {
    if let Ok(mut path) = std::env::current_exe() {
        path.pop();
        path.push(exe_name);

        path
    } else {
        PathBuf::from(exe_name)
    }
}

#[cfg(not(any(target_os = "windows")))]
pub fn get_executable_path(exe_name: &str) -> PathBuf {
    PathBuf::from(exe_name)
}
