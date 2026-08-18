//! UTS #35 Unicode Locale Identifier 结构校验与 ICU 不可解析标签的回退规范化。
//!
//! ICU4X 的 `Language` 只接受 2–3 个字母，因此 `posix` 这类 5–8 字母 language
//! 不能交给 `Locale::try_from_str` 判断合法性。

use std::collections::HashSet;

#[derive(Debug)]
struct LanguageId {
    language: String,
    script: Option<String>,
    region: Option<String>,
    variants: Vec<String>,
}

#[derive(Debug)]
enum Extension {
    Unicode {
        attributes: Vec<String>,
        keywords: Vec<(String, Vec<String>)>,
    },
    Transform {
        language: Option<LanguageId>,
        fields: Vec<(String, Vec<String>)>,
    },
    Other {
        key: char,
        values: Vec<String>,
    },
    Private {
        values: Vec<String>,
    },
}

#[derive(Debug)]
struct LocaleId {
    language: LanguageId,
    extensions: Vec<Extension>,
}

struct Cursor<'a> {
    parts: Vec<&'a str>,
    index: usize,
}

impl<'a> Cursor<'a> {
    fn new(id: &'a str) -> Self {
        Self {
            parts: id.split('-').collect(),
            index: 0,
        }
    }

    fn peek(&self) -> Option<&'a str> {
        self.parts.get(self.index).copied()
    }

    fn bump(&mut self) -> Option<&'a str> {
        let part = self.peek()?;
        self.index += 1;
        Some(part)
    }

    fn take_while(&mut self, pred: impl Fn(&str) -> bool) -> Vec<String> {
        let mut values = Vec::new();
        while self.peek().is_some_and(&pred) {
            values.push(self.bump().expect("peek").to_owned());
        }
        values
    }

    fn exhausted(&self) -> bool {
        self.index >= self.parts.len()
    }
}

pub fn is_structurally_valid(id: &str) -> bool {
    parse_locale_id(id).is_ok()
}

/// `unicode_language_id`：语言/脚本/区域/变体，不含扩展。
pub fn is_unicode_language_id(id: &str) -> bool {
    let Ok(parsed) = parse_locale_id(id) else {
        return false;
    };
    parsed.extensions.is_empty() && parsed.language.language != "root"
}

/// Unicode 扩展属性（`u-attr`），maximize 后需要保留。
pub fn unicode_attributes(id: &str) -> Vec<String> {
    let Ok(parsed) = parse_locale_id(id) else {
        return Vec::new();
    };
    for extension in &parsed.extensions {
        if let Extension::Unicode { attributes, .. } = extension {
            return attributes
                .iter()
                .map(|item| item.to_ascii_lowercase())
                .collect();
        }
    }
    Vec::new()
}

/// 结构解析得到的 Unicode 关键字；保留 ICU 会丢掉的 `cu` / `fw` 等。
pub fn unicode_keywords(id: &str) -> std::collections::BTreeMap<String, String> {
    let Ok(parsed) = parse_locale_id(id) else {
        return std::collections::BTreeMap::new();
    };
    let mut map = std::collections::BTreeMap::new();
    for extension in &parsed.extensions {
        if let Extension::Unicode { keywords, .. } = extension {
            for (key, types) in keywords {
                map.insert(
                    key.to_ascii_lowercase(),
                    types.join("-").to_ascii_lowercase(),
                );
            }
        }
    }
    map
}

/// 非 `u` 扩展后缀（如 `-a-not-assigned`），maximize/minimize 后需要接回。
pub fn other_extensions_suffix(id: &str) -> String {
    let Ok(parsed) = parse_locale_id(id) else {
        return String::new();
    };
    let mut parts = Vec::new();
    for extension in &parsed.extensions {
        if !matches!(extension, Extension::Unicode { .. }) {
            parts.push(format_extension(extension));
        }
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("-{}", parts.join("-"))
    }
}

/// 语言 / 脚本 / 地区 / 变体；不依赖 ICU 解析器。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanguageFields {
    pub language: String,
    pub script: Option<String>,
    pub region: Option<String>,
    pub variants: Vec<String>,
}

pub fn language_fields(id: &str) -> Option<LanguageFields> {
    let parsed = parse_locale_id(id).ok()?;
    Some(LanguageFields {
        language: parsed.language.language.to_ascii_lowercase(),
        script: parsed.language.script.as_deref().map(title_case),
        region: parsed
            .language
            .region
            .as_ref()
            .map(|region| region.to_ascii_uppercase()),
        variants: parsed.language.variants,
    })
}

/// 用选项覆盖后的字段重装标签；扩展按 UTS #35 排序（`x` 最后）。
pub fn format_unicode_locale(
    language: &str,
    script: Option<&str>,
    region: Option<&str>,
    variants: &[String],
    attributes: &[String],
    keywords: &std::collections::BTreeMap<String, String>,
    source_tag: &str,
) -> String {
    let extensions = match parse_locale_id(source_tag) {
        Ok(parsed) => parsed
            .extensions
            .into_iter()
            .filter(|extension| !matches!(extension, Extension::Unicode { .. }))
            .collect(),
        Err(()) => Vec::new(),
    };
    let mut keywords = keywords
        .iter()
        .map(|(key, value)| {
            let types = if value.is_empty() || value == "true" {
                Vec::new()
            } else {
                value.split('-').map(str::to_owned).collect()
            };
            (key.clone(), types)
        })
        .collect::<Vec<_>>();
    keywords.sort_by(|left, right| left.0.cmp(&right.0));
    let mut extensions = extensions;
    if !attributes.is_empty() || !keywords.is_empty() {
        extensions.push(Extension::Unicode {
            attributes: attributes.to_vec(),
            keywords,
        });
    }
    let locale = LocaleId {
        language: LanguageId {
            language: language.to_owned(),
            script: script.map(str::to_owned),
            region: region.map(str::to_owned),
            variants: variants.to_vec(),
        },
        extensions,
    };
    format_locale(&locale)
}

/// ICU 解析不了但结构合法的标签：按 UTS #35 做大小写与扩展排序。
pub fn canonicalize_without_icu(id: &str) -> String {
    let mut parsed = parse_locale_id(id).expect("调用方已通过结构校验");
    apply_language_id_aliases(&mut parsed.language);
    format_locale(&parsed)
}

fn apply_language_id_aliases(language: &mut LanguageId) {
    let tag = language.language.to_ascii_lowercase();
    let variants = language
        .variants
        .iter()
        .map(|item| item.to_ascii_lowercase())
        .collect::<Vec<_>>();
    match (tag.as_str(), variants.as_slice()) {
        ("cel", vars) if vars.iter().any(|item| item == "gaulish") => {
            language.language = "xtg".into();
            language
                .variants
                .retain(|item| !item.eq_ignore_ascii_case("gaulish"));
        }
        ("hy", vars) if vars.iter().any(|item| item == "arevela") => {
            language
                .variants
                .retain(|item| !item.eq_ignore_ascii_case("arevela"));
        }
        ("hy", vars) if vars.iter().any(|item| item == "arevmda") => {
            language.language = "hyw".into();
            language
                .variants
                .retain(|item| !item.eq_ignore_ascii_case("arevmda"));
        }
        _ => {}
    }
}

fn parse_locale_id(id: &str) -> Result<LocaleId, ()> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(());
    }
    let mut cursor = Cursor::new(id);
    let language = parse_language_id(&mut cursor)?;
    let mut seen = HashSet::new();
    let mut extensions = Vec::new();
    while let Some(part) = cursor.peek() {
        if part.len() != 1 {
            return Err(());
        }
        let key = part.as_bytes()[0].to_ascii_lowercase() as char;
        if !seen.insert(key) {
            return Err(());
        }
        cursor.bump();
        extensions.push(parse_extension(&mut cursor, key)?);
        if matches!(extensions.last(), Some(Extension::Private { .. })) {
            break;
        }
    }
    if !cursor.exhausted() {
        return Err(());
    }
    Ok(LocaleId {
        language,
        extensions,
    })
}

fn parse_language_id(cursor: &mut Cursor<'_>) -> Result<LanguageId, ()> {
    let language = match cursor.peek() {
        Some(part) if is_language(part) => cursor.bump().expect("language").to_owned(),
        _ => return Err(()),
    };
    let script = cursor
        .peek()
        .filter(|part| is_script(part))
        .map(|_| cursor.bump().expect("script").to_owned());
    let region = cursor
        .peek()
        .filter(|part| is_region(part))
        .map(|_| cursor.bump().expect("region").to_owned());
    let mut seen = HashSet::new();
    let mut variants = Vec::new();
    while cursor.peek().is_some_and(is_variant) {
        let variant = cursor.bump().expect("variant").to_ascii_lowercase();
        if !seen.insert(variant.clone()) {
            return Err(());
        }
        variants.push(variant);
    }
    Ok(LanguageId {
        language,
        script,
        region,
        variants,
    })
}

fn parse_extension(cursor: &mut Cursor<'_>, key: char) -> Result<Extension, ()> {
    match key {
        'u' => parse_unicode(cursor),
        't' => parse_transform(cursor),
        'x' => parse_private(cursor),
        key if is_other_singleton(key) => parse_other(cursor, key),
        _ => Err(()),
    }
}

fn parse_unicode(cursor: &mut Cursor<'_>) -> Result<Extension, ()> {
    if cursor.peek().is_some_and(is_unicode_attribute) {
        let attributes = cursor.take_while(is_unicode_attribute);
        let keywords = parse_keywords(cursor);
        return Ok(Extension::Unicode {
            attributes,
            keywords,
        });
    }
    let keywords = parse_keywords(cursor);
    if keywords.is_empty() {
        return Err(());
    }
    Ok(Extension::Unicode {
        attributes: Vec::new(),
        keywords,
    })
}

fn parse_keywords(cursor: &mut Cursor<'_>) -> Vec<(String, Vec<String>)> {
    let mut keywords = Vec::new();
    while cursor.peek().is_some_and(is_unicode_key) {
        let key = cursor.bump().expect("ukey").to_owned();
        let types = cursor.take_while(is_unicode_type);
        keywords.push((key, types));
    }
    keywords
}

fn parse_transform(cursor: &mut Cursor<'_>) -> Result<Extension, ()> {
    let language = if cursor.peek().is_some_and(is_language) {
        Some(parse_language_id(cursor)?)
    } else {
        None
    };
    let mut fields = Vec::new();
    while let Some(field) = parse_tfield(cursor)? {
        fields.push(field);
    }
    if language.is_none() && fields.is_empty() {
        return Err(());
    }
    Ok(Extension::Transform { language, fields })
}

fn parse_tfield(cursor: &mut Cursor<'_>) -> Result<Option<(String, Vec<String>)>, ()> {
    let Some(key) = cursor.peek().filter(|part| is_tkey(part)) else {
        return Ok(None);
    };
    let key = key.to_owned();
    cursor.bump();
    let values = cursor.take_while(is_unicode_type);
    if values.is_empty() {
        return Err(());
    }
    Ok(Some((key, values)))
}

fn parse_other(cursor: &mut Cursor<'_>, key: char) -> Result<Extension, ()> {
    let values = cursor.take_while(is_other_value);
    if values.is_empty() {
        return Err(());
    }
    Ok(Extension::Other { key, values })
}

fn parse_private(cursor: &mut Cursor<'_>) -> Result<Extension, ()> {
    let values = cursor.take_while(is_private_value);
    if values.is_empty() {
        return Err(());
    }
    Ok(Extension::Private { values })
}

fn format_locale(locale: &LocaleId) -> String {
    let mut parts = format_language(&locale.language);
    let mut extensions = locale.extensions.iter().collect::<Vec<_>>();
    extensions.sort_by_key(|extension| extension.sort_key());
    for extension in extensions {
        parts.push('-');
        parts.push_str(&format_extension(extension));
    }
    parts
}

fn format_transform_language(language: &LanguageId) -> String {
    let mut parts = language.language.to_ascii_lowercase();
    if let Some(script) = &language.script {
        parts.push('-');
        parts.push_str(&script.to_ascii_lowercase());
    }
    if let Some(region) = &language.region {
        parts.push('-');
        parts.push_str(&region.to_ascii_lowercase());
    }
    let mut variants = language
        .variants
        .iter()
        .map(|variant| variant.to_ascii_lowercase())
        .collect::<Vec<_>>();
    variants.sort();
    for variant in variants {
        parts.push('-');
        parts.push_str(&variant);
    }
    parts
}

fn format_language(language: &LanguageId) -> String {
    let mut parts = language.language.to_ascii_lowercase();
    if let Some(script) = &language.script {
        parts.push('-');
        parts.push_str(&title_case(script));
    }
    if let Some(region) = &language.region {
        parts.push('-');
        parts.push_str(&region.to_ascii_uppercase());
    }
    let mut variants = language
        .variants
        .iter()
        .map(|variant| variant.to_ascii_lowercase())
        .collect::<Vec<_>>();
    variants.sort();
    for variant in variants {
        parts.push('-');
        parts.push_str(&variant);
    }
    parts
}

fn format_extension(extension: &Extension) -> String {
    match extension {
        Extension::Unicode {
            attributes,
            keywords,
        } => format_unicode(attributes, keywords),
        Extension::Transform { language, fields } => format_transform(language.as_ref(), fields),
        Extension::Other { key, values } => format_parts(*key, values),
        Extension::Private { values } => format_parts('x', values),
    }
}

fn format_unicode(attributes: &[String], keywords: &[(String, Vec<String>)]) -> String {
    let mut attributes = attributes
        .iter()
        .map(|item| item.to_ascii_lowercase())
        .collect::<Vec<_>>();
    attributes.sort();
    let mut keywords = keywords
        .iter()
        .map(|(key, types)| {
            (
                key.to_ascii_lowercase(),
                types
                    .iter()
                    .map(|item| item.to_ascii_lowercase())
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    keywords.sort_by(|left, right| left.0.cmp(&right.0));
    let mut parts = String::from("u");
    for attribute in attributes {
        parts.push('-');
        parts.push_str(&attribute);
    }
    for (key, types) in keywords {
        parts.push('-');
        parts.push_str(&key);
        for item in types {
            parts.push('-');
            parts.push_str(&item);
        }
    }
    parts
}

fn format_transform(language: Option<&LanguageId>, fields: &[(String, Vec<String>)]) -> String {
    let mut parts = String::from("t");
    if let Some(language) = language {
        parts.push('-');
        // ECMA-402 / test262：`t` 扩展内嵌语言的 script 保持小写。
        parts.push_str(&format_transform_language(language));
    }
    let mut fields = fields
        .iter()
        .map(|(key, values)| {
            (
                key.to_ascii_lowercase(),
                values
                    .iter()
                    .map(|item| item.to_ascii_lowercase())
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    fields.sort_by(|left, right| left.0.cmp(&right.0));
    for (key, values) in fields {
        parts.push('-');
        parts.push_str(&key);
        for value in values {
            parts.push('-');
            parts.push_str(&value);
        }
    }
    parts
}

fn format_parts(key: char, values: &[String]) -> String {
    let mut parts = key.to_ascii_lowercase().to_string();
    for value in values {
        parts.push('-');
        parts.push_str(&value.to_ascii_lowercase());
    }
    parts
}

impl Extension {
    fn sort_key(&self) -> (u8, char) {
        match self {
            Self::Unicode { .. } => (0, 'u'),
            Self::Transform { .. } => (0, 't'),
            Self::Other { key, .. } => (0, key.to_ascii_lowercase()),
            Self::Private { .. } => (1, 'x'),
        }
    }
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => {
            let mut title = first.to_ascii_uppercase().to_string();
            title.push_str(&chars.as_str().to_ascii_lowercase());
            title
        }
        None => String::new(),
    }
}

fn is_language(part: &str) -> bool {
    let len = part.len();
    is_alpha(part) && ((2..=3).contains(&len) || (5..=8).contains(&len))
}

fn is_script(part: &str) -> bool {
    part.len() == 4 && is_alpha(part)
}

fn is_region(part: &str) -> bool {
    (part.len() == 2 && is_alpha(part)) || (part.len() == 3 && is_digit(part))
}

fn is_variant(part: &str) -> bool {
    let len = part.len();
    (5..=8).contains(&len) && is_alnum(part)
        || len == 4 && part.as_bytes()[0].is_ascii_digit() && is_alnum(part)
}

fn is_unicode_key(part: &str) -> bool {
    part.len() == 2
        && part.as_bytes()[0].is_ascii_alphanumeric()
        && part.as_bytes()[1].is_ascii_alphabetic()
}

fn is_unicode_attribute(part: &str) -> bool {
    is_unicode_type(part)
}

fn is_unicode_type(part: &str) -> bool {
    (3..=8).contains(&part.len()) && is_alnum(part)
}

fn is_tkey(part: &str) -> bool {
    part.len() == 2
        && part.as_bytes()[0].is_ascii_alphabetic()
        && part.as_bytes()[1].is_ascii_digit()
}

fn is_other_singleton(key: char) -> bool {
    key.is_ascii_digit() || matches!(key, 'a'..='s' | 'v' | 'w' | 'y' | 'z')
}

fn is_other_value(part: &str) -> bool {
    (2..=8).contains(&part.len()) && is_alnum(part)
}

fn is_private_value(part: &str) -> bool {
    (1..=8).contains(&part.len()) && is_alnum(part)
}

fn is_alpha(part: &str) -> bool {
    !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphabetic())
}

fn is_digit(part: &str) -> bool {
    !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_alnum(part: &str) -> bool {
    !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::{canonicalize_without_icu, is_structurally_valid};

    #[test]
    fn five_letter_language_is_valid() {
        assert!(is_structurally_valid("posix"));
        assert_eq!(canonicalize_without_icu("POSIX"), "posix");
    }

    #[test]
    fn unicode_key_second_char_cannot_be_digit() {
        assert!(!is_structurally_valid("en-u-c0"));
        assert!(is_structurally_valid("en-u-0c"));
    }

    #[test]
    fn incomplete_transform_is_invalid() {
        assert!(!is_structurally_valid("en-t"));
        assert!(!is_structurally_valid("en-t-d0"));
    }

    #[test]
    fn rejects_duplicates_and_private_only() {
        assert!(!is_structurally_valid("de-gregory-gregory"));
        assert!(!is_structurally_valid("x-foo"));
        assert!(!is_structurally_valid("zh-hak-CN"));
    }

    #[test]
    fn sorts_extensions_and_grandfathered_variants() {
        assert_eq!(
            canonicalize_without_icu("en-u-baz-a-bar-x-u-foo"),
            "en-a-bar-u-baz-x-u-foo"
        );
        assert_eq!(canonicalize_without_icu("cel-gaulish"), "xtg");
        assert_eq!(canonicalize_without_icu("hy-arevela"), "hy");
        assert_eq!(canonicalize_without_icu("hy-arevmda"), "hyw");
    }
}
