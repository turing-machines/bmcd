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
use std::str::Utf8Error;
use thiserror::Error;
use tokio::time::Instant;

#[derive(Error, Debug, PartialEq)]
pub enum AuthenticationError {
    #[error("error trying to parse credentials: {0}")]
    ParseError(String),
    #[error("credentials incorrect")]
    IncorrectCredentials,
    #[error("token expired {:?}s ago",
            Instant::now().duration_since(*.0).as_secs())]
    TokenExpired(Instant),
    #[error("token {0} is not registered")]
    NoMatch(String),
    #[error("cannot parse authorization header: {0}")]
    HttpParseError(String),
    #[error("{0} authentication not supported")]
    SchemeNotSupported(String),
    #[error("no authorization header provided")]
    Empty,
}

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

impl AuthenticationError {
    pub fn into_basic_error(self) -> SchemedAuthError {
        SchemedAuthError(Some(Scheme::Basic), self)
    }

    pub fn into_bearer_error(self) -> SchemedAuthError {
        SchemedAuthError(Some(Scheme::Bearer), self)
    }

    pub fn into_unknown_error(self) -> SchemedAuthError {
        SchemedAuthError(None, self)
    }
}

#[derive(Debug, PartialEq)]
pub enum Scheme {
    Basic,
    Bearer,
}

#[derive(Debug, PartialEq)]
pub struct SchemedAuthError(Option<Scheme>, pub AuthenticationError);

impl SchemedAuthError {
    pub fn challenge(&self, realm: &str) -> String {
        match self.0 {
            Some(Scheme::Basic) => format!("Basic realm={}", realm),
            Some(Scheme::Bearer) => format!(
                r#"Bearer realm="{}" error="invalid_token" error_description="{}""#,
                realm, self.1
            ),
            None => format!(r#"realm="{}""#, realm),
        }
    }
}

impl ToString for SchemedAuthError {
    fn to_string(&self) -> String {
        self.1.to_string()
    }
}
