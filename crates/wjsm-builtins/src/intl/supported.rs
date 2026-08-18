//! `Intl.supportedValuesOf` 与 `getCanonicalLocales` 的已 coerce 入口。

use wjsm_intl_data::supported_values;

use super::canonicalize::{IntlError, canonicalize_locale_list};

pub fn get_canonical_locales(tags: &[String]) -> Result<Vec<String>, IntlError> {
    canonicalize_locale_list(tags)
}

pub fn supported_values_of(key: &str) -> Result<Vec<String>, IntlError> {
    supported_values(key).map_err(IntlError::range)
}

#[cfg(test)]
mod tests {
    use super::supported_values_of;

    #[test]
    fn calendars_include_gregory() {
        let values = supported_values_of("calendar").expect("calendar");
        assert!(values.iter().any(|item| item == "gregory"));
    }

    #[test]
    fn rejects_unknown_key() {
        assert!(supported_values_of("hourCycle").is_err());
    }
}
