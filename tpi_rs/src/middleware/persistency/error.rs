use std::{error::Error, fmt::Display};

#[derive(Debug)]
pub enum PersistencyError {
    UnknownFormat,
    UnsupportedVersion(u32),
    SerializationError(bincode::Error),
    IoError(std::io::Error),
    UnknownKey(String),
}

impl Error for PersistencyError {}

impl Display for PersistencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PersistencyError::UnknownFormat => {
                write!(f, "not a {} persistency file", env!("CARGO_PKG_NAME"))
            }
            PersistencyError::UnsupportedVersion(version) => {
                write!(f, "version {} not supported", version)
            }
            PersistencyError::SerializationError(e) => f.write_str(&e.to_string()),
            PersistencyError::IoError(e) => f.write_str(&e.to_string()),
            PersistencyError::UnknownKey(key) => {
                write!(f, "{} is not registered in persistency storage", key)
            }
        }
    }
}

impl From<bincode::Error> for PersistencyError {
    fn from(value: bincode::Error) -> Self {
        Self::SerializationError(value)
    }
}

impl From<std::io::Error> for PersistencyError {
    fn from(value: std::io::Error) -> Self {
        Self::IoError(value)
    }
}
