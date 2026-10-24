use serde::{Serialize, Serializer};

/// Every fallible core operation returns this. The message is what the user
/// sees (it crosses the bridge as a plain string), so it must never contain a
/// secret: no API keys, tokens or request bodies.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Msg(String),
    #[error("Access denied: {0} is outside any open project")]
    OutsideProject(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{message}")]
    Http { status: u16, message: String },
}

impl Error {
    pub fn msg(m: impl Into<String>) -> Self {
        Error::Msg(m.into())
    }

    /// HTTP status when the error came from BlazeAPI, else None.
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Http { status, .. } => Some(*status),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        // reqwest's Display can include the request URL; ours never carry
        // credentials in the URL, but keep the message generic anyway.
        if e.is_timeout() {
            Error::msg("The request timed out. Check your connection and try again.")
        } else if e.is_connect() {
            Error::msg("Cannot reach BlazeAPI. Check your connection and try again.")
        } else {
            Error::msg(format!("Network error: {}", e.without_url()))
        }
    }
}

impl Serialize for Error {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
