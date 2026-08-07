use std::{env, ffi::OsString, fmt};

use thiserror::Error;

pub trait SecretResolver: Send + Sync {
    fn resolve(&self, secret_ref: &str) -> Result<SecretValue, SecretResolveError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EnvSecretResolver;

impl SecretResolver for EnvSecretResolver {
    fn resolve(&self, secret_ref: &str) -> Result<SecretValue, SecretResolveError> {
        resolve_environment_with(secret_ref, env::var_os)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    fn new(value: String) -> Result<Self, SecretResolveError> {
        if value.is_empty() || value.chars().any(char::is_control) {
            return Err(SecretResolveError::InvalidValue);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_exposed(self) -> String {
        self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue(<redacted>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SecretResolveError {
    #[error("secret reference is empty or invalid")]
    InvalidReference,
    #[error("referenced secret is missing")]
    Missing,
    #[error("referenced secret is not valid UTF-8")]
    NotUnicode,
    #[error("referenced secret value is empty or contains control characters")]
    InvalidValue,
}

fn resolve_environment_with<F>(
    secret_ref: &str,
    getter: F,
) -> Result<SecretValue, SecretResolveError>
where
    F: FnOnce(&str) -> Option<OsString>,
{
    if !valid_environment_reference(secret_ref) {
        return Err(SecretResolveError::InvalidReference);
    }
    let value = getter(secret_ref).ok_or(SecretResolveError::Missing)?;
    let value = value
        .into_string()
        .map_err(|_| SecretResolveError::NotUnicode)?;
    SecretValue::new(value)
}

fn valid_environment_reference(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some('A'..='Z' | '_'))
        && chars.all(|character| matches!(character, 'A'..='Z' | '0'..='9' | '_'))
        && value.len() <= 256
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_environment_secret_without_exposing_debug_value() {
        let secret = resolve_environment_with("TEST_PASSWORD", |_| Some(OsString::from("s3cret")))
            .expect("secret should resolve");

        assert_eq!(secret.expose(), "s3cret");
        assert_eq!(format!("{secret:?}"), "SecretValue(<redacted>)");
        assert!(!format!("{secret:?}").contains("s3cret"));
    }

    #[test]
    fn rejects_missing_secret() {
        assert_eq!(
            resolve_environment_with("TEST_PASSWORD", |_| None),
            Err(SecretResolveError::Missing)
        );
    }

    #[test]
    fn rejects_invalid_secret_reference() {
        assert_eq!(
            resolve_environment_with("bad-ref", |_| Some(OsString::from("value"))),
            Err(SecretResolveError::InvalidReference)
        );
    }

    #[test]
    fn rejects_empty_or_control_character_secret() {
        assert_eq!(
            resolve_environment_with("TEST_PASSWORD", |_| Some(OsString::from(""))),
            Err(SecretResolveError::InvalidValue)
        );
        assert_eq!(
            resolve_environment_with("TEST_PASSWORD", |_| Some(OsString::from("bad\nvalue"))),
            Err(SecretResolveError::InvalidValue)
        );
    }
}
