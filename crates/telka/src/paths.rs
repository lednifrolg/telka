//! Where telka keeps its files.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::AppError;

/// `<value>/telka` when the variable holds an absolute path, else `<home>/<fallback>/telka`.
fn base(value: Option<OsString>, home: &Path, fallback: &str) -> PathBuf {
    value
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(fallback))
        .join("telka")
}

/// Config and state directories.
#[derive(Debug, Clone)]
pub struct Paths {
    config: PathBuf,
    state: PathBuf,
}

impl Paths {
    /// The usual locations of the system, or both under `TELKA_HOME` when it is set.
    pub fn discover() -> Result<Self, AppError> {
        if let Some(home) = env::var_os("TELKA_HOME") {
            return Ok(Self::under(Path::new(&home)));
        }
        let home = env::home_dir()
            .filter(|home| home.is_absolute())
            .ok_or_else(|| AppError::Usage("cannot find the home directory".into()))?;
        Ok(Self::of_system(&home))
    }

    /// XDG locations: `~/.config/telka` and `~/.local/state/telka`.
    #[cfg(not(windows))]
    fn of_system(home: &Path) -> Self {
        Self {
            config: base(env::var_os("XDG_CONFIG_HOME"), home, ".config"),
            state: base(env::var_os("XDG_STATE_HOME"), home, ".local/state"),
        }
    }

    /// The config in `%APPDATA%\telka`, which may roam with the user; the state, which belongs
    /// to this computer, in `%LOCALAPPDATA%\telka\state`.
    #[cfg(windows)]
    fn of_system(home: &Path) -> Self {
        Self {
            config: base(env::var_os("APPDATA"), home, r"AppData\Roaming"),
            state: base(env::var_os("LOCALAPPDATA"), home, r"AppData\Local").join("state"),
        }
    }

    pub fn under(home: &Path) -> Self {
        Self {
            config: home.join("config"),
            state: home.join("state"),
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config.join("config.toml")
    }

    /// The paired TV with its client key.
    pub fn tv(&self) -> PathBuf {
        self.state.join("tv.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_win_only_with_absolute_paths() {
        let root = Path::new(if cfg!(windows) { r"C:\" } else { "/" });
        let home = root.join("home");
        assert_eq!(
            base(Some(root.join("state").into()), &home, "fallback"),
            root.join("state/telka")
        );
        assert_eq!(
            base(Some("relative".into()), &home, "fallback"),
            home.join("fallback/telka")
        );
        assert_eq!(base(None, &home, "fallback"), home.join("fallback/telka"));
        assert_eq!(
            Paths::under(Path::new("/x")).tv(),
            Path::new("/x/state/tv.json")
        );
        assert_eq!(
            Paths::under(Path::new("/x")).config_file(),
            Path::new("/x/config/config.toml")
        );
    }
}
