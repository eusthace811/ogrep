use std::fmt;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Json(serde_json::Error),
    IndexNotFound(String),
    IndexCorrupted(String),
    Regex(String),
    Server(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Json(e) => write!(f, "JSON error: {e}"),
            Self::IndexNotFound(p) => write!(f, "index not found at {p}"),
            Self::IndexCorrupted(msg) => write!(f, "corrupted index: {msg}"),
            Self::Regex(msg) => write!(f, "regex error: {msg}"),
            Self::Server(msg) => write!(f, "server error: {msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    /// The wrapped I/O and JSON errors stay reachable, so a caller walking the
    /// chain (anyhow's `{:#}`, for one) reports the cause and not just ours.
    #[test]
    fn source_is_the_wrapped_error() {
        let io = Error::from(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing file",
        ));
        assert_eq!(io.source().unwrap().to_string(), "missing file");
        let json = Error::from(serde_json::from_str::<serde_json::Value>("{").unwrap_err());
        assert!(json.source().unwrap().is::<serde_json::Error>());
        for error in [
            Error::IndexNotFound("x".into()),
            Error::IndexCorrupted("x".into()),
            Error::Regex("x".into()),
            Error::Server("x".into()),
        ] {
            assert!(error.source().is_none(), "{error}");
        }
    }
}
