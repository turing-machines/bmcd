// Copyright 2023 Turing Machines
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
use serde::Deserialize;
use serde_with::serde_as;
use serde_with::DurationSeconds;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub tls: Tls,
    pub store: Store,
    pub authentication: Authentication,
    pub host: String,
    pub port: u16,
    pub www: PathBuf,
    pub redirect_http: bool,
    pub log: Log,
}

#[serde_as]
#[derive(Debug, Deserialize)]
pub struct Store {
    #[serde_as(as = "Option<DurationSeconds<u64>>")]
    pub write_timeout: Option<Duration>,
}

#[serde_as]
#[derive(Debug, Deserialize)]
pub struct Authentication {
    pub authentication_attempts: usize,
    #[serde_as(as = "DurationSeconds<u64>")]
    pub token_expires: Duration,
}

#[derive(Debug, Deserialize)]
pub struct Tls {
    pub private_key: PathBuf,
    pub certificate: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct Log {
    pub stdout: bool,
    pub directive: String,
}

impl TryFrom<PathBuf> for Config {
    type Error = anyhow::Error;

    fn try_from(value: PathBuf) -> Result<Self, Self::Error> {
        let file = OpenOptions::new().read(true).open(value)?;
        Ok(serde_yaml::from_reader(file)?)
    }
}
