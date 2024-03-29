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
use super::authentication_errors::AuthenticationError;

pub trait PasswordValidator {
    fn validate(hash: &str, password: &str) -> Result<(), AuthenticationError>;
}

pub struct UnixValidator {}

impl PasswordValidator for UnixValidator {
    fn validate(hash: &str, password: &str) -> Result<(), AuthenticationError> {
        tracing::debug!(
            "computed={}",
            pwhash::unix::crypt(password, hash).unwrap_or_default()
        );
        if !pwhash::unix::verify(password, hash) {
            Err(AuthenticationError::IncorrectCredentials)
        } else {
            Ok(())
        }
    }
}
