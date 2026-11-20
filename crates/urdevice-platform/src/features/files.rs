//! File transfer (#17): desktop OS adapters.
//!
//! The download folder (R8.6): `Downloads/UrDevice` by default, changeable
//! in Settings and stored in the app data directory. The same code serves
//! Windows, macOS, and Linux; only the default folder comes from the OS
//! (`directories::UserDirs`).

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use urdevice_core::services::files::FilesHost;

const SETTING_FILE: &str = "download-folder.txt";

/// The default download folder: `<Downloads>/UrDevice`.
pub fn default_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|u| u.download_dir().map(Path::to_path_buf))
        .or_else(|| directories::UserDirs::new().map(|u| u.home_dir().join("Downloads")))
        .unwrap_or_else(|| std::env::temp_dir().join("Downloads"))
        .join("UrDevice")
}

/// The desktop's [`FilesHost`]: the download folder, kept in a small
/// setting file next to the trust store.
#[derive(Debug)]
pub struct DesktopFiles {
    data_dir: PathBuf,
    dir: RwLock<PathBuf>,
}

impl DesktopFiles {
    /// Load the setting from `data_dir`.
    pub fn new(data_dir: &Path) -> Self {
        let saved = std::fs::read_to_string(data_dir.join(SETTING_FILE))
            .ok()
            .map(|s| PathBuf::from(s.trim_end_matches(['\r', '\n'])))
            .filter(|p| p.is_absolute());
        Self {
            data_dir: data_dir.to_path_buf(),
            dir: RwLock::new(saved.unwrap_or_else(default_dir)),
        }
    }

    /// The current download folder.
    pub fn dir(&self) -> PathBuf {
        match self.dir.read() {
            Ok(d) => d.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    /// Change the download folder. Only an absolute path to an existing
    /// folder (not a link) is taken.
    pub fn set_dir(&self, dir: &Path) -> std::io::Result<()> {
        let meta = std::fs::symlink_metadata(dir)?;
        if !dir.is_absolute() || !meta.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "not a folder",
            ));
        }
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::write(
            self.data_dir.join(SETTING_FILE),
            dir.as_os_str().to_string_lossy().as_bytes(),
        )?;
        match self.dir.write() {
            Ok(mut d) => *d = dir.to_path_buf(),
            Err(p) => *p.into_inner() = dir.to_path_buf(),
        }
        Ok(())
    }
}

impl FilesHost for DesktopFiles {
    fn incoming_dir(&self) -> PathBuf {
        self.dir()
    }
}

/// Open a folder in the system file manager.
pub fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let cmd = "xdg-open";
    if let Err(e) = std::process::Command::new(cmd).arg(dir).spawn() {
        tracing::warn!(error = %e, "could not open the folder");
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn the_folder_setting_is_kept() {
        let d = std::env::temp_dir().join(format!("urdevice-dl-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = DesktopFiles::new(&d);
        assert!(f.dir().ends_with("UrDevice"));
        assert!(f.set_dir(Path::new("relative")).is_err());
        f.set_dir(&d).unwrap();
        assert_eq!(DesktopFiles::new(&d).incoming_dir(), d);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
