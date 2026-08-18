//! Unicode 大小写变换，锁定 ICU4X / Unicode 17.0 数据。

use icu::casemap::CaseMapper;
use icu::locale::{Locale, langid};

/// 与 locale 无关的 Unicode 默认大小写（`String.prototype.toLowerCase` / `toUpperCase`）。
pub fn case_map(text: &str, uppercase: bool) -> String {
    let mapper = CaseMapper::new();
    let root = langid!("und");
    if uppercase {
        mapper.uppercase_to_string(text, &root).into_owned()
    } else {
        mapper.lowercase_to_string(text, &root).into_owned()
    }
}

/// locale 敏感大小写（`toLocaleLowerCase` / `toLocaleUpperCase`）。
pub fn locale_case_map(text: &str, locale: &str, uppercase: bool) -> Result<String, String> {
    let locale = Locale::try_from_str(locale).map_err(|error| error.to_string())?;
    let mapper = CaseMapper::new();
    Ok(if uppercase {
        mapper.uppercase_to_string(text, &locale.id).into_owned()
    } else {
        mapper.lowercase_to_string(text, &locale.id).into_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::{case_map, locale_case_map};

    #[test]
    fn turkish_dotless_i() {
        assert_eq!(locale_case_map("I", "tr", false).expect("tr"), "ı");
        assert_eq!(locale_case_map("i", "tr", true).expect("tr"), "İ");
    }

    #[test]
    fn default_case_keeps_ascii() {
        assert_eq!(case_map("AbC", false), "abc");
        assert_eq!(case_map("AbC", true), "ABC");
    }
}
