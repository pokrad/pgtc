//! Position of pgtc's own console window on Windows (D-069).
//!
//! Only when pgtc got a console window of its own (started from Explorer, a shortcut,
//! Win+R) and only in the classic console (conhost): the placement is restored at
//! startup and saved on exit and when the window's close button is used. A terminal
//! the user opened is never moved. Nothing happens on other platforms.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The stored window placement: the normal (restored) rectangle in screen
/// coordinates and whether the window is maximized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Placement {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    maximized: bool,
}

impl Placement {
    /// A rectangle with a size (a broken file must not shrink the window to nothing).
    fn is_usable(&self) -> bool {
        self.right > self.left && self.bottom > self.top
    }
}

/// The placement in `window.toml`; `None` when the file is missing or unreadable.
fn load(file: &Path) -> Option<Placement> {
    let text = std::fs::read_to_string(file).ok()?;
    match toml::from_str::<Placement>(&text) {
        Ok(p) if p.is_usable() => Some(p),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!(file = %file.display(), "window placement ignored: {e}");
            None
        }
    }
}

fn store(file: &Path, placement: &Placement) {
    let result = toml::to_string_pretty(placement)
        .map_err(std::io::Error::other)
        .and_then(|text| {
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(file, text)
        });
    match result {
        Ok(()) => tracing::debug!(?placement, "window placement saved"),
        Err(e) => tracing::warn!(file = %file.display(), "cannot save window placement: {e}"),
    }
}

#[cfg(windows)]
pub use sys::{save, start};

/// Other platforms: a terminal program has no window of its own (D-069).
#[cfg(not(windows))]
pub fn start(_paths: &pgc_core::paths::Paths) {}

#[cfg(not(windows))]
pub fn save() {}

#[cfg(windows)]
mod sys {
    use std::path::PathBuf;
    use std::sync::OnceLock;

    use pgc_core::paths::Paths;
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromRect};
    use windows_sys::Win32::System::Console::{
        CTRL_CLOSE_EVENT, GetConsoleProcessList, GetConsoleWindow, SetConsoleCtrlHandler,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowPlacement, SW_SHOWMAXIMIZED, SW_SHOWMINIMIZED, SW_SHOWNORMAL, SetWindowPlacement,
        WINDOWPLACEMENT, WPF_RESTORETOMAXIMIZED,
    };
    use windows_sys::core::BOOL;

    use super::Placement;

    /// `window.toml`, set by [`start`] only when the window is pgtc's own.
    static WINDOW_FILE: OnceLock<PathBuf> = OnceLock::new();

    /// The console window when pgtc owns it: pgtc is the only process on the console
    /// and the console is not hosted by Windows Terminal.
    fn own_window() -> Option<HWND> {
        if std::env::var_os("WT_SESSION").is_some() {
            return None;
        }
        let mut pids = [0u32; 4];
        // SAFETY: the buffer holds `pids.len()` process ids.
        let count = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) };
        if count != 1 {
            return None;
        }
        // SAFETY: no arguments; a null handle means no console window.
        let hwnd = unsafe { GetConsoleWindow() };
        (!hwnd.is_null()).then_some(hwnd)
    }

    fn current(hwnd: HWND) -> Option<Placement> {
        // SAFETY: WINDOWPLACEMENT is plain data; zero is a valid start value.
        let mut wp: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
        wp.length = size_of::<WINDOWPLACEMENT>() as u32;
        // SAFETY: `wp` is a valid WINDOWPLACEMENT with its length set.
        if unsafe { GetWindowPlacement(hwnd, &mut wp) } == 0 {
            return None;
        }
        let show = wp.showCmd as i32;
        // a minimized window is saved as it will be restored
        let maximized = show == SW_SHOWMAXIMIZED
            || (show == SW_SHOWMINIMIZED && wp.flags & WPF_RESTORETOMAXIMIZED != 0);
        let r = wp.rcNormalPosition;
        Some(Placement {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
            maximized,
        })
    }

    fn apply(hwnd: HWND, p: &Placement) {
        let rect = RECT {
            left: p.left,
            top: p.top,
            right: p.right,
            bottom: p.bottom,
        };
        // a monitor was unplugged or the resolution changed: Windows' default place
        // SAFETY: `rect` is a valid RECT.
        if unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONULL) }.is_null() {
            tracing::info!(?p, "window placement is on no monitor, not restored");
            return;
        }
        // SAFETY: WINDOWPLACEMENT is plain data; zero is a valid start value.
        let mut wp: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
        wp.length = size_of::<WINDOWPLACEMENT>() as u32;
        wp.showCmd = if p.maximized {
            SW_SHOWMAXIMIZED
        } else {
            SW_SHOWNORMAL
        } as u32;
        wp.rcNormalPosition = rect;
        // SAFETY: `wp` is a valid WINDOWPLACEMENT with its length set.
        if unsafe { SetWindowPlacement(hwnd, &wp) } == 0 {
            tracing::warn!(?p, "cannot restore the window placement");
        } else {
            tracing::debug!(?p, "window placement restored");
        }
    }

    /// The close button: save while the window still exists (the process ends after
    /// the handler returns).
    unsafe extern "system" fn on_console_event(event: u32) -> BOOL {
        if event == CTRL_CLOSE_EVENT {
            save();
        }
        0 // not handled: the default handler runs
    }

    /// At startup, before the TUI: restore the saved placement and watch for the close
    /// button — only for pgtc's own console window.
    pub fn start(paths: &Paths) {
        let Some(hwnd) = own_window() else {
            tracing::debug!("console window not owned by pgtc, placement not managed");
            return;
        };
        let file = paths.window_file();
        if let Some(p) = super::load(&file) {
            apply(hwnd, &p);
        }
        let _ = WINDOW_FILE.set(file);
        // SAFETY: `on_console_event` is a valid handler routine for the whole run.
        if unsafe { SetConsoleCtrlHandler(Some(on_console_event), 1) } == 0 {
            tracing::warn!("cannot watch the console close button");
        }
    }

    /// Save the current placement (on exit and on the close button).
    pub fn save() {
        let Some(file) = WINDOW_FILE.get() else {
            return;
        };
        // SAFETY: no arguments; a null handle means no console window.
        let hwnd = unsafe { GetConsoleWindow() };
        if hwnd.is_null() {
            return;
        }
        if let Some(p) = current(hwnd) {
            super::store(file, &p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("window.toml");
        let p = Placement {
            left: -1200,
            top: 40,
            right: -100,
            bottom: 900,
            maximized: true,
        };
        store(&file, &p);
        assert_eq!(load(&file), Some(p));
    }

    #[test]
    fn missing_or_broken_file_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("window.toml");
        assert_eq!(load(&file), None);
        std::fs::write(&file, "left = 1\n").unwrap();
        assert_eq!(load(&file), None);
        std::fs::write(
            &file,
            "left = 10\ntop = 10\nright = 10\nbottom = 500\nmaximized = false\n",
        )
        .unwrap();
        assert_eq!(load(&file), None, "a window without width is not used");
    }
}
