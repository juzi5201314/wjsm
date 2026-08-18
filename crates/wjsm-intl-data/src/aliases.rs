//! ICU4X `LocaleCanonicalizer` 只处理语言/脚本/地区与 `rg`/`sd`；
//! ECMA-402 还要求 UTS #35 的 Unicode / Transform 扩展类型别名。

use icu::locale::Locale;
use icu::locale::extensions::transform::{Value as TransformValue, key as tkey};
use icu::locale::extensions::unicode::value;

/// `kb`/`kc`/`kh`/`kk`/`kn` 的 `yes` 规范成 `true`，序列化时省略 `true`。
const TRUE_KEYS: &[&str] = &["kb", "kc", "kh", "kk", "kn"];

pub fn apply_extension_aliases(locale: &mut Locale) {
    apply_unicode_aliases(locale);
    apply_transform_aliases(locale);
}

fn apply_unicode_aliases(locale: &mut Locale) {
    let entries: Vec<_> = locale
        .extensions
        .unicode
        .keywords
        .iter()
        .map(|(item, item_value)| (*item, item_value.to_string()))
        .collect();
    for (item, item_value) in entries {
        let key_name = item.as_str();
        let canonical = remap_unicode_type(key_name, &item_value);
        if TRUE_KEYS.contains(&key_name) && matches!(canonical.as_str(), "true" | "yes" | "") {
            locale.extensions.unicode.keywords.set(item, value!("true"));
            continue;
        }
        if canonical != item_value
            && let Ok(parsed) = canonical.parse()
        {
            locale.extensions.unicode.keywords.set(item, parsed);
        }
    }
}

fn apply_transform_aliases(locale: &mut Locale) {
    if let Some(field) = locale.extensions.transform.fields.get(&tkey!("m0"))
        && field.to_string() == "names"
        && let Ok(canonical) = "prprname".parse::<TransformValue>()
    {
        locale
            .extensions
            .transform
            .fields
            .set(tkey!("m0"), canonical);
    }
}

pub fn canonicalize_unicode_keyword(key: &str, value: &str) -> String {
    remap_unicode_type(key, value)
}

fn remap_unicode_type(key_name: &str, item_value: &str) -> String {
    match (key_name, item_value) {
        ("ca", "ethiopic-amete-alem") => "ethioaa",
        ("ca", "islamicc") => "islamic-civil",
        ("ks", "primary") => "level1",
        ("ks", "tertiary") => "level3",
        ("tz", "cnckg") => "cnsha",
        ("tz", "eire") => "iedub",
        ("tz", "est") => "papty",
        ("tz", "gmt0") => "gmt",
        ("tz", "uct" | "zulu") => "utc",
        ("ms", "imperial") => "uksystem",
        (key_name, "yes") if TRUE_KEYS.contains(&key_name) => "true",
        (_, value) => value,
    }
    .to_owned()
}
