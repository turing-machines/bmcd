use serde::Deserialize;
use std::fs::OpenOptions;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub tls: Tls,
}

#[derive(Debug, Deserialize)]
pub struct Tls {
    pub private_key: PathBuf,
    pub certificate: PathBuf,
}

impl TryFrom<PathBuf> for Config {
    type Error = anyhow::Error;

    fn try_from(value: PathBuf) -> Result<Self, Self::Error> {
        let file = OpenOptions::new().read(true).open(value)?;
        Ok(serde_yaml::from_reader(file)?)
    }
}
