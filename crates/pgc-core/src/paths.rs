//! Per-OS file locations (D-013, D-028). All paths are computed here only.
//!
//! | Platform | Config dir                           | Data dir              |
//! |----------|--------------------------------------|-----------------------|
//! | Linux    | `~/.config/pgtc`                     | `~/.local/share/pgtc` |
//! | macOS    | `~/Library/Application Support/pgtc` | same                  |
//! | Windows  | `%APPDATA%\pgtc`                     | same                  |
//!
//! The base directories come from the environment (no extra crate, D-039):
//! Windows `APPDATA` (else `USERPROFILE\AppData\Roaming`); Linux `XDG_CONFIG_HOME` /
//! `XDG_DATA_HOME` when absolute (else `~/.config` / `~/.local/share`); macOS
//! `~/Library/Application Support`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::{APP_NAME, Error, Result};

const SETTINGS_FILE: &str = "settings.toml";
const KEYMAP_FILE: &str = "keymap.toml";
const TEMPLATES_DIR: &str = "developer_templates";
const THEMES_DIR: &str = "themes";
const LOGS_DIR: &str = "logs";

/// Which rules to apply (the current OS, or another one in tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Os {
    Windows,
    Mac,
    /// Linux and other Unix systems (XDG).
    Unix,
}

const CURRENT_OS: Os = if cfg!(windows) {
    Os::Windows
} else if cfg!(target_os = "macos") {
    Os::Mac
} else {
    Os::Unix
};

type Env<'a> = &'a dyn Fn(&str) -> Option<OsString>;

fn real_env(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

/// An environment variable holding an absolute path (empty / relative values are
/// ignored, as the XDG specification requires).
fn env_path(env: Env<'_>, name: &str) -> Option<PathBuf> {
    env(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

fn home_dir(os: Os, env: Env<'_>) -> Option<PathBuf> {
    match os {
        Os::Windows => env_path(env, "USERPROFILE"),
        Os::Mac | Os::Unix => env_path(env, "HOME"),
    }
}

/// Base config and data directories of the user.
fn base_dirs(os: Os, env: Env<'_>) -> Option<(PathBuf, PathBuf)> {
    match os {
        Os::Windows => {
            let roaming = env_path(env, "APPDATA")
                .or_else(|| Some(home_dir(os, env)?.join("AppData").join("Roaming")))?;
            Some((roaming.clone(), roaming))
        }
        Os::Mac => {
            let support = home_dir(os, env)?
                .join("Library")
                .join("Application Support");
            Some((support.clone(), support))
        }
        Os::Unix => {
            let config = env_path(env, "XDG_CONFIG_HOME")
                .or_else(|| Some(home_dir(os, env)?.join(".config")))?;
            let data = env_path(env, "XDG_DATA_HOME")
                .or_else(|| Some(home_dir(os, env)?.join(".local").join("share")))?;
            Some((config, data))
        }
    }
}

fn pgpass_for(os: Os, env: Env<'_>) -> Option<PathBuf> {
    if let Some(p) = env("PGPASSFILE").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    match os {
        Os::Windows => Some(base_dirs(os, env)?.0.join("postgresql").join("pgpass.conf")),
        Os::Mac | Os::Unix => Some(home_dir(os, env)?.join(".pgpass")),
    }
}

/// User directories of pgtc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    config_dir: PathBuf,
    data_dir: PathBuf,
}

impl Paths {
    /// Paths of the current user, taken from the environment.
    pub fn from_os() -> Result<Self> {
        let (config, data) = base_dirs(CURRENT_OS, &real_env).ok_or(Error::NoHomeDir)?;
        Ok(Self {
            config_dir: config.join(APP_NAME),
            data_dir: data.join(APP_NAME),
        })
    }

    /// Paths rooted at explicit directories (tests, future overrides).
    pub fn with_dirs(config_dir: impl Into<PathBuf>, data_dir: impl Into<PathBuf>) -> Self {
        Self {
            config_dir: config_dir.into(),
            data_dir: data_dir.into(),
        }
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// `<config>/settings.toml`
    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join(SETTINGS_FILE)
    }

    /// `<config>/keymap.toml` (D-016)
    pub fn keymap_file(&self) -> PathBuf {
        self.config_dir.join(KEYMAP_FILE)
    }

    /// `<data>/developer_templates` (D-013)
    pub fn user_templates_dir(&self) -> PathBuf {
        self.data_dir.join(TEMPLATES_DIR)
    }

    /// `<data>/themes` (D-014)
    pub fn user_themes_dir(&self) -> PathBuf {
        self.data_dir.join(THEMES_DIR)
    }

    /// `<data>/logs` (D-029)
    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join(LOGS_DIR)
    }
}

/// PostgreSQL password file (D-008): `PGPASSFILE` if set, otherwise
/// `%APPDATA%\postgresql\pgpass.conf` on Windows and `~/.pgpass` elsewhere (as libpq).
pub fn pgpass_file() -> Option<PathBuf> {
    pgpass_for(CURRENT_OS, &real_env)
}

/// System-wide built-in developer templates directory (D-013). It normally does not
/// exist (D-022); then the embedded templates are used.
pub fn system_templates_dir() -> PathBuf {
    system_data_dir().join(TEMPLATES_DIR)
}

#[cfg(windows)]
fn system_data_dir() -> PathBuf {
    let program_data = std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into());
    PathBuf::from(program_data).join(APP_NAME)
}

#[cfg(target_os = "macos")]
fn system_data_dir() -> PathBuf {
    PathBuf::from("/Library/Application Support").join(APP_NAME)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn system_data_dir() -> PathBuf {
    PathBuf::from("/usr/share").join(APP_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake environment (owned values).
    fn env_of(vars: Vec<(&'static str, String)>) -> impl Fn(&str) -> Option<OsString> {
        move |name| {
            vars.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    /// An absolute path on the OS the tests run on.
    fn abs(rest: &str) -> String {
        if cfg!(windows) {
            format!(r"C:\{rest}")
        } else {
            format!("/{rest}")
        }
    }

    #[test]
    fn layout_is_flat() {
        let p = Paths::with_dirs("cfg", "data");
        assert_eq!(p.settings_file(), Path::new("cfg").join("settings.toml"));
        assert_eq!(p.keymap_file(), Path::new("cfg").join("keymap.toml"));
        assert_eq!(
            p.user_templates_dir(),
            Path::new("data").join("developer_templates")
        );
        assert_eq!(p.user_themes_dir(), Path::new("data").join("themes"));
        assert_eq!(p.logs_dir(), Path::new("data").join("logs"));
    }

    #[test]
    fn unix_xdg_rules() {
        let home = abs("home");
        let env = env_of(vec![("HOME", home.clone())]);
        let (config, data) = base_dirs(Os::Unix, &env).unwrap();
        assert_eq!(config, Path::new(&home).join(".config"));
        assert_eq!(data, Path::new(&home).join(".local").join("share"));
        assert_eq!(
            pgpass_for(Os::Unix, &env).unwrap(),
            Path::new(&home).join(".pgpass")
        );

        let xdg = abs("xdg");
        let env = env_of(vec![
            ("HOME", home.clone()),
            ("XDG_CONFIG_HOME", xdg.clone()),
            ("XDG_DATA_HOME", "relative/is/ignored".to_owned()),
        ]);
        let (config, data) = base_dirs(Os::Unix, &env).unwrap();
        assert_eq!(config, PathBuf::from(&xdg));
        assert_eq!(data, Path::new(&home).join(".local").join("share"));
        assert!(base_dirs(Os::Unix, &env_of(vec![])).is_none(), "no HOME");
    }

    #[test]
    fn windows_and_macos_rules() {
        let appdata = abs("roaming");
        let profile = abs("profile");
        let env = env_of(vec![
            ("APPDATA", appdata.clone()),
            ("USERPROFILE", profile.clone()),
        ]);
        let (config, data) = base_dirs(Os::Windows, &env).unwrap();
        assert_eq!(config, PathBuf::from(&appdata));
        assert_eq!(config, data);
        assert_eq!(
            pgpass_for(Os::Windows, &env).unwrap(),
            Path::new(&appdata).join("postgresql").join("pgpass.conf")
        );
        // without APPDATA: the profile's Roaming folder
        let env = env_of(vec![("USERPROFILE", profile.clone())]);
        let (config, _) = base_dirs(Os::Windows, &env).unwrap();
        assert_eq!(config, Path::new(&profile).join("AppData").join("Roaming"));

        let env = env_of(vec![("HOME", profile.clone())]);
        let (config, data) = base_dirs(Os::Mac, &env).unwrap();
        assert_eq!(
            config,
            Path::new(&profile)
                .join("Library")
                .join("Application Support")
        );
        assert_eq!(config, data);
    }

    #[test]
    fn pgpassfile_wins() {
        let file = abs("pass");
        let env = env_of(vec![("PGPASSFILE", file.clone())]);
        for os in [Os::Windows, Os::Mac, Os::Unix] {
            assert_eq!(pgpass_for(os, &env).unwrap(), PathBuf::from(&file));
        }
    }

    #[test]
    fn os_paths_end_with_app_name() {
        if let Ok(p) = Paths::from_os() {
            assert!(p.config_dir().ends_with(APP_NAME));
            assert!(p.data_dir().ends_with(APP_NAME));
        }
        assert!(system_templates_dir().ends_with(Path::new(APP_NAME).join(TEMPLATES_DIR)));
    }
}
