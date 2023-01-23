use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

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

pub(crate) fn path_to_string(path: &Path) -> Result<String> {
    path.to_path_buf()
        .into_os_string()
        .into_string()
        .map_err(|s| Error::OsStringConversion { s })
}
