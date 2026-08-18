//! RegExp 字面量 early error：在 lower 期用 `regress` 校验 pattern/flags。
//!
//! ECMAScript 要求非法 Unicode property escape 等在 parse/early 阶段抛 SyntaxError；
//! SWC 只保留字面量原文，真实语法校验由统一 RegExp owner `regress` 完成。

use std::collections::HashSet;

/// 校验 RegExp 字面量；失败时返回可嵌入 `SyntaxError:` 诊断的说明。
pub(crate) fn validate_regexp_literal(pattern: &str, flags: &str) -> Result<(), String> {
    let mut seen = HashSet::new();
    for flag in flags.chars() {
        if !matches!(flag, 'd' | 'g' | 'i' | 'm' | 's' | 'u' | 'v' | 'y') {
            return Err(format!("Invalid regular expression flag: '{flag}'"));
        }
        if !seen.insert(flag) {
            return Err(format!("Duplicate regular expression flag: '{flag}'"));
        }
    }
    if seen.contains(&'u') && seen.contains(&'v') {
        return Err("Regular expression flags 'u' and 'v' are mutually exclusive".to_owned());
    }
    let engine_flags: String = flags
        .chars()
        .filter(|flag| matches!(flag, 'i' | 'm' | 's' | 'u' | 'v'))
        .collect();
    regress::Regex::with_flags(pattern, engine_flags.as_str())
        .map(|_| ())
        .map_err(|error| format!("Invalid regular expression: {error}"))
}

#[cfg(test)]
mod tests {
    use super::validate_regexp_literal;

    #[test]
    fn accepts_unicode_property_escape_with_u() {
        assert!(validate_regexp_literal(r"\p{Letter}", "u").is_ok());
        assert!(validate_regexp_literal(r"\p{sc=Latn}", "u").is_ok());
        assert!(validate_regexp_literal(r"\p{Script_Extensions=Hani}", "u").is_ok());
    }

    #[test]
    fn rejects_unknown_property_with_u() {
        let err = validate_regexp_literal(r"\p{NotARealProperty}", "u").unwrap_err();
        assert!(err.contains("Invalid regular expression"), "{err}");
    }

    #[test]
    fn rejects_property_of_strings_with_u_only() {
        let err = validate_regexp_literal(r"\p{RGI_Emoji}", "u").unwrap_err();
        assert!(err.contains("Invalid regular expression"), "{err}");
    }

    #[test]
    fn unicode_17_sidetic_script_is_known() {
        // Sidetic 为 Unicode 17 新增 Script；与 Phase 1 manifest `unicode: 17.0.0` 对齐。
        assert!(validate_regexp_literal(r"\p{Script=Sidetic}", "u").is_ok());
        let ch = char::from_u32(0x10940).expect("Sidetic start");
        let re = regress::Regex::with_flags(r"\p{Script=Sidetic}", "u").unwrap();
        assert!(re.find(&ch.to_string()).is_some());
    }
}
