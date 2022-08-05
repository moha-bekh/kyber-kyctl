use std::path::PathBuf;

pub fn get_resource_path(resource_name: &str) -> PathBuf {
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
pub fn get_executable_path(exe_name: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    let path = get_resource_path(exe_name);

    #[cfg(not(any(target_os = "windows")))]
    let path = PathBuf::from(exe_name);

    path
}
