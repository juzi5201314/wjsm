//! 后端无关的国际化数据入口。
//!
//! ICU4X compiled_data、UTS #46（`idna`）与 WHATWG Encoding（`encoding_rs`）
//! 只允许经本 crate 进入生产二进制。禁止 Cranelift / native ABI 类型。
//! 数据由 rustc 链进 `wjsm` / `wjsm-exec` stub，不进 `.wjsm` 或 startup snapshot。
//!
//! Phase 2 之后 locale / format / text 始终编译：host-native 的 `Intl` API 会引用它们。
//! `keep_compiled_data()` 仍只在发行构建强制留住尚未被 JS 路径引用的实验数据。

pub mod aliases;
pub mod casemap;
pub mod coverage;
pub mod datetime;
mod datetime_range;
pub mod duration;
pub mod enumeration;
pub mod format;
pub mod locale;
pub mod locale_info;
pub mod manifest;
pub mod normalize;
pub mod number;
mod number_parts;
mod number_round;
pub mod number_symbols;
pub mod tag;
pub mod text;
pub mod zone;

pub use encoding_rs;
pub use icu;
pub use idna;

pub use aliases::canonicalize_unicode_keyword;
pub use casemap::{case_map, locale_case_map};
pub use coverage::{CoverageError, LocaleCoverage, probe_locale};
pub use datetime::{DateTimeFormatSpec, OwnedDateTimeFormatter};
pub use duration::{DurationFormatSpec, DurationUnitSpec, parse_iso_duration};
pub use enumeration::{
    available_calendars, available_collations, available_currencies, available_numbering_systems,
    available_time_zones, available_units, canonicalize_time_zone, collation_supported,
    default_ignore_punctuation, is_well_formed_unit_identifier, supported_values,
};
pub use format::{
    CollatorSensitivity, FormatPart, OwnedCollator, OwnedDecimalFormatter, OwnedDurationFormatter,
    OwnedListFormatter, OwnedPluralRules, OwnedRelativeTimeFormatter, calendar, collator,
    format_and_list, format_month, format_number, parse_timezone, plural_rules,
};
pub use locale::{
    UnicodeExtensionMap, canonicalize_locale, canonicalize_unicode_locale_id, default_locale,
    expand_likely_subtags, fallback_steps, is_available_locale, is_structurally_valid_language_tag,
    is_unicode_language_id, minimize_likely_subtags, parse_locale, unicode_extensions,
};
pub use locale_info::{
    WeekInfo, default_hour_cycle, hour_cycle_12, locale_calendars, locale_collations,
    locale_hour_cycles, locale_numbering_systems, locale_text_direction, locale_time_zones,
    locale_week_info,
};
pub use manifest::{DATA_MANIFEST, DataManifest, canonical_json, manifest_sha256};
pub use normalize::{NormalizationForm, normalize};
pub use number::{NumberFormatSpec, OwnedNumberFormatter, compare_math_strings};
pub use number_symbols::{currency_digits, locale_nan, substitute_digits};
pub use text::{
    DisplayNameType, OwnedDisplayNames, OwnedSegmenter, SegmentGranularity, TextSegment,
    domain_to_ascii_uts46, domain_to_unicode_uts46, encoding_for_label, language_display_name,
    region_display_name, word_segment_count,
};
pub use zone::{
    default_time_zone, ensure_zone_convertible, time_zone_display_name, utc_offset_seconds,
    utc_to_wall_millis,
};

/// smoke matrix 使用的 locale；发行清单与测试共用。
pub const SMOKE_LOCALES: &[&str] = &[
    "en-US", "zh-CN", "de-DE", "es-ES", "ar", "th", "tr", "ja-JP",
];

/// 强制把 compiled_data 留在 rustc 链接的 **发行** stub 里。
///
/// ICU4X 的 compiled_data 可被 DCE 删掉。发行构建用 `#[used]` constructor 指针
/// 留住各类数据入口。debug `wjsm` 依赖 JS `Intl` 路径的实际引用保留数据。
pub fn keep_compiled_data() {
    #[cfg(not(debug_assertions))]
    std::hint::black_box(KEEP_COMPILED_DATA_CTORS);
}

#[cfg(not(debug_assertions))]
#[used]
static KEEP_COMPILED_DATA_CTORS: &[fn()] = &[
    coverage::keep_compiled_data,
    locale::keep_locale_data,
    format::keep_format_data,
    text::keep_text_data,
];
