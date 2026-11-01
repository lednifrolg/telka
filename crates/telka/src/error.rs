//! Errors and their exit codes.

use std::path::PathBuf;
use std::process::ExitCode;

/// Something stopped a command. Every variant maps to a documented exit code.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Usage(String),
    #[error("not paired with a TV yet; run `telka pair`")]
    NotPaired,
    #[error("the TV no longer accepts telka's key; run `telka pair` to pair again")]
    KeyRejected,
    #[error(
        "another TV answers at {address} ({found}, not the paired {expected}); run `telka pair` \
         to pair with it"
    )]
    WrongTv {
        address: String,
        expected: String,
        found: String,
    },
    #[error(
        "the device at {address} presented an unknown certificate, so it may not be an LG TV \
         ({found})"
    )]
    UnknownCertificate { address: String, found: String },
    /// The TV is off, asleep or not at its address.
    #[error("{0}")]
    Unreachable(String),
    #[error(transparent)]
    Tv(webos::Error),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is unreadable ({message}); fix or delete it")]
    Corrupt { path: PathBuf, message: String },
}

impl AppError {
    /// 2 usage, 4 pairing needed, 1 anything else.
    pub fn exit_code(&self) -> ExitCode {
        ExitCode::from(match self {
            Self::Usage(_) => 2,
            Self::NotPaired
            | Self::KeyRejected
            | Self::WrongTv { .. }
            | Self::UnknownCertificate { .. } => 4,
            Self::Unreachable(_) | Self::Tv(_) | Self::Io { .. } | Self::Corrupt { .. } => 1,
        })
    }

    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

impl From<webos::Error> for AppError {
    fn from(error: webos::Error) -> Self {
        match error {
            webos::Error::KeyRejected => Self::KeyRejected,
            // A permission the pairing does not grant: nothing to retry, so a usage error.
            webos::Error::Denied { message } => Self::Usage(format!(
                "the TV does not allow this ({message}); pairing with LG's signed manifest \
                 (`telka unpair`, then `telka pair`) may"
            )),
            other => Self::Tv(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_follow_the_documented_table() {
        let code = |error: AppError| format!("{:?}", error.exit_code());
        assert_eq!(
            code(AppError::Usage("x".into())),
            format!("{:?}", ExitCode::from(2))
        );
        assert_eq!(
            code(AppError::NotPaired),
            format!("{:?}", ExitCode::from(4))
        );
        assert_eq!(
            code(webos::Error::KeyRejected.into()),
            format!("{:?}", ExitCode::from(4))
        );
        assert_eq!(
            code(AppError::Unreachable("x".into())),
            format!("{:?}", ExitCode::from(1))
        );
        assert_eq!(
            code(
                webos::Error::Denied {
                    message: "401".into()
                }
                .into()
            ),
            format!("{:?}", ExitCode::from(2))
        );
    }
}
