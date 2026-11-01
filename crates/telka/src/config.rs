//! `config.toml`: optional settings.

use std::fs;
use std::io;
use std::net::IpAddr;

use serde::Deserialize;

use crate::error::AppError;
use crate::paths::Paths;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The TV's address, when it has a fixed one: telka then never looks for it on the network.
    pub tv: Option<IpAddr>,
}

pub fn load(paths: &Paths) -> Result<Config, AppError> {
    let path = paths.config_file();
    match fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).map_err(|e| AppError::Corrupt {
            path,
            message: e.message().to_owned(),
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(AppError::io(path, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tv_address_is_optional_and_unknown_keys_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        assert_eq!(load(&paths).unwrap().tv, None);
        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(paths.config_file(), "tv = \"192.168.1.30\"\n").unwrap();
        assert_eq!(
            load(&paths).unwrap().tv,
            Some("192.168.1.30".parse().unwrap())
        );
        fs::write(paths.config_file(), "colour = true\n").unwrap();
        assert!(matches!(load(&paths), Err(AppError::Corrupt { .. })));
    }
}
