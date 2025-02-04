use std::path::PathBuf;

use crate::host_events;

pub(crate) fn generate_session_id() -> u32 {
    use rand::prelude::*;
    rand::rng().random()
}

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

pub(crate) trait TypeConvert<T> {
    fn convert(&self) -> T;
}

impl<F, T> TypeConvert<Vec<T>> for Vec<F>
where
    F: TypeConvert<T>,
{
    fn convert(&self) -> Vec<T> {
        self.iter().map(|i| i.convert()).collect()
    }
}

impl TypeConvert<host_events::Display> for avservice_types::Display {
    fn convert(&self) -> host_events::Display {
        host_events::Display {
            id: self.id,
            name: self.name.clone(),
            width: self.width,
            height: self.height,
            x: self.x,
            y: self.y,
        }
    }
}

impl TypeConvert<inputservice_types::Display> for avservice_types::Display {
    fn convert(&self) -> inputservice_types::Display {
        inputservice_types::Display {
            id: self.id,
            width: self.width,
            height: self.height,
            x: self.x,
            y: self.y,
        }
    }
}
