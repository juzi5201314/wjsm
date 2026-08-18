//! Locale 解析、likely subtags、canonicalization 与 fallback。
//!
//! ICU4X 2.2 没有独立 LocaleMatcher 类型；ECMA-402 的 locale negotiation
//! 由 canonicalizer + expander + fallbacker 数据支撑，算法在 `wjsm-builtins`。

use std::collections::BTreeMap;
use std::sync::OnceLock;

use icu::locale::{Locale, LocaleCanonicalizer, LocaleExpander, LocaleFallbacker};

/// Unicode 扩展关键字（`u` 扩展）的规范化键值。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UnicodeExtensionMap {
    pub entries: BTreeMap<String, String>,
}

pub fn parse_locale(id: &str) -> Result<Locale, icu::locale::ParseError> {
    Locale::try_from_str(id)
}

pub fn expand_likely_subtags(id: &str) -> Result<Locale, icu::locale::ParseError> {
    let mut locale = parse_locale(id)?;
    expander().maximize(&mut locale.id);
    Ok(locale)
}

pub fn minimize_likely_subtags(id: &str) -> Result<Locale, icu::locale::ParseError> {
    let mut locale = parse_locale(id)?;
    expander().minimize(&mut locale.id);
    Ok(locale)
}

pub fn canonicalize_locale(id: &str) -> Result<Locale, icu::locale::ParseError> {
    let mut locale = parse_locale(id)?;
    canonicalizer().canonicalize(&mut locale);
    Ok(locale)
}

/// ECMA-402 `CanonicalizeUnicodeLocaleId`：结构合法后交给 ICU4X 做规范形。
pub fn canonicalize_unicode_locale_id(id: &str) -> Result<String, String> {
    if !is_structurally_valid_language_tag(id) {
        return Err(format!("Invalid language tag: {id}"));
    }
    let icu = match canonicalize_locale(id) {
        Ok(mut locale) => {
            crate::aliases::apply_extension_aliases(&mut locale);
            locale.to_string()
        }
        // ICU4X 不接受 5–8 字母 language；结构合法时按 UTS #35 回退。
        Err(_) => crate::tag::canonicalize_without_icu(id),
    };
    // ICU 序列化不一定按 UTS #35 排扩展；再走一遍结构规范化。
    if crate::tag::is_structurally_valid(&icu) {
        Ok(crate::tag::canonicalize_without_icu(&icu))
    } else {
        Ok(icu)
    }
}

/// 结构合法性以 UTS #35 Unicode Locale Identifier 为准，不依赖 ICU 解析器。
pub fn is_structurally_valid_language_tag(id: &str) -> bool {
    crate::tag::is_structurally_valid(id)
}

pub fn is_unicode_language_id(id: &str) -> bool {
    crate::tag::is_unicode_language_id(id)
}

/// 实现定义的默认 locale：`LC_ALL` / `LANG`，非法或 `C`/`POSIX` 回退到 `en`。
pub fn default_locale() -> String {
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            std::env::var("LC_ALL")
                .or_else(|_| std::env::var("LANG"))
                .ok()
                .and_then(|value| posix_locale_to_bcp47(&value))
                .unwrap_or_else(|| "en".to_owned())
        })
        .clone()
}

/// `zxx` 与没有 likely subtags 的未知语言（如 `xyz`）不进入 availableLocales。
pub fn is_available_locale(id: &str) -> bool {
    let Ok(locale) = parse_locale(id) else {
        return false;
    };
    if locale.id.language.as_str() == "zxx" {
        return false;
    }
    if locale.id.script.is_some() || locale.id.region.is_some() {
        return true;
    }
    match expand_likely_subtags(id) {
        Ok(maximized) => maximized.id.script.is_some() || maximized.id.region.is_some(),
        Err(_) => false,
    }
}

/// ICU4X fallback 链，供诊断；规范 `BestAvailableLocale` 仍按子标签剥离实现。
pub fn fallback_steps(id: &str) -> Result<Vec<String>, icu::locale::ParseError> {
    let locale = parse_locale(id)?;
    let fallbacker = LocaleFallbacker::new();
    let mut iterator = fallbacker
        .for_config(Default::default())
        .fallback_for((&locale).into());
    let mut steps = Vec::new();
    loop {
        let current = iterator.get().to_string();
        let done = current == "und";
        steps.push(current);
        if done {
            break;
        }
        iterator.step();
    }
    Ok(steps)
}

pub fn unicode_extensions(id: &str) -> Result<UnicodeExtensionMap, icu::locale::ParseError> {
    let locale = canonicalize_locale(id)?;
    let mut entries = BTreeMap::new();
    for (key, value) in locale.extensions.unicode.keywords.iter() {
        entries.insert(key.as_str().to_owned(), value.to_string());
    }
    Ok(UnicodeExtensionMap { entries })
}

pub fn keep_locale_data() {
    let _ = LocaleExpander::new_extended();
    let _ = LocaleCanonicalizer::new_extended();
    let _ = LocaleFallbacker::new();
    let _ = expand_likely_subtags("zh");
    let _ = canonicalize_locale("zh-CN");
    let _ = minimize_likely_subtags("zh-Hans-CN");
}

fn expander() -> LocaleExpander {
    LocaleExpander::new_extended()
}

fn canonicalizer() -> LocaleCanonicalizer {
    LocaleCanonicalizer::new_extended()
}

fn posix_locale_to_bcp47(value: &str) -> Option<String> {
    let stripped = value.split('.').next().unwrap_or(value);
    let stripped = stripped.split('@').next().unwrap_or(stripped);
    if matches!(stripped, "" | "C" | "POSIX") {
        return None;
    }
    let tag = stripped.replace('_', "-");
    canonicalize_unicode_locale_id(&tag).ok()
}

#[cfg(test)]
mod tests {
    use super::{
        canonicalize_unicode_locale_id, expand_likely_subtags, is_available_locale,
        is_structurally_valid_language_tag, minimize_likely_subtags,
    };

    #[test]
    fn zh_cn_maximizes_to_hans() {
        let locale = expand_likely_subtags("zh-CN").expect("zh-CN");
        assert_eq!(locale.to_string(), "zh-Hans-CN");
    }

    #[test]
    fn grandfathered_in_canonicalizes_to_id() {
        assert_eq!(canonicalize_unicode_locale_id("in").expect("in"), "id");
        assert_eq!(
            canonicalize_unicode_locale_id("cel-gaulish").expect("cel"),
            "xtg"
        );
    }

    #[test]
    fn extensions_are_sorted_and_posix_survives() {
        assert_eq!(
            canonicalize_unicode_locale_id("en-u-baz-a-bar-x-u-foo").expect("ext"),
            "en-a-bar-u-baz-x-u-foo"
        );
        assert_eq!(
            canonicalize_unicode_locale_id("posix").expect("posix"),
            "posix"
        );
        assert_eq!(
            canonicalize_unicode_locale_id("aar-x-private").expect("aar"),
            "aa-x-private"
        );
    }

    #[test]
    fn five_letter_language_posix_is_canonical() {
        assert_eq!(
            canonicalize_unicode_locale_id("posix").expect("posix"),
            "posix"
        );
    }

    #[test]
    fn unicode_type_aliases_and_true_omission() {
        assert_eq!(
            canonicalize_unicode_locale_id("und-u-ca-ethiopic-amete-alem").expect("ca"),
            "und-u-ca-ethioaa"
        );
        assert_eq!(
            canonicalize_unicode_locale_id("und-u-kb-yes").expect("kb"),
            "und-u-kb"
        );
        assert_eq!(
            canonicalize_unicode_locale_id("und-Latn-t-und-hani-m0-names").expect("t"),
            "und-Latn-t-und-hani-m0-prprname"
        );
    }

    #[test]
    fn rejects_empty_and_dangling_dash() {
        assert!(!is_structurally_valid_language_tag(""));
        assert!(!is_structurally_valid_language_tag("en-us-"));
        assert!(!is_structurally_valid_language_tag("-en-us"));
    }

    #[test]
    fn zxx_is_not_available() {
        assert!(!is_available_locale("zxx"));
        assert!(!is_available_locale("xyz"));
        assert!(is_available_locale("en"));
        assert!(is_available_locale("sr"));
    }

    #[test]
    fn zh_hans_cn_minimizes() {
        let locale = minimize_likely_subtags("zh-Hans-CN").expect("minimize");
        assert_eq!(locale.to_string(), "zh");
    }
}
