use std::{fmt::Display, str::Utf8Error};
use tokio::time::Instant;

#[derive(Debug, PartialEq)]
pub enum AuthenticationError {
    ParseError(String),
    IncorrectCredentials,
    TokenExpired(Instant),
    NoMatch(String),
    HttpParseError(String),
    SchemeNotSupported(String),
    Empty,
}

impl Display for AuthenticationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthenticationError::ParseError(e) => {
                write!(f, "error trying to parse credentials: {}", e)
            }
            AuthenticationError::IncorrectCredentials => write!(f, "credentials incorrect"),
            AuthenticationError::TokenExpired(instant) => write!(
                f,
                "token expired {}s ago",
                Instant::now().duration_since(*instant).as_secs()
            ),
            AuthenticationError::NoMatch(token) => write!(f, "token {} is not registered", token),
            AuthenticationError::Empty => write!(f, "no authorization header provided"),
            AuthenticationError::HttpParseError(token) => {
                write!(f, "cannot parse authorization header: {}", token)
            }
            AuthenticationError::SchemeNotSupported(scheme) => {
                write!(f, "{} authentication not supported", scheme)
            }
        }
    }
}

impl std::error::Error for AuthenticationError {}

impl From<serde_json::Error> for AuthenticationError {
    fn from(value: serde_json::Error) -> Self {
        Self::ParseError(value.to_string())
    }
}

impl From<base64::DecodeError> for AuthenticationError {
    fn from(value: base64::DecodeError) -> Self {
        Self::ParseError(value.to_string())
    }
}

impl From<Utf8Error> for AuthenticationError {
    fn from(value: Utf8Error) -> Self {
        Self::ParseError(value.to_string())
    }
}
