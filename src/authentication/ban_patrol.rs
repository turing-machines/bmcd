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
use humantime::format_duration;
use std::{collections::HashMap, ops::Add, time::Duration};
use tokio::time::Instant;

const BAN_LEVELS: usize = 10;
const BAN_DURATION: Duration = Duration::from_secs(60);

/// Book-keeps how many consecutive failed attempts a given peer has made to
/// authenticate itself. When a given threshold is exceeded, every consecutive
/// failed attempt exponentially increases the cool down period in which the
/// peer is blocked from authenticate itself up to a limit declared in
/// [`BAN_LEVELS`].
pub struct BanPatrol {
    penalties: HashMap<String, (usize, Instant)>,
    max_authentication_attempts: usize,
}

impl BanPatrol {
    /// construct an instance. `max_authentication_attempts` is not allowed to
    /// be 0.
    pub fn new(max_authentication_attempts: usize) -> Self {
        assert!(
            max_authentication_attempts != 0,
            "chosing a value of 0 for the maximum permitted \
                authentication attempts is not allowed."
        );

        Self {
            penalties: HashMap::new(),
            max_authentication_attempts,
        }
    }

    /// Verifies if a given peer is banned and therefore is denied to authenticate
    pub fn patrole_ban(&mut self, peer: &str) -> Result<(), AuthenticationError> {
        let Some((attempts, deadline)) = self.penalties.get(peer) else {
            return Ok(());
        };

        if deadline > &Instant::now() {
            tracing::info!(
                "{} banned for: {}. attempts: {}",
                peer,
                format_duration(deadline.duration_since(Instant::now())),
                attempts,
            );
            Err(AuthenticationError::ExceededAllowedAttempts(*deadline))
        } else {
            Ok(())
        }
    }

    pub fn clear_penalties(&mut self, peer: &str) {
        self.penalties.remove(peer);
    }

    pub fn penalize(&mut self, peer: &str) -> Result<(), AuthenticationError> {
        let (attempts, deadline) = self
            .penalties
            .entry(peer.to_string())
            .or_insert((0, Instant::now()));
        *attempts += 1;

        if attempts >= &mut self.max_authentication_attempts {
            let attempts_exceeded_allowed = *attempts - self.max_authentication_attempts;
            let multiplier = 1 << attempts_exceeded_allowed.min(BAN_LEVELS);
            *deadline = Instant::now().add(BAN_DURATION * multiplier);
            return self.patrole_ban(peer);
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_parole_consequtive() {
        let mut patrol = BanPatrol::new(1);
        assert!(patrol.patrole_ban("peer").is_ok());
        assert!(patrol.patrole_ban("peer2").is_ok());
        assert!(patrol.patrole_ban("peer").is_ok());
    }

    #[test]
    fn test_penalties() {
        let mut patrol = BanPatrol::new(1);
        assert!(patrol.patrole_ban("peer").is_ok());
        assert!(patrol.patrole_ban("peer1").is_ok());

        let expected = Instant::now().add(BAN_DURATION);
        let denied = patrol.penalize("peer1").unwrap_err();
        assert!(
            matches!(denied, AuthenticationError::ExceededAllowedAttempts(expiry) if expiry > expected)
        );
        let denied = patrol.patrole_ban("peer1").unwrap_err();
        assert!(
            matches!(denied, AuthenticationError::ExceededAllowedAttempts(expiry) if expiry > expected)
        );
        assert!(patrol.patrole_ban("peer").is_ok());
        let denied = patrol.penalize("peer").unwrap_err();
        assert!(
            matches!(denied, AuthenticationError::ExceededAllowedAttempts(expiry) if expiry > expected)
        );
        let denied = patrol.patrole_ban("peer").unwrap_err();
        assert!(
            matches!(denied, AuthenticationError::ExceededAllowedAttempts(expiry) if expiry > expected)
        );

        // clear peer1
        patrol.clear_penalties("peer1");
        assert!(patrol.patrole_ban("peer1").is_ok());
        let denied = patrol.patrole_ban("peer").unwrap_err();
        assert!(
            matches!(denied, AuthenticationError::ExceededAllowedAttempts(expiry) if expiry > expected)
        );
    }

    #[test]
    fn test_penalty_expiry_cap() {
        let mut patrol = BanPatrol::new(1);
        let mut last_ban = Instant::now();
        (0..BAN_LEVELS).for_each(|i|{
             let expected = Instant::now().add((1 <<i) *BAN_DURATION);
             let upper_limit = expected.add(Duration::from_secs(2));
             let denied = patrol.penalize("peer").unwrap_err();
             assert!(
                 matches!(denied, AuthenticationError::ExceededAllowedAttempts(expiry) if expiry > expected && expiry < upper_limit)
             );
             last_ban = expected;
        });

        let start_time = patrol.penalties.get("peer").unwrap().1;
        assert!(patrol.penalize("peer").is_err());
        let denied = patrol.penalize("peer").unwrap_err();
        let expected = start_time.add((1 << (BAN_LEVELS - 1)) * BAN_DURATION);
        let upper_limit = expected.add(Duration::from_secs(2));
        if let AuthenticationError::ExceededAllowedAttempts(expiry) = denied {
            print!(
                "expiry={} expected={}",
                expiry.duration_since(start_time).as_secs(),
                expected.duration_since(start_time).as_secs()
            );
        }
        assert!(
            matches!(denied, AuthenticationError::ExceededAllowedAttempts(expiry) if expiry > expected && expiry < upper_limit)
        );
    }
}
