use anyhow::Result;
use serde::Serialize;

pub(super) fn to_json<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(Into::into)
}
