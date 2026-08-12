use std::collections::HashSet;

use wjsm_ir::{Constant, ModuleId, Program};

use crate::{ArtifactFormatError, ModuleManifest};

pub(crate) fn verify_artifact(
    program: &Program,
    manifest: &ModuleManifest,
) -> Result<(), ArtifactFormatError> {
    program
        .verify()
        .map_err(|error| ArtifactFormatError::InvalidIr(error.to_string()))?;

    if manifest.modules.is_empty() {
        return Err(ArtifactFormatError::InvalidManifest(
            "module manifest is empty".into(),
        ));
    }

    let mut ids = HashSet::with_capacity(manifest.modules.len());
    let mut urls = HashSet::with_capacity(manifest.modules.len());
    for module in &manifest.modules {
        if !ids.insert(module.id) {
            return Err(ArtifactFormatError::InvalidManifest(format!(
                "duplicate module id {}",
                module.id.0
            )));
        }
        if let Err(reason) = validate_logical_url(&module.logical_url) {
            return Err(ArtifactFormatError::InvalidManifest(format!(
                "invalid logical module URL {:?}: {reason}",
                module.logical_url
            )));
        }
        if !urls.insert(module.logical_url.as_str()) {
            return Err(ArtifactFormatError::InvalidManifest(format!(
                "duplicate logical module URL {}",
                module.logical_url
            )));
        }
    }

    ensure_module_exists(manifest.entry, &ids, "entry")?;
    for module in &manifest.modules {
        for dependency in &module.static_dependencies {
            ensure_module_exists(*dependency, &ids, "static dependency")?;
        }
        for (_, dependency) in &module.dynamic_dependencies {
            ensure_module_exists(*dependency, &ids, "dynamic dependency")?;
        }
    }
    for constant in program.constants() {
        if let Constant::ModuleId(module_id) = constant {
            ensure_module_exists(*module_id, &ids, "IR module constant")?;
        }
    }
    Ok(())
}

fn validate_logical_url(url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Err("URL is empty".into());
    }
    if url.contains('\\') {
        return Err("URL contains a backslash".into());
    }
    for component in url.split('/') {
        validate_logical_url_component(component)
            .map_err(|reason| format!("{reason}; component={component:?}"))?;
    }
    Ok(())
}

fn validate_logical_url_component(component: &str) -> Result<(), &'static str> {
    if component.is_empty() {
        return Err("URL contains an empty component");
    }
    let source = component.as_bytes();
    let mut decoded = Vec::with_capacity(source.len());
    let mut index = 0;
    while index < source.len() {
        if source[index] != b'%' {
            decoded.push(source[index]);
            index += 1;
            continue;
        }
        let Some(encoded) = source.get(index + 1..index + 3) else {
            return Err("URL contains truncated percent encoding");
        };
        let Some(high) = canonical_hex(encoded[0]) else {
            return Err("URL contains non-canonical percent encoding");
        };
        let Some(low) = canonical_hex(encoded[1]) else {
            return Err("URL contains non-canonical percent encoding");
        };
        let byte = (high << 4) | low;
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            return Err("URL percent-encodes an unreserved byte");
        }
        decoded.push(byte);
        index += 3;
    }
    if decoded == b"." || decoded == b".." {
        return Err("URL contains a relative traversal component");
    }
    if decoded.contains(&b'/') || decoded.contains(&b'\\') {
        return Err("URL encodes a path separator");
    }
    if decoded.contains(&0) {
        return Err("URL encodes NUL");
    }
    Ok(())
}

#[cfg(test)]
fn valid_logical_url(url: &str) -> bool {
    validate_logical_url(url).is_ok()
}

fn canonical_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::valid_logical_url;

    #[test]
    fn accepts_relative_and_percent_encoded_logical_urls() {
        assert!(valid_logical_url("errors/for_await_non_iterable.js"));
        assert!(valid_logical_url("modules/main_%FF.js"));
    }

    #[test]
    fn rejects_noncanonical_or_escaping_logical_urls() {
        assert!(!valid_logical_url("../main.js"));
        assert!(!valid_logical_url("%2E%2E/main.js"));
        assert!(!valid_logical_url("dir/%2Fetc"));
        assert!(!valid_logical_url("main_%ff.js"));
    }
}

fn ensure_module_exists(
    id: ModuleId,
    ids: &HashSet<ModuleId>,
    context: &str,
) -> Result<(), ArtifactFormatError> {
    if ids.contains(&id) {
        Ok(())
    } else {
        Err(ArtifactFormatError::InvalidManifest(format!(
            "{context} references missing module id {}",
            id.0
        )))
    }
}
