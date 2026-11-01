//! Where telka keeps its files.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::AppError;

/// `<value>/telka` when the XDG variable holds an absolute path, else
/// `<home>/<fallback>/telka`.
fn xdg(value: Option<OsString>, home: &Path, fallback: &str) -> PathBuf {
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
    /// The XDG locations (`~/.config/telka`, `~/.local/state/telka`), or both under
    /// `TELKA_HOME` when it is set.
    pub fn discover() -> Result<Self, AppError> {
        if let Some(home) = env::var_os("TELKA_HOME") {
            return Ok(Self::under(Path::new(&home)));
        }
        let home = env::home_dir()
            .filter(|home| home.is_absolute())
            .ok_or_else(|| AppError::Usage("cannot find the home directory".into()))?;
        Ok(Self {
            config: xdg(env::var_os("XDG_CONFIG_HOME"), &home, ".config"),
            state: xdg(env::var_os("XDG_STATE_HOME"), &home, ".local/state"),
        })
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
    fn xdg_variables_must_be_absolute() {
        let home = Path::new("/home/user");
        assert_eq!(
            xdg(Some("/tmp/state".into()), home, ".local/state"),
            Path::new("/tmp/state/telka")
        );
        assert_eq!(
            xdg(Some("relative".into()), home, ".local/state"),
            Path::new("/home/user/.local/state/telka")
        );
        assert_eq!(
            xdg(None, home, ".local/state"),
            Path::new("/home/user/.local/state/telka")
        );
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
