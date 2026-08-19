use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! opaque_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub(crate) fn new() -> Self {
                Self(Uuid::new_v4().simple().to_string())
            }

            pub(crate) fn parse(value: String) -> Result<Self, InvalidOpaqueId> {
                if valid_opaque_id(&value) {
                    Ok(Self(value))
                } else {
                    Err(InvalidOpaqueId)
                }
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

opaque_id!(CacheSourceId);
opaque_id!(CacheFileId);
opaque_id!(GenerationId);

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GenerationKey {
    pub source_id: CacheSourceId,
    pub file_id: CacheFileId,
    pub generation_id: GenerationId,
}

impl GenerationKey {
    #[must_use]
    pub fn new(
        source_id: CacheSourceId,
        file_id: CacheFileId,
        generation_id: GenerationId,
    ) -> Self {
        Self {
            source_id,
            file_id,
            generation_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InvalidOpaqueId;

fn valid_opaque_id(value: &str) -> bool {
    value.len() == 32
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        && Uuid::parse_str(value).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_are_opaque_uuid_values() {
        let source = CacheSourceId::new();
        let file = CacheFileId::new();
        let generation = GenerationId::new();

        for value in [source.as_str(), file.as_str(), generation.as_str()] {
            assert_eq!(value.len(), 32);
            assert!(value.bytes().all(|byte| byte.is_ascii_hexdigit()));
            assert!(Uuid::parse_str(value).is_ok());
        }
    }

    #[test]
    fn malformed_ids_are_rejected() {
        assert!(CacheSourceId::parse("../logs".to_owned()).is_err());
        assert!(CacheFileId::parse("not-a-uuid".to_owned()).is_err());
        assert!(GenerationId::parse("0".repeat(31)).is_err());
    }
}
