use std::{fs, path::Path};

use serde::Deserialize;
use thiserror::Error;

use crate::{
    AppConfig, AppConfigV2, CONFIG_VERSION, CONFIG_VERSION_V2, ConfigLoadError, ConfigV2LoadError,
};

#[derive(Debug)]
pub enum ConfigDocument {
    V1(AppConfig),
    V2(AppConfigV2),
}

impl ConfigDocument {
    pub fn from_json_str(input: &str) -> Result<Self, ConfigDocumentLoadError> {
        let probe: VersionProbe = serde_json::from_str(input)?;
        match probe.version {
            CONFIG_VERSION => AppConfig::from_json_str(input)
                .map(Self::V1)
                .map_err(ConfigDocumentLoadError::V1),
            CONFIG_VERSION_V2 => AppConfigV2::from_json_str(input)
                .map(Self::V2)
                .map_err(ConfigDocumentLoadError::V2),
            version => Err(ConfigDocumentLoadError::UnsupportedVersion(version)),
        }
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigDocumentLoadError> {
        let input = fs::read_to_string(path).map_err(ConfigDocumentLoadError::Read)?;
        Self::from_json_str(&input)
    }
}

#[derive(Debug, Deserialize)]
struct VersionProbe {
    version: u32,
}

#[derive(Debug, Error)]
pub enum ConfigDocumentLoadError {
    #[error("failed to read configuration")]
    Read(#[source] std::io::Error),

    #[error("failed to parse configuration version")]
    Parse(#[from] serde_json::Error),

    #[error("failed to load v1 configuration")]
    V1(#[source] ConfigLoadError),

    #[error("failed to load v2 configuration")]
    V2(#[source] ConfigV2LoadError),

    #[error("unsupported configuration version: {0}")]
    UnsupportedVersion(u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_v1_configuration() {
        let input = r#"{
            "version": 1,
            "sources": [{
                "source_id": "local",
                "name": "local",
                "service": "local",
                "environment": "test",
                "root": "/var/log/local",
                "files": ["application.log"]
            }]
        }"#;

        assert!(matches!(
            ConfigDocument::from_json_str(input),
            Ok(ConfigDocument::V1(_))
        ));
    }

    #[test]
    fn routes_v2_local_configuration() {
        let input = r#"{
            "version": 2,
            "sources": [{
                "source_id": "local",
                "name": "local",
                "service": "local",
                "environment": "test",
                "backend": {"type": "local"},
                "root": "/var/log/local",
                "files": ["application.log"]
            }]
        }"#;

        assert!(matches!(
            ConfigDocument::from_json_str(input),
            Ok(ConfigDocument::V2(_))
        ));
    }

    #[test]
    fn rejects_unknown_version() {
        let error = ConfigDocument::from_json_str(r#"{"version": 99}"#)
            .expect_err("unknown version should fail");
        assert!(matches!(
            error,
            ConfigDocumentLoadError::UnsupportedVersion(99)
        ));
    }
}
