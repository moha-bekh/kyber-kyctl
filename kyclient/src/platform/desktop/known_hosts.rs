// Project Kyber: known_hosts.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0-or-later
//
// This file is both under dual license: AGPLv3 and a Commercial one.
//
// ----
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

//! SSH-style known_hosts file for certificate fingerprint storage.
//!
//! Stores host fingerprints in a simple text format:
//! hostname sha256:xx:xx:xx:...

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use log::{error, info, warn};

use crate::{Error, Result};

/// Get the known_hosts file path.
/// - Linux: ~/.local/share/kyber/known_hosts
/// - macOS/iOS: ~/Library/Application Support/kyber/known_hosts
/// - Windows: %LOCALAPPDATA%\kyber\known_hosts
/// - Android: Uses KYBER_CACHEDIR environment variable
///
/// On iOS, `HOME` is set to the app sandbox root by the OS.
/// See: <https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/FileSystemOverview/FileSystemOverview.html>
fn get_known_hosts_path() -> Result<PathBuf> {
    #[cfg(target_os = "android")]
    {
        let cache_dir = std::env::var("KYBER_CACHEDIR").map_err(|_| {
            error!("KYBER_CACHEDIR not set");
            Error::FileNotFound(PathBuf::from("KYBER_CACHEDIR"))
        })?;
        Ok(PathBuf::from(cache_dir).join("known_hosts"))
    }

    #[cfg(target_os = "windows")]
    {
        let local_app_data = std::env::var("LOCALAPPDATA").map_err(|_| {
            error!("LOCALAPPDATA not set");
            Error::FileNotFound(PathBuf::from("LOCALAPPDATA"))
        })?;
        Ok(PathBuf::from(local_app_data)
            .join("kyber")
            .join("known_hosts"))
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        let home = std::env::var("HOME").map_err(|_| {
            error!("HOME not set");
            Error::FileNotFound(PathBuf::from("HOME"))
        })?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("kyber")
            .join("known_hosts"))
    }

    #[cfg(not(any(
        target_os = "android",
        target_os = "ios",
        target_os = "windows",
        target_os = "macos"
    )))]
    {
        let home = std::env::var("HOME").map_err(|_| {
            error!("HOME not set");
            Error::FileNotFound(PathBuf::from("HOME"))
        })?;
        Ok(PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("kyber")
            .join("known_hosts"))
    }
}

/// Thread-safe known hosts storage.
#[derive(Clone, Debug)]
pub struct KnownHosts {
    hosts: Arc<RwLock<HashMap<String, String>>>,
}

impl Default for KnownHosts {
    fn default() -> Self {
        Self::new()
    }
}

impl KnownHosts {
    /// Create a new empty KnownHosts instance.
    pub fn new() -> Self {
        Self {
            hosts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Load known hosts from the default file.
    pub fn load() -> Result<Self> {
        let path = get_known_hosts_path()?;
        Self::load_from_path(&path)
    }

    /// Load known hosts from a specific path.
    pub fn load_from_path(path: &PathBuf) -> Result<Self> {
        let hosts = match File::open(path) {
            Ok(file) => {
                let reader = BufReader::new(file);
                let mut hosts = HashMap::new();

                for (line_num, line) in reader.lines().enumerate() {
                    let line = match line {
                        Ok(l) => l,
                        Err(e) => {
                            error!("Failed to read known_hosts line: {e}");
                            return Err(Error::IO(e));
                        }
                    };

                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }

                    let parts: Vec<&str> = line.splitn(2, ' ').collect();
                    if parts.len() != 2 {
                        warn!("Invalid known_hosts line {}: {}", line_num + 1, line);
                        continue;
                    }

                    hosts.insert(parts[0].to_string(), parts[1].to_string());
                }

                info!("Loaded {} known hosts from {:?}", hosts.len(), path);
                hosts
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                info!("No known_hosts file at {:?}", path);
                HashMap::new()
            }
            Err(e) => {
                error!("Failed to open known_hosts: {e}");
                return Err(Error::IO(e));
            }
        };

        Ok(Self {
            hosts: Arc::new(RwLock::new(hosts)),
        })
    }

    /// Save known hosts to the default file.
    pub fn save(&self) -> Result<()> {
        let path = get_known_hosts_path()?;
        self.save_to_path(&path)
    }

    /// Save known hosts to a specific path.
    ///
    /// Uses atomic write (temp file + rename) to prevent data loss on crash.
    pub fn save_to_path(&self, path: &PathBuf) -> Result<()> {
        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                error!("Failed to create known_hosts directory: {e}");
                Error::IO(e)
            })?;
        }

        let hosts = self.hosts.read().map_err(|_| Error::ThreadPanicked)?;

        // Write to a temporary file first for atomic operation
        let temp_path = path.with_extension("tmp");
        let mut file = File::create(&temp_path).map_err(|e| {
            error!("Failed to create temp known_hosts file: {e}");
            Error::IO(e)
        })?;

        for (hostname, fingerprint) in hosts.iter() {
            writeln!(file, "{} {}", hostname, fingerprint).map_err(|e| {
                error!("Failed to write known_hosts: {e}");
                Error::IO(e)
            })?;
        }

        // Ensure data is flushed to disk before rename
        file.sync_all().map_err(|e| {
            error!("Failed to sync known_hosts file: {e}");
            Error::IO(e)
        })?;

        // Atomic rename
        if let Err(e) = fs::rename(&temp_path, path) {
            error!("Failed to rename known_hosts file: {e}");
            let _ = fs::remove_file(&temp_path);
            return Err(Error::IO(e));
        }

        info!("Saved {} known hosts to {:?}", hosts.len(), path);
        Ok(())
    }

    /// Get the fingerprint for a hostname.
    pub fn get(&self, hostname: &str) -> Option<String> {
        let hosts = self.hosts.read().ok()?;
        hosts.get(hostname).cloned()
    }

    /// Add or update a host's fingerprint.
    pub fn insert(&self, hostname: &str, fingerprint: &str) -> Result<()> {
        let mut hosts = self.hosts.write().map_err(|_| Error::ThreadPanicked)?;
        hosts.insert(hostname.to_string(), fingerprint.to_string());
        Ok(())
    }

    /// Remove a host from known hosts.
    pub fn remove(&self, hostname: &str) -> Result<bool> {
        let mut hosts = self.hosts.write().map_err(|_| Error::ThreadPanicked)?;
        Ok(hosts.remove(hostname).is_some())
    }

    /// Check if a host exists in known hosts.
    pub fn contains(&self, hostname: &str) -> bool {
        self.hosts
            .read()
            .map(|h| h.contains_key(hostname))
            .unwrap_or(false)
    }
}

/// Add a host to the known_hosts file (public API for CLI tools).
pub fn add_host(hostname: &str, fingerprint: &str) -> Result<()> {
    let known_hosts = KnownHosts::load()?;
    known_hosts.insert(hostname, fingerprint)?;
    known_hosts.save()
}

/// Remove a host from the known_hosts file (public API for CLI tools).
pub fn remove_host(hostname: &str) -> Result<bool> {
    let known_hosts = KnownHosts::load()?;
    let removed = known_hosts.remove(hostname)?;
    if removed {
        known_hosts.save()?;
    }
    Ok(removed)
}

/// List all hosts in the known_hosts file (public API for CLI tools).
pub fn list_hosts() -> Result<Vec<(String, String)>> {
    let known_hosts = KnownHosts::load()?;
    let hosts = known_hosts
        .hosts
        .read()
        .map_err(|_| Error::ThreadPanicked)?;
    Ok(hosts.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_new_known_hosts_is_empty() {
        let kh = KnownHosts::new();
        assert!(!kh.contains("example.com"));
        assert!(kh.get("example.com").is_none());
    }

    #[test]
    fn test_insert_and_get() {
        let kh = KnownHosts::new();
        let fingerprint = "sha256:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99";

        kh.insert("example.com", fingerprint).unwrap();

        assert!(kh.contains("example.com"));
        assert_eq!(kh.get("example.com"), Some(fingerprint.to_string()));
        assert!(!kh.contains("other.com"));
    }

    #[test]
    fn test_insert_overwrites() {
        let kh = KnownHosts::new();
        let fp1 = "sha256:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99";
        let fp2 = "sha256:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00";

        kh.insert("example.com", fp1).unwrap();
        kh.insert("example.com", fp2).unwrap();

        assert_eq!(kh.get("example.com"), Some(fp2.to_string()));
    }

    #[test]
    fn test_remove() {
        let kh = KnownHosts::new();
        let fingerprint = "sha256:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99";

        kh.insert("example.com", fingerprint).unwrap();
        assert!(kh.contains("example.com"));

        let removed = kh.remove("example.com").unwrap();
        assert!(removed);
        assert!(!kh.contains("example.com"));

        // Removing non-existent host returns false
        let removed = kh.remove("example.com").unwrap();
        assert!(!removed);
    }

    #[test]
    fn test_save_and_load() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_path_buf();

        let fp1 = "sha256:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99";
        let fp2 = "sha256:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00";

        // Create and save
        let kh = KnownHosts::new();
        kh.insert("host1.example.com", fp1).unwrap();
        kh.insert("host2.example.com", fp2).unwrap();
        kh.save_to_path(&path).unwrap();

        // Load and verify
        let loaded = KnownHosts::load_from_path(&path).unwrap();
        assert_eq!(loaded.get("host1.example.com"), Some(fp1.to_string()));
        assert_eq!(loaded.get("host2.example.com"), Some(fp2.to_string()));
    }

    #[test]
    fn test_load_with_comments_and_empty_lines() {
        let mut temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_path_buf();

        // Write a file with comments and empty lines
        writeln!(temp_file, "# This is a comment").unwrap();
        writeln!(temp_file).unwrap();
        writeln!(temp_file, "example.com sha256:aa:bb:cc:dd").unwrap();
        writeln!(temp_file, "  # Indented comment").unwrap();
        writeln!(temp_file, "other.com sha256:11:22:33:44").unwrap();

        let loaded = KnownHosts::load_from_path(&path).unwrap();
        assert!(loaded.contains("example.com"));
        assert!(loaded.contains("other.com"));
        assert_eq!(
            loaded.get("example.com"),
            Some("sha256:aa:bb:cc:dd".to_string())
        );
    }

    #[test]
    fn test_load_nonexistent_file() {
        let path = PathBuf::from("/nonexistent/path/known_hosts");
        let kh = KnownHosts::load_from_path(&path).unwrap();
        assert!(!kh.contains("any.host"));
    }

    #[test]
    fn test_multiple_hosts() {
        let kh = KnownHosts::new();
        let hosts = vec![
            ("a.example.com", "sha256:aa"),
            ("b.example.com", "sha256:bb"),
            ("c.example.com", "sha256:cc"),
        ];

        for (host, fp) in &hosts {
            kh.insert(host, fp).unwrap();
        }

        for (host, fp) in &hosts {
            assert_eq!(kh.get(host), Some(fp.to_string()));
        }
    }
}
