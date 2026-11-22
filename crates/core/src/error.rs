use std::fmt;

/// Pure-logic failure. The store and server map `Validation` and `Quality` to 400 and `Conflict` to 409.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Validation(String),
    Conflict(String),
    /// An agent save that is too thin to keep; the message is one line the agent can act on (HTTP 400 `quality`).
    Quality(String),
}

impl Error {
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(message) => write!(f, "validation error: {message}"),
            Self::Conflict(message) => write!(f, "conflict: {message}"),
            Self::Quality(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for Error {}
