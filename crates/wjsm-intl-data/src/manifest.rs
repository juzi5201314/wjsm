//! 跨 crate 单一数据版本契约。升级 ICU4X 必须同时改清单与 hash 测试。

use sha2::{Digest, Sha256};

/// 嵌入二进制的 CLDR/Unicode 数据版本。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataManifest {
    pub icu4x: &'static str,
    pub cldr: &'static str,
    pub unicode: &'static str,
    pub uts46: &'static str,
    pub tzdb: &'static str,
    pub iso4217: &'static str,
    pub encoding: &'static str,
    pub regexp: &'static str,
    pub coverage: &'static str,
    pub smoke_locales: &'static [&'static str],
}

/// ICU4X 2.2 compiled_data：CLDR 48.2、TZDB 2026a、Unicode 17（CLDR 48 基线）。
/// RegExp Unicode property escapes 由 `regress` 0.11 提供，UCD 同为 Unicode 17。
pub const DATA_MANIFEST: DataManifest = DataManifest {
    icu4x: "2.2.0",
    cldr: "48.2",
    unicode: "17.0.0",
    uts46: "Unicode 17.0 UTS #46 (idna 1.1.0 / WHATWG URL)",
    tzdb: "2026a",
    iso4217: "CLDR 48.2",
    encoding: "WHATWG Encoding Standard (encoding_rs 0.8.35)",
    regexp: "regress 0.11.1 (Unicode 17.0.0 property escapes)",
    coverage: "full",
    smoke_locales: crate::SMOKE_LOCALES,
};

/// 字段按字母序排列的紧凑 JSON，作为稳定 hash 输入。
pub const CANONICAL_JSON: &str = r#"{"cldr":"48.2","coverage":"full","encoding":"WHATWG Encoding Standard (encoding_rs 0.8.35)","icu4x":"2.2.0","iso4217":"CLDR 48.2","regexp":"regress 0.11.1 (Unicode 17.0.0 property escapes)","smoke_locales":["en-US","zh-CN","de-DE","es-ES","ar","th","tr","ja-JP"],"tzdb":"2026a","unicode":"17.0.0","uts46":"Unicode 17.0 UTS #46 (idna 1.1.0 / WHATWG URL)"}"#;

pub const DATA_MANIFEST_SHA256: &str =
    "ab4855fe416ff1264b388ce52a40ff0f238944846eeccfdad114c723cfe219df";

pub fn canonical_json() -> &'static str {
    CANONICAL_JSON
}

pub fn manifest_sha256() -> String {
    hex_sha256(CANONICAL_JSON.as_bytes())
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{DATA_MANIFEST, DATA_MANIFEST_SHA256, canonical_json, manifest_sha256};

    #[test]
    fn canonical_json_matches_manifest_fields() {
        let json = canonical_json();
        assert!(json.contains(DATA_MANIFEST.icu4x));
        assert!(json.contains(DATA_MANIFEST.cldr));
        assert!(json.contains(DATA_MANIFEST.unicode));
        assert!(json.contains(DATA_MANIFEST.tzdb));
        assert!(json.contains(DATA_MANIFEST.coverage));
        assert!(json.contains(DATA_MANIFEST.regexp));
        for locale in DATA_MANIFEST.smoke_locales {
            assert!(json.contains(locale), "missing {locale}");
        }
    }

    #[test]
    fn manifest_hash_is_stable() {
        assert_eq!(manifest_sha256(), DATA_MANIFEST_SHA256);
    }

    #[test]
    fn coverage_is_full_not_english_only() {
        assert_eq!(DATA_MANIFEST.coverage, "full");
        assert!(DATA_MANIFEST.smoke_locales.len() > 1);
        assert!(DATA_MANIFEST.smoke_locales.contains(&"zh-CN"));
    }
}
