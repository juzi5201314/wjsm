//! `BestAvailableLocale` / `LookupSupportedLocales` / `ResolveLocale`。

use std::collections::BTreeMap;

use wjsm_intl_data::{default_locale, is_available_locale, unicode_extensions};

use super::canonicalize::{IntlError, canonicalize_unicode_locale_id};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedLocale {
    pub locale: String,
    pub data_locale: String,
    pub extensions: BTreeMap<String, String>,
}

/// ECMA-402 BestAvailableLocale：从完整标签向左剥离子标签直到命中 availableLocales。
pub fn best_available_locale(locale: &str) -> Option<String> {
    let mut candidate = locale.to_owned();
    loop {
        if is_available_locale(&candidate) {
            return Some(candidate);
        }
        let pos = candidate.rfind('-')?;
        let cut = if pos >= 2 && candidate.as_bytes().get(pos - 2) == Some(&b'-') {
            pos - 2
        } else {
            pos
        };
        candidate.truncate(cut);
    }
}

pub fn lookup_supported_locales(requested: &[String]) -> Result<Vec<String>, IntlError> {
    let mut supported = Vec::new();
    for tag in requested {
        let no_extensions = strip_unicode_extensions(tag);
        if best_available_locale(&no_extensions).is_some()
            && !supported.iter().any(|item| item == tag)
        {
            supported.push(tag.clone());
        }
    }
    Ok(supported)
}

/// `ResolveLocale`：选项覆盖 locale 扩展；不支持的值忽略；选项与扩展不同则不写入结果 locale。
pub fn resolve_locale(
    requested: &[String],
    relevant_extension_keys: &[&str],
    options: &BTreeMap<String, String>,
) -> Result<ResolvedLocale, IntlError> {
    resolve_locale_filtered(requested, relevant_extension_keys, options, |_, _, _| true)
}

pub fn resolve_locale_filtered(
    requested: &[String],
    relevant_extension_keys: &[&str],
    options: &BTreeMap<String, String>,
    is_supported: impl Fn(&str, &str, &str) -> bool,
) -> Result<ResolvedLocale, IntlError> {
    let mut found = None;
    for tag in requested {
        let no_extensions = strip_unicode_extensions(tag);
        if let Some(available) = best_available_locale(&no_extensions) {
            found = Some((tag.clone(), available));
            break;
        }
    }
    let (requested_tag, data_locale) = match found {
        Some(pair) => pair,
        None => {
            let default = default_locale();
            (default.clone(), default)
        }
    };
    let requested_ext = unicode_extensions(&requested_tag)
        .map(|map| map.entries)
        .unwrap_or_default();
    let mut extensions = BTreeMap::new();
    let mut locale_additions = BTreeMap::new();
    for key in relevant_extension_keys {
        let locale_value = requested_ext
            .get(*key)
            .map(|value| normalize_extension_value(value))
            .filter(|value| is_supported(key, value, &data_locale));
        let option_value = options
            .get(*key)
            .map(|value| normalize_extension_value(value))
            .filter(|value| is_supported(key, value, &data_locale));
        match (locale_value, option_value) {
            (Some(locale_value), Some(option_value)) if locale_value == option_value => {
                locale_additions.insert((*key).to_owned(), option_value.clone());
                extensions.insert((*key).to_owned(), option_value);
            }
            (Some(_), Some(option_value)) => {
                extensions.insert((*key).to_owned(), option_value);
            }
            (Some(locale_value), None) => {
                locale_additions.insert((*key).to_owned(), locale_value.clone());
                extensions.insert((*key).to_owned(), locale_value);
            }
            (None, Some(option_value)) => {
                extensions.insert((*key).to_owned(), option_value);
            }
            (None, None) => {}
        }
    }
    let locale = if locale_additions.is_empty() {
        canonicalize_unicode_locale_id(&data_locale).unwrap_or_else(|_| data_locale.clone())
    } else {
        let mut tag = data_locale.clone();
        tag.push_str("-u");
        for (key, value) in &locale_additions {
            tag.push('-');
            tag.push_str(key);
            if !value.is_empty() && value != "true" {
                tag.push('-');
                tag.push_str(value);
            }
        }
        canonicalize_unicode_locale_id(&tag).unwrap_or(data_locale.clone())
    };
    Ok(ResolvedLocale {
        locale,
        data_locale,
        extensions,
    })
}

fn normalize_extension_value(value: &str) -> String {
    if value.is_empty() {
        "true".into()
    } else {
        value.to_owned()
    }
}

fn strip_unicode_extensions(tag: &str) -> String {
    let lower = tag.to_ascii_lowercase();
    if let Some(index) = find_extension_start(&lower) {
        tag[..index].trim_end_matches('-').to_owned()
    } else {
        tag.to_owned()
    }
}

fn find_extension_start(tag: &str) -> Option<usize> {
    let mut start = 0;
    while let Some(rel) = tag[start..].find('-') {
        let pos = start + rel;
        let rest = &tag[pos + 1..];
        let subtag = rest.split('-').next().unwrap_or("");
        if subtag.len() == 1 {
            return Some(pos);
        }
        start = pos + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        best_available_locale, lookup_supported_locales, resolve_locale, resolve_locale_filtered,
    };

    #[test]
    fn rejects_zxx() {
        assert!(best_available_locale("zxx").is_none());
        assert!(best_available_locale("en").is_some());
    }

    #[test]
    fn supported_locales_skip_zxx() {
        let requested = ["en".to_owned(), "zxx".to_owned()];
        let supported = lookup_supported_locales(&requested).expect("lookup");
        assert!(supported.iter().any(|item| item == "en"));
        assert!(!supported.iter().any(|item| item == "zxx"));
    }

    #[test]
    fn option_override_drops_locale_extension() {
        let mut options = BTreeMap::new();
        options.insert("kn".into(), "true".into());
        let resolved =
            resolve_locale(&["en-u-kn-false".into()], &["kn"], &options).expect("resolve");
        assert_eq!(resolved.locale, "en");
        assert_eq!(
            resolved.extensions.get("kn").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn matching_option_keeps_locale_extension() {
        let mut options = BTreeMap::new();
        options.insert("kn".into(), "true".into());
        let resolved =
            resolve_locale(&["en-u-kn-true".into()], &["kn"], &options).expect("resolve");
        assert_eq!(resolved.locale, "en-u-kn");
        assert_eq!(
            resolved.extensions.get("kn").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn unsupported_locale_extension_is_ignored() {
        let resolved = resolve_locale_filtered(
            &["en-US-u-co-standard".into()],
            &["co"],
            &BTreeMap::new(),
            |_, value, _| value != "standard",
        )
        .expect("resolve");
        assert_eq!(resolved.locale, "en-US");
        assert!(!resolved.extensions.contains_key("co"));
    }

    #[test]
    fn empty_boolean_extension_defaults_to_true() {
        let resolved =
            resolve_locale(&["en-u-kn".into()], &["kn"], &BTreeMap::new()).expect("resolve");
        assert_eq!(
            resolved.extensions.get("kn").map(String::as_str),
            Some("true")
        );
        assert_eq!(resolved.locale, "en-u-kn");
    }
}
