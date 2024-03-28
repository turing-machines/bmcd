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
use super::authentication_errors::SchemedAuthError;
use super::ban_patrol::BanPatrol;
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
use tokio::time::{Duration, Instant};

pub struct AuthenticationContext<P>
where
    P: PasswordValidator + 'static,
{
    token_store: HashMap<String, Instant>,
    passwds: HashMap<String, String>,
    password_validator: PhantomData<P>,
    expire_timeout: Duration,
    ban_patrol: BanPatrol,
}

impl<P> AuthenticationContext<P>
where
    P: PasswordValidator + 'static,
{
    pub fn with_unix_validator(
        password_entries: impl Iterator<Item = (String, String)>,
        expire_timeout: Duration,
        authentication_attempts: usize,
    ) -> AuthenticationContext<UnixValidator> {
        AuthenticationContext::<UnixValidator> {
            token_store: HashMap::new(),
            passwds: HashMap::from_iter(password_entries),
            password_validator: PhantomData::<UnixValidator>,
            expire_timeout,
            ban_patrol: BanPatrol::new(authentication_attempts),
        }
    }

    pub fn reload_password_cache(
        &mut self,
        password_entries: impl Iterator<Item = (String, String)>,
    ) {
        self.passwds = HashMap::from_iter(password_entries);
    }

    /// This function piggy-backs removes of expired tokens on an authentication
    /// request. This imposes a small penalty on each request. Its deemed not
    /// significant enough to justify optimization given the expected volume
    /// of incoming authentication requests.
    async fn new_and_remove_expired_tokens(&mut self, key: String) {
        self.token_store.retain(|_, last_access| {
            let duration = Instant::now().saturating_duration_since(*last_access);
            duration <= self.expire_timeout
        });

        self.token_store.insert(key, Instant::now());
    }

    async fn authorize_bearer(
        &mut self,
        peer: &str,
        token: &str,
    ) -> Result<(), AuthenticationError> {
        self.ban_patrol.patrole_ban(peer)?;

        let Some(last_access) = self.token_store.get_mut(token) else {
            return Err(self
                .ban_patrol
                .penalize(peer)
                .err()
                .unwrap_or(AuthenticationError::NoMatch(token.to_string())));
        };

        let instant = *last_access;
        let duration = Instant::now().saturating_duration_since(instant);
        if duration < self.expire_timeout {
            *last_access = Instant::now();
            self.ban_patrol.clear_penalties(peer);
            return Ok(());
        }

        self.token_store.remove(token);
        Err(AuthenticationError::TokenExpired(instant))
    }

    fn validate_credentials(
        &mut self,
        peer: &str,
        username: &str,
        password: &str,
    ) -> Result<(), AuthenticationError> {
        self.ban_patrol.patrole_ban(peer)?;

        match self
            .passwds
            .get(username)
            .ok_or(AuthenticationError::IncorrectCredentials)
            .and_then(|pass| P::validate(pass, password))
        {
            Ok(_) => {
                tracing::debug!("{username} validated successfully");
                self.ban_patrol.clear_penalties(peer);
                Ok(())
            }
            Err(AuthenticationError::IncorrectCredentials) => Err(self
                .ban_patrol
                .penalize(peer)
                .err()
                .unwrap_or(AuthenticationError::IncorrectCredentials)),
            Err(err) => Err(err),
        }
    }

    async fn authorize_basic(
        &mut self,
        peer: &str,
        credentials: &str,
    ) -> Result<(), AuthenticationError> {
        let decoded = general_purpose::STANDARD.decode(credentials)?;
        let utf8 = std::str::from_utf8(&decoded)?;
        let Some((user, pass)) = utf8.split_once(':') else {
            return Err(AuthenticationError::ParseError(
                "basic authentication formatted wrong".to_string(),
            ));
        };

        self.validate_credentials(peer, user, pass)
    }

    pub async fn authorize_request(
        &mut self,
        peer: &str,
        http_authorization_line: &str,
    ) -> Result<(), SchemedAuthError> {
        match http_authorization_line.split_once(' ') {
            Some(("Bearer", token)) => self
                .authorize_bearer(peer, token)
                .await
                .map_err(AuthenticationError::into_bearer_error),
            Some(("Basic", credentials)) => self
                .authorize_basic(peer, credentials)
                .await
                .map_err(AuthenticationError::into_basic_error),
            Some((auth, _)) => {
                Err(AuthenticationError::SchemeNotSupported(auth.to_string()).into_unknown_error())
            }
            None => Err(
                AuthenticationError::HttpParseError(http_authorization_line.to_string())
                    .into_basic_error(),
            ),
        }
    }

    pub async fn authenticate_request(
        &mut self,
        peer: &str,
        body: &[u8],
    ) -> Result<Session, AuthenticationError> {
        let credentials = serde_json::from_slice::<Login>(body)?;

        self.validate_credentials(peer, &credentials.username, &credentials.password)?;

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
            token_store: HashMap::from_iter(token_data),
            passwds: HashMap::from_iter(user_data),
            password_validator: PhantomData::<UnixValidator>,
            expire_timeout: Duration::from_secs(20),
            ban_patrol: BanPatrol::new(10),
        }
    }

    #[actix_web::test]
    async fn test_token_failures() {
        let now = Instant::now();
        let twenty_sec_ago = now.sub(Duration::from_secs(20));
        let mut context = build_test_context(
            [("123".to_string(), now), ("2".to_string(), twenty_sec_ago)],
            Vec::new(),
        );

        assert_eq!(
            context
                .authorize_request("peer", "Bearer 1234")
                .await
                .unwrap_err()
                .1,
            AuthenticationError::NoMatch("1234".to_string())
        );

        assert_eq!(
            context
                .authorize_request("peer", "Bearer 2")
                .await
                .unwrap_err()
                .1,
            AuthenticationError::TokenExpired(twenty_sec_ago)
        );

        // After expired error, the token gets removed. Subsequent calls for that token will
        // therefore return "NoMatch"
        assert_eq!(
            context
                .authorize_request("peer", "Bearer 2")
                .await
                .unwrap_err()
                .1,
            AuthenticationError::NoMatch("2".to_string())
        );
    }

    #[actix_web::test]
    async fn test_happy_flow() {
        let mut context = build_test_context(
            [
                ("123".to_string(), Instant::now()),
                ("2".to_string(), Instant::now().sub(Duration::from_secs(20))),
            ],
            Vec::new(),
        );
        assert_eq!(
            Ok(()),
            context.authorize_request("peer1", "Bearer 123").await
        );
    }

    #[actix_web::test]
    async fn authentication_errors() {
        let mut context = build_test_context(
            Vec::new(),
            [("test_user".to_string(), "password".to_string())],
        );

        assert!(matches!(
            context
                .authenticate_request("peer1", b"{not a valid json")
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
            context
                .authenticate_request("peer", &json)
                .await
                .unwrap_err(),
            AuthenticationError::IncorrectCredentials
        );
        let json = serde_json::to_vec(&Login {
            username: "test_user".to_string(),
            password: "1234".to_string(),
        })
        .unwrap();

        assert_eq!(
            context
                .authenticate_request("peer", &json)
                .await
                .unwrap_err(),
            AuthenticationError::IncorrectCredentials
        );
    }

    #[actix_web::test]
    async fn pass_authentication() {
        let mut context = build_test_context(
            Vec::new(),
            [("test_user".to_string(), "password".to_string())],
        );
        let json = serde_json::to_vec(&Login {
            username: "test_user".to_string(),
            password: "password".to_string(),
        })
        .unwrap();

        assert_eq!(
            context
                .authenticate_request("peer", &json)
                .await
                .unwrap_err(),
            AuthenticationError::IncorrectCredentials
        );
    }
}
