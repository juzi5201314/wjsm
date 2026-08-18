//! `CanonicalizeLocaleList` / `CanonicalizeUnicodeLocaleId`。

use wjsm_intl_data::{
    canonicalize_unicode_locale_id as data_canonicalize, is_structurally_valid_language_tag,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntlErrorKind {
    Range,
    Type,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntlError {
    pub kind: IntlErrorKind,
    pub message: String,
}

impl IntlError {
    pub fn range(message: impl Into<String>) -> Self {
        Self {
            kind: IntlErrorKind::Range,
            message: message.into(),
        }
    }

    pub fn r#type(message: impl Into<String>) -> Self {
        Self {
            kind: IntlErrorKind::Type,
            message: message.into(),
        }
    }
}

pub fn canonicalize_unicode_locale_id(tag: &str) -> Result<String, IntlError> {
    if !is_structurally_valid_language_tag(tag) {
        return Err(IntlError::range(format!("Invalid language tag: {tag}")));
    }
    data_canonicalize(tag).map_err(IntlError::range)
}

/// 对已经是字符串的 locale 列表做规范化和去重，保持首次出现顺序。
pub fn canonicalize_locale_list(tags: &[String]) -> Result<Vec<String>, IntlError> {
    let mut seen = Vec::with_capacity(tags.len());
    for tag in tags {
        let canonical = canonicalize_unicode_locale_id(tag)?;
        if !seen.iter().any(|item| item == &canonical) {
            seen.push(canonical);
        }
    }
    Ok(seen)
}

#[cfg(test)]
mod tests {
    use super::{canonicalize_locale_list, canonicalize_unicode_locale_id};

    #[test]
    fn canonicalizes_and_dedups() {
        let tags = ["DE-de".to_owned(), "de-DE".to_owned(), "en".to_owned()];
        let result = canonicalize_locale_list(&tags).expect("list");
        assert_eq!(result, ["de-DE", "en"]);
    }

    #[test]
    fn rejects_empty_tag() {
        assert!(canonicalize_unicode_locale_id("").is_err());
    }
}
