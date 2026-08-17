//! 分段、display names、duration、unit、IDNA 与 WHATWG Encoding 标签。

use encoding_rs::Encoding;
use icu::experimental::dimension::units::formatter::UnitsFormatter;
use icu::experimental::displaynames::multi::{LanguageDisplayNames, RegionDisplayNames};
use icu::experimental::duration::options::DurationFormatterOptions;
use icu::experimental::duration::{Duration, DurationFormatter, ValidatedDurationFormatterOptions};
use icu::locale::Locale;
use icu::locale::subtags::{Language, Region};
use icu::segmenter::options::{SentenceBreakInvariantOptions, WordBreakInvariantOptions};
use icu::segmenter::{GraphemeClusterSegmenter, SentenceSegmenter, WordSegmenter};
use idna::domain_to_ascii;

pub fn encoding_for_label(label: &str) -> Option<&'static Encoding> {
    Encoding::for_label(label.as_bytes())
}

pub fn domain_to_ascii_uts46(domain: &str) -> Result<String, idna::Errors> {
    domain_to_ascii(domain)
}

pub fn word_segment_count(text: &str) -> usize {
    let segmenter = WordSegmenter::new_auto(WordBreakInvariantOptions::default());
    segmenter.segment_str(text).count()
}

pub fn region_display_name(locale: &Locale, region_code: &str) -> Result<String, String> {
    let names = RegionDisplayNames::try_new(locale.into(), Default::default()).map_err(err)?;
    let parsed = Region::try_from_str(region_code).map_err(err)?;
    names
        .of(parsed)
        .map(str::to_string)
        .ok_or_else(|| format!("missing region display name for {region_code}"))
}

pub fn language_display_name(locale: &Locale, language_code: &str) -> Result<String, String> {
    let names = LanguageDisplayNames::try_new(locale.into(), Default::default()).map_err(err)?;
    let parsed = Language::try_from_str(language_code).map_err(err)?;
    names
        .of(parsed)
        .map(str::to_string)
        .ok_or_else(|| format!("missing language display name for {language_code}"))
}

pub fn format_duration(locale: &Locale) -> Result<String, String> {
    let options = ValidatedDurationFormatterOptions::validate(DurationFormatterOptions::default())
        .map_err(err)?;
    let formatter = DurationFormatter::try_new(locale.into(), options).map_err(err)?;
    let duration = Duration {
        hours: 1,
        minutes: 2,
        ..Duration::default()
    };
    Ok(formatter.format(&duration).to_string())
}

pub fn format_unit(locale: &Locale) -> Result<String, String> {
    let formatter =
        UnitsFormatter::try_new(locale.into(), "meter", Default::default()).map_err(err)?;
    Ok(formatter
        .format_fixed_decimal(&icu::decimal::input::Decimal::from(3))
        .to_string())
}

pub fn keep_text_data() {
    let _ = GraphemeClusterSegmenter::new();
    let _ = SentenceSegmenter::new(SentenceBreakInvariantOptions::default());
    let _ = WordSegmenter::new_auto(WordBreakInvariantOptions::default());
    let _ = RegionDisplayNames::try_new(icu::locale::locale!("en").into(), Default::default());
    let _ = LanguageDisplayNames::try_new(icu::locale::locale!("en").into(), Default::default());
    if let Ok(options) =
        ValidatedDurationFormatterOptions::validate(DurationFormatterOptions::default())
    {
        let _ = DurationFormatter::try_new(icu::locale::locale!("en").into(), options);
    }
    let _ = UnitsFormatter::try_new(
        icu::locale::locale!("en").into(),
        "meter",
        Default::default(),
    );
    let _ = domain_to_ascii("example.com");
    let _ = Encoding::for_label(b"utf-8");
    let _ = Encoding::for_label(b"gbk");
    let _ = Encoding::for_label(b"shift_jis");
}

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use encoding_rs::Encoding;

    use super::{domain_to_ascii_uts46, encoding_for_label, word_segment_count};

    #[test]
    fn idna_maps_chinese_domain() {
        let ascii = domain_to_ascii_uts46("例子.测试").expect("idna");
        assert!(ascii.starts_with("xn--"), "{ascii}");
    }

    #[test]
    fn encoding_labels_include_legacy_cjk() {
        assert_eq!(
            encoding_for_label("utf-8").map(Encoding::name),
            Some("UTF-8")
        );
        assert!(encoding_for_label("gbk").is_some());
        assert!(encoding_for_label("shift_jis").is_some());
        assert!(encoding_for_label("windows-1252").is_some());
    }

    #[test]
    fn thai_and_japanese_word_segment() {
        assert!(word_segment_count("ทุกสองสัปดาห์") > 2);
        assert!(word_segment_count("こんにちは世界") > 2);
    }
}
