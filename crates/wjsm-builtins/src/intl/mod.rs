//! ECMA-402 抽象操作：已 coerce 的 Rust 输入，不含 native ABI。

mod canonicalize;
mod options;
mod resolve;
mod supported;

pub use canonicalize::{
    IntlError, IntlErrorKind, canonicalize_locale_list, canonicalize_unicode_locale_id,
};
pub use options::{
    CollatorRecord, DateTimeFormatRecord, DisplayNamesRecord, DurationFormatRecord,
    ListFormatRecord, LocaleRecord, NumberFormatRecord, PluralRulesRecord, RelativeTimeRecord,
    SegmenterRecord,
};
pub use resolve::{
    ResolvedLocale, best_available_locale, lookup_supported_locales, resolve_locale,
    resolve_locale_filtered,
};
pub use supported::{get_canonical_locales, supported_values_of};
