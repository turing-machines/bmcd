use super::authentication_errors::AuthenticationError;

pub trait PasswordValidator {
    fn validate(hash: &str, password: &str) -> Result<(), AuthenticationError>;
}

pub struct UnixValidator {}

impl PasswordValidator for UnixValidator {
    fn validate(hash: &str, password: &str) -> Result<(), AuthenticationError> {
        log::debug!(
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
