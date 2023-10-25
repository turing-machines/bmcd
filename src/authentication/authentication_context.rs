// Copyright 2023 Turing Machines
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
use super::authentication_errors::AuthenticationError;
use super::passwd_validator::PasswordValidator;
use super::passwd_validator::UnixValidator;
use base64::{engine::general_purpose, Engine as _};
use rand::distributions::Alphanumeric;
use rand::thread_rng;
use rand::Rng;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::marker::PhantomData;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

pub struct AuthenticationContext<P>
where
    P: PasswordValidator + 'static,
{
    token_store: Mutex<HashMap<String, Instant>>,
    passwds: HashMap<String, String>,
    password_validator: PhantomData<P>,
    expire_timeout: Duration,
}

impl<P> AuthenticationContext<P>
where
    P: PasswordValidator + 'static,
{
    pub fn with_unix_validator(
        password_entries: impl Iterator<Item = (String, String)>,
        expire_timeout: Duration,
    ) -> AuthenticationContext<UnixValidator> {
        AuthenticationContext::<UnixValidator> {
            token_store: Mutex::new(HashMap::new()),
            passwds: HashMap::from_iter(password_entries),
            password_validator: PhantomData::<UnixValidator>,
            expire_timeout,
        }
    }

    /// This function piggy-backs removes of expired tokens on an authentication
    /// request. This imposes a small penalty on each request. Its deemed not
    /// significant enough to justify optimalization given the expected volume
    /// of incoming requests.
    async fn new_and_remove_expired_tokens(&self, key: String) {
        let mut store = self.token_store.lock().await;

        store.retain(|_, last_access| {
            let duration = Instant::now().saturating_duration_since(*last_access);
            duration <= self.expire_timeout
        });

        store.insert(key, Instant::now());
    }

    async fn authorize_bearer(&self, token: &str) -> Result<(), AuthenticationError> {
        let mut store = self.token_store.lock().await;
        let Some(last_access) = store.get_mut(token) else {
            return Err(AuthenticationError::NoMatch(token.to_string()));
        };

        let instant = *last_access;
        let duration = Instant::now().saturating_duration_since(instant);
        if duration < self.expire_timeout {
            *last_access = Instant::now();
            return Ok(());
        }

        store.remove(token);
        Err(AuthenticationError::TokenExpired(instant))
    }

    fn validate_credentials(
        &self,
        username: &str,
        password: &str,
    ) -> Result<(), AuthenticationError> {
        let Some(pass) = self.passwds.get(username) else {
            log::debug!("user {} not in database", username);
            return Err(AuthenticationError::IncorrectCredentials);
        };

        P::validate(pass, password)
    }

    async fn authorize_basic(&self, credentials: &str) -> Result<(), AuthenticationError> {
        let decoded = general_purpose::STANDARD.decode(credentials)?;
        let utf8 = std::str::from_utf8(&decoded)?;
        let Some((user, pass)) = utf8.split_once(':') else {
            return Err(AuthenticationError::ParseError(
                "basic authorization formatted wrong".to_string(),
            ));
        };

        self.validate_credentials(user, pass)
    }

    pub async fn authorize_request(
        &self,
        http_authorization_line: &str,
    ) -> Result<(), AuthenticationError> {
        match http_authorization_line.split_once(' ') {
            Some(("Bearer", token)) => self.authorize_bearer(token).await,
            Some(("Basic", credentials)) => self.authorize_basic(credentials).await,
            Some((auth, _)) => Err(AuthenticationError::SchemeNotSupported(auth.to_string())),
            None => Err(AuthenticationError::HttpParseError(
                http_authorization_line.to_string(),
            )),
        }
    }

    pub async fn authenticate_request(&self, body: &[u8]) -> Result<Session, AuthenticationError> {
        let credentials = serde_json::from_slice::<Login>(body)?;

        self.validate_credentials(&credentials.username, &credentials.password)?;

        let token: String = thread_rng()
            .sample_iter(&Alphanumeric)
            .take(64)
            .map(char::from)
            .collect();
        self.new_and_remove_expired_tokens(token.clone()).await;

        Ok(Session {
            id: token, // according Redfish spec, id refers to the session id.
            // which is not equal to the access-token. for now use
            // the token.
            name: "User Session".to_string(),
            description: "User Session".to_string(),
            username: credentials.username,
        })
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Session {
    pub id: String,
    name: String,
    description: String,
    username: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct Login {
    username: String,
    password: String,
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::ops::Sub;

    pub struct DummyValidator {}
    impl PasswordValidator for DummyValidator {
        fn validate(
            _: &str,
            _: &str,
        ) -> Result<(), crate::authentication::authentication_errors::AuthenticationError> {
            Ok(())
        }
    }

    pub struct FalseValidator {}
    impl PasswordValidator for FalseValidator {
        fn validate(
            _: &str,
            _: &str,
        ) -> Result<(), crate::authentication::authentication_errors::AuthenticationError> {
            Err(AuthenticationError::IncorrectCredentials)
        }
    }

    pub fn build_test_context(
        token_data: impl IntoIterator<Item = (String, Instant)>,
        user_data: impl IntoIterator<Item = (String, String)>,
    ) -> AuthenticationContext<UnixValidator> {
        AuthenticationContext {
            token_store: Mutex::new(HashMap::from_iter(token_data)),
            passwds: HashMap::from_iter(user_data),
            password_validator: PhantomData::<UnixValidator>,
            expire_timeout: Duration::from_secs(20),
        }
    }

    #[actix_web::test]
    async fn test_token_failures() {
        let now = Instant::now();
        let twenty_sec_ago = now.sub(Duration::from_secs(20));
        let context = build_test_context(
            [("123".to_string(), now), ("2".to_string(), twenty_sec_ago)],
            Vec::new(),
        );

        assert_eq!(
            context.authorize_request("Bearer 1234").await.unwrap_err(),
            AuthenticationError::NoMatch("1234".to_string())
        );

        assert_eq!(
            context.authorize_request("Bearer 2").await.unwrap_err(),
            AuthenticationError::TokenExpired(twenty_sec_ago)
        );

        // After expired error, the token gets removed. Subsequent calls for that token will
        // therefore return "NoMatch"
        assert_eq!(
            context.authorize_request("Bearer 2").await.unwrap_err(),
            AuthenticationError::NoMatch("2".to_string())
        );
    }

    #[actix_web::test]
    async fn test_happy_flow() {
        let context = build_test_context(
            [
                ("123".to_string(), Instant::now()),
                ("2".to_string(), Instant::now().sub(Duration::from_secs(20))),
            ],
            Vec::new(),
        );
        assert_eq!(Ok(()), context.authorize_request("Bearer 123").await);
    }

    #[actix_web::test]
    async fn authentication_errors() {
        let context = build_test_context(
            Vec::new(),
            [("test_user".to_string(), "password".to_string())],
        );

        assert!(matches!(
            context
                .authenticate_request(b"{not a valid json")
                .await
                .unwrap_err(),
            AuthenticationError::ParseError(_)
        ));

        let json = serde_json::to_vec(&Login {
            username: "John".to_string(),
            password: "1234".to_string(),
        })
        .unwrap();

        assert_eq!(
            context.authenticate_request(&json).await.unwrap_err(),
            AuthenticationError::IncorrectCredentials
        );
        let json = serde_json::to_vec(&Login {
            username: "test_user".to_string(),
            password: "1234".to_string(),
        })
        .unwrap();

        assert_eq!(
            context.authenticate_request(&json).await.unwrap_err(),
            AuthenticationError::IncorrectCredentials
        );
    }

    #[actix_web::test]
    async fn pass_authentication() {
        let context = build_test_context(
            Vec::new(),
            [("test_user".to_string(), "password".to_string())],
        );
        let json = serde_json::to_vec(&Login {
            username: "test_user".to_string(),
            password: "password".to_string(),
        })
        .unwrap();

        assert_eq!(
            context.authenticate_request(&json).await.unwrap_err(),
            AuthenticationError::IncorrectCredentials
        );
    }
}
