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
        let Some((attempts, start_time)) = self.penalties.get(peer) else {
            return Ok(());
        };

        self.patrole_peer(peer, attempts, start_time)
    }

    pub fn clear_penalties(&mut self, peer: &str) {
        self.penalties.remove(peer);
    }

    pub fn penalize(&mut self, peer: &str) -> Result<(), AuthenticationError> {
        let (attempts, start_time) = self
            .penalties
            .entry(peer.to_string())
            .or_insert((0, Instant::now()));
        *attempts += 1;

        // TODO: cannot re-alias &mut _ to &_?
        let attempts = *attempts;
        let time = *start_time;
        self.patrole_peer(peer, &attempts, &time)
    }

    fn patrole_peer(
        &self,
        peer: &str,
        attempts: &usize,
        start_time: &Instant,
    ) -> Result<(), AuthenticationError> {
        if attempts >= &self.max_authentication_attempts {
            let severity = 1 << (attempts - self.max_authentication_attempts).min(BAN_LEVELS);
            let deadline = start_time.add(BAN_DURATION * severity);

            if deadline > Instant::now() {
                log::info!(
                    "{} banned for: {}s. severity: {}",
                    peer,
                    format_duration(deadline.duration_since(Instant::now())),
                    severity,
                );
                return Err(AuthenticationError::ExceededAllowedAttempts(deadline));
            }
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
        assert!(patrol.patrole_ban("peer".to_string()).is_ok());
        assert!(patrol.patrole_ban("peer2".to_string()).is_ok());
        assert!(patrol.patrole_ban("peer".to_string()).is_ok());
    }

    #[test]
    fn test_penalties() {
        let mut patrol = BanPatrol::new(1);
        assert!(patrol.patrole_ban("peer".to_string()).is_ok());
        assert!(patrol.patrole_ban("peer1".to_string()).is_ok());

        let expected = Instant::now().add(BAN_DURATION);
        let denied = patrol.penalize("peer1".to_string()).unwrap_err();
        assert!(
            matches!(denied, AuthenticationError::ExceededAllowedAttempts(expiry) if expiry > expected)
        );
        let denied = patrol.patrole_ban("peer1".to_string()).unwrap_err();
        assert!(
            matches!(denied, AuthenticationError::ExceededAllowedAttempts(expiry) if expiry > expected)
        );
        assert!(patrol.patrole_ban("peer".to_string()).is_ok());
        let denied = patrol.penalize("peer".to_string()).unwrap_err();
        assert!(
            matches!(denied, AuthenticationError::ExceededAllowedAttempts(expiry) if expiry > expected)
        );
        let denied = patrol.patrole_ban("peer".to_string()).unwrap_err();
        assert!(
            matches!(denied, AuthenticationError::ExceededAllowedAttempts(expiry) if expiry > expected)
        );

        // clear peer1
        patrol.clear_penalties("peer1");
        assert!(patrol.patrole_ban("peer1".to_string()).is_ok());
        let denied = patrol.patrole_ban("peer".to_string()).unwrap_err();
        assert!(
            matches!(denied, AuthenticationError::ExceededAllowedAttempts(expiry) if expiry > expected)
        );
    }

    #[test]
    fn test_penalty_expiry_cap() {
        let mut patrol = BanPatrol::new(1);
        let now = Instant::now();
        (0..BAN_LEVELS).for_each(|i|{
             let expected = now.add((1 <<i) *BAN_DURATION);
             assert!(patrol.penalize("peer".to_string()).is_err());
             let denied = patrol.patrole_ban("peer".to_string()).unwrap_err();
             assert!(
                 matches!(denied, AuthenticationError::ExceededAllowedAttempts(expiry) if expiry > expected)
             );
        });

        let start_time = patrol.penalties.get("peer").unwrap().1;
        assert!(patrol.penalize("peer".to_string()).is_err());
        assert!(patrol.penalize("peer".to_string()).is_err());
        let expected = start_time.add((1 << BAN_LEVELS) * BAN_DURATION);
        let denied = patrol.patrole_ban("peer".to_string()).unwrap_err();
        assert!(
            matches!(denied, AuthenticationError::ExceededAllowedAttempts(expiry) if expiry == expected)
        );
    }
}
