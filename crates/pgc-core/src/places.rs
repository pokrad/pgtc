//! Places for the file dialogs (D-070): the home folder, then the drives (Windows) or
//! `/` and the mounted volumes (Linux, macOS).

use std::path::{Path, PathBuf};

/// A place: where it leads and a short note shown next to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub path: PathBuf,
    /// What the listing shows for the place (`~`, `C:\`, `/mnt/usb`).
    pub name: String,
    /// A volume label or kind of drive; may be empty.
    pub note: String,
}

impl Place {
    fn new(path: PathBuf, note: &str) -> Self {
        Self {
            name: path.display().to_string(),
            path,
            note: note.to_owned(),
        }
    }
}

/// The places of this computer: home first, then the platform's drives / volumes.
pub fn places() -> Vec<Place> {
    let mut out = Vec::new();
    if let Some(home) = crate::paths::home_dir() {
        out.push(Place {
            name: "~".to_owned(),
            note: home.display().to_string(),
            path: home,
        });
    }
    out.extend(platform_places());
    out
}

/// Where the file dialogs start (D-070): the documents folder, else the home folder,
/// else the current directory.
pub fn start_dir() -> PathBuf {
    documents_dir()
        .filter(|d| d.is_dir())
        .or_else(|| crate::paths::home_dir().filter(|d| d.is_dir()))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default()
}

/// The user's documents folder: the *Documents* known folder (follows a moved or
/// OneDrive Documents).
#[cfg(windows)]
pub fn documents_dir() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath};

    let mut raw: windows_sys::core::PWSTR = std::ptr::null_mut();
    // SAFETY: a valid folder id, no flags, the current user (null token); `raw` gets
    // a string allocated by the shell, freed below in every case.
    let hr =
        unsafe { SHGetKnownFolderPath(&FOLDERID_Documents, 0, std::ptr::null_mut(), &mut raw) };
    let path = if hr >= 0 && !raw.is_null() {
        // SAFETY: on success `raw` is a NUL-terminated UTF-16 string.
        let len = (0..).take_while(|&i| unsafe { *raw.add(i) } != 0).count();
        // SAFETY: `len` characters before the NUL.
        let wide = unsafe { std::slice::from_raw_parts(raw, len) };
        Some(PathBuf::from(std::ffi::OsString::from_wide(wide)))
    } else {
        None
    };
    // SAFETY: allocated by SHGetKnownFolderPath (null is allowed).
    unsafe { CoTaskMemFree(raw as *const _) };
    path
}

/// The user's documents folder: `~/Documents`.
#[cfg(target_os = "macos")]
pub fn documents_dir() -> Option<PathBuf> {
    Some(crate::paths::home_dir()?.join("Documents"))
}

/// The user's documents folder: `XDG_DOCUMENTS_DIR` from `user-dirs.dirs`, else
/// `~/Documents`.
#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn documents_dir() -> Option<PathBuf> {
    let home = crate::paths::home_dir()?;
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    std::fs::read_to_string(config.join("user-dirs.dirs"))
        .ok()
        .and_then(|text| xdg_documents(&text, &home))
        .or_else(|| Some(home.join("Documents")))
}

/// `XDG_DOCUMENTS_DIR="$HOME/…"` (or an absolute path) from a `user-dirs.dirs` file.
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
fn xdg_documents(text: &str, home: &Path) -> Option<PathBuf> {
    let value = text.lines().find_map(|l| {
        let v = l.trim().strip_prefix("XDG_DOCUMENTS_DIR=")?;
        Some(v.trim().trim_matches('"'))
    })?;
    if let Some(rest) = value.strip_prefix("$HOME") {
        Some(home.join(rest.trim_start_matches('/')))
    } else {
        Some(PathBuf::from(value)).filter(|p| p.is_absolute())
    }
}

#[cfg(windows)]
fn platform_places() -> Vec<Place> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    };
    // GetDriveTypeW results (WindowsProgramming)
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOTE: u32 = 4;
    const DRIVE_CDROM: u32 = 5;
    const DRIVE_RAMDISK: u32 = 6;

    // SAFETY: no arguments.
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = char::from(b'A' + i as u8);
        let root = format!("{letter}:\\");
        let wide: Vec<u16> = root.encode_utf16().chain([0]).collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 string.
        let kind = unsafe { GetDriveTypeW(wide.as_ptr()) };
        // only local drives are asked for a label: a disconnected network drive or
        // an empty CD drive could take long to answer
        let note = match kind {
            DRIVE_FIXED | DRIVE_RAMDISK | DRIVE_REMOVABLE => {
                let mut label = [0u16; 261];
                // SAFETY: `wide` is NUL-terminated, `label` holds `label.len()`
                // characters, the other out parameters may be null.
                let ok = unsafe {
                    GetVolumeInformationW(
                        wide.as_ptr(),
                        label.as_mut_ptr(),
                        label.len() as u32,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        0,
                    )
                };
                let end = label.iter().position(|&c| c == 0).unwrap_or(label.len());
                if ok != 0 && end > 0 {
                    String::from_utf16_lossy(&label[..end])
                } else if kind == DRIVE_REMOVABLE {
                    "Removable".to_owned()
                } else {
                    String::new()
                }
            }
            DRIVE_REMOTE => "Network".to_owned(),
            DRIVE_CDROM => "CD/DVD".to_owned(),
            _ => String::new(),
        };
        out.push(Place::new(PathBuf::from(root), &note));
    }
    out
}

#[cfg(target_os = "macos")]
fn platform_places() -> Vec<Place> {
    let mut out = vec![Place::new(PathBuf::from("/"), "")];
    out.extend(sub_folders(Path::new("/Volumes")));
    out
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn platform_places() -> Vec<Place> {
    let mut out = vec![Place::new(PathBuf::from("/"), "")];
    let user = std::env::var_os("USER")
        .filter(|u| !u.is_empty())
        .or_else(|| crate::paths::home_dir()?.file_name().map(Into::into));
    if let Some(user) = user {
        out.extend(sub_folders(&Path::new("/media").join(&user)));
        out.extend(sub_folders(&Path::new("/run/media").join(&user)));
    }
    out.extend(sub_folders(Path::new("/mnt")));
    out
}

/// The folders in `dir`, sorted by name (mount points of volumes).
#[cfg_attr(windows, allow(dead_code))]
fn sub_folders(dir: &Path) -> Vec<Place> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.into_iter().map(|p| Place::new(p, "")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_comes_first_and_roots_follow() {
        let places = places();
        if crate::paths::home_dir().is_some() {
            assert_eq!(places[0].name, "~");
        }
        // every platform has at least one root (a drive, or `/`)
        assert!(places.iter().any(|p| p.path.parent().is_none()));
    }

    #[test]
    fn xdg_documents_folder() {
        let home = Path::new("/home/me");
        let text =
            "# comment\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_DOCUMENTS_DIR=\"$HOME/Dokumente\"\n";
        assert_eq!(xdg_documents(text, home), Some(home.join("Dokumente")));
        // an absolute path (a Unix path is not absolute on Windows)
        if cfg!(unix) {
            let text = "XDG_DOCUMENTS_DIR=\"/data/docs\"\n";
            assert_eq!(xdg_documents(text, home), Some(PathBuf::from("/data/docs")));
        }
        assert_eq!(xdg_documents("XDG_MUSIC_DIR=\"$HOME/Music\"", home), None);
    }

    #[test]
    fn start_dir_exists() {
        assert!(start_dir().is_dir());
    }

    #[test]
    fn sub_folders_are_sorted_folders_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("b")).unwrap();
        std::fs::create_dir(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("file"), "x").unwrap();
        let names: Vec<_> = sub_folders(dir.path())
            .into_iter()
            .map(|p| p.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a", "b"]);
        assert!(sub_folders(&dir.path().join("missing")).is_empty());
    }
}
