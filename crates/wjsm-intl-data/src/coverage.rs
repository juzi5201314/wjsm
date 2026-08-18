//! 多语言 smoke matrix：证明 compiled_data 覆盖声明 locale 与数据类别。

use crate::format::{
    calendar, collator, format_and_list, format_month, format_number, parse_timezone, plural_rules,
};
use crate::locale::{expand_likely_subtags, parse_locale};
use crate::normalize::{NormalizationForm, normalize};
use crate::text::{
    domain_to_ascii_uts46, encoding_for_label, format_duration, format_unit, language_display_name,
    region_display_name, word_segment_count,
};
use icu::calendar::AnyCalendarKind;
use icu::plurals::PluralCategory;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocaleCoverage {
    pub locale: String,
    pub maximized: String,
    pub number: String,
    pub month: String,
    pub plural: String,
    pub list: String,
    pub word_segments: usize,
    pub region_name: String,
    pub language_name: String,
    pub duration: String,
    pub unit: String,
    pub timezone_known: bool,
    pub collation_ready: bool,
}

#[derive(Debug)]
pub struct CoverageError {
    pub locale: String,
    pub category: &'static str,
    pub message: String,
}

impl std::fmt::Display for CoverageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} / {}: {}",
            self.locale, self.category, self.message
        )
    }
}

impl std::error::Error for CoverageError {}

pub fn probe_locale(id: &str) -> Result<LocaleCoverage, CoverageError> {
    let locale = parse_locale(id).map_err(|error| fail(id, "locale", error))?;
    let maximized = expand_likely_subtags(id).map_err(|error| fail(id, "likely-subtags", error))?;
    let number = format_number(&locale, 1234).map_err(|error| fail(id, "number", error))?;
    let month = format_month(&locale).map_err(|error| fail(id, "datetime", error))?;
    let rules = plural_rules(&locale).map_err(|error| fail(id, "plural", error))?;
    let list =
        format_and_list(&locale, &["A", "B", "C"]).map_err(|error| fail(id, "list", error))?;
    let sample = word_sample(id);
    let word_segments = word_segment_count(sample);
    let region_name = region_display_name(&locale, region_of(id))
        .map_err(|error| fail(id, "display-name", error))?;
    let language_name = language_display_name(&locale, language_of(id))
        .map_err(|error| fail(id, "display-name", error))?;
    let duration = format_duration(&locale).map_err(|error| fail(id, "duration", error))?;
    let unit = format_unit(&locale).map_err(|error| fail(id, "unit", error))?;
    let timezone = parse_timezone(timezone_of(id));
    let _ = calendar(calendar_of(id));
    let collation = collator(&locale).map_err(|error| fail(id, "collation", error))?;
    let _ = collation.compare("a", "b");
    let _ = normalize("é", NormalizationForm::Nfc);
    Ok(LocaleCoverage {
        locale: locale.to_string(),
        maximized: maximized.to_string(),
        number,
        month,
        plural: format!("{:?}", rules.category_for(2_u32)),
        list,
        word_segments,
        region_name,
        language_name,
        duration,
        unit,
        timezone_known: !timezone.is_unknown(),
        collation_ready: true,
    })
}

pub fn keep_compiled_data() {
    crate::locale::keep_locale_data();
    crate::format::keep_format_data();
    crate::text::keep_text_data();
    let _ = normalize("e\u{0301}", NormalizationForm::Nfc);
    let _ = domain_to_ascii_uts46("例子.测试");
    let _ = encoding_for_label("gbk");
    let _ = PluralCategory::Other;
}

fn fail(locale: &str, category: &'static str, error: impl std::fmt::Display) -> CoverageError {
    CoverageError {
        locale: locale.to_string(),
        category,
        message: error.to_string(),
    }
}

fn language_of(id: &str) -> &str {
    id.split(['-', '_']).next().unwrap_or(id)
}

fn region_of(id: &str) -> &str {
    match id {
        "zh-CN" => "CN",
        "de-DE" => "DE",
        "es-ES" => "ES",
        "ja-JP" => "JP",
        "en-US" => "US",
        "ar" => "SA",
        "th" => "TH",
        "tr" => "TR",
        _ => "US",
    }
}

fn timezone_of(id: &str) -> &'static str {
    match id {
        "zh-CN" => "Asia/Shanghai",
        "de-DE" => "Europe/Berlin",
        "es-ES" => "Europe/Madrid",
        "ja-JP" => "Asia/Tokyo",
        "ar" => "Asia/Riyadh",
        "th" => "Asia/Bangkok",
        "tr" => "Europe/Istanbul",
        _ => "America/New_York",
    }
}

fn calendar_of(id: &str) -> AnyCalendarKind {
    match id {
        "ja-JP" => AnyCalendarKind::Japanese,
        "zh-CN" => AnyCalendarKind::Chinese,
        "th" => AnyCalendarKind::Buddhist,
        "ar" => AnyCalendarKind::HijriUmmAlQura,
        _ => AnyCalendarKind::Gregorian,
    }
}

fn word_sample(id: &str) -> &'static str {
    match language_of(id) {
        "zh" => "你好世界",
        "ja" => "こんにちは世界",
        "th" => "ทุกสองสัปดาห์",
        "ar" => "مرحبا بالعالم",
        _ => "hello world",
    }
}

#[cfg(test)]
mod tests {
    use super::probe_locale;
    use crate::SMOKE_LOCALES;
    use crate::text::{domain_to_ascii_uts46, encoding_for_label};

    #[test]
    fn smoke_matrix_covers_required_locales() {
        for locale in SMOKE_LOCALES {
            let sample = probe_locale(locale).unwrap_or_else(|error| panic!("{error}"));
            assert!(sample.collation_ready, "{locale} collation");
            assert!(sample.timezone_known, "{locale} timezone");
            assert!(!sample.number.is_empty(), "{locale} number");
            assert!(!sample.month.is_empty(), "{locale} month");
            assert!(!sample.list.is_empty(), "{locale} list");
            assert!(!sample.duration.is_empty(), "{locale} duration");
            assert!(!sample.unit.is_empty(), "{locale} unit");
            assert!(!sample.region_name.is_empty(), "{locale} region name");
            assert!(sample.word_segments >= 2, "{locale} segmenter");
        }
    }

    #[test]
    fn non_english_locales_are_not_english_only() {
        let es = probe_locale("es-ES").expect("es-ES");
        assert!(
            es.month.to_lowercase().contains("enero"),
            "es month: {}",
            es.month
        );

        let zh = probe_locale("zh-CN").expect("zh-CN");
        assert!(
            !zh.month.is_ascii() || !zh.number.is_ascii(),
            "zh-CN should use non-ASCII data: month={} number={}",
            zh.month,
            zh.number
        );

        let ar = probe_locale("ar").expect("ar");
        assert!(
            !ar.number.is_ascii() || !ar.month.is_ascii(),
            "ar should use non-ASCII data: month={} number={}",
            ar.month,
            ar.number
        );

        let th = probe_locale("th").expect("th");
        assert!(
            !th.month.is_ascii() || !th.number.is_ascii(),
            "th should use non-ASCII data: month={} number={}",
            th.month,
            th.number
        );

        let ja = probe_locale("ja-JP").expect("ja-JP");
        assert!(
            !ja.month.is_ascii() || !ja.region_name.is_ascii(),
            "ja-JP should use non-ASCII data: month={} region={}",
            ja.month,
            ja.region_name
        );

        let de = probe_locale("de-DE").expect("de-DE");
        assert!(
            de.month.to_lowercase().contains("januar"),
            "de month: {}",
            de.month
        );

        let tr = probe_locale("tr").expect("tr");
        assert!(
            tr.month.to_lowercase().contains("ocak"),
            "tr month: {}",
            tr.month
        );
    }

    #[test]
    fn idna_and_encoding_data_are_present() {
        let ascii = domain_to_ascii_uts46("münchen.de").expect("idna");
        assert!(
            ascii.contains("xn--") || ascii.contains("munchen"),
            "{ascii}"
        );
        assert!(encoding_for_label("gbk").is_some());
    }

    #[test]
    fn node_icu_data_env_does_not_change_coverage() {
        let before = probe_locale("es-ES").expect("before");
        let previous = std::env::var("NODE_ICU_DATA").ok();
        unsafe {
            std::env::set_var("NODE_ICU_DATA", "/nonexistent/icu");
        }
        let after = probe_locale("es-ES").expect("after");
        match previous {
            Some(value) => unsafe { std::env::set_var("NODE_ICU_DATA", value) },
            None => unsafe { std::env::remove_var("NODE_ICU_DATA") },
        }
        assert_eq!(before.month, after.month);
        assert_eq!(before.number, after.number);
    }
}
