use serde::Serialize;

use crate::{ToolError, ToolErrorCode};

pub fn serialize_with_limit<T>(value: &T, max_response_bytes: usize) -> Result<String, ToolError>
where
    T: Serialize,
{
    let serialized = serde_json::to_string(value).map_err(|_| ToolError::internal_error())?;
    if serialized.len() > max_response_bytes {
        return Err(ToolError::new(ToolErrorCode::ResourceLimit));
    }
    Ok(serialized)
}
