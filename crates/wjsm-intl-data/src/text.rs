//! 分段、display names、duration、unit、IDNA 与 WHATWG Encoding 标签。

use encoding_rs::Encoding;
use icu::experimental::dimension::currency::CurrencyCode;
use icu::experimental::dimension::currency::long_formatter::LongCurrencyFormatter;
use icu::experimental::dimension::units::formatter::UnitsFormatter;
use icu::experimental::displaynames::multi::{
    LanguageDisplayNames, LocaleDisplayNamesFormatter, RegionDisplayNames, ScriptDisplayNames,
};
use icu::experimental::displaynames::{DisplayNamesOptions, LanguageDisplay, Style};
use icu::experimental::duration::options::DurationFormatterOptions;
use icu::experimental::duration::{Duration, DurationFormatter, ValidatedDurationFormatterOptions};
use icu::locale::Locale;
use icu::locale::subtags::{Language, Region, Script};
use icu::segmenter::options::{
    SentenceBreakInvariantOptions, SentenceBreakOptions, WordBreakInvariantOptions,
    WordBreakOptions,
};
use icu::segmenter::{GraphemeClusterSegmenter, SentenceSegmenter, WordSegmenter};
use idna::{domain_to_ascii, domain_to_unicode};
use tinystr::TinyAsciiStr;

/// DisplayNames 的 type。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayNameType {
    Language,
    Region,
    Script,
    Currency,
    Calendar,
    DateTimeField,
}

pub struct OwnedDisplayNames {
    locale: Locale,
    kind: DisplayNameType,
    options: DisplayNamesOptions,
}

impl OwnedDisplayNames {
    pub fn try_new(
        locale: &str,
        kind: DisplayNameType,
        style: &str,
        language_display: &str,
    ) -> Result<Self, String> {
        Ok(Self {
            locale: Locale::try_from_str(locale).map_err(err)?,
            kind,
            options: display_options(style, language_display),
        })
    }

    pub fn of(&self, code: &str) -> Result<Option<String>, String> {
        match self.kind {
            DisplayNameType::Language => language_name(&self.locale, self.options, code),
            DisplayNameType::Region => {
                let names = RegionDisplayNames::try_new((&self.locale).into(), self.options)
                    .map_err(err)?;
                let parsed = Region::try_from_str(code).map_err(err)?;
                Ok(names.of(parsed).map(str::to_string))
            }
            DisplayNameType::Script => {
                let names = ScriptDisplayNames::try_new((&self.locale).into(), self.options)
                    .map_err(err)?;
                let parsed = Script::try_from_str(code).map_err(err)?;
                Ok(names.of(parsed).map(str::to_string))
            }
            DisplayNameType::Currency => currency_name(&self.locale, code),
            DisplayNameType::Calendar => Ok(calendar_name(self.locale.id.language.as_str(), code)),
            DisplayNameType::DateTimeField => {
                Ok(date_time_field_name(self.locale.id.language.as_str(), code))
            }
        }
    }
}

fn display_options(style: &str, language_display: &str) -> DisplayNamesOptions {
    let mut options = DisplayNamesOptions::default();
    options.style = Some(match style {
        "narrow" => Style::Narrow,
        "short" => Style::Short,
        _ => Style::Long,
    });
    options.language_display = if language_display == "standard" {
        LanguageDisplay::Standard
    } else {
        LanguageDisplay::Dialect
    };
    options
}

fn language_name(
    locale: &Locale,
    options: DisplayNamesOptions,
    code: &str,
) -> Result<Option<String>, String> {
    if let Ok(tag) = Locale::try_from_str(code) {
        let formatter =
            LocaleDisplayNamesFormatter::try_new(locale.into(), options).map_err(err)?;
        return Ok(Some(formatter.of(&tag).into_owned()));
    }
    let names = LanguageDisplayNames::try_new(locale.into(), options).map_err(err)?;
    let parsed = Language::try_from_str(code).map_err(err)?;
    Ok(names.of(parsed).map(str::to_string))
}

fn currency_name(locale: &Locale, code: &str) -> Result<Option<String>, String> {
    if !crate::available_currencies().contains(&code) {
        return Ok(None);
    }
    let Ok(tiny) = TinyAsciiStr::<3>::try_from_str(code) else {
        return Ok(None);
    };
    let formatter = LongCurrencyFormatter::try_new(locale.into(), &CurrencyCode(tiny)).ok();
    let Some(formatter) = formatter else {
        return Ok(None);
    };
    let rendered = formatter
        .format_fixed_decimal(&icu::decimal::input::Decimal::from(1))
        .to_string();
    Ok(strip_leading_number(&rendered))
}

fn strip_leading_number(text: &str) -> Option<String> {
    let rest = text
        .trim()
        .trim_start_matches(|ch: char| ch.is_ascii_digit() || ch == ',' || ch == '.' || ch == ' ')
        .trim();
    if rest.is_empty() {
        None
    } else {
        Some(rest.to_owned())
    }
}

fn calendar_name(language: &str, code: &str) -> Option<String> {
    let en = match code {
        "buddhist" => "Buddhist Calendar",
        "chinese" => "Chinese Calendar",
        "coptic" => "Coptic Calendar",
        "dangi" => "Dangi Calendar",
        "ethioaa" => "Ethiopic (Amete Alem) Calendar",
        "ethiopic" => "Ethiopic Calendar",
        "gregory" => "Gregorian Calendar",
        "hebrew" => "Hebrew Calendar",
        "indian" => "Indian Calendar",
        "islamic-civil" => "Islamic Calendar (civil)",
        "islamic-tbla" => "Islamic Calendar (tabular)",
        "islamic-umalqura" => "Islamic Calendar (Umm al-Qura)",
        "iso8601" => "ISO-8601 Calendar",
        "japanese" => "Japanese Calendar",
        "persian" => "Persian Calendar",
        "roc" => "Minguo Calendar",
        _ => return None,
    };
    if language == "en" {
        Some(en.into())
    } else {
        None
    }
}

fn date_time_field_name(language: &str, code: &str) -> Option<String> {
    if language != "en" {
        return None;
    }
    Some(
        match code {
            "era" => "era",
            "year" => "year",
            "quarter" => "quarter",
            "month" => "month",
            "weekOfYear" => "week",
            "weekday" => "day of the week",
            "day" => "day",
            "dayPeriod" => "AM/PM",
            "hour" => "hour",
            "minute" => "minute",
            "second" => "second",
            "timeZoneName" => "time zone",
            _ => return None,
        }
        .into(),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SegmentGranularity {
    Grapheme,
    Word,
    Sentence,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextSegment {
    pub utf16_start: u32,
    pub utf16_end: u32,
    pub word_like: bool,
}

#[derive(Clone, Debug)]
pub struct OwnedSegmenter {
    granularity: SegmentGranularity,
    locale: Option<Locale>,
}

impl OwnedSegmenter {
    pub fn new(granularity: SegmentGranularity) -> Self {
        Self {
            granularity,
            locale: None,
        }
    }

    pub fn try_new(locale: &str, granularity: SegmentGranularity) -> Result<Self, String> {
        Ok(Self {
            granularity,
            locale: Some(Locale::try_from_str(locale).map_err(err)?),
        })
    }

    pub fn break_offsets(&self, text: &str) -> Vec<usize> {
        self.utf8_breaks(text)
            .into_iter()
            .map(|(end, _)| end)
            .collect()
    }

    /// ECMA-402 分段下标是 UTF-16 码元。
    pub fn break_utf16_offsets(&self, text: &str) -> Vec<u32> {
        self.segments_utf16(text)
            .into_iter()
            .flat_map(|segment| [segment.utf16_start, segment.utf16_end])
            .fold(Vec::new(), |mut acc, offset| {
                if acc.last() != Some(&offset) {
                    acc.push(offset);
                }
                acc
            })
    }

    pub fn segments_utf16(&self, text: &str) -> Vec<TextSegment> {
        let breaks = self.utf8_breaks(text);
        let mut segments = Vec::with_capacity(breaks.len().saturating_sub(1));
        let mut start = 0usize;
        for (end, word_like) in breaks.into_iter().skip(1) {
            segments.push(TextSegment {
                utf16_start: utf16_len(&text[..start]),
                utf16_end: utf16_len(&text[..end]),
                word_like,
            });
            start = end;
        }
        segments
    }

    fn utf8_breaks(&self, text: &str) -> Vec<(usize, bool)> {
        match self.granularity {
            SegmentGranularity::Grapheme => GraphemeClusterSegmenter::new()
                .segment_str(text)
                .map(|end| (end, false))
                .collect(),
            SegmentGranularity::Word => word_breaks(self.locale.as_ref(), text),
            SegmentGranularity::Sentence => sentence_breaks(self.locale.as_ref(), text),
        }
    }
}

fn word_breaks(locale: Option<&Locale>, text: &str) -> Vec<(usize, bool)> {
    let id = locale.map(|locale| locale.id.clone());
    let mut options = WordBreakOptions::default();
    options.content_locale = id.as_ref();
    let Ok(segmenter) = WordSegmenter::try_new_auto(options) else {
        return WordSegmenter::new_auto(WordBreakInvariantOptions::default())
            .segment_str(text)
            .map(|end| (end, false))
            .collect();
    };
    let mut out = Vec::new();
    let mut iter = segmenter.as_borrowed().segment_str(text);
    while let Some(end) = iter.next() {
        let word_like = end != 0 && iter.is_word_like();
        out.push((end, word_like));
    }
    out
}

fn sentence_breaks(locale: Option<&Locale>, text: &str) -> Vec<(usize, bool)> {
    let id = locale.map(|locale| locale.id.clone());
    let mut options = SentenceBreakOptions::default();
    options.content_locale = id.as_ref();
    if let Ok(segmenter) = SentenceSegmenter::try_new(options) {
        return segmenter
            .as_borrowed()
            .segment_str(text)
            .map(|end| (end, false))
            .collect();
    }
    SentenceSegmenter::new(SentenceBreakInvariantOptions::default())
        .segment_str(text)
        .map(|end| (end, false))
        .collect()
}

fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

/// 解析 WHATWG Encoding 标签；`replacement` 映射返回 `None`（供 TextDecoder）。
pub fn encoding_for_label(label: &str) -> Option<&'static Encoding> {
    Encoding::for_label_no_replacement(label.as_bytes())
}

pub fn domain_to_ascii_uts46(domain: &str) -> Result<String, idna::Errors> {
    domain_to_ascii(domain)
}

/// UTS #46 ToUnicode。失败时仍返回映射后的字符串（与 Node `domainToUnicode` 一致）。
pub fn domain_to_unicode_uts46(domain: &str) -> String {
    domain_to_unicode(domain).0
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
