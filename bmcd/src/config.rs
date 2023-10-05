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
