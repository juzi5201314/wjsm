//! Locale 解析、likely subtags、canonicalization 与 fallback。
//!
//! ICU4X 2.2 没有独立 LocaleMatcher 类型；ECMA-402 的 locale negotiation
//! 由 canonicalizer + expander + fallbacker 数据支撑，算法在 Phase 2 实现。

use icu::locale::{Locale, LocaleCanonicalizer, LocaleExpander, LocaleFallbacker};

pub fn parse_locale(id: &str) -> Result<Locale, icu::locale::ParseError> {
    Locale::try_from_str(id)
}

pub fn expand_likely_subtags(id: &str) -> Result<Locale, icu::locale::ParseError> {
    let mut locale = parse_locale(id)?;
    let expander = LocaleExpander::new_extended();
    expander.maximize(&mut locale.id);
    Ok(locale)
}

pub fn canonicalize_locale(id: &str) -> Result<Locale, icu::locale::ParseError> {
    let mut locale = parse_locale(id)?;
    let canonicalizer = LocaleCanonicalizer::new_extended();
    canonicalizer.canonicalize(&mut locale);
    Ok(locale)
}

pub fn keep_locale_data() {
    let _ = LocaleExpander::new_extended();
    let _ = LocaleCanonicalizer::new_extended();
    let _ = LocaleFallbacker::new();
    let _ = expand_likely_subtags("zh");
    let _ = canonicalize_locale("zh-CN");
}

#[cfg(test)]
mod tests {
    use super::expand_likely_subtags;

    #[test]
    fn zh_cn_maximizes_to_hans() {
        let locale = expand_likely_subtags("zh-CN").expect("zh-CN");
        assert_eq!(locale.to_string(), "zh-Hans-CN");
    }
}
