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
use std::borrow::Cow;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PersistencyError<'a> {
    #[error("not a {} persistency file", env!("CARGO_PKG_NAME"))]
    UnknownFormat,
    #[error("version {0} not supported")]
    UnsupportedVersion(u32),
    #[error("key: {0}, {1}")]
    SerializationError(Cow<'a, str>, bincode::Error),
    #[error("IO Err:")]
    IoError(#[from] std::io::Error),
    #[error("{0} is not registered in persistency storage")]
    UnknownKey(String),
}

impl<'a> PersistencyError<'a> {
    pub fn serialization<C: Into<Cow<'a, str>>>(
        context: C,
        error: bincode::Error,
    ) -> PersistencyError<'a> {
        PersistencyError::SerializationError(context.into(), error)
    }
}
